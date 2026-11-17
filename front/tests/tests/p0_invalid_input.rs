//! Independent admission checks for broken LLVM input and unchanged valid input.
use std::process::{Command, Output};

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, bitcode, c_dir, front, out_dir, run};

fn translate(name: &str, bytes: &[u8]) -> (Output, std::path::PathBuf) {
    let input = out_dir().join(format!("p0-admission-{name}.bc"));
    let program = input.with_extension("bn");
    std::fs::write(&input, bytes).expect("write independent input");
    let _ = std::fs::remove_file(&program);
    let output = Command::new(front())
        .arg(&input)
        .arg("-o")
        .arg(&program)
        .args(["--region-budget=1000", "--stack-bytes=65536"])
        .output()
        .expect("front executes");
    (output, program)
}

fn refused(name: &str, bytes: &[u8], named: bool) {
    let (output, program) = translate(name, bytes);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{name}: {stderr}");
    assert!(!program.exists(), "{name}: wrote a device program");
    assert!(!stderr.contains("Stack dump") && !stderr.contains("PLEASE submit"), "{name}: {stderr}");
    if named {
        assert!(stderr.contains("the input is not valid LLVM IR"), "{name}: {stderr}");
    } else {
        assert!(stderr.contains("error:"), "{name}: no diagnostic: {stderr}");
    }
}

#[test]
fn syntax_errors_and_damaged_bitcode_exit_cleanly_without_output() {
    for (name, bytes) in [
        ("prose", b"This is a text file, not LLVM IR.\n".as_slice()),
        ("return-type", b"define i32 @main() { ret i64 0 }\n".as_slice()),
        ("wrong-magic", b"BC\0\0garbage".as_slice()),
        ("magic-only", b"BC\xc0\xde".as_slice()),
        ("short-record", b"BC\xc0\xde\xff\xff\xff\xff".as_slice()),
    ] {
        refused(name, bytes, false);
    }
    let input = bitcode(&c_dir().join("invalid_byval_align.c"), "p0-truncated-input", CLANG_FLAGS);
    let bytes = std::fs::read(input).expect("real bitcode");
    for length in [5, 8, 12, 16, 32, 64, bytes.len() / 2, bytes.len() - 1] {
        refused(&format!("cut-{length}"), &bytes[..length], false);
    }
}

#[test]
fn verifier_failures_are_named_and_do_not_write_output() {
    for (name, text) in [
        ("phi", "define i32 @main() {\nentry:\n br label %done\ndone:\n %p = phi i32 [1, %done]\n ret i32 %p\n}\n"),
        (
            "dominance",
            "define i32 @main(i1 %b) {\nentry:\n br i1 %b, label %yes, label %no\nyes:\n %v = add i32 1, 2\n br label %no\nno:\n ret i32 %v\n}\n",
        ),
    ] {
        refused(name, text.as_bytes(), true);
    }
}

#[test]
fn valid_textual_llvm_input_runs_unchanged() {
    let text = b"define i32 @main() { ret i32 37 }\n";
    let (output, program) = translate("valid", text);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let module = bennett_ir::load(&std::fs::read_to_string(program).expect("program")).expect("valid program");
    assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(37)));
}

#[test]
fn a_plain_text_file_is_named_as_invalid_llvm_input() {
    // LLVM's grammar cannot parse this; the front prints LLVM's diagnostic, then names the input.
    refused("prose-named", b"This is not LLVM IR.\n", true);
}

#[test]
fn an_empty_file_is_the_empty_module() {
    // The testing pass's witness asked for a refusal. LLVM's assembly language defines a module as a list
    // of top-level entities, which may be empty (`llvm-as` accepts an empty file, and LLVM's reader gives
    // an empty module that its verifier accepts), and the front accepts any module without `main`, as
    // for a library unit. So an empty file is valid input: an empty device program, status 0.
    let (output, program) = translate("empty", b"");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    bennett_ir::load(&std::fs::read_to_string(program).expect("program")).expect("a valid empty device program");
}

#[test]
#[ignore = "defect: malformed bitcode metadata causes SIGSEGV inside LLVM's reader"]
fn a_dangling_subprogram_type_is_a_named_refusal_instead_of_a_signal() {
    // Reduced from a one-bit corruption of clang's debug bitcode. These 44 bytes contain MODULE
    // version 2 and one METADATA SUBPROGRAM record (18 operands, all 0 except operand 6 = 2).
    // Its type refers to missing metadata. The scoped fatal-error handler cannot catch SIGSEGV
    // from MetadataLoader::parseMetadata. No LLVM assembler or platform-specific path is needed.
    let bytes = [
        0x42, 0x43, 0xc0, 0xde, 0x21, 0x0c, 0, 0, 8, 0, 0, 0, 0x0b, 2, 0x21, 0x0f, 3, 0, 0, 0, 4, 0, 0, 0, 0xab, 0x24,
        0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    refused("dangling-metadata", &bytes, true);
}
