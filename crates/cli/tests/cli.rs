//! The `bennett` command on the examples: its report and its exit status.

use std::process::{Command, Output};

fn bennett(args: &[&str]) -> Output {
    let examples = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
    Command::new(env!("CARGO_BIN_EXE_bennett"))
        .current_dir(examples)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("cannot run bennett: {error}"))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn check_accepts_every_example() {
    for example in ["sum.bn", "calls.bn", "thesis.bn", "fault.bn", "grover.bn"] {
        let output = bennett(&["check", example]);
        assert!(output.status.success(), "{example}: {}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn run_reports_result_console_and_steps() {
    let output = bennett(&["run", "calls.bn"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("result: 64\n"), "{text}");
    assert!(text.contains("console: \"hello\\n\"\n"), "{text}");
    assert!(text.contains("steps: "), "{text}");
}

#[test]
fn run_reverse_reports_the_return_to_entry_and_the_journal() {
    let output = bennett(&["run", "sum.bn", "--arg", "10", "--reverse", "--stats", "--budget", "1000"]);
    let text = stdout(&output);
    assert!(text.contains("result: 45\n"), "{text}");
    assert!(text.contains("returned to entry: classical state identical"), "{text}");
    assert!(text.contains("bytes per step"), "{text}");
    assert!(text.contains("bounds: step budget 1000, basis configurations none"), "{text}");
}

#[test]
fn a_fault_is_a_result_with_its_own_exit_status() {
    let output = bennett(&["run", "sum.bn", "--arg", "100", "--budget", "10", "--reverse"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("fault: budget (code 1)"), "{text}");
    assert!(text.contains("reverse: undid 10 steps; returned to entry: classical state identical"), "{text}");
}

#[test]
fn bad_input_is_an_error_with_a_location() {
    let output = bennett(&["run", "sum.bn", "--arg", "1", "--arg", "2"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("@main takes 1 arguments, 2 given"));
    let output = bennett(&["run", "no-such-file.bn"]);
    assert_eq!(output.status.code(), Some(2));
}
