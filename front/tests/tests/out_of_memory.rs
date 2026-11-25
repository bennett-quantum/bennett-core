//! The device function the C runtime's allocator calls when its arena cannot hold a request: it ends the run
//! with the named fault `out_of_memory`, keeping the console output written before it.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, entry, lower, lower_text, options, out_dir, write_c};
use bennett_ir::Fault;

const DECLARATIONS: &str = "_Noreturn void __bennett_out_of_memory(void);\nvoid __bennett_effect(char);\n";

#[test]
fn a_call_from_a_nested_function_ends_the_run_with_out_of_memory() {
    let source = write_c(
        "out_of_memory_nested",
        &format!(
            r#"{DECLARATIONS}
volatile unsigned long wanted = 3;
__attribute__((noinline)) static char *take(unsigned long n) {{
    static char arena[2];
    if (n > sizeof arena) __bennett_out_of_memory();
    return arena;
}}
int main(void) {{
    __bennett_effect('a');
    for (unsigned long n = 1; n <= wanted; ++n) {{
        char *p = take(n);
        p[0] = (char)('0' + n);
        __bennett_effect(p[0]);
    }}
    __bennett_effect('z');
    return 4;
}}
"#
        ),
    );
    for (level, flags) in [("O0", CLANG_FLAGS), ("O2", CLANG_O2_FLAGS)] {
        let tag = format!("out_of_memory_nested_{level}");
        let text = lower_text(&source, &tag, flags, &[]).unwrap_or_else(|e| panic!("{tag}: {e}"));
        assert!(text.contains("fault out_of_memory"), "{tag}:\n{text}");
        let module = lower(&source, &tag, flags, &[]);
        let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
        assert_eq!(executor.run(), Outcome::Faulted(Fault::OutOfMemory), "{tag}");
        assert_eq!(executor.tape(), b"a12", "{tag}");
    }
}

#[test]
fn out_of_memory_declarations_require_noreturn_void_void() {
    for (name, declaration) in [
        ("parameter", "_Noreturn void __bennett_out_of_memory(int);"),
        ("result", "_Noreturn int __bennett_out_of_memory(void);"),
        ("vararg", "_Noreturn void __bennett_out_of_memory(int, ...);"),
        ("returns", "void __bennett_out_of_memory(void);"),
        ("defined", "_Noreturn void __bennett_out_of_memory(void) { for (;;) {} }"),
    ] {
        let tag = format!("out_of_memory_bad_{name}");
        let source = write_c(
            &tag,
            &format!(
                "{declaration}\n__typeof__(&__bennett_out_of_memory) volatile keep = &__bennett_out_of_memory;\nint main(void) {{ return 0; }}"
            ),
        );
        let error = lower_text(&source, &tag, CLANG_FLAGS, &[]).expect_err("must refuse");
        assert!(error.starts_with("error:") && error.contains("@__bennett_out_of_memory"), "{tag}: {error}");
    }
}

#[test]
fn out_of_memory_attributes_cannot_license_removing_the_call() {
    let path = out_dir().join("out_of_memory_pure.ll");
    std::fs::write(
        &path,
        "declare void @__bennett_out_of_memory() noreturn memory(none)\ndefine i32 @main() {\n call void @__bennett_out_of_memory()\n unreachable\n}\n",
    )
    .expect("write IR");
    let error = lower_text(&path, "out_of_memory_pure", CLANG_FLAGS, &[]).expect_err("must refuse");
    assert!(error.contains("attributes that let LLVM remove or move its calls"), "{error}");
}

#[test]
fn attributes_on_one_call_cannot_license_removing_it() {
    // The declaration is right; only the call says it reads no memory and returns, which would let LLVM delete
    // it and leave the `unreachable` after it, a different fault.
    for (name, attributes) in [("pure", "memory(none)"), ("reads", "memory(read)"), ("returns", "willreturn")] {
        let tag = format!("out_of_memory_call_{name}");
        let path = out_dir().join(format!("{tag}.ll"));
        std::fs::write(
            &path,
            format!(
                "declare void @__bennett_out_of_memory() noreturn\ndefine i32 @main() {{\n call void @__bennett_out_of_memory() nounwind {attributes}\n unreachable\n}}\n"
            ),
        )
        .expect("write IR");
        let error = lower_text(&path, &tag, CLANG_FLAGS, &[]).expect_err("must refuse");
        assert!(
            error.contains("a call of the device function @__bennett_out_of_memory has attributes"),
            "{tag}: {error}"
        );
    }
}
