//! `setjmp` and `longjmp` (`front/tests/c/setjmp.c`): each `jump_*` function on 32 inputs against the same
//! file compiled natively, from unoptimised and optimised clang output and with `--taint-all`; whole runs
//! un-run back to their start; and what is refused, by name.

use std::collections::BTreeMap;
use std::process::Command;

use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, RUN_BUDGET, c_dir, entry, lower, lower_text, native_build, options, run, write_c,
};
use bennett_ir::ValidModule;

/// `(function, input) -> result` from the native build.
fn native() -> BTreeMap<(String, u64), i32> {
    let executable = native_build(&c_dir().join("setjmp.c"), "setjmp", &["-DNATIVE"]);
    let output = Command::new(&executable).output().unwrap_or_else(|e| panic!("cannot run {executable:?}: {e}"));
    assert!(output.status.success(), "the native build failed");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let parse = |s: &str| s.parse::<i64>().unwrap_or_else(|e| panic!("{line:?}: {e}"));
            ((fields[0].to_owned(), parse(fields[1]) as u64), parse(fields[2]) as i32)
        })
        .collect()
}

fn device(module: &ValidModule, name: &str, x: u64) -> i32 {
    match run(module, name, &[x], 0) {
        Outcome::Returned(Some(value)) => value as u32 as i32,
        other => panic!("@{name}({x}): {other:?}"),
    }
}

#[test]
fn every_jump_agrees_with_the_native_build() {
    let expected = native();
    let names: std::collections::BTreeSet<&String> = expected.keys().map(|(name, _)| name).collect();
    assert_eq!(names.len(), 10, "every function is printed natively: {names:?}");
    let source = c_dir().join("setjmp.c");
    let builds: [(&[&str], &[&str], &str); 3] =
        [(CLANG_FLAGS, &[], ""), (CLANG_O2_FLAGS, &[], ".o2"), (CLANG_FLAGS, &["--taint-all"], ".taint-all")];
    for (clang_flags, front_flags, suffix) in builds {
        let module = lower(&source, &format!("setjmp{suffix}"), clang_flags, front_flags);
        for ((name, x), value) in &expected {
            assert_eq!(device(&module, name, *x), *value, "@{name}({x}) {clang_flags:?} {front_flags:?}");
        }
    }
}

/// The lowered program uses the device's own instructions, not calls.
#[test]
fn the_jumps_are_device_instructions() {
    let text = lower_text(&c_dir().join("setjmp.c"), "setjmp.text", CLANG_FLAGS, &[]).expect("lowers");
    let setjmps = text.lines().filter(|line| line.contains(" = setjmp ")).count();
    let longjmps = text.lines().filter(|line| line.trim_start().starts_with("longjmp ")).count();
    assert!(setjmps >= 11 && longjmps >= 11, "{setjmps} setjmp, {longjmps} longjmp");
    assert!(!text.contains("call @setjmp") && !text.contains("call @longjmp"));
}

/// Forward then back returns to exactly the state at entry, jumps and discarded frames included.
#[test]
fn every_jump_un_runs_to_the_start() {
    let module = lower(&c_dir().join("setjmp.c"), "setjmp.reverse", CLANG_FLAGS, &[]);
    let names = ["jump_across_frames", "jump_repeatedly", "jump_nested", "jump_frees_allocations", "jump_in_a_loop"];
    for name in names {
        for x in [0, 5, 11] {
            let options = Options { reversible: true, budget: Some(RUN_BUDGET), ..options(0) };
            let mut executor = Executor::new(&module, entry(&module, name), &[x], options).expect("starts");
            let start = executor.snapshot();
            let outcome = executor.run();
            assert!(matches!(outcome, Outcome::Returned(Some(_))), "@{name}({x}): {outcome:?}");
            let steps = executor.stats().steps;
            assert_eq!(executor.reverse().expect("reverses"), steps, "@{name}({x})");
            assert_eq!(executor.snapshot().classical, start.classical, "@{name}({x})");
        }
    }
}

/// A jump to a function that has returned is undefined in C; on the device it is the named fault when no
/// frame of that function stands where the record says (here a deeper frame of another function does).
#[test]
fn a_jump_to_a_returned_function_is_a_named_fault() {
    let source = write_c(
        "setjmp_returned",
        "#include <setjmp.h>\nstatic jmp_buf b;\n__attribute__((noinline)) int set(void) { return setjmp(b); }\n\
         __attribute__((noinline)) void go(int n) { volatile int pad[4]; pad[0] = n; if (n) go(n - 1); else \
         longjmp(b, 1); }\nint main(void) {\n  set();\n  go(3);\n  return 0;\n}\n",
    );
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = lower(&source, "setjmp_returned", flags, &[]);
        assert_eq!(run(&module, "main", &[], 0), Outcome::Faulted(bennett_ir::Fault::BadIndirect), "{flags:?}");
    }
}

#[test]
fn what_is_not_handled_is_refused_by_name() {
    let cases = [
        (
            "setjmp_savemask",
            "#include <setjmp.h>\nsigjmp_buf b;\nint main(void) {\n  if (sigsetjmp(b, 1)) return 1;\n  \
             siglongjmp(b, 1);\n}\n",
            "a call to @__sigsetjmp_tail, which has no definition",
            4,
        ),
        (
            "setjmp_address",
            "#include <setjmp.h>\nint (*volatile p)(struct __jmp_buf_tag *) = _setjmp;\nint main(void) {\n  \
             return 0;\n}\n",
            "the address of @_setjmp",
            2,
        ),
        // setjmp's effects are opaque to LLVM (it returns twice), so a function using it writes memory as
        // far as the front can tell and is not run coherently: a debt (docs/DESIGN.md, "Non-local jumps").
        (
            "setjmp_controlled",
            "#include \"bennett.h\"\n#include <setjmp.h>\n__attribute__((noinline)) unsigned char f(unsigned char x) \
             {\n  jmp_buf b;\n  int r = setjmp(b);\n  if (r == 0) longjmp(b, x + 1);\n  return (unsigned char)r;\n}\n\
             int main(void) {\n  unsigned char x = 0, y = 0;\n  bennett_h(bennett_bit(&x, 0));\n  y ^= f(x);\n  \
             return (int)(bennett_measure(&x) + bennett_measure(&y));\n}\n",
            "a call that depends on a qubit, to a function that writes memory",
            12,
        ),
        (
            "frame_address",
            "volatile void *p;\n__attribute__((noinline)) void f(void) { p = __builtin_frame_address(1); }\n\
             int main(void) {\n  f();\n  return 0;\n}\n",
            "the frame address of a caller",
            2,
        ),
    ];
    for (name, text, message, line) in cases {
        let source = write_c(name, text);
        for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
            let error = lower_text(&source, name, flags, &[]).expect_err(name);
            assert!(error.contains(message), "{name} {flags:?}: {error}");
            assert!(error.contains(&format!("{name}.c:{line}")) || !error.contains(".c:"), "{name}: {error}");
        }
    }
}
