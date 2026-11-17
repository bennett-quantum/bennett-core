//! Reporting witnesses for changed-line machine mutants. Counts come from the two-qubit fixture.

use std::process::Command;

#[test]
fn only_a_run_that_leaves_qubits_over_reports_the_warning() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/leftover-warning.bn");
    for (entry, machine, status, warning) in
        [("dirty", true, 0, true), ("clean", true, 0, false), ("dirty", false, 1, false), ("clean", false, 0, false)]
    {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bennett"));
        // The longest fixture entry takes five instructions; bound a broken forward instruction too.
        command.args(["run", fixture, "--entry", entry, "--budget", "20"]);
        if machine {
            command.arg("--machine-meaning");
        }
        let output = command.output().expect("run fixture");
        let report = String::from_utf8(output.stdout).expect("UTF-8 report");
        assert_eq!(output.status.code(), Some(status), "{entry}, machine {machine}: {report}");
        assert_eq!(report.contains("warning:"), warning, "{entry}, machine {machine}: {report}");
        if warning {
            assert!(report.contains("warning: 2 qubits left over"), "{report}");
        }
    }
}
