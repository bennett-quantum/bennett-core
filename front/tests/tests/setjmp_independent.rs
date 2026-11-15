//! Independent native oracles for defined locals, control expressions, copied/nested records and VLAs.
use std::collections::BTreeMap;
use std::process::Command;

use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, entry, lower, native_build, options, out_dir, root};

#[test]
fn directed_jumps_agree_at_both_native_optimization_levels_and_un_run() {
    let source = c_dir().join("setjmp_independent.c");
    let mut references = Vec::new();
    for opt in ["-O0", "-O2"] {
        let executable = native_build(&source, &format!("setjmp_independent{opt}"), &["-DNATIVE", opt]);
        let output = Command::new(executable).output().expect("runs natively");
        assert!(output.status.success());
        let rows: BTreeMap<(String, u64), u128> = String::from_utf8(output.stdout)
            .expect("native text")
            .lines()
            .map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                ((fields[0].to_owned(), fields[1].parse().expect("input")), fields[2].parse().expect("result"))
            })
            .collect();
        assert_eq!(rows.len(), 8 * 16);
        references.push(rows);
    }
    assert_eq!(references[0], references[1], "only defined C values are observed");
    let builds: [(&[&str], &[&str], &str); 4] = [
        (CLANG_FLAGS, &[], "o0"),
        (CLANG_O2_FLAGS, &[], "o2"),
        (&["-O0"], &[], "optnone"),
        (CLANG_FLAGS, &["--taint-all"], "taint"),
    ];
    for (clang, front, suffix) in builds {
        let module = lower(&source, &format!("setjmp_independent.{suffix}"), clang, front);
        for ((name, x), expected) in &references[0] {
            let mut exec =
                Executor::new(&module, entry(&module, name), &[*x], Options { reversible: true, ..options(0) })
                    .expect("starts");
            let start = exec.snapshot();
            assert_eq!(exec.run(), Outcome::Returned(Some(*expected)), "{name}({x}), {suffix}");
            let steps = exec.stats().steps;
            assert_eq!(exec.reverse().expect("un-runs"), steps);
            assert_eq!(exec.snapshot(), start, "{name}({x}), {suffix}: exact reversal");
        }
    }
}

// Requires rt/musl/build.py first. Keep the wrong-answer checks executable without making the fast
// suite depend on a separately built library. Both witnesses inspect only defined C/POSIX state.
fn saved_environment_witness(name: &str) {
    let source = c_dir().join(format!("{name}.c"));
    for level in ["-O0", "-O2"] {
        let tag = format!("{name}.{level}");
        let native = native_build(&source, &tag, &[level]);
        assert_eq!(Command::new(native).status().expect("runs natively").code(), Some(0));
        let program = out_dir().join(format!("{tag}.bn"));
        let built = Command::new("python3")
            .arg(root().join("rt/musl/link.py"))
            .arg(&source)
            .arg(format!("--clang-flag={level}"))
            .arg("-o")
            .arg(&program)
            .output()
            .expect("runs link.py");
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        let module = bennett_ir::load(&std::fs::read_to_string(program).expect("device text")).expect("valid");
        let mut exec = Executor::new(
            &module,
            entry(&module, "__bennett_entry"),
            &[],
            Options { budget: Some(100_000_000), ..options(0) },
        )
        .expect("starts");
        let actual = exec.run();
        eprintln!("{tag}: native 0, device {actual:?}, {} steps", exec.stats().steps);
        assert_eq!(actual, Outcome::Returned(Some(0)), "{tag}: the original mask must be restored");
    }
}

#[test]
#[ignore = "F1: revisiting sigsetjmp loses the copied environment's run-time savemask; needs musl"]
fn a_copied_environment_keeps_its_original_runtime_savemask() {
    saved_environment_witness("setjmp_saved_mask");
}

#[test]
#[ignore = "F2: revisiting sigsetjmp restores the last buffer's mask instead of the target's; needs musl"]
fn interleaved_environments_restore_the_target_buffers_mask() {
    saved_environment_witness("setjmp_saved_buffer");
}
