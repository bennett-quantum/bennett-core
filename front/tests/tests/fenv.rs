//! The floating-point environment against the native program (clang 18 with glibc on x86-64): every operation
//! under each of the four rounding modes, its result's bits and the exception flags it raised, and `<fenv.h>`
//! itself, with musl linked on the device (`rt/musl/fenv.c`, the state in `rt/softfloat/libgcc.c`). The C
//! programs say what they compare (`front/tests/c/fenv_ops.c`, `fenv_api.c`, `fenv_pows.c`,
//! `round_to_integer_bounds.c`). The rounding modes and
//! ISO C's five exceptions are the guarantee; x86's denormal-operand flag, compared beside them, is outside it:
//! best effort, its known gap (glibc's `fmal`) not compared.
use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, lower, lower_text, native_build_with, out_dir, root, run, write_c,
};
use bennett_ir::Fault;
use std::path::Path;
use std::process::Command;

/// A bound on the steps of one run, far above the programs': the operation table takes about 5·10^7.
const STEP_BUDGET: u64 = 2_000_000_000;

/// Links `source` with musl at the clang options `flags` and runs it: the outcome and what it printed.
fn device(source: &Path, name: &str, flags: &[&str]) -> (Outcome, String) {
    let workspace = root().canonicalize().expect("workspace");
    let build = Command::new("python3").arg(workspace.join("rt/musl/build.py")).output().expect("musl build");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let file = out_dir().join(format!("{name}.bn"));
    let mut link = Command::new("python3");
    link.arg(workspace.join("rt/musl/link.py")).arg(source).arg("-o").arg(&file);
    for flag in flags {
        link.arg(format!("--clang-flag={flag}"));
    }
    let linked = link.output().expect("musl link");
    assert!(linked.status.success(), "{}", String::from_utf8_lossy(&linked.stderr));
    let module = bennett_ir::load(&std::fs::read_to_string(&file).expect("device text")).expect("valid");
    let entry = module.function_named("__bennett_entry").expect("entry");
    let options = Options { budget: Some(STEP_BUDGET), ..Options::default() };
    let mut e = Executor::new(&module, entry, &[], options).expect("start");
    let outcome = e.run();
    (outcome, String::from_utf8(e.tape().to_vec()).expect("device text output"))
}

/// The native program's output at `-O0` and at `-O2`, which must agree.
fn native(source: &Path, name: &str, flags: &[&str]) -> String {
    let mut outputs = Vec::new();
    for level in ["-O0", "-O2"] {
        let mut all = vec![level];
        all.extend(flags);
        let exe = native_build_with(source, &format!("{name}{level}"), &all, &["-lm"]);
        let run = Command::new(exe).output().expect("native run");
        assert_eq!(run.status.code(), Some(0), "native {name} {level}");
        outputs.push(String::from_utf8(run.stdout).expect("native text"));
    }
    assert_eq!(outputs[0], outputs[1], "native {name}: -O0 and -O2 differ");
    outputs.remove(0)
}

/// `source` on the device at each of `levels`, with `flags`, prints what the native program prints.
fn agrees(file: &str, name: &str, levels: &[&str], flags: &[&str]) {
    let source = root().join("front/tests/c").join(file);
    let expected = native(&source, name, flags);
    for level in levels {
        let mut all = vec![*level];
        all.extend(flags);
        let (outcome, printed) = device(&source, &format!("{name}{level}"), &all);
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{name} {level}");
        let differences: Vec<String> = expected
            .lines()
            .zip(printed.lines())
            .filter(|(a, b)| a != b)
            .map(|(a, b)| format!("native {a}\ndevice {b}"))
            .collect();
        assert!(
            differences.is_empty(),
            "{name} {level}: {} lines differ:\n{}",
            differences.len(),
            differences.join("\n")
        );
        assert_eq!(expected.lines().count(), printed.lines().count(), "{name} {level}");
    }
}

#[test]
fn every_operation_rounds_and_raises_flags_as_native_code() {
    agrees("fenv_ops.c", "fenv_ops", &["-O0"], &[]);
}

#[test]
fn every_operation_rounds_and_raises_flags_as_native_code_from_o2_without_errno() {
    // -fno-math-errno makes the C library's functions builtins: then they are the runtime's directly.
    agrees("fenv_ops.c", "fenv_ops_o2", &["-O2"], &["-fno-math-errno"]);
}

#[test]
fn integer_powers_raise_the_denormal_operand_flag_as_native_code() {
    agrees("fenv_pows.c", "fenv_pows", &["-O0", "-O2"], &[]);
}

/// lround, llround, lrint and llrint on each format, directly and through pointers, in each mode, just inside, at
/// and just outside the 64-bit range of both signs: the result and the flags in each unit. lroundl and llroundl of
/// 2^63 - 0.5 rounded to 2^63 without glibc's invalid (in SSE) before.
#[test]
fn rounding_to_64_bits_at_the_edges_of_the_range_raises_flags_as_native_code() {
    agrees("round_to_integer_bounds.c", "round_to_integer_bounds", &["-O0", "-O2"], &[]);
    agrees("round_to_integer_bounds.c", "round_to_integer_bounds_o2", &["-O2"], &["-fno-math-errno"]);
}

#[test]
fn the_environment_functions_behave_as_glibcs() {
    agrees("fenv_api.c", "fenv_api", &["-O0", "-O2"], &[]);
}

#[test]
#[ignore = "heavy: 1.6·10^5 seeded random operations a seed (about 10^8 device steps); the directed table runs by default"]
fn random_operands_round_and_raise_flags_as_native_code() {
    for seed in ["-DSEED=1", "-DSEED=2", "-DSEED=3"] {
        agrees("fenv_ops.c", &format!("fenv_random{}", &seed[7..]), &["-O0", "-O2"], &["-DRANDOM=2000", seed]);
    }
}

/// An environment that asks for what is not carried (a trap, x87 precision other than extended, flush to zero) or
/// sets a reserved MXCSR bit is a named fault, never a success that changes nothing.
#[test]
fn an_environment_that_is_not_carried_is_a_named_fault() {
    for (name, change) in [
        ("fenv_trap", "env.__control_word &= ~FE_INEXACT;"),
        ("fenv_precision", "env.__control_word &= ~0x300;"),
        ("fenv_flush", "env.__mxcsr |= 0x8040;"),
        // A reserved MXCSR bit, which faults natively.
        ("fenv_reserved", "env.__mxcsr |= 1u << 16;"),
    ] {
        let source = write_c(
            name,
            &format!(
                "#include <fenv.h>\n#include <stdio.h>\nint main(void) {{\n  fenv_t env;\n  fegetenv(&env);\n  {change}\n  \
                 fesetenv(&env);\n  puts(\"set\");\n  fesetenv(FE_DFL_ENV);\n  return 0;\n}}\n"
            ),
        );
        let (outcome, printed) = device(&source, name, &["-O0"]);
        assert_eq!(outcome, Outcome::Faulted(Fault::Unreachable), "{name}: {printed}");
        assert_eq!(printed, "", "{name}");
    }
}

/// Floating point under a qubit branch stays accepted in a program that cannot read the flags, whose writes the
/// front deletes, and is refused by name in one that can, where every routine writes them.
#[test]
fn reading_the_flags_makes_floating_point_under_a_qubit_branch_a_refusal() {
    let source = write_c(
        "fenv_controlled",
        "#include \"bennett.h\"\nint __bennett_fegetexcept(void);\n\
         unsigned long long f(double x) {\n  _Bool q = 0;\n  unsigned long long y = 0;\n  bennett_h(&q);\n  \
         if (q) {\n    double r = x * 0.5 + 1.0;\n    __builtin_memcpy(&y, &r, 8);\n  }\n  \
         unsigned long long r = bennett_measure(&y);\n  return r ^ bennett_measure(&q) ^ (unsigned)READ;\n}\n",
    );
    let mut flags = CLANG_FLAGS.to_vec();
    flags.push("-DREAD=0");
    lower_text(&source, "fenv_controlled_unread", &flags, &[]).expect("accepted without the flags read");
    flags.pop();
    flags.push("-DREAD=__bennett_fegetexcept()");
    let error = lower_text(&source, "fenv_controlled_read", &flags, &[]).expect_err("refused with the flags read");
    assert!(
        error
            .contains("@bennett.float.__muldf3, which may write memory, controlled by a value that depends on a qubit"),
        "{error}"
    );
}

/// LLVM's minimum and maximum of two largest finite doubles raise nothing: the sum that makes the NaN result is
/// computed only when it is the result (glibc's fminimum raises nothing there either). The flags are read through
/// a pointer, as a program may read them.
#[test]
fn minimum_and_maximum_of_finite_operands_raise_no_flag() {
    for (name, intrinsic) in [("fenv_minimum", "minimum"), ("fenv_maximum", "maximum")] {
        let source = out_dir().join(format!("{name}.ll"));
        std::fs::write(
            &source,
            format!(
                "target datalayout = \"e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128\"\n\
                 target triple = \"x86_64-pc-linux-gnu\"\n@a = global double 0x7FEFFFFFFFFFFFFF\n\
                 declare i32 @__bennett_fegetexcept()\n@read = global ptr @__bennett_fegetexcept\n\
                 declare double @llvm.experimental.constrained.{intrinsic}.f64(double, double, metadata)\n\
                 define i32 @main() strictfp {{\n  %x = load volatile double, ptr @a\n  %y = load volatile double, ptr @a\n  \
                 %m = call double @llvm.experimental.constrained.{intrinsic}.f64(double %x, double %y, metadata !\"fpexcept.strict\") strictfp\n  \
                 store volatile double %m, ptr @a\n  %r = load volatile ptr, ptr @read\n  %f = call i32 %r()\n  ret i32 %f\n}}\n"
            ),
        )
        .expect("write the IR");
        let module = lower(&source, name, &["-O0"], &[]);
        assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(0)), "{name}");
    }
}

/// A strict function that holds any value of a 16-bit floating type (`_Float16`, `__bf16`) is refused by name, one
/// rule drawn by type: there clang 18's x86-64 back end gives `_Float16`'s encoding for `(__bf16)1.0f` (0x3c00),
/// cannot compile most `__bf16` operations, and gives different results at -O0 and -O2 (`fabs` of a signalling
/// `_Float16` NaN, the sign of a `__bf16`), so no native result is determined. Operations, conversions,
/// reinterpretations and moves alike.
#[test]
fn a_strict_function_holding_a_half_or_bfloat_value_is_refused_by_name() {
    for (name, declaration, operation) in [
        ("fenv_bfloat_trunc", "volatile float f = 1.0f;\nvolatile __bf16 r;", "r = (__bf16)f;"),
        ("fenv_bfloat_abs", "volatile __bf16 v, r;", "r = __builtin_elementwise_abs(v);"),
        ("fenv_bfloat_class", "volatile __bf16 v;\nvolatile int r;", "r = __builtin_isfpclass(v, 1);"),
        ("fenv_half_abs", "volatile _Float16 v, r;", "r = __builtin_fabsf16(v);"),
        ("fenv_half_add", "volatile _Float16 v, r;", "r = v + v;"),
        ("fenv_bfloat_sign", "volatile __bf16 v = -1;\nvolatile int r;", "r = !!__builtin_signbit(v);"),
        ("fenv_half_sign", "volatile _Float16 v = -1;\nvolatile int r;", "r = !!__builtin_signbit(v);"),
        ("fenv_half_move", "volatile _Float16 v, r;", "r = v;"),
    ] {
        let source = write_c(
            name,
            &format!("#pragma STDC FENV_ACCESS ON\n{declaration}\nint main(void) {{\n  {operation}\n  return 0;\n}}\n"),
        );
        for (flags, level) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
            let error = lower_text(&source, &format!("{name}_{level}"), flags, &[]).expect_err("refused");
            assert!(
                error.contains("in @main, a function with `#pragma STDC FENV_ACCESS ON`"),
                "{name} {level}: {error}"
            );
        }
    }
    // Outside a strict function the same operations are computed.
    let source = write_c(
        "fenv_half_plain",
        "volatile _Float16 v = 1.5, r;\nint main(void) {\n  r = __builtin_fabsf16(v) + v;\n  return 0;\n}\n",
    );
    lower_text(&source, "fenv_half_plain", CLANG_FLAGS, &[]).expect("accepted outside FENV_ACCESS");
}

/// A direct read of the flags after an overflow gives overflow and inexact: the front's evaluation of runtime calls
/// on constants, a read of the flags among them (it has no operands), is not done where the flags are read, since
/// LLVM's Evaluator would give their initial value.
#[test]
fn a_direct_read_of_the_flags_is_not_evaluated_away() {
    let source = write_c(
        "fenv_direct_read",
        "#pragma STDC FENV_ACCESS ON\nint __bennett_fegetexcept(void);\nvolatile double a = 1.7976931348623157e308;\n\
         int main(void) {\n  volatile double m = a + a;\n  (void)m;\n  return __bennett_fegetexcept();\n}\n",
    );
    let module = lower(&source, "fenv_direct_read", CLANG_FLAGS, &[]);
    // x86's overflow (8) and inexact (32).
    assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(40)));
}

/// An over-refusal (no wrong result): a non-strict helper that holds a `_Float16` value, taking and returning only
/// its bits, is accepted at -O0, but clang inlines it at -O2 into the strict caller, which then holds the value, so
/// the device refuses what native code computes (`3c00 0`).
#[test]
#[ignore = "defect: over-refusal of a non-strict _Float16 helper inlined into a strict function at -O2 (docs/DESIGN.md, floating point)"]
fn a_non_strict_half_helper_inlined_into_a_strict_caller_is_computed() {
    let source = write_c(
        "fenv_inline_plain",
        "#include <stdio.h>\n#include <fenv.h>\nstatic inline unsigned short mag(unsigned short u) {\n  _Float16 h;\n  \
         __builtin_memcpy(&h, &u, 2);\n  h = __builtin_fabsf16(h);\n  __builtin_memcpy(&u, &h, 2);\n  return u;\n}\n\
         #pragma STDC FENV_ACCESS ON\nvolatile unsigned short u = 0xbc00;\nint main(void) {\n  \
         printf(\"%04x %x\\n\", mag(u), fetestexcept(FE_INVALID));\n  return 0;\n}\n",
    );
    let expected = native(&source, "fenv_inline_plain", &[]);
    for level in ["-O0", "-O2"] {
        let (outcome, printed) = device(&source, &format!("fenv_inline_plain{level}"), &[level]);
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{level}");
        assert_eq!(printed, expected, "{level}");
    }
}
