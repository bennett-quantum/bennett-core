//! The complex multiply and divide helpers (`__mulsc3`, `__divsc3`, `__muldc3`, `__divdc3`, `__mulxc3`,
//! `__divxc3`): libgcc's sources (`rt/libgcc`, vendored unmodified), linked into the program by
//! `bennett-front` when it calls them (`front/src/Complex.h`) and lowered with its floating point.
//!
//! The reference is the host's default libgcc, exactly as in the C corpus. Raw NaN bits are measured
//! separately by `rt/libgcc/exact`; these fast grouped checks normalize NaNs as the float contract permits.

#![cfg(target_arch = "x86_64")]

use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, bitcode, c_dir, lower, lower_text, native_build_with, run, write_c,
};

/// What `complex_helpers.c` prints natively at `-O0` and at `-O2`, which must agree.
fn native_hashes() -> Vec<u64> {
    let source = c_dir().join("complex_helpers.c");
    let mut outputs = Vec::new();
    for level in ["-O0", "-O2"] {
        let flags = ["-DNATIVE", level];
        let executable = native_build_with(&source, &format!("complex_helpers{level}"), &flags, &[]);
        let output = Command::new(&executable).output().unwrap_or_else(|e| panic!("cannot run {executable:?}: {e}"));
        assert!(output.status.success(), "native complex_helpers {level} failed");
        let values: Vec<u64> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.parse().unwrap_or_else(|e| panic!("{line:?}: {e}")))
            .collect();
        outputs.push(values);
    }
    assert_eq!(outputs[0], outputs[1], "complex_helpers: the native build at -O0 and at -O2");
    outputs.swap_remove(0)
}

/// Every group of results of the helpers, called directly and through `*` and `/`, on the cross product of
/// directed values (zeros, infinities, NaNs, the smallest subnormal, the largest finite value), agrees with
/// the natively compiled helpers, NaNs counted as one value.
#[test]
fn complex_products_and_quotients_agree_with_libgcc_on_annex_g_cases() {
    let expected = native_hashes();
    let source = c_dir().join("complex_helpers.c");
    for (flags, suffix) in [(CLANG_FLAGS, ""), (CLANG_O2_FLAGS, ".o2")] {
        let module = lower(&source, &format!("complex_helpers{suffix}"), flags, &[]);
        let context = format!("complex_helpers {flags:?}");
        assert_eq!(run(&module, "results", &[], 0), Outcome::Returned(Some(expected.len() as u128)), "{context}");
        let differ: Vec<String> = (0..expected.len())
            .filter_map(|i| match run(&module, "result", &[i as u64], 0) {
                Outcome::Returned(Some(value)) if value == u128::from(expected[i]) => None,
                other => Some(format!("group {i} (routine {}, a {}, b {}): {other:?}", i / 64, i % 64 / 8, i % 8)),
            })
            .collect();
        assert!(differ.is_empty(), "{context}: {} of {} differ:\n{}", differ.len(), expected.len(), differ.join("\n"));
    }
}

/// All six libgcc helpers are self-contained, including long-double division. A program defining a
/// helper keeps its own; a program calling none links none.
#[test]
fn the_helpers_are_linked_only_where_the_program_calls_them_without_defining_them() {
    let source = write_c(
        "complex_long_double_divide",
        "volatile long double a = 1, b = 2, c = 3, d = 4;\n\
         int main(void) {\n  long double _Complex x, y;\n  __real__ x = a; __imag__ x = b;\n  \
         __real__ y = c; __imag__ y = d;\n  return (int)(100 * __real__ (x / y));\n}\n",
    );
    let module = lower(&source, "complex_long_double_divide", CLANG_FLAGS, &[]);
    assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(44)));

    let source = write_c(
        "complex_own_mulsc3",
        "float _Complex __mulsc3(float a, float b, float c, float d) {\n  (void)a; (void)b; (void)c; (void)d;\n  \
         float _Complex z;\n  __real__ z = 4; __imag__ z = 2;\n  return z;\n}\n\
         int main(void) { float _Complex z = __mulsc3(1, 1, 1, 1); return (int)(__real__ z * 10 + __imag__ z); }\n",
    );
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = lower(&source, "complex_own_mulsc3", flags, &[]);
        assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(42)), "{flags:?}");
    }

    let source =
        write_c("complex_none", "volatile double a = 1, b = 2;\nint main(void) { return (int)(a * b + a / b); }\n");
    let text = lower_text(&source, "complex_none", CLANG_FLAGS, &[]).unwrap_or_else(|e| panic!("{e}"));
    assert!(!text.contains("c3"), "{text}");
}

/// The helpers come from a bitcode file chosen at link time (`--complex-runtime`): one whose `__muldc3`
/// gives 7 + 0i is used (the program returns the real part plus 10); a helper it does not define, one it defines with another type, and a file that
/// cannot be read are refused by name.
#[test]
fn the_complex_runtime_is_replaceable_at_link_time() {
    let runtime = write_c(
        "complex_other_runtime",
        "double _Complex __muldc3(double a, double b, double c, double d) {\n  (void)a; (void)b; (void)c; (void)d;\n  \
         double _Complex z;\n  __real__ z = 7; __imag__ z = 0;\n  return z;\n}\n\
         float __divdc3(float a) { return a; }\n",
    );
    let runtime = bitcode(&runtime, "complex_other_runtime", &["-O2"]);
    let option = format!("--complex-runtime={}", runtime.display());
    let program = |operation: &str| {
        format!(
            "double _Complex __{operation}dc3(double, double, double, double);\n\
             volatile double a = 1, b = 2, c = 3, d = 4;\n\
             int main(void) {{ return (int)(__real__ __{operation}dc3(a, b, c, d) + 10); }}\n"
        )
    };
    let source = write_c("complex_with_other_runtime", &program("mul"));
    let module = lower(&source, "complex_with_other_runtime", CLANG_FLAGS, &[&option]);
    assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(17)));
    let module = lower(&source, "complex_with_libgcc", CLANG_FLAGS, &[]);
    assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(5)), "1*3 - 2*4 + 10");
    let source = write_c("complex_wrong_type", &program("div"));
    let message = lower_text(&source, "complex_wrong_type", CLANG_FLAGS, &[&option]).expect_err("accepted");
    assert!(message.contains("which the complex-arithmetic runtime") && message.contains("float (float)"), "{message}");
    let source = write_c(
        "complex_missing_helper",
        "volatile float a = 1;\nfloat _Complex __mulsc3(float, float, float, float);\n\
         int main(void) { return (int)__real__ __mulsc3(a, a, a, a); }\n",
    );
    let message = lower_text(&source, "complex_missing_helper", CLANG_FLAGS, &[&option]).expect_err("accepted");
    assert!(
        message.contains("@__mulsc3, which the complex-arithmetic runtime") && message.contains("does not define"),
        "{message}"
    );
    let missing = format!("--complex-runtime={}", runtime.with_extension("missing").display());
    let message = lower_text(&source, "complex_no_runtime", CLANG_FLAGS, &[&missing]).expect_err("accepted");
    assert!(message.contains("cannot read the complex-arithmetic runtime"), "{message}");
}

/// Runs a script of `rt/libgcc/exact` and returns its standard output, failing with all of its output.
fn libgcc_script(script: &str, args: &[&str]) -> String {
    let path = bennett_front_tests::root().join("rt/libgcc/exact").join(script);
    let output = Command::new("python3")
        .arg("-I")
        .arg(&path)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {path:?}: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{script} failed:\n{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    stdout
}

/// The native reference is the vendored source: the six members the default clang link takes from the host's
/// libgcc.a are what the host's GCC 13.3.0 builds from `rt/libgcc` with Bennett's `config/` alone
/// (instructions, relocations and constants identical; `rt/libgcc/VENDOR.md`).
#[test]
fn the_native_reference_archive_is_the_vendored_libgcc_source() {
    let out = bennett_front_tests::out_dir().join("libgcc-archive");
    let report = libgcc_script("archive.py", &["--out", out.to_str().unwrap()]);
    assert_eq!(report.matches("identical to the vendored source").count(), 6, "{report}");
}

/// Every float/double result in which the device differs from the GCC archive is an operand order: the
/// production helper IR with ONE fixed operand order per `fadd`/`fmul` (as a compiled program has one)
/// reproduces the archive's raw bits, NaNs included, on all 422,144 tuples per helper of the exact sweep;
/// in IR operand order it is the device bit for bit (`rt/libgcc/exact/diagnose.py`). Native only.
#[test]
fn the_archive_nan_choices_are_operand_orders_of_the_production_ir() {
    let out = bennett_front_tests::out_dir().join("libgcc-commute");
    let report = libgcc_script("commute.py", &["--random", "262144", "--out", out.to_str().unwrap()]);
    let differing: Vec<u64> = report
        .lines()
        .map(|line| {
            assert!(line.ends_with("leaves 0 differing"), "{report}");
            line.split(", ").nth(1).and_then(|s| s.split(' ').next()).and_then(|n| n.parse().ok()).expect(line)
        })
        .collect();
    // The NaN-only counts of rt/libgcc/README.md: mulsc3, divsc3, muldc3, divdc3.
    assert_eq!(differing, [13_744, 4_470, 13_597, 19_141], "{report}");
}

/// Witness of a debt: noncanonical x87 operands (an unnormal, an unnormal zero, a pseudo-denormal, a
/// pseudo-infinity, a pseudo-NaN, written through raw memory) through `__mulxc3`/`__divxc3` differ from the
/// hardware in non-NaN ways (`complex_noncanonical_x87.c`). Measured over 800 tuples per class and helper,
/// one noncanonical operand among canonical ones: unnormals, unnormal zeros and pseudo-infinities differ in
/// most; pseudo-denormals agree in every product and differ in 12 of 800 quotients by an ulp; pseudo-NaNs
/// differ in NaN bits and in 4 quotients in a zero's sign. Not a libgcc question: the route is x87-compatible
/// validation and normalization of long-double operand encodings at the operation boundary of the
/// floating-point lowering (`rt/libgcc/README.md`).
#[test]
#[ignore = "debt: noncanonical x87 encodings are not validated by the long-double lowering (rt/libgcc/README.md)"]
fn noncanonical_x87_operands_agree_with_the_hardware() {
    let source = c_dir().join("complex_noncanonical_x87.c");
    let executable = native_build_with(&source, "complex_noncanonical_x87", &["-DNATIVE"], &[]);
    let output = Command::new(&executable).output().unwrap_or_else(|e| panic!("cannot run {executable:?}: {e}"));
    assert!(output.status.success(), "native complex_noncanonical_x87 failed");
    let expected: Vec<u64> =
        String::from_utf8_lossy(&output.stdout).lines().map(|line| line.parse().expect(line)).collect();
    for (flags, suffix) in [(CLANG_FLAGS, ""), (CLANG_O2_FLAGS, ".o2")] {
        let module = lower(&source, &format!("complex_noncanonical_x87{suffix}"), flags, &[]);
        assert_eq!(run(&module, "results", &[], 0), Outcome::Returned(Some(expected.len() as u128)));
        let differ: Vec<String> = (0..expected.len())
            .filter_map(|i| match run(&module, "result", &[i as u64], 0) {
                Outcome::Returned(Some(value)) if value == u128::from(expected[i]) => None,
                other => Some(format!("witness {} word {}: native {:#x}, device {other:?}", i / 4, i % 4, expected[i])),
            })
            .collect();
        assert!(
            differ.is_empty(),
            "{flags:?}: {} of {} words differ:\n{}",
            differ.len(),
            expected.len(),
            differ.join("\n")
        );
    }
}

/// Whether `hex` (8, 16 or 20 digits: float, double, x87 sign-exponent then significand) encodes a NaN.
fn is_nan_bits(hex: &str) -> bool {
    let v = u128::from_str_radix(hex, 16).unwrap_or_else(|e| panic!("{hex:?}: {e}"));
    match hex.len() {
        8 => v & 0x7fff_ffff > 0x7f80_0000,
        16 => v & 0x7fff_ffff_ffff_ffff > 0x7ff0_0000_0000_0000,
        20 => (v >> 64) & 0x7fff == 0x7fff && v & ((1 << 63) - 1) != 0,
        _ => panic!("{hex:?}: not a float, double or x87 encoding"),
    }
}

/// At libgcc's scaling thresholds, product overflow, subnormal and subnormal-ratio edges
/// (`complex_boundaries.c`), every raw result bit of all six helpers, called directly and through `*` and `/`
/// at -O0 and -O2, equals the default native libgcc's, except which NaN a NaN result is (the floating-point
/// contract; every such difference in the full sample is an operand order, `rt/libgcc/README.md`). Found
/// with 20,000 tuples per format at both levels (480,000 results each): no other difference.
#[test]
fn raw_bits_agree_with_libgcc_at_the_scaling_boundaries() {
    use bennett_exec::Executor;
    use bennett_front_tests::{entry, options};
    let source = c_dir().join("complex_boundaries.c");
    let executable = native_build_with(&source, "complex_boundaries", &["-DNATIVE", "-ffp-contract=off"], &[]);
    let output = Command::new(&executable).output().unwrap_or_else(|e| panic!("cannot run {executable:?}: {e}"));
    assert!(output.status.success(), "native complex_boundaries failed");
    let expected = String::from_utf8(output.stdout).expect("hex");
    assert_eq!(expected.lines().count(), 3 * 2000 * 8);
    for (flags, suffix) in [(CLANG_FLAGS, ""), (CLANG_O2_FLAGS, ".o2")] {
        let module = lower(&source, &format!("complex_boundaries{suffix}"), flags, &[]);
        let run_options = bennett_exec::Options { budget: Some(200_000_000), ..options(0) };
        let mut executor = Executor::new(&module, entry(&module, "main"), &[], run_options).expect("starts");
        assert_eq!(executor.run(), Outcome::Returned(Some(0)), "{flags:?}");
        let device = String::from_utf8(executor.tape().to_vec()).expect("hex");
        assert_eq!(device.lines().count(), expected.lines().count(), "{flags:?}");
        let (mut nan_only, mut differ) = (0, Vec::new());
        for (i, (d, n)) in device.lines().zip(expected.lines()).enumerate() {
            if d == n {
                continue;
            }
            if d.len() == n.len() && is_nan_bits(d) && is_nan_bits(n) {
                nan_only += 1;
            } else {
                differ.push(format!("component {i}: device {d}, native {n}"));
            }
        }
        assert!(
            differ.is_empty(),
            "{flags:?}: {} differ beyond NaN bits ({nan_only} only in NaN bits):\n{}",
            differ.len(),
            differ.join("\n")
        );
    }
}
