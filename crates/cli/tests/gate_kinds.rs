//! Gate kinds at run time through the CLI (`fixtures/gate-kinds.bn`): the counting device counts each as
//! the gate its code names, the OpenQASM file writes that gate's name, and a code outside its set stops the
//! run with `bad_gate`.

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/gate-kinds.bn";

fn bennett(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bennett"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("run")
        .args(args)
        .output()
        .expect("ran bennett")
}

fn report(args: &[&str]) -> String {
    let output = bennett(args);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("a text report")
}

fn kinds(gate: u64, rotation: u64, angle: f64) -> [String; 6] {
    [
        "--arg".into(),
        gate.to_string(),
        "--arg".into(),
        rotation.to_string(),
        "--arg".into(),
        angle.to_bits().to_string(),
    ]
}

#[test]
fn the_counting_device_counts_the_gates_the_codes_name() {
    let args = kinds(2, 12, 0.7);
    let mut command = vec!["--device", "count", FIXTURE];
    command.extend(args.iter().map(String::as_str));
    let counted = report(&command);
    assert!(
        counted.contains("one-qubit gates: 1\ntwo-qubit gates: 2\nToffoli gates: 0\npeak qubits: 2\n"),
        "{counted}"
    );
}

#[test]
fn the_openqasm_file_writes_the_gates_the_codes_name() {
    for (gate, rotation, name, rotated) in [(2, 12, "y", "p"), (4, 9, "h", "rx"), (8, 11, "tdg", "rz")] {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("kinds-{}-{gate}.qasm", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let target = path.to_str().expect("a UTF-8 path");
        let args = kinds(gate, rotation, 0.7);
        let mut command = vec!["--device", "qasm", "--qasm-output", target, FIXTURE];
        command.extend(args.iter().map(String::as_str));
        report(&command);
        let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
        std::fs::remove_file(&path).expect("remove the OpenQASM file");
        assert!(text.contains(&format!("\nctrl @ {name} q0[0], q0[1];\n")), "{text}");
        assert!(text.contains(&format!("\nctrl @ {rotated}(0.7) q0[0], q0[1];\n")), "{text}");
    }
}

#[test]
fn a_code_outside_its_set_stops_the_run_with_bad_gate() {
    for (gate, rotation) in [(0, 9), (9, 9), (1, 1), (1, 13)] {
        let args = kinds(gate, rotation, 0.7);
        let mut command = vec![FIXTURE];
        command.extend(args.iter().map(String::as_str));
        let output = bennett(&command);
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(text.contains("bad_gate (code 23)"), "{gate} {rotation}: {text}");
    }
}
