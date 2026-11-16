//! Calls through a prototype that differs from the callee's definition: against native x86-64 where the
//! convention determines the result, refused or stopped by name where it does not.
use std::path::Path;
use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, lower, lower_text, native_build_with, run, write_c};
use bennett_ir::Fault;

const INPUTS: &[u64] =
    &[0, 1, 7, 0x7fff_ffff_ffff_ffff, 0x8000_0000_0000_0000, u64::MAX, 0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];
const SHAPES: usize = 21;
const MODES: usize = 4;

fn native(source: &Path, tag: &str, flags: &[&str]) -> String {
    let mut flags = flags.to_vec();
    flags.extend_from_slice(&["-DNATIVE", "-w"]);
    let exe = native_build_with(source, tag, &flags, &[]);
    let output = Command::new(&exe).output().expect("run native");
    assert!(output.status.success(), "{exe:?}");
    String::from_utf8(output.stdout).expect("text")
}

#[test]
fn calls_through_another_prototype_match_native() {
    let source = c_dir().join("prototypes.c");
    let expected = native(&source, "prototypes.native0", &["-O0"]);
    assert_eq!(expected, native(&source, "prototypes.native2", &["-O2"]), "native -O0 and -O2 must agree");
    let words: Vec<u64> = expected.lines().map(|s| u64::from_str_radix(s, 16).expect("hex word")).collect();
    assert_eq!(words.len(), SHAPES * MODES * INPUTS.len());
    for (flags, extra, suffix) in
        [(CLANG_FLAGS, &[][..], "o0"), (CLANG_O2_FLAGS, &[][..], "o2"), (CLANG_FLAGS, &["--taint-all"][..], "taint")]
    {
        let mut flags = flags.to_vec();
        flags.push("-w");
        let tag = format!("prototypes.{suffix}");
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
        eprintln!("{tag}: {} identical native/device words", words.len());
    }
}

/// Lowers `text` (a whole C program); returns the front's error, or the program.
fn front(name: &str, text: &str) -> Result<String, String> {
    let mut flags = CLANG_FLAGS.to_vec();
    flags.push("-w");
    lower_text(&write_c(name, text), name, &flags, &[])
}

#[test]
fn direct_calls_the_convention_does_not_determine_are_refused_by_name() {
    let cases = [
        // c-testsuite's mismatched-prototype reduction: the callee reads the upper half of a 32-bit argument.
        (
            "upper_half",
            "long f(long x) { return x; }\nint main(void) { return ((int (*)(int))f)(5) - 5; }\n",
            "parameter 1 of @f reads bits 32 to 63, which are not set",
        ),
        (
            "missing",
            "long f(long a, long b) { return a + b; }\nint main(void) { return ((long (*)(long))f)(1); }\n",
            "parameter 2 of @f, which is read and not passed",
        ),
        (
            "wide_result",
            "int f(int a) { return a; }\nint main(void) { return (int)((long (*)(int))f)(1); }\n",
            "the result of @f: bits 32 to 63, which are not set",
        ),
        (
            "void_result",
            "void f(int a) { (void)a; }\nint main(void) { return ((int (*)(int))f)(1); }\n",
            "the result of @f: a value that is not passed",
        ),
        (
            "float_to_double",
            "double f(double d) { return d; }\nint main(void) { return (int)((double (*)(float))f)(1.0f); }\n",
            "parameter 1 of @f reads bits 32 to 63, which are not set",
        ),
        (
            "class_cast",
            "long f(double d) { return (long)d; }\nint main(void) { return (int)((long (*)(long))f)(1); }\n",
            "native clang -O2 passes it as a cast",
        ),
        (
            "variadic_float",
            "long v(int n, ...) { return n; }\nint main(void) { return (int)((long (*)(int, float))v)(1, 2.0f); }\n",
            "a variadic floating-point argument of @v from a call without a variadic prototype",
        ),
        (
            "memory_result",
            "struct B { long a[4]; };\nstruct B f(long x) { struct B b = {{x, x, x, x}}; return b; }\n\
          int main(void) { return (int)((long (*)(long))f)(1); }\n",
            "a result @f returns in memory on one side only",
        ),
        (
            "long_double",
            "long double f(long double x) { return x; }\nint main(void) { return (int)((double (*)(double))f)(1.0); }\n",
            "passed in memory, where the caller passes something else",
        ),
    ];
    for (name, text, expected) in cases {
        let name = format!("refused_{name}");
        match front(&name, text) {
            Ok(program) => panic!("{name}: accepted\n{program}"),
            Err(message) => assert!(
                message.starts_with("not handled yet: a call of @")
                    && message.contains(expected)
                    && message.contains(".c:"),
                "{name}: {message}"
            ),
        }
    }
}

/// clang relies on a caller's extension of a narrow argument: natively a callee of `signed char` given the
/// int 300 returns 300 at -O2 and 44 at -O0. The device checks the extension when the call runs.
#[test]
fn a_narrow_argument_that_is_not_its_own_extension_stops_by_name() {
    let text = "__attribute__((noinline)) int f(signed char c) { return c; }\n\
                int (*volatile p)(int) = (int (*)(int))f;\n\
                int direct(int x) { return ((int (*)(int))f)(x); }\n\
                int indirect(int x) { return p(x); }\n\
                int unprototyped(int x) { return ((int (*)())f)(x); }\n";
    for (flags, tag) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let mut flags = flags.to_vec();
        flags.push("-w");
        let name = format!("narrow_check_{tag}");
        let module = lower(&write_c(&name, text), &name, &flags, &[]);
        for entry in ["direct", "indirect", "unprototyped"] {
            let minus = u64::from((-100i32).cast_unsigned());
            for (x, outcome) in [
                (44, Outcome::Returned(Some(44))),
                (minus, Outcome::Returned(Some(u128::from(minus)))),
                (300, Outcome::Faulted(Fault::Unreachable)),
                (200, Outcome::Faulted(Fault::Unreachable)),
            ] {
                assert_eq!(run(&module, entry, &[x], 0), outcome, "{name} {entry}({x})");
            }
        }
    }
}

/// Through a pointer, a target the convention does not determine stops with a named fault: `unreachable`
/// where the device call would run, `bad_indirect` (unchanged) where its widths differ. Never a value.
#[test]
fn undetermined_calls_through_pointers_stop_by_name() {
    let text = "__attribute__((noinline)) long f(double d) { return (long)d; }\n\
                __attribute__((noinline)) long g(long a, long b) { return a + b; }\n\
                __attribute__((noinline)) long h(long a) { return a + 1; }\n\
                long (*volatile p)(long);\n\
                long (*volatile q)();\n\
                long through(long which, long x) { p = which == 0 ? (long (*)(long))f : which == 1 ? \
                (long (*)(long))g : h; return p(x); }\n\
                long unprototyped(long which, long x) { q = which == 0 ? (long (*)())f : which == 1 ? \
                (long (*)())g : (long (*)())h; return q(x); }\n";
    for (flags, tag) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let mut flags = flags.to_vec();
        flags.push("-w");
        let name = format!("pointer_undetermined_{tag}");
        let module = lower(&write_c(&name, text), &name, &flags, &[]);
        for entry in ["through", "unprototyped"] {
            assert_eq!(run(&module, entry, &[0, 5], 0), Outcome::Faulted(Fault::Unreachable), "{name} {entry} f");
            assert_eq!(run(&module, entry, &[2, 5], 0), Outcome::Returned(Some(6)), "{name} {entry} h");
        }
        assert_eq!(run(&module, "through", &[1, 5], 0), Outcome::Faulted(Fault::BadIndirect), "{name} g");
        assert_eq!(run(&module, "unprototyped", &[1, 5], 0), Outcome::Faulted(Fault::BadIndirect), "{name} g");
    }
}
