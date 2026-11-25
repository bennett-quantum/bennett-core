//! The floating-point environment against the native program (clang 18 with glibc on x86-64): every operation
//! under each of the four rounding modes, its result's bits and the exception flags it raised, and `<fenv.h>`
//! itself, with musl linked on the device (`rt/musl/fenv.c`, the state in `rt/softfloat/libgcc.c`). The C
//! programs say what they compare (`front/tests/c/fenv_ops.c`, `fenv_api.c`).
use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{CLANG_FLAGS, lower_text, native_build_with, out_dir, root, write_c};
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

/// An environment that asks for what is not carried (a trap, x87 precision other than extended, flush to zero,
/// two rounding modes) is a named fault, never a success that changes nothing.
#[test]
fn an_environment_that_is_not_carried_is_a_named_fault() {
    for (name, change) in [
        ("fenv_trap", "env.__control_word &= ~FE_INEXACT;"),
        ("fenv_precision", "env.__control_word &= ~0x300;"),
        ("fenv_flush", "env.__mxcsr |= 0x8040;"),
        ("fenv_modes", "env.__mxcsr |= FE_UPWARD << 3;"),
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
