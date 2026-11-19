//! Independent native comparisons of aggregate results, including separate caller/callee compilation.

use std::path::{Path, PathBuf};
use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, STACK_BYTES, bitcode, c_dir, native_build_with, out_dir, root, run,
};
use bennett_ir::ValidModule;

const INPUTS: &[u64] =
    &[0, 1, 7, 0x7fff_ffff_ffff_ffff, 0x8000_0000_0000_0000, u64::MAX, 0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];

fn checked(command: &mut Command) -> String {
    let output = command.output().expect("run build tool or native oracle");
    assert!(output.status.success(), "{command:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("tool output is text")
}

fn clang() -> PathBuf {
    std::env::var_os("CLANG").map_or_else(|| PathBuf::from("clang"), PathBuf::from)
}

fn lower_bitcode(bitcode: &Path, name: &str, extra: &[&str]) -> ValidModule {
    let program = out_dir().join(format!("{name}.bn"));
    let front =
        std::env::var_os("BENNETT_FRONT").map_or_else(|| root().join("target/front/bennett-front"), PathBuf::from);
    checked(
        Command::new(front)
            .arg(bitcode)
            .arg("-o")
            .arg(&program)
            .arg(format!("--stack-bytes={STACK_BYTES}"))
            .arg("--region-budget=100000")
            .args(extra),
    );
    bennett_ir::load(&std::fs::read_to_string(program).expect("read device program")).expect("valid device program")
}

fn oracle(executable: &Path) -> Vec<u128> {
    checked(&mut Command::new(executable))
        .lines()
        .map(|line| u128::from_str_radix(line, 16).expect("native result word"))
        .collect()
}

#[test]
fn aggregate_shapes_and_call_kinds_agree_with_separately_compiled_native() {
    let source = c_dir().join("abi_independent.c");
    let levels = [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")];
    let mut native = None;
    for (callee_flags, callee_tag) in levels {
        let mut flags = callee_flags.to_vec();
        flags.push("-DABI_CALLEE");
        let callee = bitcode(&source, &format!("abi.callee.{callee_tag}"), &flags);
        let object = out_dir().join(format!("abi.callee.{callee_tag}.o"));
        checked(Command::new(clang()).args(&flags).arg("-c").arg(&source).arg("-o").arg(&object));
        for (caller_flags, caller_tag) in levels {
            let tag = format!("abi.{caller_tag}.{callee_tag}");
            let mut flags = caller_flags.to_vec();
            flags.push("-DABI_NATIVE");
            let executable = native_build_with(&source, &tag, &flags, &[object.to_str().expect("object path")]);
            let expected = oracle(&executable);
            assert_eq!(expected.len(), 33 * 4 * INPUTS.len());
            if let Some(previous) = &native {
                assert_eq!(&expected, previous, "native build combinations disagree");
            } else {
                native = Some(expected.clone());
            }
            let caller = bitcode(&source, &format!("abi.caller.{caller_tag}"), caller_flags);
            let linked = out_dir().join(format!("{tag}.bc"));
            checked(Command::new("llvm-link-18").arg(&caller).arg(&callee).arg("-o").arg(&linked));
            let module = lower_bitcode(&linked, &tag, &[]);
            assert_eq!(run(&module, "cases", &[], 0), Outcome::Returned(Some(33)));
            for (i, &expected) in expected.iter().enumerate() {
                let case = i / (4 * INPUTS.len());
                let mode = i / INPUTS.len() % 4;
                let input = INPUTS[i % INPUTS.len()];
                assert_eq!(
                    run(&module, "result", &[case as u64, input, mode as u64], 0),
                    Outcome::Returned(Some(expected)),
                    "{tag}: case {case}, mode {mode}, input {input:#x}"
                );
            }
            eprintln!("{tag}: {} exact 128-bit native/device result words", expected.len());
        }
    }
}

#[test]
fn exact_llvm_field_widths_padding_arrays_and_poison_match_native() {
    let source = c_dir().join("abi_independent.ll");
    let driver = c_dir().join("abi_independent_ir_driver.c");
    let mut previous = None;
    for (flags, tag) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let tag = format!("abi.ir.{tag}");
        let executable = native_build_with(&source, &tag, flags, &[driver.to_str().expect("driver path")]);
        let expected = oracle(&executable);
        assert_eq!(expected.len(), 12 * 2 * INPUTS.len());
        if let Some(previous) = &previous {
            assert_eq!(&expected, previous, "native IR optimisation levels disagree");
        } else {
            previous = Some(expected.clone());
        }
        let bc = bitcode(&source, &tag, flags);
        let module = lower_bitcode(&bc, &tag, &[]);
        for (i, &value) in expected.iter().enumerate() {
            let case = i / (2 * INPUTS.len());
            let mode = i / INPUTS.len() % 2;
            let input = INPUTS[i % INPUTS.len()];
            assert_eq!(
                run(&module, "result", &[case as u64, input, mode as u64], 0),
                Outcome::Returned(Some(value)),
                "{tag} case {case} mode {mode} input {input:#x}"
            );
        }
        eprintln!("{tag}: {} exact result words", expected.len());
    }
}

#[test]
fn accepted_aggregate_results_preserve_bits_and_phases_under_qubit_control() {
    use bennett_front_tests::{MAX_CONFIGURATIONS, branches, lower_text};
    let source = c_dir().join("abi_independent_controlled.c");
    let include = format!("-I{}", root().join("front/include").display());
    for shape in 0..8 {
        let define = format!("-DABI_SHAPE={shape}");
        let native = native_build_with(&source, &format!("abi.control.{shape}"), &["-DNATIVE", &include, &define], &[]);
        let text = checked(&mut Command::new(native));
        let expected: Vec<(u64, u64)> = text
            .lines()
            .map(|line| {
                let mut words = line.split_whitespace().map(|w| w.parse::<u64>().expect("native result"));
                (words.next().expect("y"), words.next().expect("phase"))
            })
            .collect();
        assert_eq!(expected.len(), 8);
        for (flags, extra, suffix) in [
            (CLANG_FLAGS, &[][..], "o0"),
            (CLANG_O2_FLAGS, &[][..], "o2"),
            (CLANG_FLAGS, &["--taint-all"][..], "taint"),
        ] {
            let tag = format!("abi.control.{shape}.{suffix}");
            let mut flags = flags.to_vec();
            flags.push(&define);
            let text = match lower_text(&source, &tag, &flags, extra) {
                Ok(text) => text,
                Err(message) => {
                    // At O2 clang splits the aggregates of shapes 3 and 4 (wider than 64 bits) into fields before
                    // the front sees them, so one call's value reaches both consumers through two values: no
                    // register of its own yet (docs/DESIGN.md, "Evaluations the front repeats or moves").
                    let wide = matches!(shape, 3 | 4) && suffix == "o2";
                    assert!(
                        (wide && message.contains("places, each computing it again"))
                            || (shape >= 6
                                && suffix == "o2"
                                && message.contains("a value that depends on a qubit, used by ordinary code")),
                        "unexpected refusal {tag}: {message}"
                    );
                    eprintln!("{tag}: named pointer/vector memory-result debt: {message}");
                    continue;
                }
            };
            let module = bennett_ir::load(&text).expect("valid controlled program");
            let (columns, state) = branches(&module, "ratio", &[], MAX_CONFIGURATIONS);
            assert_eq!(state.len(), 16, "{tag}");
            for (k, &(y, parity)) in expected.iter().enumerate() {
                for phase in 0..2 {
                    let (_, amplitude) = state
                        .iter()
                        .find(|(values, _)| {
                            columns.iter().zip(values).all(|(column, value)| {
                                *value
                                    == match column.as_str() {
                                        "q" => k as u64,
                                        "y" => y,
                                        "phase" => phase,
                                        _ => panic!("unexpected {column}"),
                                    }
                            })
                        })
                        .unwrap_or_else(|| panic!("{tag}: missing k={k} phase={phase}: {state:?}"));
                    let sign = if phase & parity != 0 { -1.0 } else { 1.0 };
                    assert!(
                        (amplitude.re - sign * 0.25).abs() < 1e-12 && amplitude.im.abs() < 1e-12,
                        "{tag}: {amplitude}"
                    );
                }
            }
            eprintln!("{tag}: 16 exact branches and phases");
        }
    }
}

/// Requires `python3 rt/musl/build.py --jobs 3`; kept separate from fast front tests.
#[test]
#[ignore = "needs the separately built musl bitcode library; run after rt/musl/build.py"]
fn musl_linked_aggregate_matrix_preserves_all_printed_words() {
    use bennett_exec::Executor;
    use bennett_front_tests::{entry, options};
    let source = c_dir().join("abi_independent.c");
    let levels = [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")];
    for (callee_flags, callee_tag) in levels {
        let mut flags = callee_flags.to_vec();
        flags.push("-DABI_CALLEE");
        let callee = bitcode(&source, &format!("abi.musl.callee.{callee_tag}"), &flags);
        let object = out_dir().join(format!("abi.musl.callee.{callee_tag}.o"));
        checked(Command::new(clang()).args(&flags).arg("-c").arg(&source).arg("-o").arg(&object));
        for (caller_flags, caller_tag) in levels {
            let tag = format!("abi.musl.{caller_tag}.{callee_tag}");
            let mut flags = caller_flags.to_vec();
            flags.push("-DABI_NATIVE");
            let native = native_build_with(&source, &tag, &flags, &[object.to_str().expect("object path")]);
            let expected = checked(&mut Command::new(native));
            assert_eq!(expected.lines().count(), 1056);
            let caller = bitcode(&source, &format!("{tag}.caller"), &flags);
            let linked = out_dir().join(format!("{tag}.bc"));
            checked(Command::new("llvm-link-18").arg(&caller).arg(&callee).arg("-o").arg(&linked));
            // link.py accepts LLVM bitcode as its source too. Preserve separate optimisation levels,
            // then resolve printf and run musl's own startup/exit exactly as for a C source.
            let program = out_dir().join(format!("{tag}.bn"));
            checked(Command::new("python3").arg(root().join("rt/musl/link.py")).arg(&linked).arg("-o").arg(&program));
            let module = bennett_ir::load(&std::fs::read_to_string(&program).expect("read musl program"))
                .expect("valid musl program");
            let mut executor = Executor::new(
                &module,
                entry(&module, "__bennett_entry"),
                &[],
                bennett_exec::Options { budget: Some(100_000_000), ..options(0) },
            )
            .expect("start musl");
            assert_eq!(executor.run(), Outcome::Returned(Some(0)), "{tag}");
            assert_eq!(executor.tape(), expected.as_bytes(), "{tag}");
            eprintln!(
                "{tag}: 1056 identical printed u128 words ({} bytes), {} steps",
                expected.len(),
                executor.stats().steps
            );
        }
    }
}

#[test]
#[ignore = "ABI-D1: packed 129-bit C bitfields are refused after LLVM merges their copy into i136"]
fn packed_129_bit_c_return_agrees_with_native() {
    use bennett_front_tests::lower;
    let source = c_dir().join("abi_packed129.c");
    for (flags, level) in [(CLANG_FLAGS, "-O0"), (CLANG_O2_FLAGS, "-O2")] {
        let tag = format!("abi.packed129.{level}");
        let native = native_build_with(&source, &tag, &[level, "-DNATIVE"], &[]);
        let expected = oracle(&native);
        assert_eq!(expected.len(), INPUTS.len());
        let module = lower(&source, &tag, flags, &[]);
        for (&input, &value) in INPUTS.iter().zip(&expected) {
            assert_eq!(run(&module, "result", &[input], 0), Outcome::Returned(Some(value)));
        }
    }
}
