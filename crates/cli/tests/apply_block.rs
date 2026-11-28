//! An apply to a block through the CLI (`fixtures/apply-block.bn`): the simulator gives `(x + 1) mod 4`
//! for `x = 2`, with a step report under `--stats`; the counting device and the OpenQASM writer, which
//! compile `apply` to gates, refuse it by name until that compilation is built.

use std::process::Command;

const FIXTURE: &str = "tests/fixtures/apply-block.bn";

fn bennett(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bennett"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("run")
        .args(args)
        .output()
        .expect("ran bennett")
}

#[test]
fn the_simulator_evaluates_it_and_reports_its_steps() {
    let output = bennett(&["--stats", FIXTURE]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report = String::from_utf8(output.stdout).expect("text");
    assert!(report.starts_with("result: 3\n"), "{report}");
    assert!(
        report.contains("\n  apply 1 @inc budget 20: 1 inputs, largest step count 6, 0 at the budget\n"),
        "{report}"
    );
}

#[test]
fn the_counter_and_the_openqasm_writer_refuse_it_by_name() {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("block-{}.qasm", std::process::id()));
    let target = path.to_str().expect("a UTF-8 path");
    for (device, args) in [
        ("count", vec!["--device", "count", FIXTURE]),
        ("qasm", vec!["--device", "qasm", "--qasm-output", target, FIXTURE]),
    ] {
        let output = bennett(&args);
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(!output.status.success(), "{device}: {text}");
        assert!(text.contains("apply to a block is evaluated by the simulator only"), "{device}: {text}");
    }
    let _ = std::fs::remove_file(&path);
}
