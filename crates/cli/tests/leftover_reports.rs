//! Leaving history behind must warn even when detailed statistics were not requested.

use std::process::Command;

#[test]
fn only_nonzero_leftovers_warn_and_the_warning_counts_the_qubits() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/leftovers.bn");
    for (entry, leftovers) in [("clean", 0), ("dirty", 2), ("rewind", 1)] {
        let output = Command::new(env!("CARGO_BIN_EXE_bennett"))
            .args(["run", fixture, "--entry", entry, "--machine-meaning", "--budget", "20"])
            .output()
            .expect("run bennett");
        let report = String::from_utf8(output.stdout).expect("UTF-8 report");
        assert!(output.status.success(), "{entry}: {report}; {:?}", output.stderr);
        let warnings: Vec<_> = report.lines().filter(|line| line.starts_with("warning:")).collect();
        if leftovers == 0 {
            assert!(warnings.is_empty(), "{entry}: {report}");
        } else {
            assert_eq!(warnings.len(), 1, "{entry}: {report}");
            assert!(warnings[0].starts_with(&format!("warning: {leftovers} qubits left over")), "{report}");
        }
    }
}
