//! A `qfree` of a register that was not zero is the partial trace; the simulator reports it, even when
//! detailed statistics were not requested, in strict mode and on the machine alike.

use std::process::Command;

#[test]
fn only_a_nonzero_release_is_reported_with_its_count_weight_and_qubits() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/leftovers.bn");
    for mode in [None, Some("--machine-meaning")] {
        for (entry, nonzero) in [("clean", false), ("dirty", true)] {
            let output = Command::new(env!("CARGO_BIN_EXE_bennett"))
                .args(["run", fixture, "--entry", entry, "--budget", "20"])
                .args(mode)
                .output()
                .expect("run bennett");
            let report = String::from_utf8(output.stdout).expect("UTF-8 report");
            assert!(output.status.success(), "{entry}: {report}; {:?}", output.stderr);
            let reports: Vec<_> = report.lines().filter(|line| line.starts_with("simulator report:")).collect();
            if nonzero {
                // `x %q[0]` makes the register 1 in its only configuration: one release of weight 1, two qubits.
                assert_eq!(reports.len(), 1, "{entry}: {report}");
                assert!(reports[0].starts_with("simulator report: 1 `qfree` released"), "{report}");
                assert!(reports[0].contains("weight 1e0 in all") && reports[0].contains("(2 qubits kept"), "{report}");
            } else {
                assert!(reports.is_empty(), "{entry}: {report}");
            }
            assert!(!report.contains("fault"), "{entry}: {report}");
        }
    }
}
