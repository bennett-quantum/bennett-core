//! The complex multiply and divide helpers (`__mulsc3`, `__divsc3`, `__muldc3`, `__divdc3`, `__mulxc3`,
//! `__divxc3`): compiler-rt's sources (`rt/compiler-rt`, vendored unmodified), linked into the program by
//! `bennett-front` when it calls them (`front/src/Complex.h`) and lowered with its floating point.
//!
//! The reference is the same sources compiled natively, on this machine's floating-point hardware. A
//! native build with the system's default compiler runtime (libgcc) is not the reference: its division
//! is a different algorithm and differs in the last bit and in the sign of zeros
//! (`rt/compiler-rt/README.md`).

#![cfg(target_arch = "x86_64")]

use std::path::PathBuf;
use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, bitcode, c_dir, lower, lower_text, native_build_with, root, run, write_c,
};

/// The vendored helpers natively needed by `complex_helpers.c`.
fn helper_sources() -> Vec<PathBuf> {
    let dir = root().join("rt/compiler-rt/compiler-rt-18.1.8/lib/builtins");
    ["mulsc3.c", "divsc3.c", "muldc3.c", "divdc3.c", "mulxc3.c"].iter().map(|name| dir.join(name)).collect()
}

/// What `complex_helpers.c` prints natively at `-O0` and at `-O2`, which must agree.
fn native_hashes() -> Vec<u64> {
    let source = c_dir().join("complex_helpers.c");
    let helpers: Vec<String> = helper_sources().iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let mut outputs = Vec::new();
    for level in ["-O0", "-O2"] {
        let mut flags = vec!["-DNATIVE", level];
        flags.extend(helpers.iter().map(String::as_str));
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
fn complex_products_and_quotients_agree_with_compiler_rt_on_annex_g_cases() {
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

/// `__divxc3` calls the C library's `logbl` and `scalbnl`: without a library it is refused by name, in
/// `__divxc3`. A program that defines a helper itself keeps its own, and one that calls none links none.
#[test]
fn the_helpers_are_linked_only_where_the_program_calls_them_without_defining_them() {
    let source = write_c(
        "complex_long_double_divide",
        "volatile long double a = 1, b = 2, c = 3, d = 4;\n\
         int main(void) {\n  long double _Complex x, y;\n  __real__ x = a; __imag__ x = b;\n  \
         __real__ y = c; __imag__ y = d;\n  return (int)__real__ (x / y);\n}\n",
    );
    let message = lower_text(&source, "complex_long_double_divide", CLANG_FLAGS, &[]).expect_err("accepted");
    assert!(
        message.starts_with("not handled yet: a call to @logbl, which has no definition")
            && message.contains("in @__divxc3"),
        "{message}"
    );

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
    let module = lower(&source, "complex_with_compiler_rt", CLANG_FLAGS, &[]);
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
