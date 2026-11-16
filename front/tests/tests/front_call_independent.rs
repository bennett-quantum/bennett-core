//! Independent executed native oracles for by-value calls and over-aligned variadics.
use std::path::Path;
use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, lower, native_build_with, out_dir, root, run};

const INPUTS: &[u64] =
    &[0, 1, 7, 0x7fff_ffff_ffff_ffff, 0x8000_0000_0000_0000, u64::MAX, 0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];

fn checked(command: &mut Command) -> String {
    let output = command.output().expect("run tool");
    assert!(output.status.success(), "{command:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("text output")
}

fn oracle(source: &Path, tag: &str, flags: &[&str]) -> String {
    let mut flags = flags.to_vec();
    flags.push("-DNATIVE");
    let exe = native_build_with(source, tag, &flags, &[]);
    checked(&mut Command::new(exe))
}

fn compare(source: &str, shapes: usize, modes: usize, isa: &[&str], tag: &str) {
    let source = c_dir().join(source);
    // The CPU need not implement AVX-512: its variadic ABI is exercised on the
    // device against the same C executed natively without AVX-512 instructions.
    let mut native0 = CLANG_FLAGS.to_vec();
    let mut native2 = CLANG_O2_FLAGS.to_vec();
    for &define in isa.iter().filter(|flag| flag.starts_with("-D")) {
        native0.push(define);
        native2.push(define);
    }
    let expected = oracle(&source, &format!("{tag}.native0"), &native0);
    assert_eq!(expected, oracle(&source, &format!("{tag}.native2"), &native2));
    let words: Vec<u128> = expected.lines().map(|s| u128::from_str_radix(s, 16).expect("hex word")).collect();
    assert_eq!(words.len(), shapes * modes * INPUTS.len());
    if isa == ["-mavx2"] && std::is_x86_feature_detected!("avx2") {
        let mut flags = CLANG_O2_FLAGS.to_vec();
        flags.extend_from_slice(isa);
        assert_eq!(expected, oracle(&source, &format!("{tag}.native_avx2"), &flags));
    }
    for (flags, extra, suffix) in
        [(CLANG_FLAGS, &[][..], "o0"), (CLANG_O2_FLAGS, &[][..], "o2"), (CLANG_FLAGS, &["--taint-all"][..], "taint")]
    {
        let mut flags = flags.to_vec();
        flags.extend_from_slice(isa);
        let mut extra = extra.to_vec();
        extra.push("--stack-bytes=1048576");
        let tag = format!("{tag}.{suffix}");
        let module = lower(&source, &tag, &flags, &extra);
        for (i, &expected) in words.iter().enumerate() {
            let shape = i / (modes * INPUTS.len());
            let mode = i / INPUTS.len() % modes;
            assert_eq!(
                run(&module, "result", &[shape as u64, INPUTS[i % INPUTS.len()], mode as u64], 0),
                Outcome::Returned(Some(expected)),
                "{tag}: shape {shape}, mode {mode}, input {}",
                i % INPUTS.len()
            );
        }
        eprintln!("{tag}: {} identical native/device words", words.len());
    }
}

#[test]
fn by_value_size_alignment_and_call_paths_match_native() {
    compare("front_call_independent.c", 27, 5, &[], "front.call");
}

#[test]
fn over_aligned_varargs_copied_and_forwarded_match_native() {
    for (isa, tag) in [(&[][..], "base"), (&["-mavx2"][..], "avx2"), (&["-mavx512f"][..], "avx512")] {
        compare("front_vararg_independent.c", 10, 1, isa, &format!("front.vararg.{tag}"));
    }
}

#[test]
#[ignore = "needs the separately built musl bitcode library; run after rt/musl/build.py --jobs 3"]
fn musl_linked_vararg_matrix_matches_native_stdout() {
    use bennett_exec::Executor;
    use bennett_front_tests::{entry, options};
    let source = c_dir().join("front_vararg_independent.c");
    let expected = oracle(&source, "front.vararg.musl.native", CLANG_O2_FLAGS);
    for (isa, tag) in [("", "base"), ("-mavx2", "avx2"), ("-mavx512f", "avx512")] {
        for level in ["-O0", "-O2"] {
            let program = out_dir().join(format!("front.vararg.musl.{tag}.{level}.bn"));
            let mut command = Command::new("python3");
            command
                .arg(root().join("rt/musl/link.py"))
                .arg(&source)
                .arg("-o")
                .arg(&program)
                .args(["--clang-flag=-DNATIVE", "--clang-flag=-fno-vectorize", "--clang-flag=-fno-slp-vectorize"])
                .arg(format!("--clang-flag={level}"));
            if !isa.is_empty() {
                command.arg(format!("--clang-flag={isa}"));
            }
            checked(&mut command);
            let module = bennett_ir::load(&std::fs::read_to_string(&program).expect("program")).expect("valid module");
            let mut executor = Executor::new(
                &module,
                entry(&module, "__bennett_entry"),
                &[],
                bennett_exec::Options { budget: Some(100_000_000), ..options(0) },
            )
            .expect("executor");
            assert_eq!(executor.run(), Outcome::Returned(Some(0)), "{tag} {level}");
            assert_eq!(executor.tape(), expected.as_bytes(), "{tag} {level}");
            eprintln!("musl {tag} {level}: 80 identical words, {} steps", executor.stats().steps);
        }
    }
}

#[test]
fn by_value_calls_under_qubit_control_match_native_where_accepted() {
    use bennett_front_tests::{MAX_CONFIGURATIONS, branches};
    let source = c_dir().join("front_call_controlled.c");
    for shape in 0..4 {
        let define = format!("-DSHAPE={shape}");
        let expected: Vec<u64> = oracle(&source, &format!("front.control.{shape}.native"), &[&define])
            .lines()
            .map(|v| v.parse().expect("native word"))
            .collect();
        for (flags, extra, level) in [
            (CLANG_FLAGS, &[][..], "o0"),
            (CLANG_O2_FLAGS, &[][..], "o2"),
            (CLANG_FLAGS, &["--taint-all"][..], "taint"),
        ] {
            let mut flags = flags.to_vec();
            flags.push(&define);
            let tag = format!("front.control.{shape}.{level}");
            let module = lower(&source, &tag, &flags, extra);
            let (columns, state) = branches(&module, "ratio", &[], MAX_CONFIGURATIONS);
            assert_eq!(state.len(), 8, "{tag}");
            for (values, amp) in state {
                let q = values[columns.iter().position(|s| s == "q").expect("q")];
                let y = values[columns.iter().position(|s| s == "y").expect("y")];
                assert_eq!(y, expected[q as usize], "{tag}: q={q}");
                assert!((amp.re - 1.0 / 8.0_f64.sqrt()).abs() < 1e-12 && amp.im.abs() < 1e-12, "{tag}: {amp}");
            }
            eprintln!("{tag}: 8 exact branches and amplitudes");
        }
    }
}

#[test]
fn musttail_by_value_is_refused_by_name() {
    use bennett_front_tests::{lower_text, write_c};
    let source = write_c(
        "front.musttail",
        "struct S { long x[8]; };\nextern long sink(struct S);\nlong f(struct S s) { __attribute__((musttail)) return sink(s); }\n",
    );
    let error = lower_text(&source, "front.musttail", CLANG_FLAGS, &[]).expect_err("musttail is refused");
    assert!(error.contains("musttail") && error.contains("passed by value"), "{error}");
}

#[test]
#[ignore = "needs the separately built musl bitcode library; run after rt/musl/build.py --jobs 3"]
fn musl_vprintf_after_aligned_arguments_matches_native_stdout() {
    use bennett_exec::Executor;
    use bennett_front_tests::{entry, options};
    let source = c_dir().join("front_vararg_printf.c");
    let expected = oracle(&source, "front.printf.native", CLANG_O2_FLAGS);
    for (isa, tag) in [("", "base"), ("-mavx2", "avx2"), ("-mavx512f", "avx512")] {
        for level in ["-O0", "-O2"] {
            let program = out_dir().join(format!("front.printf.{tag}.{level}.bn"));
            let keep = out_dir().join(format!("front.printf.{tag}.{level}.keep"));
            let mut command = Command::new("python3");
            command
                .arg(root().join("rt/musl/link.py"))
                .arg(&source)
                .arg("-o")
                .arg(&program)
                .arg("--keep")
                .arg(&keep)
                .arg(format!("--clang-flag={level}"))
                .args(["--clang-flag=-fno-vectorize", "--clang-flag=-fno-slp-vectorize"]);
            if !isa.is_empty() {
                command.arg(format!("--clang-flag={isa}"));
            }
            checked(&mut command);
            for taint in [false, true] {
                if taint {
                    checked(
                        Command::new(root().join("target/front/bennett-front"))
                            .arg(keep.join("linked.bc"))
                            .arg("-o")
                            .arg(&program)
                            .args(["--taint-all", "--stack-bytes=1048576", "--region-budget=100000"]),
                    );
                }
                let module =
                    bennett_ir::load(&std::fs::read_to_string(&program).expect("program")).expect("valid module");
                let mut executor =
                    Executor::new(&module, entry(&module, "__bennett_entry"), &[], options(0)).expect("executor");
                assert_eq!(executor.run(), Outcome::Returned(Some(0)), "{tag} {level} taint={taint}");
                assert_eq!(executor.tape(), expected.as_bytes(), "{tag} {level} taint={taint}");
                eprintln!(
                    "musl vprintf {tag} {level} taint={taint}: identical stdout, {} steps",
                    executor.stats().steps
                );
            }
        }
    }
}

#[test]
fn maximum_valid_variadic_alignment_matches_native() {
    compare("front_vararg_independent.c", 10, 1, &["-DPAGE_ALIGN=16384"], "front.vararg.max_align");
}
