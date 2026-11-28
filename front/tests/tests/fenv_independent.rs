//! Independent tests of the C floating-point environment (`<fenv.h>`) against native clang 18 + glibc
//! on x86-64, at `-O0` and `-O2`:
//! - every rounding mode across `+ - * /`, `sqrt`, `fma`, format and integer conversions, and
//!   `rint`/`nearbyint`/`lrint`/`llrint` and siblings (both builtins and library calls);
//! - each of the five ISO C exception flags (inexact, underflow at the subnormal boundary, overflow,
//!   division by zero, invalid), tested and cleared one by one;
//! - `fegetenv`/`fesetenv`/`feholdexcept`/`feupdateenv` and `fegetexceptflag`/`fesetexceptflag` round
//!   trips with flags set;
//! - x87 (`long double`) and SSE (`float`/`double`) modes and flags set apart across all 16 mode pairs;
//! - documented refusals and named faults at `-O0` and `-O2`.

use std::path::Path;
use std::process::Command;
use std::sync::Once;

use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, lower_text, native_build_with, out_dir, root, write_c};
use bennett_ir::Fault;

/// Step budget per run; each independent test program takes well under 10^7 steps.
const STEP_BUDGET: u64 = 500_000_000;

/// Ensures the musl bitcode library is built once per test process.
fn ensure_musl() {
    static MUSL: Once = Once::new();
    MUSL.call_once(|| {
        let workspace = root().canonicalize().expect("workspace");
        let build = Command::new("python3")
            .arg(workspace.join("rt/musl/build.py"))
            .arg("--jobs")
            .arg("3")
            .output()
            .expect("musl build");
        assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    });
}

/// Links `source` with musl at `flags` and runs it on the device, returning outcome and stdout.
fn device(source: &Path, name: &str, flags: &[&str]) -> (Outcome, String) {
    ensure_musl();
    let workspace = root().canonicalize().expect("workspace");
    let file = out_dir().join(format!("{name}.bn"));
    let mut link = Command::new("python3");
    link.arg(workspace.join("rt/musl/link.py")).arg(source).arg("-o").arg(&file);
    for flag in flags {
        link.arg(format!("--clang-flag={flag}"));
    }
    let linked = link.output().expect("musl link");
    assert!(linked.status.success(), "{name}: {}", String::from_utf8_lossy(&linked.stderr));
    let module = bennett_ir::load(&std::fs::read_to_string(&file).expect("device text")).expect("valid");
    let entry = module.function_named("__bennett_entry").expect("entry");
    let options = Options { budget: Some(STEP_BUDGET), ..Options::default() };
    let mut executor = Executor::new(&module, entry, &[], options).expect("start");
    let outcome = executor.run();
    (outcome, String::from_utf8(executor.tape().to_vec()).expect("device text output"))
}

/// Builds and runs `source` natively with clang 18 + glibc at `-O0` and `-O2`, asserting they agree.
fn native(source: &Path, name: &str, flags: &[&str]) -> String {
    let mut outputs = Vec::new();
    for level in ["-O0", "-O2"] {
        let mut all = vec![level];
        all.extend(flags);
        let exe = native_build_with(source, &format!("{name}{level}"), &all, &["-lm"]);
        let run = Command::new(exe).output().expect("native run");
        assert_eq!(run.status.code(), Some(0), "native {name} {level}: {}", String::from_utf8_lossy(&run.stderr));
        outputs.push(String::from_utf8(run.stdout).expect("native text"));
    }
    assert_eq!(outputs[0], outputs[1], "native {name}: -O0 and -O2 differ");
    outputs.remove(0)
}

/// Compares device output at each level in `levels` (with `flags`) verbatim against native output.
fn agrees(file: &str, name: &str, levels: &[&str], flags: &[&str]) {
    let source = root().join("front/tests/c").join(file);
    let expected = native(&source, name, flags);
    assert!(!expected.is_empty(), "{name}: empty output");
    for level in levels {
        let mut all = vec![*level];
        all.extend(flags);
        let (outcome, printed) = device(&source, &format!("{name}{level}"), &all);
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{name} {level}");
        let differences: Vec<String> = expected
            .lines()
            .zip(printed.lines())
            .filter(|(a, b)| a != b)
            .take(20)
            .map(|(a, b)| format!("native: {a}\ndevice: {b}"))
            .collect();
        assert!(
            differences.is_empty(),
            "{name} {level}: lines differ (first {} shown):\n{}",
            differences.len(),
            differences.join("\n")
        );
        assert_eq!(expected.lines().count(), printed.lines().count(), "{name} {level}: line count");
    }
}

/// All four rounding modes across `+ - * /`, `sqrt`, `fma`, float/double/long double and integer
/// conversions, and `rint`/`nearbyint`/`lrint`/`llrint` (and library calls via function pointers).
#[test]
fn rounding_modes_across_operations_and_conversions_agree_with_native() {
    agrees("fenv_indep_rounding.c", "fenv_indep_rounding", &["-O0", "-O2"], &[]);
}

/// Same rounding suite with `-fno-math-errno` at `-O2` so math calls lower directly to builtins.
#[test]
fn rounding_modes_without_math_errno_agree_with_native() {
    agrees("fenv_indep_rounding.c", "fenv_indep_rounding_noerrno", &["-O2"], &["-fno-math-errno"]);
}

/// All five ISO C exception flags (inexact, underflow at the subnormal boundary with after-rounding
/// tininess, overflow, division by zero, invalid), tested and cleared one by one, plus `-O2`
/// strictness preservation (dead-result operations and non-CSE across `feclearexcept`).
#[test]
fn exception_flags_boundaries_and_single_flag_clearing_agree_with_native() {
    agrees("fenv_indep_flags.c", "fenv_indep_flags", &["-O0", "-O2"], &[]);
}

/// Round trips of `fegetenv`/`fesetenv`, `feholdexcept`/`feupdateenv` (including nested and
/// `FE_DFL_ENV`), `fegetexceptflag`/`fesetexceptflag`, and invalid `fesetround` rejection.
#[test]
fn environment_and_flag_round_trips_agree_with_native() {
    agrees("fenv_indep_env.c", "fenv_indep_env", &["-O0", "-O2"], &[]);
}

/// x87 (`long double`) and SSE (`float`/`double`) rounding modes and exception flags set apart
/// across all 16 mode pairs, interleaved operations, `feholdexcept`/`feupdateenv`, and resync.
#[test]
fn x87_and_sse_units_set_apart_agree_with_native() {
    agrees("fenv_indep_units.c", "fenv_indep_units", &["-O0", "-O2"], &[]);
}

/// Unsupported `fesetenv` and `feupdateenv` settings stop with the named `Unreachable` fault and
/// produce no output, at both `-O0` and `-O2`: every individual unmasked x87 or SSE exception,
/// every non-extended x87 precision setting, FTZ alone, DAZ alone, and reserved MXCSR bits.
#[test]
fn unsupported_environment_settings_fault_at_o0_and_o2() {
    let cases: &[(&str, &str, &str)] = &[
        ("x87_trap_inv", "env.__control_word &= ~0x0001u;", "fesetenv(&env);"),
        ("x87_trap_den", "env.__control_word &= ~0x0002u;", "fesetenv(&env);"),
        ("x87_trap_dz", "env.__control_word &= ~0x0004u;", "fesetenv(&env);"),
        ("x87_trap_ovf", "env.__control_word &= ~0x0008u;", "fesetenv(&env);"),
        ("x87_trap_uf", "env.__control_word &= ~0x0010u;", "fesetenv(&env);"),
        ("x87_trap_inx", "env.__control_word &= ~0x0020u;", "fesetenv(&env);"),
        ("sse_trap_inv", "env.__mxcsr &= ~0x0080u;", "fesetenv(&env);"),
        ("sse_trap_den", "env.__mxcsr &= ~0x0100u;", "fesetenv(&env);"),
        ("sse_trap_dz", "env.__mxcsr &= ~0x0200u;", "fesetenv(&env);"),
        ("sse_trap_ovf", "env.__mxcsr &= ~0x0400u;", "fesetenv(&env);"),
        ("sse_trap_uf", "env.__mxcsr &= ~0x0800u;", "fesetenv(&env);"),
        ("sse_trap_inx", "env.__mxcsr &= ~0x1000u;", "fesetenv(&env);"),
        ("x87_prec_single", "env.__control_word = (unsigned short)(env.__control_word & ~0x0300u);", "fesetenv(&env);"),
        (
            "x87_prec_resv",
            "env.__control_word = (unsigned short)((env.__control_word & ~0x0300u) | 0x0100u);",
            "fesetenv(&env);",
        ),
        (
            "x87_prec_double",
            "env.__control_word = (unsigned short)((env.__control_word & ~0x0300u) | 0x0200u);",
            "fesetenv(&env);",
        ),
        ("sse_ftz_only", "env.__mxcsr |= 0x8000u;", "fesetenv(&env);"),
        ("sse_daz_only", "env.__mxcsr |= 0x0040u;", "fesetenv(&env);"),
        ("sse_reserved_16", "env.__mxcsr |= 1u << 16;", "fesetenv(&env);"),
        ("sse_reserved_31", "env.__mxcsr |= 1u << 31;", "fesetenv(&env);"),
        ("feupdateenv_bad", "env.__mxcsr |= 0x8000u;", "feupdateenv(&env);"),
    ];
    let mut arms = String::new();
    for (idx, &(_, mutate, apply)) in cases.iter().enumerate() {
        arms.push_str(&format!("  case {idx}: {{ {mutate} {apply} break; }}\n"));
    }
    let source = write_c(
        "fenv_indep_bad_envs",
        &format!(
            "#include <fenv.h>\n#include <stdio.h>\n#pragma STDC FENV_ACCESS ON\n\
             __attribute__((noinline)) int trigger_bad_env(int which) {{\n\
               fenv_t env;\n  fegetenv(&env);\n  switch (which) {{\n{arms}  default: return 1;\n  }}\n\
               return 2;\n}}\n\
             int main(void) {{\n  int r = trigger_bad_env(0);\n  puts(\"unexpected\");\n  return r;\n}}\n"
        ),
    );
    ensure_musl();
    let workspace = root().canonicalize().expect("workspace");
    for level in ["-O0", "-O2"] {
        let file = out_dir().join(format!("fenv_indep_bad_envs{level}.bn"));
        let linked = Command::new("python3")
            .arg(workspace.join("rt/musl/link.py"))
            .arg(&source)
            .arg("-o")
            .arg(&file)
            .arg(format!("--clang-flag={level}"))
            .output()
            .expect("musl link");
        assert!(linked.status.success(), "{level}: {}", String::from_utf8_lossy(&linked.stderr));
        let module = bennett_ir::load(&std::fs::read_to_string(&file).expect("device text")).expect("valid");
        // Verify full musl startup + main() faults before reaching puts("unexpected").
        let entry = module.function_named("__bennett_entry").expect("entry");
        let options = Options { budget: Some(STEP_BUDGET), ..Options::default() };
        let mut main_exec = Executor::new(&module, entry, &[], options).expect("start main");
        assert_eq!(main_exec.run(), Outcome::Faulted(Fault::Unreachable), "main {level}");
        assert!(main_exec.tape().is_empty(), "main {level} printed output");
        // Verify all 20 individual unsupported settings fault at this optimisation level.
        let trigger = module.function_named("trigger_bad_env").expect("trigger_bad_env");
        for (idx, &(name, _, _)) in cases.iter().enumerate() {
            let mut exec = Executor::new(&module, trigger, &[idx as u64], options).expect("start");
            assert_eq!(exec.run(), Outcome::Faulted(Fault::Unreachable), "{name} {level}");
            assert!(exec.tape().is_empty(), "{name} {level} printed output");
        }
    }
}

/// Strict (`#pragma STDC FENV_ACCESS ON`) functions holding any `_Float16` or `__bf16` value
/// (parameters, returns, arithmetic, comparisons, conversions, intrinsics, structs, vectors,
/// indirect calls) are refused by name at both `-O0` and `-O2`; non-strict functions stay accepted.
#[test]
fn strict_functions_holding_16bit_floats_are_refused_at_o0_and_o2() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "half_param",
            "__attribute__((noinline)) int f(_Float16 x) { volatile _Float16 t = x; return (int)t; }\n\
             volatile _Float16 g = 1.0f16;\nint main(void) { return f(g); }\n",
            "@f",
        ),
        (
            "half_ret",
            "__attribute__((noinline)) _Float16 f(int x) { return (_Float16)x; }\n\
             volatile int g = 1;\nint main(void) { volatile _Float16 r = f(g); (void)r; return 0; }\n",
            "@f",
        ),
        (
            "half_mul",
            "volatile _Float16 a = 1.5f16, b = 2.0f16, r;\nint main(void) { r = a * b; return 0; }\n",
            "@main",
        ),
        (
            "half_cmp",
            "volatile _Float16 a = 1.5f16, b = 2.0f16;\nvolatile int r;\nint main(void) { r = a < b; return 0; }\n",
            "@main",
        ),
        (
            "half_to_double",
            "volatile _Float16 a = 1.5f16;\nvolatile double r;\nint main(void) { r = (double)a; return 0; }\n",
            "@main",
        ),
        (
            "half_from_double",
            "volatile double a = 1.5;\nvolatile _Float16 r;\nint main(void) { r = (_Float16)a; return 0; }\n",
            "@main",
        ),
        (
            "half_copysign",
            "volatile _Float16 a = 1.5f16, b = -2.0f16, r;\n\
             int main(void) { r = __builtin_copysignf16(a, b); return 0; }\n",
            "@main",
        ),
        (
            "half_struct",
            "struct S { _Float16 x; _Float16 y; };\nvolatile struct S g = {1.0f16, 2.0f16};\n\
             __attribute__((noinline)) int take(struct S s) { volatile _Float16 t = s.x; return t == 1.0f16; }\n\
             int main(void) { struct S s = g; return take(s); }\n",
            "@take",
        ),
        (
            "half_vector",
            "typedef _Float16 hf2 __attribute__((vector_size(4)));\nvolatile hf2 a = {1.0f16, 2.0f16}, r;\n\
             int main(void) { r = a + a; return 0; }\n",
            "@main",
        ),
        (
            "bfloat_param",
            "__attribute__((noinline)) int f(__bf16 x) { volatile __bf16 t = x; (void)t; return 0; }\n\
             volatile __bf16 g;\nint main(void) { return f(g); }\n",
            "@f",
        ),
        ("bfloat_move", "volatile __bf16 a, b;\nint main(void) { b = a; return 0; }\n", "@main"),
        (
            "bfloat_to_float",
            "volatile __bf16 a;\nvolatile float r;\nint main(void) { r = (float)a; return 0; }\n",
            "@main",
        ),
        (
            "bfloat_from_double",
            "volatile double a = 1.5;\nvolatile __bf16 r;\nint main(void) { r = (__bf16)a; return 0; }\n",
            "@main",
        ),
        (
            "bfloat_vector",
            "typedef __bf16 bf2 __attribute__((vector_size(4)));\nvolatile bf2 a, r;\n\
             int main(void) { r = a; return 0; }\n",
            "@main",
        ),
    ];
    for &(name, body, func) in cases {
        let source = write_c(&format!("fenv_indep_16_{name}"), &format!("#pragma STDC FENV_ACCESS ON\n{body}"));
        for (flags, level) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
            let error = match lower_text(&source, &format!("fenv_indep_16_{name}_{level}"), flags, &[]) {
                Ok(text) => panic!("{name} {level} must be refused:\n{text}"),
                Err(error) => error,
            };
            assert!(
                error.contains("`#pragma STDC FENV_ACCESS ON`") && error.contains(func),
                "{name} {level}: unexpected diagnostic: {error}"
            );
        }
    }

    // Contrast: outside `#pragma STDC FENV_ACCESS ON`, `_Float16` and `__bf16` stay accepted.
    let plain = write_c(
        "fenv_indep_16_plain",
        "volatile _Float16 h = 1.5f16, hr;\nvolatile __bf16 b, br;\n\
         int main(void) { hr = h + h; br = (__bf16)((float)b + 1.0f); return 0; }\n",
    );
    for (flags, level) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        lower_text(&plain, &format!("fenv_indep_16_plain_{level}"), flags, &[])
            .unwrap_or_else(|e| panic!("non-strict 16-bit floats accepted at {level}: {e}"));
    }
}

/// Non-IEEE subnormal modes promised via `-fdenormal-fp-math` / `-fdenormal-fp-math-f32` are
/// refused by name at `-O0` and `-O2` under `#pragma STDC FENV_ACCESS ON`, while `ieee` is accepted.
#[test]
fn non_ieee_denormal_modes_under_fenv_access_are_refused_at_o0_and_o2() {
    let source = write_c(
        "fenv_indep_denorm",
        "#pragma STDC FENV_ACCESS ON\nvolatile float fa = 1.0f, fb = 2.0f, fr;\n\
         volatile double da = 1.0, db = 2.0, dr;\n\
         int main(void) { fr = fa + fb; dr = da + db; return 0; }\n",
    );
    let mode_cases: &[(&str, &[&str])] = &[
        ("preserve_sign", &["-fdenormal-fp-math=preserve-sign"]),
        ("positive_zero", &["-fdenormal-fp-math=positive-zero"]),
        ("dynamic", &["-fdenormal-fp-math=dynamic"]),
        ("f32_preserve_sign", &["-Xclang", "-fdenormal-fp-math-f32=preserve-sign"]),
    ];
    for &(tag, extra) in mode_cases {
        for (base, level) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
            let mut flags = base.to_vec();
            flags.extend_from_slice(extra);
            let error = match lower_text(&source, &format!("fenv_indep_denorm_{tag}_{level}"), &flags, &[]) {
                Ok(text) => panic!("{tag} {level} must be refused:\n{text}"),
                Err(error) => error,
            };
            assert!(error.contains("denormal-fp-math") && error.contains("not carried"), "{tag} {level}: {error}");
        }
    }
    for (base, level) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let mut flags = base.to_vec();
        flags.push("-fdenormal-fp-math=ieee");
        lower_text(&source, &format!("fenv_indep_denorm_ieee_{level}"), &flags, &[])
            .unwrap_or_else(|e| panic!("ieee denormal mode accepted at {level}: {e}"));
    }
}

/// Reading the floating-point flags through `__bennett_fegetunit` (as `fegetenv` does) makes
/// floating-point operations under a qubit branch a named refusal at both `-O0` and `-O2`, while a
/// program that never reads the flags keeps them accepted.
#[test]
fn reading_unit_flags_refuses_floating_point_under_qubit_control_at_o0_and_o2() {
    let source = write_c(
        "fenv_indep_qubit_flags",
        "#include \"bennett.h\"\nint __bennett_fegetunit(int);\n\
         unsigned long long f(double x) {\n  _Bool q = 0;\n  unsigned long long y = 0;\n  bennett_h(&q);\n  \
         if (q) {\n    double r = x + 1.0;\n    __builtin_memcpy(&y, &r, 8);\n  }\n  \
         return bennett_measure(&y) ^ bennett_measure(&q) ^ (unsigned)READ;\n}\n",
    );
    for (base, level) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let mut ok_flags = base.to_vec();
        ok_flags.push("-DREAD=0");
        lower_text(&source, &format!("fenv_indep_qubit_ok_{level}"), &ok_flags, &[])
            .unwrap_or_else(|e| panic!("accepted without flag read at {level}: {e}"));

        let mut bad_flags = base.to_vec();
        bad_flags.push("-DREAD=__bennett_fegetunit(0)");
        let error = match lower_text(&source, &format!("fenv_indep_qubit_bad_{level}"), &bad_flags, &[]) {
            Ok(text) => panic!("refused with unit flag read at {level}:\n{text}"),
            Err(error) => error,
        };
        assert!(
            error.contains("which may write memory, controlled by a value that depends on a qubit"),
            "{level}: {error}"
        );
    }
}

/// `__builtin_set_flt_rounds` (`llvm.set.rounding`) and `__builtin_flt_rounds` (`llvm.get.rounding`)
/// set and read all four rounding modes for `float`, `double`, and `long double` without `<fenv.h>`,
/// agreeing with native clang 18 at `-O0` and `-O2`.
#[test]
fn builtin_set_flt_rounds_agrees_with_native() {
    let source = write_c(
        "fenv_indep_set_flt_rounds",
        "#include <stdio.h>\n#include <string.h>\n#pragma STDC FENV_ACCESS ON\n\
         static volatile float f1 = 1.0f, f3 = 3.0f;\n\
         static volatile double d1 = 1.0, d3 = 3.0;\n\
         static volatile long double l1 = 1.0L, l3 = 3.0L;\n\
         int main(void) {\n\
           for (int mode = 0; mode < 4; ++mode) {\n\
             __builtin_set_flt_rounds(mode);\n\
             float fq = f1 / f3;\n\
             double dq = d1 / d3;\n\
             long double lq = l1 / l3;\n\
             unsigned fu = 0;\n\
             unsigned long long du = 0;\n\
             unsigned char lb[10];\n\
             memcpy(&fu, &fq, 4);\n\
             memcpy(&du, &dq, 8);\n\
             memcpy(lb, &lq, 10);\n\
             printf(\"%d %d %08x %016llx %02x%02x%02x\\n\", mode, __builtin_flt_rounds(), fu, du, lb[9], lb[1], lb[0]);\n\
           }\n\
           __builtin_set_flt_rounds(1);\n\
           return 0;\n\
         }\n",
    );
    let expected = native(&source, "fenv_indep_set_flt_rounds", &[]);
    for level in ["-O0", "-O2"] {
        let (outcome, printed) = device(&source, &format!("fenv_indep_set_flt_rounds{level}"), &[level]);
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{level}");
        assert_eq!(printed, expected, "{level}");
    }
}

/// `lroundl` and `llroundl` of `0x1.fffffffffffffffep62L` (`2^63 - 0.5L`) round away from zero to `+2^63`,
/// which overflows signed 64-bit `long`/`long long`. Native glibc raises `FE_INVALID` (in SSE); the device
/// wrapped the carry into `INT64_MIN` without raising any exception flag until `rt/softfloat/libgcc.c`'s
/// `round_to_integer` raised it too.
///
/// Expected (native clang 18 + glibc at `-O0` and `-O2`, with and without `-fno-math-errno`):
///   `lroundl inv=1 sse=01 x87=00`
///   `llroundl inv=1 sse=01 x87=00`
/// Observed on the device before the fix (`-O0`, `-O2`, and `-O2 -fno-math-errno`):
///   `lroundl inv=0 sse=00 x87=00`
///   `llroundl inv=0 sse=00 x87=00`
#[test]
fn lroundl_and_llroundl_at_two_to_63_minus_half_raise_invalid() {
    let source = write_c(
        "fenv_indep_lroundl_overflow",
        "#include <fenv.h>\n#include <math.h>\n#include <stdio.h>\n#pragma STDC FENV_ACCESS ON\n\
         #define FIVE_FLAGS (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)\n\
         static volatile long double x = 0x1.fffffffffffffffep62L;\n\
         static void check(const char *name, long long v) {\n\
           (void)v;\n  fenv_t e;\n  fegetenv(&e);\n\
           printf(\"%s inv=%d sse=%02x x87=%02x\\n\", name, fetestexcept(FE_INVALID) != 0,\n\
                  e.__mxcsr & FIVE_FLAGS, e.__status_word & FIVE_FLAGS);\n\
         }\n\
         int main(void) {\n\
           feclearexcept(FE_ALL_EXCEPT); check(\"lroundl\", lroundl(x));\n\
           feclearexcept(FE_ALL_EXCEPT); check(\"llroundl\", llroundl(x));\n\
           return 0;\n\
         }\n",
    );
    let expected = native(&source, "fenv_indep_lroundl_overflow", &[]);
    let mut mismatches = Vec::new();
    for (level, extra) in [("-O0", &[][..]), ("-O2", &[][..]), ("-O2", &["-fno-math-errno"][..])] {
        let mut flags = vec![level];
        flags.extend_from_slice(extra);
        let (outcome, printed) =
            device(&source, &format!("fenv_indep_lroundl_overflow{level}_{}", extra.len()), &flags);
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{level} {extra:?}");
        if printed != expected {
            mismatches.push(format!("{level} {extra:?}:\nobserved:\n{printed}expected:\n{expected}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
