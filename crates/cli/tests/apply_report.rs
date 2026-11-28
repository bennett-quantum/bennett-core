//! The step report of each `apply` through the CLI: `bennett run --stats` prints, per `apply` the simulator
//! evaluates directly, its position, function, inputs, largest step count and the inputs at the budget
//! (`fixtures/apply-report.bn`).

use std::process::Command;

#[test]
fn run_with_stats_prints_one_report_line_per_apply() {
    let output = Command::new(env!("CARGO_BIN_EXE_bennett"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["run", "--stats", "--allow-region-faults", "tests/fixtures/apply-report.bn"])
        .output()
        .expect("ran bennett");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report = String::from_utf8(output.stdout).expect("a text report");
    // Input 3 takes 1 + 4 * 3 + 2 + 1 steps: the first branch, four per pass, the last test and its branch,
    // and the return.
    assert!(
        report.contains("\n  apply 1 @count budget 100: 4 inputs, largest step count 16, 0 at the budget\n"),
        "{report}"
    );
    assert!(
        report.contains("\n  apply 2 @count budget 1: 4 inputs, largest step count none, 4 at the budget\n"),
        "{report}"
    );
}
