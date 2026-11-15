//! Floating point as library code: `float` and `double` rewritten into integers and calls of the compiler
//! runtime-call interface over Berkeley SoftFloat 3e (`front/src/SoftFloat.h`). Each `float_*.c` program
//! (`front/tests/c`) has `result(i)`, the bits of its i-th result, and `results()`; the device's results
//! are compared one by one with the native build's, at -O0 and -O2 natively (which must agree with each
//! other: `a * b + c` is unfused at both) and from unoptimised and optimised clang output on the device.

use std::path::Path;
use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, MAX_CONFIGURATIONS, bitcode, branches, c_dir, lower, lower_text, native_build_with,
    native_exit_status, root, run, write_c,
};
use bennett_ir::ValidModule;
use num_complex::Complex64;

/// clang without `-fmath-errno`: `__builtin_sqrt` and the like are then LLVM intrinsics, not calls of the
/// C library.
const NO_ERRNO: &str = "-fno-math-errno";

fn with<'a>(flags: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
    flags.iter().chain(extra).copied().collect()
}

/// What `source` prints when built natively at `-O0` and at `-O2` (with `-DNATIVE`, and with `driver` if
/// given), which must agree: a result per line, or its high and low 64 bits.
fn native_results(source: &Path, name: &str, driver: Option<&Path>) -> Vec<u128> {
    let mut outputs = Vec::new();
    for level in ["-O0", "-O2"] {
        let mut flags = vec!["-DNATIVE", NO_ERRNO, level, "-Wno-override-module"];
        let driver = driver.map(|d| d.to_string_lossy().into_owned());
        if let Some(driver) = &driver {
            flags.push(driver);
        }
        let executable = native_build_with(source, &format!("{name}{level}"), &flags, &["-lm"]);
        let output = Command::new(&executable).output().unwrap_or_else(|e| panic!("cannot run {executable:?}: {e}"));
        assert!(output.status.success(), "native {name} {level} failed");
        let values: Vec<u128> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let parts: Vec<u128> =
                    line.split(' ').map(|p| p.parse().unwrap_or_else(|e| panic!("{line:?}: {e}"))).collect();
                parts.iter().fold(0, |value, part| value << 64 | part)
            })
            .collect();
        outputs.push(values);
    }
    assert_eq!(outputs[0], outputs[1], "{name}: the native build at -O0 and at -O2");
    outputs.swap_remove(0)
}

/// The device's `result(i)` for every i, as `module` computes them.
fn device_results(module: &ValidModule, count: usize, context: &str) -> Vec<u128> {
    assert_eq!(run(module, "results", &[], 0), Outcome::Returned(Some(count as u128)), "{context}: results()");
    (0..count as u64)
        .map(|i| match run(module, "result", &[i], 0) {
            Outcome::Returned(Some(value)) => value,
            other => panic!("{context}: result({i}): {other:?}"),
        })
        .collect()
}

/// `float_<name>.c` computes on the device what it computes natively, result by result.
fn assert_results_agree(name: &str, front_flags: &[&str]) {
    let source = c_dir().join(format!("{name}.c"));
    assert_source_agrees(&source, name, None, front_flags);
}

fn assert_source_agrees(source: &Path, name: &str, driver: Option<&Path>, front_flags: &[&str]) {
    let expected = native_results(source, name, driver);
    for (flags, suffix) in [(with(CLANG_FLAGS, &[NO_ERRNO]), ""), (with(CLANG_O2_FLAGS, &[NO_ERRNO]), ".o2")] {
        let context = format!("{name} {flags:?} {front_flags:?}");
        let module = lower(source, &format!("{name}{suffix}"), &flags, front_flags);
        let device = device_results(&module, expected.len(), &context);
        let differ: Vec<String> = (0..expected.len())
            .filter(|&i| device[i] != expected[i])
            .map(|i| format!("result({i}) = {:#x}, natively {:#x}", device[i], expected[i]))
            .collect();
        assert!(differ.is_empty(), "{context}: {} of {} differ:\n{}", differ.len(), expected.len(), differ.join("\n"));
    }
}

#[test]
fn arithmetic_square_roots_multiply_adds_and_conversions_agree_with_native_on_directed_values() {
    assert_results_agree("float_values", &[]);
}

/// long double, the x87 80-bit format, over SoftFloat's extF80: every operation, comparison, class and
/// conversion on directed values (`float_long_double.c`).
#[test]
fn long_double_arithmetic_comparisons_and_conversions_agree_with_native_on_directed_values() {
    assert_results_agree("float_long_double", &[]);
}

/// Ordinary C on long double: structs by value, a variadic function, function pointers, arrays, select
/// and phi (`float_long_double_programs.c`).
#[test]
fn structs_variadics_function_pointers_and_arrays_of_long_doubles_agree_with_native() {
    assert_results_agree("float_long_double_programs", &[]);
    assert_results_agree("float_long_double_programs", &["--taint-all"]);
}

/// Conversions between float, double, long double and 128-bit integers (and a `_BitInt(100)`): ties and
/// sticky bits, the edges of the ranges, and out of range, as the native program's libgcc routines give
/// (`float_wide_integers.c`).
#[test]
fn conversions_to_and_from_128_bit_integers_agree_with_native() {
    assert_results_agree("float_wide_integers", &[]);
}

#[test]
fn every_comparison_and_classification_agrees_with_native_on_nans_zeros_and_infinities() {
    assert_results_agree("float_compare", &[]);
}

/// Every fcmp predicate and llvm.minimum and llvm.maximum, which C does not reach, in IR
/// (`float_predicates.ll`), printed natively by a C driver.
#[test]
fn every_fcmp_predicate_and_minimum_and_maximum_agree_with_native() {
    let driver = write_c(
        "float_predicates_driver",
        "int printf(const char *, ...);\nint results(void);\nunsigned __int128 result(int);\n\
         int main(void) {\n  for (int i = 0; i < results(); i++) {\n    unsigned __int128 r = result(i);\n    \
         printf(\"%llu %llu\\n\", (unsigned long long)(r >> 64), (unsigned long long)r);\n  }\n  return 0;\n}\n",
    );
    assert_source_agrees(&c_dir().join("float_predicates.ll"), "float_predicates", Some(&driver), &[]);
}

#[test]
fn structs_variadics_function_pointers_intrinsics_and_vectors_of_doubles_agree_with_native() {
    assert_results_agree("float_programs", &[]);
    assert_results_agree("float_programs", &["--taint-all"]);
}

/// The state of `name(args)` just before its first measurement, as (values of the C variables, in
/// allocation order), with amplitudes, against `expected`, in order.
fn assert_branches(module: &ValidModule, context: &str, name: &str, args: &[u64], expected: &[(Vec<u64>, f64)]) {
    let (_, actual) = branches(module, name, args, MAX_CONFIGURATIONS);
    let mut expected = expected.to_vec();
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(actual.len(), expected.len(), "{context} @{name}: {actual:?}");
    for ((values, amplitude), (want, weight)) in actual.iter().zip(&expected) {
        assert_eq!(values, want, "{context} @{name}");
        assert!((amplitude - Complex64::new(*weight, 0.0)).norm() < 1e-12, "{context} @{name}: {amplitude}");
    }
}

/// Doubles under quantum control (`float_controlled.c`), each result computed here in Rust: an
/// assignment under a qubit branch, a computation applied coherently to a superposition, a gate
/// controlled by a comparison of doubles, and a rotation whose angle the runtime computes on constants.
#[test]
fn doubles_compute_under_quantum_control() {
    let source = c_dir().join("float_controlled.c");
    let half = std::f64::consts::FRAC_1_SQRT_2;
    for (flags, front, suffix) in [
        (CLANG_FLAGS, &[][..], ""),
        (CLANG_O2_FLAGS, &[][..], ".o2"),
        (CLANG_FLAGS, &["--taint-all"][..], ".taint-all"),
    ] {
        let context = format!("{flags:?} {front:?}");
        let module = lower(&source, &format!("float_controlled{suffix}"), flags, front);
        for x in [3.7f64, -0.0, 1e308, f64::INFINITY] {
            let y = (x * 0.5 + 1.0).to_bits();
            assert_branches(&module, &context, "scaled", &[x.to_bits()], &[(vec![0, 0], half), (vec![1, y], half)]);
        }
        let expected: Vec<(Vec<u64>, f64)> = (0..16u64)
            .map(|k| {
                let q = (0..4).map(|i| (k >> i & 1) << (8 * i)).sum();
                let y = (k as f64 / 3.0 + 0.1).to_bits();
                (vec![q, y, u64::from(k as f64 * 0.3 > 2.0)], 0.25)
            })
            .collect();
        assert_branches(&module, &context, "ratio", &[], &expected);
        let (cos, sin) = (0.25f64.cos(), 0.25f64.sin());
        assert_branches(&module, &context, "folded", &[], &[(vec![0], cos), (vec![1], sin)]);
    }
}

/// long double under quantum control (`float_long_double_controlled.c`): a computation on long doubles
/// applied coherently to a superposition and a gate controlled by a comparison of long doubles, against
/// the values the native build computes.
#[test]
fn long_doubles_compute_under_quantum_control() {
    let source = c_dir().join("float_long_double_controlled.c");
    let include = format!("-I{}", root().join("front/include").display());
    let executable = native_build_with(&source, "float_long_double_controlled", &["-DNATIVE", &include], &[]);
    let output = Command::new(&executable).output().unwrap_or_else(|e| panic!("cannot run {executable:?}: {e}"));
    assert!(output.status.success());
    let expected: Vec<(Vec<u64>, f64)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .enumerate()
        .map(|(k, line)| {
            let values: Vec<u64> =
                line.split(' ').map(|v| v.parse().unwrap_or_else(|e| panic!("{line:?}: {e}"))).collect();
            let q = (0..4).map(|i| (k as u64 >> i & 1) << (8 * i)).sum();
            (vec![q, values[0], values[1], values[2]], 0.25)
        })
        .collect();
    assert_eq!(expected.len(), 16);
    for (flags, front, suffix) in [
        (CLANG_FLAGS, &[][..], ""),
        (CLANG_O2_FLAGS, &[][..], ".o2"),
        (CLANG_FLAGS, &["--taint-all"][..], ".taint-all"),
    ] {
        let context = format!("{flags:?} {front:?}");
        let module = lower(&source, &format!("float_long_double_controlled{suffix}"), flags, front);
        assert_branches(&module, &context, "ratio", &[], &expected);
    }
}

/// Lowering the C program `text` with `clang_flags` is refused by name, at its line, with `expected`.
fn assert_refused(name: &str, text: &str, clang_flags: &[&str], front_flags: &[&str], expected: &str) {
    let source = write_c(name, text);
    match lower_text(&source, name, &with(CLANG_FLAGS, clang_flags), front_flags) {
        Ok(program) => panic!("{name}: accepted\n{program}"),
        Err(message) => assert!(
            message.starts_with("not handled yet: ") && message.contains(expected) && message.contains(".c:"),
            "{name}: {message}"
        ),
    }
}

/// What has no soft-float routine yet, and the C library's functions, are refused by name.
#[test]
fn what_the_runtime_does_not_compute_is_refused_by_name() {
    let cases: [(&str, &str, &[&str], &str); 11] = [
        (
            "float_frem",
            "volatile double a = 5.5, b = 2;\nint main(void) { return (int)__builtin_fmod(a, b); }\n",
            &[NO_ERRNO],
            "the remainder of floating-point values (`frem`, fmod), which comes with the C library",
        ),
        (
            "float_sin",
            "volatile double a = 5.5;\nint main(void) { return (int)__builtin_sin(a); }\n",
            &[NO_ERRNO],
            "the intrinsic @llvm.sin.f64, a C library function",
        ),
        // With errno, sqrt is the C library's: it is not redirected to the runtime's sqrt.
        (
            "float_sqrt_errno",
            "double sqrt(double);\nvolatile double a = 5.5;\nint main(void) { return (int)sqrt(a); }\n",
            &[],
            "a call to @sqrt, which has no definition",
        ),
        (
            "float_strtod",
            "double strtod(const char *, char **);\nint main(void) { return (int)strtod(\"1.5\", 0); }\n",
            &[],
            "a call to @strtod, which has no definition",
        ),
        (
            "float_fmal",
            "volatile long double a = 5.5;\nint main(void) { return (int)__builtin_fmal(a, a, a); }\n",
            &[NO_ERRNO],
            "the fused multiply-add of `x86_fp80` (long double) values (`fmal`, @llvm.fma.f80): Berkeley SoftFloat \
             3e has none for the 80-bit format",
        ),
        (
            "float_from_i200",
            "volatile _BitInt(200) a = 5;\nint main(void) { return (int)(double)a; }\n",
            &[],
            "a conversion from `i200` to `double`",
        ),
        (
            "float_bfloat",
            "volatile __bf16 a = 5.5;\nint main(void) { return (int)(float)a; }\n",
            &[],
            "an operation on `bfloat` (__bf16) values",
        ),
        (
            "float_half",
            "volatile _Float16 a = 5.5;\nint main(void) { return (int)(a * a); }\n",
            &[],
            "an operation on `half` (_Float16) values",
        ),
        (
            "float_quad",
            "volatile __float128 a = 5.5;\nint main(void) { return (int)(a + a); }\n",
            &[],
            "an operation on `fp128` (__float128) values",
        ),
        (
            "float_fenv",
            "#pragma STDC FENV_ACCESS ON\nvolatile double a = 5, b = 3;\nint main(void) { return (int)(a / b); }\n",
            &[],
            "the constrained floating-point intrinsic @llvm.experimental.constrained.fdiv.f64",
        ),
        (
            "float_atomic",
            "_Atomic double a = 5;\nint main(void) { a += 1.0; return (int)a; }\n",
            &[],
            "the LLVM instruction `cmpxchg`", // clang's loop for it; the addition itself is soft-float
        ),
    ];
    for (name, text, clang_flags, expected) in cases {
        assert_refused(name, text, clang_flags, &[], expected);
    }
}

/// A program's own function with a runtime name is its own: the program's `a + b` still adds, and its
/// call of `__adddf3` calls its function.
#[test]
fn a_function_of_the_program_with_a_runtime_name_is_not_the_runtime() {
    let source = write_c(
        "float_own_adddf3",
        "double __adddf3(double a, double b) { (void)a; (void)b; return 42.0; }\n\
         volatile double a = 1.0, b = 2.0;\n\
         int main(void) { return (int)(a + b) * 100 + (int)__adddf3(a, b); }\n",
    );
    assert_eq!(native_exit_status(&source, "float_own_adddf3"), 342 & 255);
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = lower(&source, "float_own_adddf3", flags, &[]);
        assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(342)), "{flags:?}");
    }
}

/// The runtime is chosen at link time: `--float-runtime` takes any bitcode defining the interface. One
/// whose `__adddf3` always gives 7 is used; an operation it does not define, one it defines with another
/// type, and a runtime that cannot be read are refused by name.
#[test]
fn the_runtime_is_replaceable_at_link_time() {
    let runtime = write_c(
        "float_other_runtime",
        "double __adddf3(double a, double b) { (void)a; (void)b; return 7.0; }\n\
         float __subdf3(float a) { return a; }\n",
    );
    let runtime = bitcode(&runtime, "float_other_runtime", &["-O2"]);
    let option = format!("--float-runtime={}", runtime.display());
    let program = |operation: &str| {
        format!(
            "volatile double a = 1.0, b = 2.0;\nint main(void) {{\n  double r = a {operation} b;\n  \
             unsigned long long u;\n  __builtin_memcpy(&u, &r, 8);\n  return (int)(u >> 52);\n}}\n"
        )
    };
    let source = write_c("float_with_other_runtime", &program("+"));
    let module = lower(&source, "float_with_other_runtime", CLANG_FLAGS, &[&option]);
    assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(0x401)), "7.0 has the exponent 0x401");
    let module = lower(&source, "float_with_soft_float", CLANG_FLAGS, &[]);
    assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(0x400)), "3.0 has the exponent 0x400");
    assert_refused(
        "float_missing_routine",
        &program("*"),
        &[],
        &[&option],
        "an operation needing @__muldf3, which the floating-point runtime",
    );
    assert_refused("float_wrong_type", &program("-"), &[], &[&option], "defines with the type float (float)");
    let missing = format!("--float-runtime={}", runtime.with_extension("missing").display());
    let source = write_c("float_no_runtime", &program("+"));
    let message = lower_text(&source, "float_no_runtime", CLANG_FLAGS, &[&missing]).expect_err("accepted");
    assert!(message.contains("cannot read the floating-point runtime"), "{message}");
}

/// Without floating-point operations no runtime is linked, and values that are only moved (a long
/// double, the angle of a rotation) are their bits.
#[test]
fn a_program_without_floating_point_operations_links_no_runtime() {
    let source = write_c(
        "float_only_moved",
        "#include \"bennett.h\"\nstruct s { long double x; int t; };\nvolatile struct s g = {2.5L, 3};\n\
         int main(void) { struct s c = g; return c.t + ((unsigned char *)&c.x)[9]; }\n\
         unsigned long long rotated(void) { _Bool a = 0; bennett_ry(&a, 0.5); return bennett_measure(&a); }\n",
    );
    let text = lower_text(&source, "float_only_moved", CLANG_FLAGS, &[]).unwrap_or_else(|e| panic!("{e}"));
    assert!(!text.contains("bennett.float"), "{text}");
    let module = lower(&source, "float_only_moved", CLANG_FLAGS, &[]);
    assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(3 + 0x40)));
    let thesis = lower_text(&c_dir().join("thesis.c"), "float_thesis", CLANG_FLAGS, &[]).expect("thesis.c");
    assert!(!thesis.contains("bennett.float"));
}

/// An address into a global of floating-point type, as a constant (`v.f[1]` of a union of a vector and an
/// array of floats), is rewritten with the integer type: LLVM's getWithOperands keeps a GEP whose only
/// change is its source element type, which once left `[4 x float]` behind (GCC's 20050604-1).
#[test]
fn a_constant_address_into_a_global_of_floats_is_rewritten() {
    let source = write_c(
        "float_constant_address",
        "typedef float v4sf __attribute__((vector_size(16)));\nunion { v4sf v; float f[4]; } v;\n\
         int main(void) {\n  v.v += (v4sf){18.0f, 20.0f, 22.0f};\n  v.f[1] += 0.5f;\n  \
         return v.f[0] != 18.0f || v.f[1] != 20.5f || v.f[2] != 22.0f || v.f[3] != 0.0f;\n}\n",
    );
    assert_eq!(native_exit_status(&source, "float_constant_address"), 0);
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = lower(&source, "float_constant_address", flags, &[]);
        assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(0)), "{flags:?}");
    }
}

#[test]
fn fmin_and_fmax_give_the_zero_native_code_gives() {
    bennett_front_tests::assert_agrees_with_native("float_minmax_zeros", 0);
}
