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

#[test]
fn a_region_fault_is_reported_by_name_unless_allowed() {
    // fault.bn with n = 5 runs out of the region's budget on its only branch.
    let output = bennett(&["run", "fault.bn", "--arg", "5", "--stats"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("region fault: apply @triangle faulted (budget) on 1 of 1 branches"), "{text}");
    assert!(text.contains("region faults: 1 apply executions faulted for some argument value"), "{text}");

    let output = bennett(&["run", "fault.bn", "--arg", "5", "--allow-region-faults"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("result: 261\n"), "out 5, fault code 1: {text}");
    assert!(text.contains("region faults (allowed): 1 apply executions faulted for some argument value"), "{text}");

    let output = bennett(&["run", "fault.bn", "--arg", "2"]);
    assert_eq!(output.status.code(), Some(0), "no region fault: {}", stdout(&output));
    assert!(!stdout(&output).contains("region fault"));
}

#[test]
fn a_negative_result_wider_than_64_bits_is_printed_signed() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wide-negative.bn");
    let cases = [
        ("main", "340282366920938463444927863358058659840 (signed -18446744073709551616)"),
        ("min128", "170141183460469231731687303715884105728 (signed -170141183460469231731687303715884105728)"),
        ("minus_one_i65", "36893488147419103231 (signed -1)"),
        ("minus_one_i8", "255 (signed -1)"),
    ];
    for (entry, expected) in cases {
        let output = bennett(&["run", fixture, "--entry", entry]);
        assert_eq!(output.status.code(), Some(0), "@{entry}: {}", String::from_utf8_lossy(&output.stderr));
        let text = stdout(&output);
        assert!(text.contains(&format!("result: {expected}\n")), "@{entry}: {text}");
    }
}

#[test]
fn an_entry_frame_overflow_is_reported_and_reverses_without_a_step() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/entry-overflow.bn");
    for reverse in [false, true] {
        for budget in [None, Some("0")] {
            let mut args = vec!["run", fixture, "--stats"];
            if reverse {
                args.push("--reverse");
            }
            if let Some(budget) = budget {
                args.extend(["--budget", budget]);
            }
            let output = bennett(&args);
            let text = stdout(&output);
            assert_eq!(output.status.code(), Some(1), "{text}");
            assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
            assert!(text.contains("fault: stack_overflow (code 6)\n"), "{text}");
            assert!(text.contains("steps: 0\n"), "{text}");
            if reverse {
                assert!(
                    text.contains("reverse: undid 0 steps; returned to entry: classical state identical"),
                    "{text}"
                );
            }
        }
    }
}

#[test]
fn zero_budget_and_first_step_faults_report_the_named_fault_and_reverse() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/startup.bn");
    for (entry, budget, fault) in [
        ("main", Some("0"), "budget (code 1)"),
        ("empty", Some("0"), "budget (code 1)"),
        ("fault", None, "unreachable (code 5)"),
        ("divide", None, "divide_by_zero (code 2)"),
    ] {
        for reverse in [false, true] {
            let mut args = vec!["run", fixture, "--entry", entry];
            if let Some(budget) = budget {
                args.extend(["--budget", budget]);
            }
            if reverse {
                args.push("--reverse");
            }
            let output = bennett(&args);
            let text = stdout(&output);
            assert_eq!(output.status.code(), Some(1), "@{entry}: {text}");
            assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
            assert!(text.contains(&format!("fault: {fault}\n")), "{text}");
            assert!(text.contains("steps: 0\n"), "{text}");
            if reverse {
                assert!(
                    text.contains("reverse: undid 0 steps; returned to entry: classical state identical"),
                    "{text}"
                );
            }
        }
    }
}

#[test]
fn a_terminator_only_entry_returns_and_reverses() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/startup.bn");
    let output = bennett(&["run", fixture, "--entry", "empty", "--reverse"]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(text.contains("result: none\n"), "{text}");
    assert!(text.contains("reverse: undid 1 steps; returned to entry: classical state identical"), "{text}");
}

#[test]
fn missing_or_invalid_entries_report_an_error_before_execution() {
    for (fixture, entry, error) in [
        ("startup.bn", "missing", "no function @missing"),
        ("empty-module.bn", "main", "no function @main"),
        ("empty-function.bn", "main", "a function needs at least one block"),
        ("missing-terminator.bn", "main", "terminator"),
        ("unallocatable-memory.bn", "main", "cannot allocate 18446744073709551615 bytes of memory"),
    ] {
        let fixture = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
        for reverse in [false, true] {
            let mut args = vec!["run", &fixture, "--entry", entry];
            if reverse {
                args.push("--reverse");
            }
            let output = bennett(&args);
            assert_eq!(output.status.code(), Some(2));
            let text = String::from_utf8_lossy(&output.stderr);
            assert!(text.contains(error), "{text}");
            assert!(!text.contains("panicked"), "{text}");
        }
    }
}

#[test]
fn invalid_entry_arguments_report_a_start_error() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/startup.bn");
    for (entry, value, error) in [
        ("argument", None, "@argument takes 1 arguments, 0 given"),
        ("argument", Some("256"), "argument 0 (256) does not fit in i8"),
        ("register", Some("0"), "parameter 0 of @register is a register"),
    ] {
        let mut args = vec!["run", fixture, "--entry", entry, "--reverse"];
        if let Some(value) = value {
            args.extend(["--arg", value]);
        }
        let output = bennett(&args);
        assert_eq!(output.status.code(), Some(2));
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(text.contains(error), "{text}");
        assert!(!text.contains("panicked"), "{text}");
    }
}

#[test]
fn a_configuration_limit_on_the_first_gate_reports_a_stop_and_reverses() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/startup.bn");
    let output = bennett(&["run", fixture, "--entry", "gate", "--max-configurations", "1", "--reverse"]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        text.contains("stopped: a gate would raise the number of basis configurations above the limit of 1"),
        "{text}"
    );
    assert!(text.contains("steps: 0\n"), "{text}");
    assert!(text.contains("reverse: undid 0 steps; returned to entry: classical state identical"), "{text}");
}
