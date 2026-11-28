//! Handles as integers through the CLI: the counting device and the OpenQASM writer take a register whose
//! width and qubit positions are computed at run time (`fixtures/handles.bn`).

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/handles.bn";

fn bennett(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_bennett"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("run")
        .args(args)
        .output()
        .expect("ran bennett");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("a text report")
}

#[test]
fn the_counting_device_counts_a_run_time_width_and_position() {
    for width in [1u32, 5, 70] {
        let counted = bennett(&["--device", "count", FIXTURE, "--arg", &width.to_string()]);
        let expected =
            format!("one-qubit gates: 1\ntwo-qubit gates: 1\nToffoli gates: 0\npeak qubits: {}\n", width + 2);
        assert!(counted.contains(&expected), "{width}: {counted}");
    }
}

#[test]
fn the_openqasm_file_declares_the_run_time_width_and_names_the_last_qubit() {
    for width in [1u32, 5, 70] {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("handles-{}-{width}.qasm", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let target = path.to_str().expect("a UTF-8 path");
        bennett(&["--device", "qasm", "--qasm-output", target, FIXTURE, "--arg", &width.to_string()]);
        let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
        std::fs::remove_file(&path).expect("remove the OpenQASM file");
        assert!(text.contains(&format!("\nqubit[{width}] q0;\n")), "{text}");
        assert!(text.contains(&format!("\nctrl @ x q1[1], q0[{}];\n", width - 1)), "{text}");
    }
}

#[test]
fn the_counting_device_and_the_openqasm_file_take_a_slice_measurement() {
    let counted = bennett(&["--device", "count", "tests/fixtures/slice-measure.bn"]);
    assert!(counted.contains("one-qubit gates: 2\n"), "{counted}");
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("slice-{}.qasm", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let target = path.to_str().expect("a UTF-8 path");
    let report = bennett(&["--device", "qasm", "--qasm-output", target, "tests/fixtures/slice-measure.bn"]);
    let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
    std::fs::remove_file(&path).expect("remove the OpenQASM file");
    assert!(report.starts_with("result: 3"), "{report}");
    assert!(text.contains("\nmeasure q0[1]; // outcome 1\nmeasure q0[2]; // outcome 1\n"), "{text}");
    assert_eq!(text.matches("measure ").count(), 2, "{text}");
}
