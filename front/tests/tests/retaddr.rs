//! `__builtin_return_address` (`front/tests/c/retaddr.c`): each `ra_*` function on 32 inputs against the same
//! file compiled natively, from unoptimised and optimised clang output and with `--taint-all`; whole runs
//! un-run back to their start; refusal of a moved return-address call (`docs/DESIGN.md`, "Return addresses").

use std::collections::BTreeMap;
use std::process::Command;

use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, RUN_BUDGET, c_dir, entry, lower, lower_text, native_build, options, run, write_c,
};
use bennett_ir::ValidModule;

/// Level 1 natively reads the frame-pointer chain: without frame pointers the native program crashes or
/// reads another word (measured on x86-64 with clang 18 at -O2), and a sibling call skips a frame. So the
/// native reference keeps frame pointers and makes no sibling calls, which is what the device's frames give.
const NATIVE_FLAGS: &[&str] =
    &["-DNATIVE", "-fno-omit-frame-pointer", "-mno-omit-leaf-frame-pointer", "-fno-optimize-sibling-calls"];

/// `(function, input) -> result` from the native build.
fn native() -> BTreeMap<(String, u64), i32> {
    let executable = native_build(&c_dir().join("retaddr.c"), "retaddr", NATIVE_FLAGS);
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
fn every_return_address_agrees_with_the_native_build() {
    let expected = native();
    let names: std::collections::BTreeSet<&String> = expected.keys().map(|(name, _)| name).collect();
    assert_eq!(names.len(), 8, "every function is printed natively: {names:?}");
    let source = c_dir().join("retaddr.c");
    let builds: [(&[&str], &[&str], &str); 3] =
        [(CLANG_FLAGS, &[], ""), (CLANG_O2_FLAGS, &[], ".o2"), (CLANG_FLAGS, &["--taint-all"], ".taint-all")];
    for (clang_flags, front_flags, suffix) in builds {
        let module = lower(&source, &format!("retaddr{suffix}"), clang_flags, front_flags);
        for ((name, x), value) in &expected {
            assert_eq!(device(&module, name, *x), *value, "@{name}({x}) {clang_flags:?} {front_flags:?}");
        }
    }
}

/// The lowered program reads the device's frames, at the levels C asked for.
#[test]
fn return_addresses_are_device_instructions() {
    for (flags, suffix) in [(CLANG_FLAGS, ""), (CLANG_O2_FLAGS, ".o2")] {
        let text =
            lower_text(&c_dir().join("retaddr.c"), &format!("retaddr.text{suffix}"), flags, &[]).expect("lowers");
        let count = |needle: &str| text.lines().filter(|line| line.ends_with(needle)).count();
        assert!(count(" = retaddr 0") >= 3 && count(" = retaddr 1") == 1, "{flags:?}");
    }
}

/// Forward then back returns to exactly the state at entry.
#[test]
fn every_function_un_runs_to_the_start() {
    let module = lower(&c_dir().join("retaddr.c"), "retaddr.reverse", CLANG_O2_FLAGS, &[]);
    let names = ["ra_one_site", "ra_which_site", "ra_recursive", "ra_level_one", "ra_pr17377"];
    for name in names {
        for x in [0, 5, 13] {
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

/// Applying a caller-sensitive function even once would move it from its ordinary call site.
#[test]
fn a_return_address_under_qubit_control() {
    let source = write_c(
        "retaddr_controlled",
        "#include \"bennett.h\"\n__attribute__((noinline)) unsigned char f(unsigned char x) {\n  \
         void *r = __builtin_return_address(0);\n  return (unsigned char)(x + (r != 0));\n}\n\
         int main(void) {\n  unsigned char x = 0, y = 0;\n  bennett_h(bennett_bit(&x, 0));\n  y ^= f(x);\n  \
         return (int)(bennett_measure(&x) * 16 + bennett_measure(&y));\n}\n",
    );
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let error = lower_text(&source, "retaddr_controlled", flags, &[]).expect_err("named refusal");
        assert!(
            error.starts_with("not handled yet: moving a computation that can observe its caller's position"),
            "{flags:?}: {error}"
        );
    }
}
