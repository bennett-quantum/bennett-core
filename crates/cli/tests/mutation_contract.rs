//! CLI boundary and report checks from mutation testing; no wall-clock thresholds.

use std::process::{Command, Output};

fn run(entry: &str, args: &[&str]) -> Output {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mutation-boundaries.bn");
    Command::new(env!("CARGO_BIN_EXE_bennett"))
        .args(["run", fixture, "--entry", entry])
        .args(args)
        .output()
        .expect("run bennett")
}

fn text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 report")
}

#[test]
fn signed_arguments_preserve_width_boundaries_and_zero() {
    for (entry, literal, value) in [
        ("main", "0", "0"),
        ("main", "127", "127"),
        ("main", "128", "128 (signed -128)"),
        ("main", "255", "255 (signed -1)"),
        ("main", "-1", "255 (signed -1)"),
        ("main", "-128", "128 (signed -128)"),
        ("main", "-0", "0"),
        ("word", "-9223372036854775808", "9223372036854775808 (signed -9223372036854775808)"),
    ] {
        let output = run(entry, &["--arg", literal]);
        assert_eq!(output.status.code(), Some(0), "{entry} {literal}: {output:?}");
        assert!(text(&output).contains(&format!("result: {value}\n")), "{}", text(&output));
    }
    for literal in ["256", "-129"] {
        let output = run("main", &["--arg", literal]);
        assert_eq!(output.status.code(), Some(2));
        assert!(!output.stderr.is_empty());
    }
    let output = run("register", &["--arg", "0"]);
    assert_eq!(output.status.code(), Some(2), "a host cannot supply a register handle");
}

#[test]
fn successful_void_and_wide_positive_results_are_reported() {
    for (entry, expected) in [("void", "none"), ("wide_positive", "18446744073709551616")] {
        let output = run(entry, &[]);
        assert_eq!(output.status.code(), Some(0));
        assert!(text(&output).contains(&format!("result: {expected}\n")), "{}", text(&output));
    }
}

#[test]
fn journal_bytes_per_step_agree_with_the_reported_totals() {
    let output = run("main", &["--arg", "7", "--reverse", "--stats"]);
    assert_eq!(output.status.code(), Some(0));
    let report = text(&output);
    let steps: f64 =
        report.lines().find_map(|line| line.strip_prefix("steps: ")).expect("steps").parse().expect("count");
    let journal = report.lines().find_map(|line| line.strip_prefix("journal: ")).expect("journal");
    let words: Vec<_> = journal.split_whitespace().collect();
    let bytes: f64 = words[0].parse().expect("byte count");
    let per_step: f64 = words[2].parse().expect("bytes per step");
    assert!(steps > 0.0 && bytes > 0.0);
    assert!((per_step - bytes / steps).abs() <= 0.051, "{report}");
    assert!(report.contains("  forward: ") && report.contains("steps/s"), "{report}");
    assert!(report.contains("  reverse: "), "{report}");
}

#[test]
fn zero_step_fault_has_no_infinite_journal_ratio() {
    let output = run("void", &["--budget", "0", "--reverse", "--stats"]);
    assert_eq!(output.status.code(), Some(1));
    let report = text(&output);
    assert!(report.contains("steps: 0\n"), "{report}");
    assert!(report.contains("  journal: 0 bytes\n"), "{report}");
    assert!(!report.contains("NaN") && !report.contains("inf"), "{report}");
}

#[test]
fn measurement_reports_the_reversal_barrier_and_configuration_stop() {
    let output = run("measure", &["--reverse", "--stats", "--seed", "17"]);
    assert_eq!(output.status.code(), Some(0));
    let report = text(&output);
    assert!(report.contains("reverse: stopped after undoing 1 steps:"), "{report}");
    assert!(report.contains("measurement"), "{report}");
    let again = run("measure", &["--seed", "17"]);
    assert_eq!(report.lines().next(), text(&again).lines().next(), "seeded result is reproducible");
    let output = run("measure", &["--max-configurations", "1", "--stats"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output).contains("stopped:"));
    assert!(text(&output).contains("basis configurations 1"));
}

#[test]
fn check_rejects_invalid_input_and_reports_its_function_count() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mutation-boundaries.bn");
    let output = Command::new(env!("CARGO_BIN_EXE_bennett")).args(["check", fixture]).output().expect("check");
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output).contains("ok: 7 functions"));
    let output =
        Command::new(env!("CARGO_BIN_EXE_bennett")).args(["check", "missing-program.bn"]).output().expect("check");
    assert_eq!(output.status.code(), Some(2));
}
