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
