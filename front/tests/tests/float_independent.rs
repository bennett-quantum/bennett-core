//! Independent executed witnesses for the floating-point runtime and front-end legalisation.

use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, lower, native_build_with, run};

fn compare(name: &str, extra: &[&str]) {
    let source = c_dir().join(format!("{name}.c"));
    let tag = format!("{name}{}", extra.join("_").replace(['=', '-'], ""));
    let mut expected = Vec::new();
    for level in ["-O0", "-O2"] {
        let mut native_flags = vec![level, "-DNATIVE", "-ffp-contract=off", "-fno-math-errno"];
        native_flags.extend(extra);
        let executable = native_build_with(&source, &format!("{tag}{level}"), &native_flags, &["-lm"]);
        let output = Command::new(executable).output().expect("run native oracle");
        assert!(output.status.success());
        let values: Vec<u128> = String::from_utf8(output.stdout)
            .expect("native output is text")
            .lines()
            .map(|line| u128::from_str_radix(line, 16).expect("native result bits"))
            .collect();
        if expected.is_empty() {
            expected = values;
        } else {
            let differences: Vec<_> =
                expected.iter().zip(&values).enumerate().filter(|(_, (a, b))| a != b).take(12).collect();
            assert!(differences.is_empty(), "native optimisation levels disagree: {differences:?}");
        }
    }
    for (flags, suffix) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let mut flags = flags.to_vec();
        flags.extend(["-ffp-contract=off", "-fno-math-errno"]);
        flags.extend(extra);
        let module = lower(&source, &format!("{tag}.{suffix}"), &flags, &[]);
        assert_eq!(run(&module, "results", &[], 0), Outcome::Returned(Some(expected.len() as u128)));
        let mismatches: Vec<_> = expected
            .iter()
            .enumerate()
            .filter_map(|(i, &value)| {
                let actual = run(&module, "result", &[i as u64], 0);
                (actual != Outcome::Returned(Some(value)))
                    .then(|| format!("result({i}): native {value:#x}, device {actual:?}"))
            })
            .collect();
        assert!(
            mismatches.is_empty(),
            "{name} {suffix}: {} mismatches:\n{}",
            mismatches.len(),
            mismatches.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
        );
        eprintln!("{name} {suffix}: {} bit-exact results", expected.len());
    }
}

#[test]
fn short_format_chains_preserve_native_expression_precision() {
    compare("float_independent_chains", &[]);
}

#[test]
fn integer_widths_and_boundary_neighbours_match_native() {
    for width in [8, 33, 65, 128] {
        compare("float_independent_widths", &[&format!("-DTEST_WIDTH={width}")]);
    }
}

/// Run separately: each width goes through its own clang/front pipeline to avoid a giant switch.
#[test]
#[ignore = "extended sweep: 22 widths, two native and two device compilations each"]
fn extended_integer_width_sweep() {
    for width in [2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 65, 66, 79, 80, 81, 99, 100, 101, 126, 127, 128] {
        compare("float_independent_widths", &[&format!("-DTEST_WIDTH={width}")]);
    }
}

#[test]
fn correlated_fmal_and_integer_conversion_corners_match_native_runtime() {
    use bennett_front_tests::{float_runtime, out_dir};
    let dir = out_dir().join("float-independent-runtime");
    std::fs::create_dir_all(&dir).expect("create native runtime directory");
    let clang = std::env::var_os("CLANG").unwrap_or_else(|| "clang".into());
    let object = dir.join("runtime.o");
    let prefixed = dir.join("runtime-prefixed.o");
    let executable = dir.join("directed");
    let check = |command: &mut Command| {
        let output = command.output().expect("execute native build tool");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    };
    check(Command::new(&clang).args(["-O2", "-c"]).arg(float_runtime()).arg("-o").arg(&object));
    check(
        Command::new(std::env::var_os("OBJCOPY").unwrap_or_else(|| "objcopy".into()))
            .arg("--prefix-symbols=rt_")
            .arg(object)
            .arg(&prefixed),
    );
    check(
        Command::new(clang)
            .args(["-O2", "-ffp-contract=off", "-fno-fast-math"])
            .arg(c_dir().join("float_independent_fmal.c"))
            .arg(prefixed)
            .args(["-lm", "-o"])
            .arg(&executable),
    );
    let output = Command::new(executable).output().expect("run directed native comparison");
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("123368 checked, 0 mismatches"), "{report}");
    assert!(report.contains("17676 checked"), "{report}");
}

#[test]
fn cancellation_and_rounding_boundaries_compute_under_qubit_control() {
    use bennett_front_tests::{MAX_CONFIGURATIONS, branches, root};
    let source = c_dir().join("float_independent_controlled.c");
    let include = format!("-I{}", root().join("front/include").display());
    let native = native_build_with(&source, "float_independent_controlled", &["-DNATIVE", &include], &["-lm"]);
    let output = Command::new(native).output().expect("run controlled native oracle");
    assert!(output.status.success());
    let expected: Vec<Vec<u64>> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split_whitespace().map(|v| v.parse().expect("native bits")).collect())
        .collect();
    assert_eq!(expected.len(), 16);
    for (flags, extra, suffix) in
        [(CLANG_FLAGS, &[][..], "o0"), (CLANG_O2_FLAGS, &[][..], "o2"), (CLANG_FLAGS, &["--taint-all"][..], "taint")]
    {
        let module = lower(&source, &format!("float_independent_controlled.{suffix}"), flags, extra);
        let (columns, state) = branches(&module, "ratio", &[], MAX_CONFIGURATIONS);
        assert_eq!(state.len(), 16);
        for (k, values) in expected.iter().enumerate() {
            let q = (0..4).map(|i| (k as u64 >> i & 1) << (8 * i)).sum();
            let wanted = [("q", q), ("f", values[0]), ("w", values[1]), ("s", values[2])];
            let (_, amplitude) = state
                .iter()
                .find(|(actual, _)| {
                    columns.iter().zip(actual).all(|(column, value)| {
                        *value == wanted.iter().find(|(name, _)| column == name).map_or(0, |(_, value)| *value)
                    })
                })
                .unwrap_or_else(|| panic!("{suffix}: missing branch {k}: {state:?}"));
            assert!((amplitude.re - 0.25).abs() < 1e-12 && amplitude.im.abs() < 1e-12);
        }
    }
}

/// The same program runs with a 64 KiB device stack before a9ce585 and faults after it.
#[test]
#[ignore = "FT-1: aggregate-return inlining makes three conversions overflow a 64 KiB stack"]
fn three_long_double_integer_conversions_fit_the_pre_inlining_stack() {
    compare("float_inline_stack", &[]);
}
