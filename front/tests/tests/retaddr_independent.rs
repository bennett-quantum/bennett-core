//! Independent native witnesses of DESIGN.md "Return addresses" across frames, jumps and inlining.
use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, entry, lower, native_build, options};
use std::collections::BTreeMap;
use std::process::Command;

const FRAMES: &[&str] = &["-fno-omit-frame-pointer", "-mno-omit-leaf-frame-pointer", "-fno-optimize-sibling-calls"];

#[test]
fn recursion_indirection_jumps_inlining_and_stack_agree_with_native_and_reverse() {
    let source = c_dir().join("retaddr_independent.c");
    let mut native_flags = FRAMES.to_vec();
    native_flags.push("-DNATIVE");
    let executable = native_build(&source, "retaddr_independent", &native_flags);
    let output = Command::new(executable).output().expect("run native");
    assert!(output.status.success(), "native: {:?}", output.status);
    let expected: BTreeMap<_, _> = String::from_utf8(output.stdout)
        .expect("utf8")
        .lines()
        .map(|line| {
            let f: Vec<_> = line.split_whitespace().collect();
            ((f[0].to_owned(), f[1].parse::<u64>().expect("input")), f[2].parse::<u128>().expect("result"))
        })
        .collect();
    assert_eq!(expected.len(), 160);
    for (number, (clang_flags, front_flags)) in
        [(CLANG_FLAGS, &[][..]), (CLANG_O2_FLAGS, &[][..]), (CLANG_FLAGS, &["--taint-all"][..])].into_iter().enumerate()
    {
        let mut flags = clang_flags.to_vec();
        flags.extend_from_slice(FRAMES);
        let p = lower(&source, &format!("retaddr_independent.{number}"), &flags, front_flags);
        let jump = p.function(entry(&p, "ra_jump_body"));
        for name in ["ra_up1", "ra_up2"] {
            let callee = entry(&p, name);
            let calls = jump
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .filter(|inst| matches!(inst, bennett_ir::Inst::Call { callee: f, .. } if *f == callee))
                .count();
            assert_eq!(calls, 2, "{number}: the {name} observations on both sides of longjmp survive optimization");
        }
        assert!(p.function_named("ra_inline").is_none(), "always-inline witness was inlined");
        for ((name, x), want) in &expected {
            let mut e =
                Executor::new(&p, entry(&p, name), &[*x], Options { reversible: true, ..options(0) }).expect("start");
            let start = e.snapshot();
            assert_eq!(e.run(), Outcome::Returned(Some(*want)), "{number}: {name}({x})");
            e.reverse().expect("reverse");
            assert_eq!(e.snapshot().classical, start.classical, "{number}: {name}({x})");
        }
    }
    println!("480 native C results and complete reversals across three front modes");
}

/// CARGO_BUILD_JOBS=3 CMAKE_BUILD_PARALLEL_LEVEL=3 cargo test -p bennett-front-tests --test retaddr_independent musl -- --ignored --nocapture
#[test]
#[ignore = "builds musl with three jobs and runs the same native witnesses through its startup and stdio"]
fn musl_linked_return_addresses_match_the_same_native_program() {
    use bennett_front_tests::{out_dir, root, write_c};
    let workspace = root().canonicalize().expect("workspace");
    let text = std::fs::read_to_string(c_dir().join("retaddr_independent.c")).expect("source");
    let source = write_c("retaddr_independent_musl", &format!("#define NATIVE\n{text}"));
    let executable = native_build(&source, "retaddr_independent_musl", FRAMES);
    let native = Command::new(executable).output().expect("native");
    assert!(native.status.success());
    let build = Command::new("python3")
        .arg(workspace.join("rt/musl/build.py"))
        .args(["--jobs", "3"])
        .output()
        .expect("musl build");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    for opt in ["-O0", "-O2"] {
        let file = out_dir().join(format!("retaddr_independent_musl{opt}.bn"));
        let mut cmd = Command::new("python3");
        cmd.arg(workspace.join("rt/musl/link.py")).arg(&source).arg("-o").arg(&file);
        for flag in FRAMES.iter().copied().chain([opt, "-fno-vectorize", "-fno-slp-vectorize"]) {
            cmd.arg(format!("--clang-flag={flag}"));
        }
        let link = cmd.output().expect("musl link");
        assert!(link.status.success(), "{}", String::from_utf8_lossy(&link.stderr));
        let p = bennett_ir::load(&std::fs::read_to_string(file).expect("device text")).expect("device program");
        let mut e = Executor::new(&p, entry(&p, "__bennett_entry"), &[], options(0)).expect("start");
        assert_eq!(e.run(), Outcome::Returned(Some(0)), "musl {opt}");
        assert_eq!(e.tape(), native.stdout, "musl {opt}: all 160 output lines");
        println!("musl {opt}: 160 native return-address results; {} steps", e.stats().steps);
    }
}
