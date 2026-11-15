//! Constructors run, or remain in linker arrays owned by C startup; none become inert data.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, assert_agrees_with_native, c_dir, entry, lower, lower_text, native_build, options,
    out_dir, write_c,
};
use std::process::Command;

#[test]
fn the_reduced_volatile_constructor_runs() {
    assert_agrees_with_native("ctor_not_run", 5);
}

#[test]
fn automatic_startup_orders_constructors_and_runs_the_destructor() {
    let source = c_dir().join("constructors.c");
    let native = native_build(&source, "constructors.native", &["-DNATIVE"]);
    let native = Command::new(native).output().expect("run native");
    assert_eq!(native.status.code(), Some(57));
    assert_eq!(native.stdout, b"12md");
    for (flags, tag) in [(CLANG_FLAGS, "constructors"), (CLANG_O2_FLAGS, "constructors.o2")] {
        let module = lower(&source, tag, flags, &[]);
        let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
        assert_eq!(executor.run(), Outcome::Returned(Some(57)));
        assert_eq!(executor.tape(), native.stdout);
    }
}

#[test]
fn standard_bounds_let_c_startup_own_both_arrays() {
    let source = write_c(
        "startup_owned",
        r#"
#include "bennett.h"
typedef void (*fn)(void);
extern fn __init_array_start[], __init_array_end[], __fini_array_start[], __fini_array_end[];
volatile int seed = 5;
int x;
__attribute__((constructor(202))) static void second(void) { x = x * 10 + 7; __bennett_effect('2'); }
__attribute__((constructor(101))) static void first(void) { x = seed; __bennett_effect('1'); }
__attribute__((destructor(101))) static void last(void) { __bennett_effect('a'); }
__attribute__((destructor(202))) static void before_last(void) { __bennett_effect('b'); }
static void middle(void) { __bennett_effect('s'); }
__attribute__((used, section(".init_array.150"))) static fn section_entry = middle;
static void tail(void) { __bennett_effect('z'); }
__attribute__((used, section(".fini_array"))) static fn section_tail = tail;
int main(void) {
    __bennett_effect('[');
    for (fn *p = __init_array_start; p != __init_array_end; ++p) (*p)();
    int result = x;
    __bennett_effect('m');
    for (fn *p = __fini_array_end; p != __fini_array_start;) (*--p)();
    __bennett_effect(']');
    return result;
}
"#,
    );
    let module = lower(&source, "startup_owned", CLANG_FLAGS, &[]);
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
    assert_eq!(executor.run(), Outcome::Returned(Some(57)));
    assert_eq!(executor.tape(), b"[1s2mzba]");
}

#[test]
fn empty_linker_arrays_have_equal_start_and_end() {
    let source = write_c(
        "startup_empty",
        r#"
typedef void (*fn)(void);
extern fn __init_array_start[], __init_array_end[], __fini_array_start[], __fini_array_end[];
int main(void) {
    return __init_array_start != __init_array_end || __fini_array_start != __fini_array_end;
}
"#,
    );
    let module = lower(&source, "startup_empty", CLANG_FLAGS, &[]);
    assert_eq!(bennett_front_tests::run(&module, "main", &[], 0), Outcome::Returned(Some(0)));
}

#[test]
fn automatic_startup_wraps_only_the_selected_entry() {
    let source = write_c(
        "startup_entry",
        r#"
#include "bennett.h"
volatile int seed = 5;
int x;
__attribute__((constructor)) static void init(void) { x = seed; __bennett_effect('i'); }
__attribute__((destructor)) static void fini(void) { __bennett_effect('f'); }
__attribute__((noinline)) int start(int n) { return n ? start(n - 1) + 1 : x; }
"#,
    );
    let module = lower(&source, "startup_entry", CLANG_FLAGS, &["--entry=start"]);
    let mut executor = Executor::new(&module, entry(&module, "start"), &[3], options(0)).expect("starts");
    assert_eq!(executor.run(), Outcome::Returned(Some(8)));
    assert_eq!(executor.tape(), b"if");
    let error = lower_text(&source, "startup_no_entry", CLANG_FLAGS, &[]).expect_err("must name the entry");
    assert!(error.contains("constructors/destructors without a defined entry @main"), "{error}");
}

#[test]
fn unsupported_linker_semantics_are_refused_by_name() {
    for (name, source, expected) in [
        (
            "startup_script",
            "void f(void) {}\n __attribute__((used,section(\".fini_array_custom\"))) void (*p)(void) = f;\nint main(void) {return 0;}",
            "no standard numeric priority",
        ),
        (
            "startup_address",
            "void f(void) {}\n __attribute__((section(\".init_array\"))) void (*p)(void) = f;\nvoid *volatile keep = &p;\nint main(void) {return 0;}",
            "address-taken constructor/destructor section object",
        ),
        (
            "startup_bound",
            "typedef void (*fn)(void); fn __init_array_start[1]; fn __init_array_end[1];\nint main(void) {return __init_array_start != __init_array_end;}",
            "linker array bound @__init_array_start is already defined",
        ),
        (
            "startup_old",
            "void f(void) {}\n __attribute__((used,section(\".ctors\"))) void (*p)(void) = f;\nint main(void) {return 0;}",
            "startup section .ctors",
        ),
    ] {
        let source = write_c(name, source);
        let error = lower_text(&source, name, CLANG_FLAGS, &[]).expect_err("must refuse");
        assert!(error.contains(expected), "{name}: {error}");
    }
    let path = out_dir().join("startup_associated.ll");
    std::fs::write(&path, "@x = global i8 0\n@llvm.global_ctors = appending global [1 x {i32, ptr, ptr}] [{i32, ptr, ptr} {i32 10, ptr @f, ptr @x}]\ndefine void @f() { ret void }\ndefine i32 @main() { ret i32 0 }\n").expect("write IR");
    let error = lower_text(&path, "startup_associated", CLANG_FLAGS, &[]).expect_err("must refuse");
    assert!(error.contains("associated data"), "{error}");
}

#[test]
fn the_device_exit_bypasses_automatic_cleanup() {
    // `_Exit` in a constructor: no later constructor, no entry, no destructor.
    let source = write_c(
        "startup_ctor_exit",
        r#"
#include "bennett.h"
__attribute__((constructor(101))) static void first(void) { __bennett_effect('1'); __bennett_exit(11); }
__attribute__((constructor(202))) static void second(void) { __bennett_effect('2'); }
__attribute__((destructor)) static void fini(void) { __bennett_effect('d'); }
int main(void) { __bennett_effect('m'); return 1; }
"#,
    );
    let module = lower(&source, "startup_ctor_exit", CLANG_FLAGS, &[]);
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
    assert_eq!(executor.run(), Outcome::Returned(Some(11)));
    assert_eq!(executor.tape(), b"1");
    // From the entry the destructors are skipped; from a destructor the exit status
    // replaces the entry's result and the destructors not yet run are skipped.
    for (tag, body, expected_tape) in [
        ("startup_main_exit", "int main(void) { __bennett_effect('m'); __bennett_exit(6); }", b"m".as_slice()),
        ("startup_dtor_exit", "int main(void) { __bennett_effect('m'); return 1; }", b"m2".as_slice()),
    ] {
        let status = if tag == "startup_main_exit" { 6 } else { 22 };
        let source = write_c(
            tag,
            &format!(
                "#include \"bennett.h\"\n__attribute__((destructor(101))) static void d1(void) {{ __bennett_effect('1'); }}\n__attribute__((destructor(202))) static void d2(void) {{ __bennett_effect('2');{} }}\n{body}\n",
                if tag == "startup_main_exit" { "" } else { " __bennett_exit(22);" }
            ),
        );
        let module = lower(&source, tag, CLANG_FLAGS, &[]);
        let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
        assert_eq!(executor.run(), Outcome::Returned(Some(status)), "{tag}");
        assert_eq!(executor.tape(), expected_tape, "{tag}");
    }
}

#[test]
fn automatic_startup_forwards_the_entrys_arguments() {
    let source = write_c(
        "startup_argc_argv",
        r#"
#include "bennett.h"
volatile int seed = 5;
int x;
__attribute__((constructor)) static void init(void) { x = seed; __bennett_effect('i'); }
int main(int argc, char **argv) { return x + argc + (argv != 0); }
"#,
    );
    let module = lower(&source, "startup_argc_argv", CLANG_FLAGS, &[]);
    let mut executor = Executor::new(&module, entry(&module, "main"), &[3, 0], options(0)).expect("starts");
    assert_eq!(executor.run(), Outcome::Returned(Some(8)));
    assert_eq!(executor.tape(), b"i");
}

// At equal priority the native program runs an explicit section contribution before the
// entries of `llvm.global_ctors`/`llvm.global_dtors`: LLVM emits the section globals first
// and lowers the special arrays after them (one `.init_array` in the native object, section
// entry first). The front collects the special arrays first, so each program below disagrees
// with its native build: expected tapes are the native ones, observed device tapes are
// "asm", "asm" and "msa".
#[test]
#[ignore = "section contributions run after same-priority global_ctors/dtors entries, opposite to native"]
fn section_contributions_precede_same_priority_special_entries() {
    for (tag, text, expected) in [
        (
            "startup_tie_init",
            "#include \"bennett.h\"\ntypedef void (*fn)(void);\nstatic void sec(void) { __bennett_effect('s'); }\n__attribute__((used, section(\".init_array\"))) static fn se = sec;\n__attribute__((constructor)) static void ca(void) { __bennett_effect('a'); }\nint main(void) { __bennett_effect('m'); return 0; }",
            b"sam".as_slice(),
        ),
        (
            "startup_tie_init_150",
            "#include \"bennett.h\"\ntypedef void (*fn)(void);\nstatic void sec(void) { __bennett_effect('s'); }\n__attribute__((used, section(\".init_array.150\"))) static fn se = sec;\n__attribute__((constructor(150))) static void ca(void) { __bennett_effect('a'); }\nint main(void) { __bennett_effect('m'); return 0; }",
            b"sam".as_slice(),
        ),
        (
            "startup_tie_fini",
            "#include \"bennett.h\"\ntypedef void (*fn)(void);\nstatic void sec(void) { __bennett_effect('s'); }\n__attribute__((used, section(\".fini_array\"))) static fn se = sec;\n__attribute__((destructor)) static void da(void) { __bennett_effect('a'); }\nint main(void) { __bennett_effect('m'); return 0; }",
            b"mas".as_slice(),
        ),
    ] {
        let source = write_c(tag, text);
        let module = lower(&source, tag, CLANG_FLAGS, &[]);
        let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
        assert_eq!(executor.run(), Outcome::Returned(Some(0)), "{tag}");
        assert_eq!(executor.tape(), expected, "{tag}");
    }
}

// The only mention of a linker bound is an uncalled internal function, which the front's
// own pipeline deletes (it does not reach the device program). Startup ownership is decided
// before that pipeline, so the constructor is supplied as an array nobody walks and silently
// never runs: observed Returned(0) with tape "m". With no C startup left in the program the
// front must run the constructor itself (or refuse by name).
#[test]
#[ignore = "a bound reference the pipeline deletes still disables automatic constructors, silently"]
fn a_deleted_bound_reference_does_not_own_startup() {
    let path = out_dir().join("startup_dead_bound.ll");
    std::fs::write(
        &path,
        "declare void @__bennett_effect(i8)\n@__init_array_start = external global [0 x ptr]\n@seed = global i32 5\n@x = global i32 0\n@llvm.global_ctors = appending global [1 x {i32, ptr, ptr}] [{i32, ptr, ptr} {i32 65535, ptr @init, ptr null}]\ndefine internal ptr @dead() { ret ptr @__init_array_start }\ndefine void @init() { %v = load volatile i32, ptr @seed\n store i32 %v, ptr @x\n call void @__bennett_effect(i8 105)\n ret void }\ndefine i32 @main() { call void @__bennett_effect(i8 109)\n %v = load i32, ptr @x\n ret i32 %v }\n",
    )
    .expect("write IR");
    let module = lower(&path, "startup_dead_bound", CLANG_FLAGS, &[]);
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
    assert_eq!(executor.run(), Outcome::Returned(Some(5)));
    assert_eq!(executor.tape(), b"im");
}
