//! The fixed gate `y` through the CLI: the counting device counts it as one gate with at most one control,
//! and with two as it lowers every gate but `x` (two Toffolis and one two-qubit gate); the OpenQASM file
//! writes it as `stdgates.inc`'s `y`, Pauli Y as the device's (`fixtures/pauli-y.bn`). The writer's own
//! simulated outcomes must equal the simulator's at the same seed.

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/pauli-y.bn";

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
fn the_counting_device_counts_y_as_one_gate() {
    let counted = bennett(&["--device", "count", FIXTURE]);
    assert!(
        counted.contains("one-qubit gates: 2\ntwo-qubit gates: 1\nToffoli gates: 0\npeak qubits: 2\n"),
        "{counted}"
    );
}

#[test]
fn the_openqasm_file_writes_y_and_follows_the_simulator() {
    for seed in 0..4 {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("pauli-y-{}-{seed}.qasm", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let target = path.to_str().expect("a UTF-8 path");
        let seed = seed.to_string();
        let written = bennett(&["--device", "qasm", "--qasm-output", target, "--seed", &seed, FIXTURE]);
        let simulated = bennett(&["--seed", &seed, FIXTURE]);
        let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
        std::fs::remove_file(&path).expect("remove the OpenQASM file");
        assert!(text.contains("\nctrl @ y q0[0], q0[1];\n"), "{text}");
        assert!(text.contains("\ny q0[0];\n"), "{text}");
        let result = |report: &str| report.lines().find(|line| line.starts_with("result")).map(str::to_owned);
        assert_eq!(result(&written), result(&simulated), "seed {seed}: {written} against {simulated}");
        assert!(result(&written).is_some(), "{written}");
    }
}

/// With two controls the counter counts y as every gate but x: lowered to two Toffolis computing the
/// controls' AND into a scratch wire, y under that one control, and no more (x with two controls is one
/// Toffoli). The document states the same rule (`docs/SYNTAX.md`).
#[test]
fn the_counting_device_lowers_y_with_two_controls_as_it_lowers_h() {
    let count = |gate: &str| bennett(&["--device", "count", "tests/fixtures/pauli-y-two-controls.bn", "--arg", gate]);
    let (y, h, x) = (count("0"), count("1"), count("2"));
    assert!(y.contains("one-qubit gates: 0\ntwo-qubit gates: 1\nToffoli gates: 2\npeak qubits: 4\n"), "{y}");
    assert_eq!(y, h, "y and h are lowered alike");
    assert!(x.contains("one-qubit gates: 0\ntwo-qubit gates: 0\nToffoli gates: 1\npeak qubits: 3\n"), "{x}");
}
