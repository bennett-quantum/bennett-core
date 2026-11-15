//! Independent probes of constructor ordering and the post-optimiser startup decision.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, entry, lower, lower_text, native_build_with, options, out_dir, write_c,
};
use std::process::Command;

const EMIT: &str = r#"
#ifdef NATIVE
#include <unistd.h>
#include <stdlib.h>
#define emit(c) do { char byte = (c); write(1, &byte, 1); } while (0)
#define finish(c) _Exit(c)
#else
#include "bennett.h"
#define emit(c) __bennett_effect(c)
#define finish(c) __bennett_exit(c)
#endif
typedef void (*fn)(void);
"#;
const BOUNDS: &str = "extern fn __init_array_start[], __init_array_end[], __fini_array_start[], __fini_array_end[];\n";

// No CRT: the C startup under test is the only owner in both builds. The assembly
// aligns the stack before calling an ordinary C entry and exits with its result.
fn native_owned(source: &std::path::Path, tag: &str, selected: &str) -> std::path::PathBuf {
    let stub = out_dir().join(format!("{tag}.S"));
    std::fs::write(&stub, format!(".text\n.globl _start\n_start:\n xor %ebp,%ebp\n andq $-16,%rsp\n call {selected}\n mov %eax,%edi\n mov $60,%eax\n syscall\n.section .note.GNU-stack,\"\",@progbits\n")).expect("write native entry");
    native_build_with(source, tag, &["-DNATIVE", "-nostartfiles", "-fuse-ld=bfd"], &[stub.to_str().expect("path")])
}

fn compare_owned(tag: &str, body: &str, expected: &[u8], status: u128, selected: &str) {
    compare_owned_source(tag, &format!("{EMIT}{BOUNDS}\n{body}"), expected, status, selected);
}

fn compare_owned_source(tag: &str, text: &str, expected: &[u8], status: u128, selected: &str) {
    let source = write_c(tag, text);
    let native = native_owned(&source, tag, selected);
    let native = Command::new(native).output().expect("run native");
    assert_eq!(native.status.code(), Some(status as i32), "{tag} native");
    assert_eq!(native.stdout, expected, "{tag} native");
    for (flags, suffix) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let module = lower(&source, &format!("{tag}_{suffix}"), flags, &[&format!("--entry={selected}")]);
        let mut exec = Executor::new(&module, entry(&module, selected), &[], options(0)).expect("start");
        let outcome = exec.run();
        assert_eq!((outcome, exec.tape()), (Outcome::Returned(Some(status)), expected), "{tag} {suffix}");
    }
}

#[test]
fn ownership_survives_pointer_indirection() {
    for (tag, setup, first, last) in [
        ("direct", "", "__init_array_start", "__init_array_end"),
        ("global", "fn *volatile first = __init_array_start, *volatile last = __init_array_end;", "first", "last"),
        (
            "function",
            "static fn *begin(void) { return __init_array_start; } static fn *end(void) { return __init_array_end; } static fn *(*volatile first)(void) = begin; static fn *(*volatile last)(void) = end;",
            "first()",
            "last()",
        ),
        (
            "integer",
            "#include <stdint.h>\nstatic uintptr_t volatile first = (uintptr_t)__init_array_start, last = (uintptr_t)__init_array_end;",
            "(fn *)first",
            "(fn *)last",
        ),
    ] {
        compare_owned(
            &format!("independent_owned_{tag}"),
            &format!(
                r#"
volatile int seed = 5;
int x;
__attribute__((constructor)) static void init(void) {{ x = seed; emit('i'); }}
__attribute__((destructor)) static void fini(void) {{ emit('f'); }}
{setup}
int start(void) {{
    emit('[');
    for (fn *p = {first}; p != {last}; ++p) (*p)();
    emit('m');
    for (fn *p = __fini_array_end; p != __fini_array_start;) (*--p)();
    emit(']');
    return x;
}}
"#
            ),
            b"[imf]",
            5,
            "start",
        );
    }
}

#[test]
fn fini_bounds_alone_own_both_arrays() {
    compare_owned(
        "independent_fini_only",
        r#"
volatile int x;
__attribute__((constructor)) static void init(void) { x = 5; emit('i'); }
__attribute__((destructor(101))) static void low(void) { emit('l'); }
__attribute__((destructor(202))) static void high(void) { emit('h'); }
int main(void) { emit('M'); return 99; }
int start(void) {
    emit('m');
    for (fn *p = __fini_array_end; p != __fini_array_start;) (*--p)();
    return x;
}
"#,
        b"mhl",
        0,
        "start",
    );
}

#[test]
fn owned_cleanup_interleaves_callbacks_and_destructors_after_exit() {
    compare_owned(
        "independent_owned_exit",
        r#"
static fn callbacks[8];
static int n;
static void register_callback(fn f) { callbacks[n++] = f; }
static void later(void) { emit('L'); }
static void cb(void) { emit('B'); register_callback(later); }
static void at_start(void) { emit('A'); }
__attribute__((constructor)) static void init(void) { emit('i'); register_callback(at_start); }
__attribute__((destructor(101))) static void low(void) { emit('l'); }
__attribute__((destructor(202))) static void high(void) { emit('h'); }
static void cleanup(int status) {
    while (n) callbacks[--n]();
    for (fn *p = __fini_array_end; p != __fini_array_start;) (*--p)();
    finish(status);
}
static int body(void) { emit('m'); register_callback(cb); cleanup(17); return 0; }
int start(void) {
    for (fn *p = __init_array_start; p != __init_array_end; ++p) (*p)();
    return body();
}
"#,
        b"imBLAhl",
        17,
        "start",
    );
}

#[test]
fn out_of_range_priorities_are_clamped_before_sorting() {
    let tag = "independent_priority_clamp";
    let source = out_dir().join(format!("{tag}.ll"));
    std::fs::write(
        &source,
        r#"
@llvm.global_ctors = appending global [4 x {i32, ptr, ptr}] [
{i32, ptr, ptr} {i32 70000, ptr @large, ptr null},
{i32, ptr, ptr} {i32 65535, ptr @normal, ptr null},
{i32, ptr, ptr} {i32 65536, ptr @next, ptr null},
{i32, ptr, ptr} {i32 -1, ptr @maximum, ptr null}]
declare void @__bennett_effect(i8)
define internal void @large() { call void @__bennett_effect(i8 55)
ret void }
define internal void @normal() { call void @__bennett_effect(i8 54)
ret void }
define internal void @next() { call void @__bennett_effect(i8 56)
ret void }
define internal void @maximum() { call void @__bennett_effect(i8 57)
ret void }
define i32 @main() { call void @__bennett_effect(i8 109)
ret i32 0 }
"#,
    )
    .expect("write IR");
    let effect =
        write_c(&format!("{tag}_effect"), "#include <unistd.h>\nvoid __bennett_effect(char c) { write(1, &c, 1); }\n");
    let native =
        native_build_with(&source, tag, &["-Wno-override-module", "-fuse-ld=bfd"], &[effect.to_str().expect("path")]);
    let native = Command::new(native).output().expect("run native");
    assert!(native.status.success());
    assert_eq!(native.stdout, b"7689m");
    let module = lower(&source, tag, CLANG_FLAGS, &[]);
    let mut exec = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("start");
    assert_eq!(exec.run(), Outcome::Returned(Some(0)));
    assert_eq!(exec.tape(), native.stdout);
}

// A live read of an array element must not lose C ownership when the optimiser could fold it into a direct call.
// The bounds stay undefined through the optimiser, so there is nothing to fold (the check of e13a6c8 found the
// constructor or destructor run twice here).
#[test]
fn folding_an_init_bound_call_must_not_add_automatic_startup() {
    for (tag, call) in [("start", "__init_array_start[0]()"), ("end", "__init_array_end[-1]()")] {
        compare_owned(
            &format!("independent_folded_init_{tag}"),
            &format!(
                r#"
volatile int x;
__attribute__((constructor)) static void init(void) {{ ++x; emit('i'); }}
__attribute__((destructor)) static void fini(void) {{ emit('f'); }}
int start(void) {{ {call}; emit('m'); return x; }}
"#
            ),
            b"im",
            1,
            "start",
        );
    }
}

#[test]
fn folding_a_fini_bound_call_must_not_add_automatic_cleanup() {
    for (tag, call) in [("start", "__fini_array_start[0]()"), ("end", "__fini_array_end[-1]()")] {
        compare_owned(
            &format!("independent_folded_fini_{tag}"),
            &format!(
                r#"
volatile int x;
__attribute__((constructor)) static void init(void) {{ x = 5; emit('i'); }}
__attribute__((destructor)) static void fini(void) {{ emit('f'); }}
int start(void) {{ {call}; emit('m'); return x; }}
"#
            ),
            b"fm",
            0,
            "start",
        );
    }
}

// The deleted fini walk does not matter: the live, foldable init call owns both arrays.
#[test]
fn a_folded_init_call_owns_startup_when_the_fini_walk_is_deleted() {
    compare_owned(
        "independent_folded_init_dead_fini",
        r#"
volatile int x;
static int never;
__attribute__((constructor)) static void init(void) { ++x; emit('i'); }
__attribute__((destructor)) static void fini(void) { emit('f'); }
int start(void) {
    __init_array_start[0]();
    emit('m');
    if (never) for (fn *p = __fini_array_end; p != __fini_array_start;) (*--p)();
    return x;
}
"#,
        b"im",
        1,
        "start",
    );
}

// musl's own startup and exit loops (src/env/__libc_start_main.c, src/exit/exit.c): weak hidden scalar bounds walked
// as integers. With one entry per array an optimiser that knew the arrays could unroll each loop and fold its load
// into a direct call; the bounds stay undefined through the optimiser, so the loops reach the device intact.
#[test]
fn musl_shaped_walks_of_one_entry_arrays_own_startup() {
    compare_owned_source(
        "independent_musl_shape",
        &format!(
            r#"{EMIT}
#include <stdint.h>
#define weak __attribute__((__weak__))
#define hidden __attribute__((__visibility__("hidden")))
extern weak hidden void (*const __init_array_start)(void), (*const __init_array_end)(void);
extern weak hidden void (*const __fini_array_start)(void), (*const __fini_array_end)(void);
static void libc_start_init(void) {{
    uintptr_t a = (uintptr_t)&__init_array_start;
    for (; a<(uintptr_t)&__init_array_end; a+=sizeof(void(*)()))
        (*(void (**)(void))a)();
}}
static void libc_exit_fini(void) {{
    uintptr_t a = (uintptr_t)&__fini_array_end;
    for (; a>(uintptr_t)&__fini_array_start; a-=sizeof(void(*)()))
        (*(void (**)())(a-sizeof(void(*)())))();
}}
volatile int x;
__attribute__((constructor)) static void init(void) {{ ++x; emit('i'); }}
__attribute__((destructor)) static void fini(void) {{ emit('f'); }}
int start(void) {{ libc_start_init(); emit('m'); libc_exit_fini(); return x; }}
"#
        ),
        b"imf",
        1,
        "start",
    );
}

fn compare_automatic(tag: &str, body: &str, expected: &[u8], status: u128, selected: &str) {
    let native_main = if selected == "main" {
        String::new()
    } else {
        format!("\n#ifdef NATIVE\nint main(void) {{ return {selected}(); }}\n#endif\n")
    };
    let source = write_c(tag, &format!("{EMIT}{BOUNDS}\n{body}{native_main}"));
    let native = native_build_with(&source, tag, &["-DNATIVE", "-fuse-ld=bfd"], &[]);
    let native = Command::new(native).output().expect("run native");
    assert_eq!(native.status.code(), Some(status as i32), "{tag} native");
    assert_eq!(native.stdout, expected, "{tag} native");
    for (flags, suffix) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
        let module = lower(&source, &format!("{tag}_{suffix}"), flags, &[&format!("--entry={selected}")]);
        let mut exec = Executor::new(&module, entry(&module, selected), &[], options(0)).expect("start");
        assert_eq!(exec.run(), Outcome::Returned(Some(status)), "{tag} {suffix}");
        assert_eq!(exec.tape(), expected, "{tag} {suffix}");
    }
}

#[test]
fn deleted_bound_paths_restore_automatic_startup() {
    for (tag, setup, walk) in [
        (
            "branch",
            "static int never;",
            "if (never) for (fn *p = __init_array_start; p != __init_array_end; ++p) (*p)();",
        ),
        (
            "fini_branch",
            "static int never;",
            "if (never) for (fn *p = __fini_array_start; p != __fini_array_end; ++p) (*p)();",
        ),
        ("pointer", "static fn *p = __init_array_start;", "(void)p;"),
        (
            "call",
            "static int never; static void walk(void) { for (fn *p = __init_array_start; p != __init_array_end; ++p) (*p)(); }",
            "if (never) walk();",
        ),
    ] {
        compare_automatic(
            &format!("independent_deleted_{tag}"),
            &format!(
                r#"
volatile int seed = 5;
int x;
__attribute__((constructor)) static void init(void) {{ x = seed; emit('i'); }}
__attribute__((destructor)) static void fini(void) {{ emit('f'); }}
{setup}
int start(void) {{ {walk} emit('m'); return x; }}
"#
            ),
            b"imf",
            5,
            "start",
        );
    }
}

#[test]
fn no_bound_programs_keep_automatic_startup_and_exit_behavior() {
    for (tag, extra, body, expected, status) in [
        ("return", "", "emit('m'); return x;", b"12mba".as_slice(), 57),
        ("exit", "", "emit('m'); finish(17);", b"12m".as_slice(), 17),
        ("ctor_exit", "finish(19);", "emit('m'); return x;", b"1".as_slice(), 19),
    ] {
        compare_automatic(
            &format!("independent_automatic_{tag}"),
            &format!(
                r#"
volatile int seed = 5;
int x;
__attribute__((constructor(202))) static void second(void) {{ x = x * 10 + 7; emit('2'); }}
__attribute__((constructor(101))) static void first(void) {{ x = seed; emit('1'); {extra} }}
__attribute__((destructor(101))) static void low(void) {{ emit('a'); }}
__attribute__((destructor(202))) static void high(void) {{ emit('b'); }}
int main(void) {{ {body} }}
"#
            ),
            expected,
            status,
            "main",
        );
    }
}

#[test]
fn reordered_section_mixtures_match_the_same_native_module() {
    let effect =
        write_c("independent_ir_effect", "#include <unistd.h>\nvoid __bennett_effect(char c) { write(1, &c, 1); }\n");
    for seed in 0..6 {
        let mut globals = Vec::new();
        let mut retained = Vec::new();
        let mut used = Vec::new();
        let mut functions = String::from("declare void @__bennett_effect(i8)\n");
        for (kind, base) in [("init", 0), ("fini", 18)] {
            for (i, suffix) in
                [".0", ".1", ".0101", ".101", ".150", ".65535", "", ".00000", ".00150"].into_iter().enumerate()
            {
                for retained_object in [false, true] {
                    let id = base + i * 2 + usize::from(retained_object);
                    let storage = if i % 2 == 0 { "constant" } else { "global" };
                    globals.push(format!(
                        "@p{id} = internal {storage} ptr @f{id}, section \".{kind}_array{suffix}\", align 8\n"
                    ));
                    if retained_object {
                        retained.push(format!("ptr @p{id}"));
                    } else {
                        used.push(format!("ptr @p{id}"));
                    }
                }
            }
        }
        for kind in ["ctors", "dtors"] {
            let base = if kind == "ctors" { 36 } else { 41 };
            let records: Vec<_> = [70000_u32, 101, 65536, 65535, u32::MAX]
                .into_iter()
                .enumerate()
                .map(|(i, priority)| format!("{{i32, ptr, ptr}} {{i32 {priority}, ptr @f{}, ptr null}}", base + i))
                .collect();
            globals.push(format!(
                "@llvm.global_{kind} = appending global [5 x {{i32, ptr, ptr}}] [{}]\n",
                records.join(",")
            ));
        }
        globals.rotate_left(seed * 5);
        if seed % 2 != 0 {
            globals.reverse();
        }
        for id in 0..46 {
            functions.push_str(&format!(
                "define internal void @f{id}() {{ call void @__bennett_effect(i8 {})\n ret void }}\n",
                33 + id
            ));
        }
        functions.push_str("define i32 @main() { call void @__bennett_effect(i8 109)\n ret i32 0 }\n");
        let lists = format!(
            "@llvm.used = appending global [{} x ptr] [{}], section \"llvm.metadata\"\n@llvm.compiler.used = appending global [{} x ptr] [{}], section \"llvm.metadata\"\n",
            retained.len(),
            retained.join(","),
            used.len(),
            used.join(",")
        );
        let tag = format!("independent_ir_order_{seed}");
        let source = out_dir().join(format!("{tag}.ll"));
        std::fs::write(&source, format!("{}{lists}{functions}", globals.concat())).expect("write IR");
        let native = native_build_with(
            &source,
            &tag,
            &["-Wno-override-module", "-fuse-ld=bfd"],
            &[effect.to_str().expect("path")],
        );
        let native = Command::new(native).output().expect("run native");
        assert!(native.status.success(), "{tag}: native {:?}", native.status);
        for (flags, suffix) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
            let module = lower(&source, &format!("{tag}_{suffix}"), flags, &[]);
            let mut exec = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("start");
            assert_eq!(exec.run(), Outcome::Returned(Some(0)), "{tag}_{suffix}");
            assert_eq!(exec.tape(), native.stdout, "{tag}_{suffix}");
        }
    }
}

#[test]
fn several_translation_units_match_native_linked_bitcode() {
    let effect =
        write_c("independent_tu_effect", "#include <unistd.h>\nvoid __bennett_effect(char c) { write(1, &c, 1); }\n");
    for ties in [false, true] {
        let mut units = Vec::new();
        for (id, special_priority, section_priority, ch) in [(0, 101, 150, 'a'), (1, 202, 250, 'b')] {
            let priority = if ties { 101 } else { special_priority };
            let section = if ties { 101 } else { section_priority };
            let retained = if id == 0 { "" } else { "retain," };
            let source = write_c(
                &format!("independent_tu_{ties}_{id}"),
                &format!(
                    r#"
extern void __bennett_effect(char);
typedef void (*fn)(void);
__attribute__((constructor({priority}))) static void init(void) {{ __bennett_effect('{ch}'); }}
__attribute__((destructor({priority}))) static void fini(void) {{ __bennett_effect('{}'); }}
static void sec_init(void) {{ __bennett_effect('{}'); }}
static void sec_fini(void) {{ __bennett_effect('{}'); }}
__attribute__((used, {retained} section(".init_array.{section}"))) static fn pi = sec_init;
__attribute__((used, {retained} section(".fini_array.{section}"))) static fn pf = sec_fini;
"#,
                    ch.to_ascii_uppercase(),
                    if id == 0 { 's' } else { 't' },
                    if id == 0 { 'S' } else { 'T' }
                ),
            );
            units.push(source);
        }
        units.push(write_c(
            &format!("independent_tu_{ties}_main"),
            "extern void __bennett_effect(char);\nint main(void) { __bennett_effect('m'); return 0; }\n",
        ));
        for (flags, suffix) in [(CLANG_FLAGS, "o0"), (CLANG_O2_FLAGS, "o2")] {
            let bitcode: Vec<_> = units
                .iter()
                .enumerate()
                .map(|(i, source)| {
                    bennett_front_tests::bitcode(source, &format!("independent_tu_{ties}_{i}_{suffix}"), flags)
                })
                .collect();
            for reverse in [false, true] {
                let tag = format!("independent_tu_{ties}_{suffix}_{reverse}");
                let linked = out_dir().join(format!("{tag}.bc"));
                let mut objects = bitcode.clone();
                if reverse {
                    objects.reverse();
                }
                let link =
                    Command::new("llvm-link-18").args(objects).arg("-o").arg(&linked).output().expect("llvm-link-18");
                assert!(link.status.success(), "{}", String::from_utf8_lossy(&link.stderr));
                let native = native_build_with(&linked, &tag, &["-fuse-ld=bfd"], &[effect.to_str().expect("path")]);
                let native = Command::new(native).output().expect("run native");
                assert!(native.status.success());
                let module = lower(&linked, &format!("{tag}_device"), CLANG_FLAGS, &[]);
                let mut exec = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("start");
                assert_eq!(exec.run(), Outcome::Returned(Some(0)), "{tag}");
                assert_eq!(exec.tape(), native.stdout, "{tag}");
                // Distinct priorities also have the same order when compiled as separate objects.
                if !ties {
                    let separate = native_build_with(
                        &units[0],
                        &format!("{tag}_separate"),
                        &["-fuse-ld=bfd"],
                        &[
                            units[1].to_str().expect("path"),
                            units[2].to_str().expect("path"),
                            effect.to_str().expect("path"),
                        ],
                    );
                    let separate = Command::new(separate).output().expect("run separate native objects");
                    assert!(separate.status.success());
                    assert_eq!(separate.stdout, native.stdout, "{tag} separate objects");
                }
            }
        }
    }
}

#[test]
fn real_library_atexit_and_exit_are_still_named_debts() {
    for ending in ["return 17;", "exit(17);"] {
        let tag = if ending.starts_with("return") { "independent_atexit_return" } else { "independent_atexit_exit" };
        let source = write_c(
            tag,
            &format!(
                r#"
{EMIT}{BOUNDS}
#ifndef NATIVE
extern int atexit(fn);
extern _Noreturn void exit(int);
#endif
static void a(void) {{ emit('A'); }}
static void b(void) {{ emit('B'); }}
__attribute__((constructor)) static void init(void) {{ emit('i'); atexit(a); }}
__attribute__((destructor)) static void fini(void) {{ emit('f'); }}
int main(void) {{ emit('m'); atexit(b); {ending} }}
"#
            ),
        );
        let native = native_build_with(&source, tag, &["-DNATIVE", "-fuse-ld=bfd"], &[]);
        let native = Command::new(native).output().expect("run native");
        assert_eq!(native.status.code(), Some(17));
        assert_eq!(native.stdout, b"imBAf");
        let error = lower_text(&source, tag, CLANG_FLAGS, &[]).expect_err("no C runtime is linked");
        assert!(error.contains("@atexit") && error.contains("has no definition"), "{error}");
    }
}

#[test]
fn associated_section_objects_are_refused_by_name() {
    let tag = "independent_associated_section";
    let source = out_dir().join(format!("{tag}.ll"));
    std::fs::write(
        &source,
        r#"
@x = global i8 0
@p = internal global ptr @f, section ".init_array", !associated !0
@llvm.compiler.used = appending global [1 x ptr] [ptr @p], section "llvm.metadata"
define void @f() { ret void }
define i32 @main() { ret i32 0 }
!0 = !{ptr @x}
"#,
    )
    .expect("write IR");
    let error = lower_text(&source, tag, CLANG_FLAGS, &[]).expect_err("must refuse associated section");
    assert!(error.contains("!associated") && error.contains("@p"), "{error}");
}

#[test]
fn a_surviving_fini_walk_owns_startup_when_the_init_walk_is_deleted() {
    compare_owned(
        "independent_partly_deleted",
        r#"
static int never;
volatile int x;
__attribute__((constructor)) static void init(void) { x = 5; emit('i'); }
__attribute__((destructor)) static void fini(void) { emit('f'); }
int start(void) {
    if (never) for (fn *p = __init_array_start; p != __init_array_end; ++p) (*p)();
    emit('m');
    for (fn *p = __fini_array_end; p != __fini_array_start;) (*--p)();
    return x;
}
"#,
        b"mf",
        0,
        "start",
    );
}
