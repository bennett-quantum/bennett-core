//! The phase rotation through the CLI: the counting device counts it as any gate with its controls, and the
//! OpenQASM file writes it as `stdgates.inc`'s `p`, which is `diag(1, e^{iλ})` as the device's
//! (`fixtures/phase-rotation.bn`).

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/phase-rotation.bn";

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

fn bits(angle: f64) -> String {
    (angle.to_bits() as i64).to_string()
}

#[test]
fn the_counting_device_counts_phase_rotations() {
    let counted = bennett(&["--device", "count", FIXTURE, "--arg", &bits(0.7)]);
    assert!(
        counted.contains("one-qubit gates: 4\ntwo-qubit gates: 1\nToffoli gates: 0\npeak qubits: 2\n"),
        "{counted}"
    );
}

/// The OpenQASM run takes its outcomes from its own simulation of what it writes: the measured control
/// must read as in the simulator at every seed, 1 always at a half turn.
#[test]
fn the_openqasm_file_writes_the_phase_rotation_as_p_and_simulates_it() {
    for angle in [0.7, -1.89, std::f64::consts::PI] {
        for seed in ["0", "1", "2", "3"] {
            let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("phase-{}-{angle}-{seed}.qasm", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let target = path.to_str().expect("a UTF-8 path");
            let args = [FIXTURE, "--arg", &bits(angle), "--seed", seed];
            let exported = bennett(&[&["--device", "qasm", "--qasm-output", target][..], &args].concat());
            let simulated = bennett(&args);
            let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
            std::fs::remove_file(&path).expect("remove the OpenQASM file");
            assert!(text.contains(&format!("\nctrl @ p({angle:?}) q0[0], q0[1];\n")), "{text}");
            assert!(text.contains("\np(0.25) q0[1];\n"), "{text}");
            let result = |report: &str| report.lines().next().expect("a result line").to_owned();
            assert_eq!(result(&exported), result(&simulated), "{angle} seed {seed}");
            if angle == std::f64::consts::PI {
                assert!(result(&simulated).starts_with("result: 3 "), "a half turn: {simulated}");
            }
        }
    }
}
