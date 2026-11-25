//! Registers wider than 64 qubits through the CLI: the simulator, the counting device and the OpenQASM
//! writer on `fixtures/wide-registers.bn`, and the counting device and the writer on a register of the
//! greatest width in `fixtures/widest-register.bn`. Every expected count is worked by hand from the fixture.

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
/// written across columns, whether `apply` is evaluated or compiled to gates and run; and `@top_bit`, the
/// identity on a register holding only bit 127.
#[test]
fn the_simulator_runs_wide_registers_evaluated_and_compiled() {
    let wide = bennett(&[FIXTURE, "--entry", "wide"]);
    assert!(wide.starts_with("result: 633825300114114700748351602688 "), "{wide}");
    for circuit in [&[][..], &["--apply-circuit", "step-machine"][..]] {
        let applied = bennett(&[&[FIXTURE, "--entry", "applied"][..], circuit].concat());
        assert!(applied.contains("result: 18446744073709551616\n"), "{applied}");
        // The result's top bit, which only a 128-qubit output register holds.
        let top = bennett(&[&[FIXTURE, "--entry", "top_bit"][..], circuit].concat());
        assert!(top.contains(&format!("result: {}", 1u128 << 127)), "{top}");
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

const WIDEST: &str = "tests/fixtures/widest-register.bn";

/// The address-space limit for a run on the widest register: 192 MiB, far below one host entry per qubit
/// of a register of 2^32 - 1 qubits (the counter and the writer kept one before), above what the run needs.
const ADDRESS_SPACE: &str = "201326592";

/// Runs `bennett run` under the address-space limit; fails by name when `prlimit` cannot be started,
/// rather than running the widest register without the limit.
fn bennett_limited(args: &[&str]) -> String {
    let output = Command::new("prlimit")
        .args([format!("--as={ADDRESS_SPACE}"), "--core=0".into(), "--".into()])
        .arg(env!("CARGO_BIN_EXE_bennett"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("run")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("the widest-register tests need prlimit (util-linux) for their limit: {error}"));
    assert!(output.status.success(), "{:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("a text report")
}

/// A register of 2^32 - 1 qubits that uses three of them. One-qubit gates: `x` and `h`; two-qubit: the
/// CNOT and the one control left after the three controls are reduced; Toffolis: two to reduce them, two
/// to undo that. Peak: the register, the 1-qubit result register and 2 scratch qubits. Depth 7: `h`, the
/// CNOT, the two reducing Toffolis, the CNOT onto the result and the two undoing Toffolis, in sequence.
#[test]
fn the_counting_device_counts_a_register_of_the_greatest_width() {
    let counted = bennett_limited(&["--device", "count", WIDEST]);
    assert!(
        counted
            .contains("one-qubit gates: 2\ntwo-qubit gates: 2\nToffoli gates: 4\npeak qubits: 4294967298\ndepth: 7\n"),
        "{counted}"
    );
}

/// The file declares the register as one array and names only the qubits used; the partial trace of the
/// register at the end names it as one range.
#[test]
fn the_openqasm_file_of_a_register_of_the_greatest_width_names_only_the_qubits_used() {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("widest-{}.qasm", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let target = path.to_str().expect("a UTF-8 path");
    bennett_limited(&["--device", "qasm", "--qasm-outcomes", "synthetic", "--qasm-output", target, WIDEST]);
    let text = std::fs::read_to_string(&path).expect("the OpenQASM file");
    std::fs::remove_file(&path).expect("remove the OpenQASM file");
    let declarations: Vec<&str> = text.lines().filter(|line| line.starts_with("qubit[")).collect();
    assert_eq!(declarations, ["qubit[4294967295] q0;", "qubit[1] q1;", "qubit[2] q2;"]);
    for line in [
        "x q0[4294967294];",
        "ctrl @ x q0[0], q0[2147483648];",
        "ctrl @ ctrl @ x q2[0], q0[4294967294], q2[1];",
        "// release_zero requested for q2[0:1]",
        "// discard q0[0:4294967294] (partial trace)",
    ] {
        assert!(text.lines().any(|text| text == line), "{line}\n{text}");
    }
    assert!(text.len() < 1024, "{} bytes", text.len());
}
