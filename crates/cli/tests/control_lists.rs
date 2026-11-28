//! A control list in memory through the CLI (`fixtures/control-list.bn`): a qubit listed twice is one
//! control, so the counting device counts one two-qubit gate and the OpenQASM file names the control once,
//! whether the list has the entry once or twice.

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/control-list.bn";

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
fn a_qubit_listed_twice_is_counted_and_written_as_one_control() {
    for n in ["1", "2"] {
        let counted = bennett(&["--device", "count", FIXTURE, "--arg", n]);
        assert!(counted.contains("one-qubit gates: 1\ntwo-qubit gates: 1\nToffoli gates: 0\n"), "{n}: {counted}");
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("list-{}-{n}.qasm", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let target = path.to_str().expect("a UTF-8 path");
        bennett(&["--device", "qasm", "--qasm-output", target, FIXTURE, "--arg", n]);
        let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
        std::fs::remove_file(&path).expect("remove the OpenQASM file");
        assert!(text.contains("\nctrl @ x q0[0], q0[1];\n"), "{n}: {text}");
    }
}
