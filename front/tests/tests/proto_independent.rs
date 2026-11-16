//! Independent executed native oracles for the prototype-mismatch lowering.
use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, lower, native_build_with, run};

const INPUTS: &[u64] =
    &[0, 1, 7, 0x7fff_ffff_ffff_ffff, 0x8000_0000_0000_0000, u64::MAX, 0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];
const SHAPES: usize = 24;
const MODES: usize = 5;

#[test]
fn independent_register_memory_result_and_call_paths_match_executed_native() {
    let source = c_dir().join("proto_independent.c");
    let native = |level: &str| {
        let exe =
            native_build_with(&source, &format!("proto.independent.native.{level}"), &[level, "-DNATIVE", "-w"], &[]);
        let output = Command::new(exe).output().expect("execute native oracle");
        assert!(output.status.success());
        String::from_utf8(output.stdout).expect("hex words")
    };
    let reference = native("-O0");
    let optimised = native("-O2");
    for (i, (a, b)) in reference.lines().zip(optimised.lines()).enumerate() {
        assert_eq!(
            a,
            b,
            "native O0/O2: shape {}, mode {}, input {}",
            i / (MODES * INPUTS.len()),
            i / INPUTS.len() % MODES,
            i % INPUTS.len()
        );
    }
    assert_eq!(reference.lines().count(), optimised.lines().count());
    let words: Vec<u64> = reference.lines().map(|s| u64::from_str_radix(s, 16).expect("hex word")).collect();
    assert_eq!(words.len(), SHAPES * MODES * INPUTS.len());
    // Correctly typed calls are independent controls with the same inputs and outputs.
    for shape in words.as_chunks::<{ MODES * INPUTS.len() }>().0 {
        for mode in shape.as_chunks::<{ INPUTS.len() }>().0 {
            assert_eq!(mode, &shape[(MODES - 1) * INPUTS.len()..]);
        }
    }
    for (flags, extra, suffix) in
        [(CLANG_FLAGS, &[][..], "o0"), (CLANG_O2_FLAGS, &[][..], "o2"), (CLANG_FLAGS, &["--taint-all"][..], "taint")]
    {
        let mut flags = flags.to_vec();
        flags.push("-w");
        let tag = format!("proto.independent.{suffix}");
        let module = lower(&source, &tag, &flags, extra);
        for (i, &expected) in words.iter().enumerate() {
            let shape = i / (MODES * INPUTS.len());
            let mode = i / INPUTS.len() % MODES;
            assert_eq!(
                run(&module, "result", &[shape as u64, INPUTS[i % INPUTS.len()], mode as u64], 0),
                Outcome::Returned(Some(u128::from(expected))),
                "{tag}: shape {shape}, mode {mode}, input {}",
                i % INPUTS.len()
            );
        }
        eprintln!(
            "{tag}: {} exact native/device words ({} correctly typed controls)",
            words.len(),
            SHAPES * INPUTS.len()
        );
    }
}

#[test]
fn mismatched_calls_preserve_native_values_and_amplitudes_under_qubit_control() {
    use bennett_front_tests::{MAX_CONFIGURATIONS, branches};
    let source = c_dir().join("proto_controlled_independent.c");
    for shape in 0..4 {
        let define = format!("-DSHAPE={shape}");
        let native = |level: &str| {
            let exe = native_build_with(
                &source,
                &format!("proto.control.{shape}.native.{level}"),
                &[level, "-DNATIVE", "-w", &define],
                &[],
            );
            let output = Command::new(exe).output().expect("execute native basis oracle");
            assert!(output.status.success());
            String::from_utf8(output.stdout).expect("decimal words")
        };
        let expected = native("-O0");
        assert_eq!(expected, native("-O2"));
        let expected: Vec<u64> = expected.lines().map(|s| s.parse().expect("native word")).collect();
        assert_eq!(expected.len(), 8);
        for (flags, extra, suffix) in [
            (CLANG_FLAGS, &[][..], "o0"),
            (CLANG_O2_FLAGS, &[][..], "o2"),
            (CLANG_FLAGS, &["--taint-all"][..], "taint"),
        ] {
            let mut flags = flags.to_vec();
            flags.extend_from_slice(&["-w", &define]);
            let tag = format!("proto.control.{shape}.{suffix}");
            let text = match bennett_front_tests::lower_text(&source, &tag, &flags, extra) {
                Ok(text) => text,
                Err(error) if shape == 3 => {
                    assert!(
                        error.contains("a call that depends on a qubit") && error.contains("writes memory"),
                        "{tag}: {error}"
                    );
                    continue;
                }
                Err(error) => panic!("{tag}: {error}"),
            };
            let module = bennett_ir::load(&text).expect("valid program");
            let (columns, state) = branches(&module, "ratio", &[], MAX_CONFIGURATIONS);
            assert_eq!(state.len(), 8, "{tag}");
            for (values, amplitude) in state {
                let q = values[columns.iter().position(|s| s == "q").expect("q")];
                let y = values[columns.iter().position(|s| s == "y").expect("y")];
                assert_eq!(y, expected[q as usize], "{tag}: q={q}");
                let wanted = if q & 1 == 0 { 0.3_f64.cos() } else { 0.3_f64.sin() } / 2.0;
                assert!(
                    (amplitude.re - wanted).abs() < 1e-12 && amplitude.im.abs() < 1e-12,
                    "{tag}: q={q}: {amplitude}"
                );
            }
            eprintln!("{tag}: 8 matching native branches and biased amplitudes");
        }
    }
}
