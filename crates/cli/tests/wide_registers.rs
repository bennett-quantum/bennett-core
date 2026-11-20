//! Registers wider than 64 qubits through the CLI: the simulator, the counting device and the OpenQASM
//! writer on `fixtures/wide-registers.bn`. Every expected count is worked by hand from the fixture.

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/wide-registers.bn";

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

/// `@wide`: qubit 999 ends as 1 - qubit 0, so the copy of the top 100 qubits measures 2^99 or 0 (the
/// three-control gate never fires: qubit 998 is 0). `@applied`: x + 1 with x = 2^64 - 1, read and
/// written across columns, whether `apply` is evaluated or compiled to gates and run.
#[test]
fn the_simulator_runs_wide_registers_evaluated_and_compiled() {
    let wide = bennett(&[FIXTURE, "--entry", "wide"]);
    assert!(wide.starts_with("result: 633825300114114700748351602688 "), "{wide}");
    for circuit in [&[][..], &["--apply-circuit", "step-machine"][..]] {
        let applied = bennett(&[&[FIXTURE, "--entry", "applied"][..], circuit].concat());
        assert!(applied.contains("result: 18446744073709551616\n"), "{applied}");
    }
}

/// One-qubit gates: `x` and `h`. Two-qubit: the CNOT from 0 to 999 and 2 x 100 for the copy and its undoing,
/// plus the one control left after the three controls are reduced. Toffolis: two to reduce the controls,
/// two to undo that. Peak: 1000 + 100 (the 2 scratch qubits are live before the copy register exists).
#[test]
fn the_counting_device_counts_a_thousand_qubit_register() {
    let counted = bennett(&["--device", "count", FIXTURE, "--entry", "wide"]);
    assert!(
        counted.contains("one-qubit gates: 2\ntwo-qubit gates: 202\nToffoli gates: 4\npeak qubits: 1100\n"),
        "{counted}"
    );
}

/// The file declares the register as one array of 1000 qubits and names its qubits by index, including the
/// top one; its gate statements agree with the counting device.
#[test]
fn the_openqasm_file_declares_and_addresses_the_wide_register() {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("wide-{}.qasm", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let target = path.to_str().expect("a UTF-8 path");
    bennett(&["--device", "qasm", "--qasm-output", target, FIXTURE, "--entry", "wide"]);
    let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
    std::fs::remove_file(&path).expect("remove the OpenQASM file");
    let declarations: Vec<&str> = text.lines().filter(|line| line.starts_with("qubit[")).collect();
    assert_eq!(declarations, ["qubit[1000] q0;", "qubit[2] q1;", "qubit[100] q2;"]);
    assert!(text.contains("\nx q0[999];\n") && text.contains("\nctrl @ x q0[0], q0[999];\n"), "{text}");
    let statements = |prefix: &str| text.lines().filter(|line| line.starts_with(prefix)).count();
    assert_eq!((statements("ctrl @ x "), statements("ctrl @ ctrl @ x ")), (202, 4));
}
