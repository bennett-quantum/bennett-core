//! Input that is not valid LLVM IR is refused by name with status 2, never an abort inside LLVM.

use std::process::Command;

use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, STACK_BYTES, bitcode, c_dir, front, out_dir};

/// clang 18 writes a `byval` aligned past LLVM's limit (`invalid_byval_align.c`). With debug info, LLVM's
/// bitcode reader verifies the module while upgrading it and stops with a fatal error, which aborted the front
/// before; without, the front's own verification of the input finds it. Both name the input as invalid.
#[test]
fn an_over_aligned_byval_is_named_as_invalid_input_with_and_without_debug_info() {
    let source = c_dir().join("invalid_byval_align.c");
    let without_debug: Vec<&str> = CLANG_O2_FLAGS.iter().copied().filter(|flag| !flag.starts_with("-g")).collect();
    for (name, flags) in
        [("debug_o0", CLANG_FLAGS), ("debug_o2", CLANG_O2_FLAGS), ("plain_o2", without_debug.as_slice())]
    {
        let input = bitcode(&source, &format!("invalid_byval_align.{name}"), flags);
        let program = out_dir().join(format!("invalid_byval_align.{name}.bn"));
        let _ = std::fs::remove_file(&program);
        let output = Command::new(front())
            .arg(&input)
            .arg("-o")
            .arg(&program)
            .arg("--region-budget=1000")
            .arg(format!("--stack-bytes={STACK_BYTES}"))
            .output()
            .expect("bennett-front runs");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{name}: {stderr}");
        assert!(stderr.contains("error: the input is not valid LLVM IR"), "{name}: {stderr}");
        assert!(stderr.contains("Attribute 'align' exceed the max size 2^14"), "{name}: the cause: {stderr}");
        assert!(!stderr.contains("Stack dump") && !stderr.contains("PLEASE submit"), "{name}: {stderr}");
        assert!(!program.exists(), "{name}: no device program is written");
    }
}
