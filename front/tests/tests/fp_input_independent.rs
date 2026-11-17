//! Exercise the reader's three failure paths through files, stdin, and stdout output. Invalid input
//! must leave a preexisting output untouched as well as refrain from creating a new device program.
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, bitcode, front, out_dir, run, write_c};

const SENTINEL: &[u8] = b"an earlier output must survive an invalid input\n";

fn translate(name: &str, bytes: &[u8], stdin: bool, output: &str) -> Output {
    let input = out_dir().join(format!("fp-input-{name}.bc"));
    std::fs::write(&input, bytes).expect("write input");
    let mut command = Command::new(front());
    command.arg(if stdin { std::path::Path::new("-") } else { &input }).args([
        "-o",
        output,
        "--region-budget=100000",
        "--stack-bytes=65536",
    ]);
    if stdin {
        let mut child =
            command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("front runs");
        child.stdin.take().expect("stdin").write_all(bytes).expect("write stdin");
        child.wait_with_output().expect("front finishes")
    } else {
        command.output().expect("front finishes")
    }
}

fn output_path(name: &str) -> PathBuf {
    out_dir().join(format!("fp-input-{name}.bn"))
}

fn invalid(name: &str, bytes: &[u8]) {
    for stdin in [false, true] {
        let name = format!("{name}-{stdin}");
        let path = output_path(&name);
        for existing in [false, true] {
            if existing {
                std::fs::write(&path, SENTINEL).expect("old output");
            } else if path.exists() {
                std::fs::remove_file(&path).expect("remove output");
            }
            let output = translate(&name, bytes, stdin, path.to_str().expect("UTF-8 output path"));
            assert_invalid(&name, &output);
            assert!(output.stdout.is_empty(), "{name}: unexpected stdout");
            if existing {
                assert_eq!(std::fs::read(&path).expect("old output survives"), SENTINEL);
            } else {
                assert!(!path.exists(), "{name}: wrote an output");
            }
        }
        let output = translate(&name, bytes, stdin, "-");
        assert_invalid(&name, &output);
        assert!(output.stdout.is_empty(), "{name}: wrote device IR to stdout");
    }
}

fn assert_invalid(name: &str, output: &Output) {
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{name}: {error}");
    assert!(error.contains("error: the input is not valid LLVM IR"), "{name}: {error}");
    for crash in ["Stack dump", "PLEASE submit", "LLVM ERROR", "Segmentation fault"] {
        assert!(!error.contains(crash), "{name}: leaked crash report: {error}");
    }
}

#[test]
fn malformed_text_and_bitcode_are_named_without_touching_output() {
    for (name, bytes) in [
        ("lexical", b"@x = constant [4 x i8] c\"abc\n".as_slice()),
        ("missing-terminator", b"define void @main() { entry: }\n".as_slice()),
        ("duplicate", b"@x = global i8 0\n@x = global i8 1\n".as_slice()),
        ("bad-branch", b"define i32 @main() { br label %missing }\n".as_slice()),
        ("bad-intrinsic", b"declare i1 @llvm.is.fpclass.f64(double, i32)\ndefine i1 @f(double %x, i32 %mask) { %r = call i1 @llvm.is.fpclass.f64(double %x, i32 %mask)\nret i1 %r }\n".as_slice()),
        ("bad-phi", b"define i32 @f() { entry: br label %end\nend: %x = phi i32 [0, %end]\nret i32 %x }\n".as_slice()),
        ("binary", b"\0\xff\xfe\0not llvm\0".as_slice()),
    ] { invalid(name, bytes); }
    // The metadata reader dereferences a missing DISubroutineType: exercise recovery for stdin too.
    invalid(
        "metadata-crash",
        &[
            0x42, 0x43, 0xc0, 0xde, 0x21, 0x0c, 0, 0, 8, 0, 0, 0, 0x0b, 2, 0x21, 0x0f, 3, 0, 0, 0, 4, 0, 0, 0, 0xab,
            0x24, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    );
    let source = write_c("fp-input-valid", "int main(void) { return 37; }\n");
    let real = std::fs::read(bitcode(&source, "fp-input-valid", CLANG_FLAGS)).expect("real bitcode");
    for length in [1, 2, 3, 4, 7, 15, 31, real.len() / 3, real.len() - 1] {
        invalid(&format!("truncated-{length}"), &real[..length]);
    }
}

#[test]
fn valid_empty_text_and_bitcode_work_through_files_and_stdin() {
    let source = write_c("fp-input-valid-channels", "int main(void) { return 37; }\n");
    let real = std::fs::read(bitcode(&source, "fp-input-valid-channels", CLANG_FLAGS)).expect("bitcode");
    for (name, bytes, executable) in [
        ("empty", b"".as_slice(), false),
        ("comments", b"; an empty library module\n \t\n".as_slice(), false),
        ("text", b"define i32 @main() { ret i32 37 }\n".as_slice(), true),
        ("bitcode", real.as_slice(), true),
    ] {
        for stdin in [false, true] {
            let output = translate(&format!("valid-{name}-{stdin}"), bytes, stdin, "-");
            assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
            let text = String::from_utf8(output.stdout).expect("text output");
            let module = bennett_ir::load(&text).expect("valid device program");
            if executable {
                assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(37)));
            } else {
                assert!(module.function_named("main").is_none());
            }
        }
    }
}

#[test]
fn an_unreadable_file_is_distinguished_from_invalid_llvm_ir() {
    let input = out_dir().join("fp-input-nonexistent.bc");
    if input.exists() {
        std::fs::remove_file(&input).expect("remove input");
    }
    let output = Command::new(front())
        .arg(input)
        .args(["-o", "-", "--region-budget=100000", "--stack-bytes=65536"])
        .output()
        .expect("front runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("cannot read"), "{error}");
    assert!(!error.contains("not valid LLVM IR"), "{error}");
}

#[test]
fn powi_at_half_widths_keeps_its_documented_named_refusal() {
    // The documented route for these widths is still a debt (the native back end uses powf).
    // Unlike damaged IR, these are valid modules and must not be called invalid LLVM input.
    for (ty, suffix) in [("half", "f16"), ("bfloat", "bf16")] {
        let text = format!(
            "declare {ty} @llvm.powi.{suffix}.i32({ty}, i32)\n\
             define {ty} @power({ty} %x, i32 %n) {{\n\
             %r = call {ty} @llvm.powi.{suffix}.i32({ty} %x, i32 %n)\nret {ty} %r\n}}\n"
        );
        let path = output_path(&format!("powi-{suffix}"));
        if path.exists() {
            std::fs::remove_file(&path).expect("remove old output");
        }
        let output =
            translate(&format!("powi-{suffix}"), text.as_bytes(), false, path.to_str().expect("UTF-8 output path"));
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(3), "{suffix}: {error}");
        assert!(error.contains("powi") && error.contains("not handled"), "{error}");
        assert!(!error.contains("not valid LLVM IR"), "{error}");
        assert!(!path.exists(), "{suffix}: wrote output for a refused operation");
    }
}
