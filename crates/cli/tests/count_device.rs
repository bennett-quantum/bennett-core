//! Counting mode through the public CLI.

use std::process::{Command, Output};

fn count(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bennett"))
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples"))
        .args(["run", "--device", "count"])
        .args(args)
        .output()
        .expect("ran bennett")
}

#[test]
fn counts_native_and_rewritten_gates() {
    let output = count(&["../crates/cli/tests/fixtures/count-device.bn"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "result: none\nsteps: 8\nmeasurement source: synthetic bits (seed 0)\nmeasurement outcomes: none\none-qubit gates: 2\ntwo-qubit gates: 2\nToffoli gates: 7\npeak qubits: 6\ndepth: 10\n"
    );
}

#[test]
fn rejects_apply_by_name() {
    let output = count(&["thesis.bn"]);
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("stopped: cannot emit call in @g at then instruction 0: needs applied calls\n"), "{text}");
    assert!(text.contains("steps: 4\n"), "{text}");
    assert!(
        text.contains("one-qubit gates: 1\ntwo-qubit gates: 0\nToffoli gates: 0\npeak qubits: 14\ndepth: 1\n"),
        "{text}"
    );
}

#[test]
fn count_mode_reports_ordinary_results_and_preserves_the_step_budget() {
    let output = count(&["sum.bn", "--arg", "10"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("result: 45\n"));
    let output = count(&["sum.bn", "--arg", "10", "--budget", "1"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("fault: budget (code 1)\nsteps: 1\n"));
}

#[test]
fn returning_a_register_does_not_expose_a_storage_handle() {
    let fixture = "../crates/cli/tests/fixtures/count-device.bn";
    for entry in ["dirty_discard", "clean_discard"] {
        let args = [fixture, "--entry", entry];
        let simulated = Command::new(env!("CARGO_BIN_EXE_bennett"))
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples"))
            .arg("run")
            .args(args)
            .output()
            .expect("ran simulator");
        let counted = count(&args);
        for output in [simulated, counted] {
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(String::from_utf8_lossy(&output.stdout).lines().next(), Some("result: register (q1)"));
        }
    }
}

#[test]
fn counts_print_consumed_outcomes_in_order() {
    let fixture = "../crates/cli/tests/fixtures/count-device.bn";
    for (seed, outcomes, gates) in [("1", "10", 11), ("42", "01", 1)] {
        let output = count(&[fixture, "--entry", "adaptive", "--seed", seed]);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains(&format!("measurement outcomes: {outcomes}\n")), "{text}");
        assert!(text.contains(&format!("measurement source: synthetic bits (seed {seed})\n")), "{text}");
        assert!(text.contains(&format!("one-qubit gates: {gates}\n")), "{text}");
    }
}

#[test]
fn executes_and_reports_applied_loop_circuits() {
    let fixture = "../crates/cli/tests/fixtures/count-device.bn";
    let selected = [
        ("auto", "saved decisions (4 decisions)", "one proved loop with fixed continuation and exit charges"),
        ("step-machine", "step machine (17 ticks)", "step machine requested for a qubit-dependent jump"),
    ];
    for (mode, construction, reason) in selected {
        let args = [fixture, "--entry", "applied_countdown", "--arg", "19", "--apply-circuit", mode];
        let counted = count(&args);
        let simulated = Command::new(env!("CARGO_BIN_EXE_bennett"))
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples"))
            .args(["run", "--device", "simulator"])
            .args(args)
            .output()
            .expect("ran simulator");
        for output in [&counted, &simulated] {
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(text.starts_with(&format!("apply @countdown uses {construction} ({reason})\n")), "{text}");
            assert!(text.contains("steps: 9\n"), "{text}");
        }
        assert!(String::from_utf8_lossy(&simulated.stdout).contains("result: 1\n"));
    }
}
