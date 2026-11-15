//! Ordinary device calls stay outside the quantum marker classification.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, c_dir, entry, lower, lower_text, options, write_c};

#[test]
fn c_console_bytes_are_preserved() {
    let module = lower(&c_dir().join("console.c"), "console", CLANG_FLAGS, &[]);
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
    assert_eq!(executor.run(), Outcome::Returned(Some(7)));
    assert_eq!(executor.tape(), b"hi\n\0\xff");
}

#[test]
fn console_callers_are_ordinary_effectful_functions() {
    let source = write_c(
        "console_controlled",
        r#"
#include "bennett.h"
__attribute__((noinline)) void emit(char c) { __bennett_effect(c); }
int main(void) {
    _Bool q = 0;
    bennett_h(&q);
    if (q) emit('x');
    return (int)bennett_measure(&q);
}
"#,
    );
    let error = lower_text(&source, "console_controlled", CLANG_FLAGS, &[]).expect_err("must refuse");
    assert!(error.contains("a call to @emit, which may write memory, controlled"), "{error}");
}

#[test]
fn console_declarations_are_checked_before_optimisation() {
    for (name, declaration) in [
        ("width", "void __bennett_effect(int);"),
        ("result", "int __bennett_effect(char);"),
        ("vararg", "void __bennett_effect(char, ...);"),
        ("defined", "void __bennett_effect(char c) {}"),
    ] {
        let tag = format!("console_bad_{name}");
        let source = write_c(
            &tag,
            &format!(
                "{declaration}\n__typeof__(&__bennett_effect) volatile keep = &__bennett_effect;\nint main(void) {{ return 0; }}"
            ),
        );
        let error = lower_text(&source, &tag, CLANG_FLAGS, &[]).expect_err("must refuse");
        assert!(error.starts_with("error:") && error.contains("@__bennett_effect"), "{error}");
    }
}

#[test]
fn console_attributes_cannot_hide_effects() {
    let path = bennett_front_tests::out_dir().join("console_pure.ll");
    std::fs::write(&path, "declare void @__bennett_effect(i8) memory(none)\ndefine i32 @main() {\n call void @__bennett_effect(i8 1)\n ret i32 0\n}\n").expect("write IR");
    let error = lower_text(&path, "console_pure", CLANG_FLAGS, &[]).expect_err("must refuse");
    assert!(error.contains("attributes that let LLVM remove or move its calls"), "{error}");
}

#[test]
fn c_exit_ends_nested_calls_and_loops() {
    let module = lower(&c_dir().join("exit.c"), "exit", CLANG_FLAGS, &[]);
    for (name, expected) in [("from_nested", b"n".as_slice()), ("from_loop", b"0123".as_slice())] {
        for status in [0, 37, 255, u64::from(u32::MAX)] {
            let mut executor = Executor::new(&module, entry(&module, name), &[status], options(0)).expect("starts");
            assert_eq!(executor.run(), Outcome::Returned(Some(u128::from(status))));
            assert_eq!(executor.tape(), expected);
        }
    }
}

#[test]
fn exit_declarations_require_noreturn_void_int() {
    for (name, declaration) in [
        ("width", "_Noreturn void __bennett_exit(char);"),
        ("result", "_Noreturn int __bennett_exit(int);"),
        ("vararg", "_Noreturn void __bennett_exit(int, ...);"),
        ("returns", "void __bennett_exit(int);"),
        ("defined", "_Noreturn void __bennett_exit(int x) { for (;;) {} }"),
    ] {
        let tag = format!("exit_bad_{name}");
        let source = write_c(
            &tag,
            &format!(
                "{declaration}\n__typeof__(&__bennett_exit) volatile keep = &__bennett_exit;\nint main(void) {{ return 0; }}"
            ),
        );
        let error = lower_text(&source, &tag, CLANG_FLAGS, &[]).expect_err("must refuse");
        assert!(error.starts_with("error:") && error.contains("@__bennett_exit"), "{error}");
    }
}
