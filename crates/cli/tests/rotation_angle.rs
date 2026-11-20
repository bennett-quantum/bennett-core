//! A rotation by an angle computed at run time through the CLI: the counting device counts it as the
//! literal form, and the OpenQASM file carries the run's value (`fixtures/rotation-angle.bn`).

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/rotation-angle.bn";

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

fn qasm(entry: &str, angle: f64) -> String {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("angle-{entry}-{}.qasm", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let bits = (angle.to_bits() as i64).to_string();
    let target = path.to_str().expect("a UTF-8 path");
    bennett(&["--device", "qasm", "--qasm-output", target, FIXTURE, "--entry", entry, "--arg", &bits]);
    let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
    std::fs::remove_file(&path).expect("remove the OpenQASM file");
    text
}

#[test]
fn the_counting_device_counts_rotations_by_a_run_time_angle() {
    let bits = (0.7f64.to_bits() as i64).to_string();
    let counted = bennett(&["--device", "count", FIXTURE, "--arg", &bits]);
    assert!(
        counted.contains("one-qubit gates: 2\ntwo-qubit gates: 1\nToffoli gates: 0\npeak qubits: 2\n"),
        "{counted}"
    );
}

#[test]
fn the_openqasm_file_carries_the_value_of_the_run() {
    for angle in [0.7, -1.89, 1e-300] {
        let text = qasm("main", angle);
        assert!(text.contains(&format!("\nctrl @ rz({angle:?}) q0[0], q0[1];\n")), "{text}");
        assert!(text.contains(&format!("\nry({angle:?}) q0[0];\n")), "{text}");
    }
    // The angle is computed by the program: twice the argument.
    let text = qasm("doubled", 0.7);
    assert!(text.contains("\nrx(1.4) q0[0];\n"), "{text}");
}
