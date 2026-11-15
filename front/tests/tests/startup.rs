//! Constructors run, or remain in linker arrays owned by C startup; none become inert data.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, assert_agrees_with_native, c_dir, entry, lower, lower_text, native_build,
    native_build_with, options, out_dir, write_c,
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
// entries of `llvm.global_ctors`/`llvm.global_dtors` when clang emits the section globals first
// (it emits the special arrays last): they share one `.init_array` input section.
#[test]
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
// own optimiser deletes. No C startup is left to walk the arrays, so the front runs the
// constructor itself: ownership is decided on the references that survive the optimiser.
#[test]
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
    // The same from C: the only walk of the array is in a branch the optimiser folds away.
    let source = write_c(
        "startup_folded_bound",
        r#"
#include "bennett.h"
typedef void (*fn)(void);
extern fn __init_array_start[], __init_array_end[];
static int never; // never written: the optimiser folds it to 0
volatile int seed = 5;
int x;
__attribute__((constructor)) static void init(void) { x = seed; __bennett_effect('i'); }
__attribute__((destructor)) static void fini(void) { __bennett_effect('f'); }
int start(void) {
    if (never) for (fn *p = __init_array_start; p != __init_array_end; ++p) (*p)();
    __bennett_effect('m');
    return x;
}
"#,
    );
    let module = lower(&source, "startup_folded_bound", CLANG_FLAGS, &["--entry=start"]);
    let mut executor = Executor::new(&module, entry(&module, "start"), &[], options(0)).expect("starts");
    assert_eq!(executor.run(), Outcome::Returned(Some(5)));
    assert_eq!(executor.tape(), b"imf");
    // Decided after the optimiser, automatic startup still needs its entry.
    let error =
        lower_text(&source, "startup_folded_bound_no_entry", CLANG_FLAGS, &[]).expect_err("must name the entry");
    assert!(error.contains("constructors/destructors without a defined entry @main"), "{error}");
}

/// Runs `source` on the device and natively and checks both tapes are `expected` (each with
/// entry result 0). `native` builds the native program.
fn assert_tape_agrees(source: &std::path::Path, tag: &str, native: std::path::PathBuf, expected: &[u8]) {
    let output = Command::new(native).output().expect("run native");
    assert_eq!(output.status.code(), Some(0), "{tag}");
    assert_eq!(output.stdout, expected, "{tag}: native");
    for (flags, suffix) in [(CLANG_FLAGS, ""), (CLANG_O2_FLAGS, ".o2")] {
        let module = lower(source, &format!("{tag}{suffix}"), flags, &[]);
        let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
        assert_eq!(executor.run(), Outcome::Returned(Some(0)), "{tag}{suffix}");
        assert_eq!(executor.tape(), expected, "{tag}{suffix}");
    }
}

// The native order is the static link's: LLVM emits each global in module order into its object
// section (a `retain`ed one gets a section of its own; the special arrays' entries share the
// section of their priority's name), and GNU ld places numbered sections sorted by priority, ties
// by section name, before the plain sections in input order.
#[test]
fn startup_order_matches_the_native_link() {
    let source = write_c(
        "startup_link_order",
        r#"
#ifdef NATIVE
#include <stdio.h>
#define emit(c) ((void)putchar(c))
#else
#include "bennett.h"
#define emit(c) __bennett_effect(c)
#endif
typedef void (*fn)(void);
#define AT(name, ch, ...) static void name##_f(void) { emit(ch); } __attribute__((__VA_ARGS__)) static fn name = name##_f;
AT(a, 'a', used, section(".init_array"))
AT(b, 'b', retain, used, section(".init_array"))
__attribute__((constructor)) static void c(void) { emit('c'); }
AT(d, 'd', used, section(".init_array.150"))
__attribute__((constructor(150))) static void e(void) { emit('e'); }
AT(f, 'f', retain, used, section(".init_array.150"))
AT(g, 'g', used, section(".init_array.65535"))
AT(h, 'h', used, section(".init_array.101"))
AT(i, 'i', used, section(".init_array.0101"))
__attribute__((constructor(101))) static void j(void) { emit('j'); }
AT(A, 'A', used, section(".fini_array"))
AT(B, 'B', retain, used, section(".fini_array"))
__attribute__((destructor)) static void C(void) { emit('C'); }
AT(D, 'D', used, section(".fini_array.200"))
__attribute__((destructor(200))) static void E(void) { emit('E'); }
int main(void) { emit('m'); return 0; }
"#,
    );
    let native = native_build(&source, "startup_link_order", &["-DNATIVE"]);
    assert_tape_agrees(&source, "startup_link_order", native, b"ihjdefgacbmBCAED");
    // LLVM IR can place the special arrays before the section objects, and give a priority above
    // 65535, which LLVM clamps to the plain section.
    let effect =
        write_c("startup_native_effect", "#include <stdio.h>\nvoid __bennett_effect(char c) { putchar(c); }\n");
    let calls = "define internal void @f1() { call void @__bennett_effect(i8 49)\n ret void }\ndefine internal void @f2() { call void @__bennett_effect(i8 50)\n ret void }\ndefine internal void @f3() { call void @__bennett_effect(i8 51)\n ret void }\ndefine internal void @f4() { call void @__bennett_effect(i8 52)\n ret void }\ndefine internal void @f5() { call void @__bennett_effect(i8 53)\n ret void }\ndefine internal void @f6() { call void @__bennett_effect(i8 54)\n ret void }\ndefine internal void @f7() { call void @__bennett_effect(i8 55)\n ret void }\ndefine internal void @f8() { call void @__bennett_effect(i8 56)\n ret void }\ndefine i32 @main() { call void @__bennett_effect(i8 109)\n ret i32 0 }\ndeclare void @__bennett_effect(i8)\n";
    for (tag, globals, expected) in [
        (
            "startup_link_order_ir",
            "@r150 = internal global ptr @f1, section \".init_array.150\"\n@llvm.used = appending global [2 x ptr] [ptr @r150, ptr @rplain], section \"llvm.metadata\"\n@p150 = internal global ptr @f2, section \".init_array.150\"\n@llvm.global_ctors = appending global [3 x {i32, ptr, ptr}] [{i32, ptr, ptr} {i32 150, ptr @f3, ptr null}, {i32, ptr, ptr} {i32 65535, ptr @f4, ptr null}, {i32, ptr, ptr} {i32 70000, ptr @f8, ptr null}]\n@rplain = internal global ptr @f5, section \".init_array\"\n@n65535 = internal global ptr @f6, section \".init_array.65535\"\n@cplain = internal constant ptr @f7, section \".init_array\"\n@llvm.compiler.used = appending global [3 x ptr] [ptr @p150, ptr @n65535, ptr @cplain], section \"llvm.metadata\"\n",
            b"12364875m".as_slice(),
        ),
        (
            "startup_link_order_ir_fini",
            "@a = internal global ptr @f1, section \".init_array.101\"\n@b = internal global ptr @f2, section \".init_array.0101\"\n@c = internal global ptr @f3, section \".fini_array\"\n@llvm.global_dtors = appending global [1 x {i32, ptr, ptr}] [{i32, ptr, ptr} {i32 65535, ptr @f4, ptr null}]\n@d = internal global ptr @f5, section \".fini_array\"\n@llvm.used = appending global [1 x ptr] [ptr @d], section \"llvm.metadata\"\n@llvm.compiler.used = appending global [3 x ptr] [ptr @a, ptr @b, ptr @c], section \"llvm.metadata\"\n",
            b"21m543".as_slice(),
        ),
    ] {
        let path = out_dir().join(format!("{tag}.ll"));
        std::fs::write(&path, format!("{globals}{calls}")).expect("write IR");
        let native = native_build_with(&path, tag, &["-Wno-override-module"], &[effect.to_str().expect("UTF-8 path")]);
        assert_tape_agrees(&path, tag, native, expected);
    }
}
