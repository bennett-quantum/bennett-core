//! Read OpenQASM traces independently of the device that writes them.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const COUNTS: &str = "../crates/cli/tests/fixtures/count-device.bn";
const FIXTURE: &str = "../crates/cli/tests/fixtures/qasm-device.bn";
const BIN: &str = env!("CARGO_BIN_EXE_bennett");
const EXAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
const CIRCUITS: &[(&str, &[&str])] = &[
    ("grover", &["grover.bn", "--entry", "searched", "--arg", "2"]),
    ("applied", &[FIXTURE, "--entry", "applied", "--arg", "1"]),
    ("rotations", &[FIXTURE, "--entry", "rotations"]),
    ("toffolis", &[COUNTS]),
];

struct Trace(PathBuf);

impl Trace {
    fn new(case: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
        fs::create_dir_all(dir).expect("create the Cargo test directory");
        Self(dir.join(format!("qasm-{}-{case}.qasm", std::process::id())))
    }

    fn export(&self, args: &[&str]) -> String {
        report(run("qasm", args, Some(&self.0)))
    }

    fn read(&self) -> String {
        fs::read_to_string(&self.0).expect("read the OpenQASM file")
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn run(device: &str, args: &[&str], path: Option<&Path>) -> Output {
    let mut command = Command::new(BIN);
    command.current_dir(EXAMPLES).args(["run", "--device", device]);
    if let Some(path) = path {
        command.arg("--qasm-output").arg(path);
    }
    command.args(args).output().expect("run bennett")
}

fn report(output: Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("read the bennett report")
}

fn gates(text: &str) -> impl Iterator<Item = (&str, Vec<&str>)> {
    text.lines().filter_map(|line| {
        let line = line.split_once("//").map_or(line, |(code, _)| code).trim();
        if line.is_empty() {
            return None;
        }
        let gate = line.trim_start_matches("ctrl @ ");
        let (name, operands) = gate
            .strip_suffix(';')
            .expect("a complete QASM statement")
            .split_once(' ')
            .expect("a command with operands");
        match name {
            "OPENQASM" | "include" | "measure" | "reset" => None,
            declaration if declaration.starts_with("qubit[") => None,
            _ => {
                let wires: Vec<_> = operands.split(", ").collect();
                let controls = (line.len() - gate.len()) / "ctrl @ ".len();
                assert_eq!(wires.len(), controls + 1, "{line}");
                Some((name, wires))
            }
        }
    })
}

fn recount(text: &str) -> [usize; 3] {
    let mut counts = [0; 3];
    for (gate, wires) in gates(text) {
        if wires.len() == 3 {
            assert_eq!(gate, "x", "only X has two controls");
        }
        counts[wires.len() - 1] += 1;
    }
    counts
}

fn counted(args: &[&str]) -> [usize; 3] {
    let text = report(run("count", args, None));
    ["one-qubit gates: ", "two-qubit gates: ", "Toffoli gates: "]
        .map(|prefix| text.lines().find_map(|line| line.strip_prefix(prefix)).expect(prefix).parse().expect(prefix))
}

#[test]
fn file_gate_counts_match_the_counting_device() {
    for (case, args) in CIRCUITS {
        let trace = Trace::new(case);
        trace.export(args);
        assert_eq!(recount(&trace.read()), counted(args), "{case}");
    }
}

#[test]
fn reference_parser_accepts_the_circuits() {
    let traces: Vec<_> = CIRCUITS
        .iter()
        .copied()
        .chain([("adaptive", &[COUNTS, "--entry", "adaptive", "--seed", "1"][..])])
        .map(|(case, args)| {
            let trace = Trace::new(&format!("parse-{case}"));
            trace.export(args);
            trace
        })
        .collect();
    let python = r#"
from importlib.metadata import PackageNotFoundError, version
from importlib.util import find_spec
from pathlib import Path
import sys

try:
    installed = version('openqasm3')
except PackageNotFoundError:
    installed = None

if installed != '1.0.1' or find_spec('antlr4') is None:
    print('Skipped OpenQASM syntax check. Install openqasm3[parser]==1.0.1.', file=sys.stderr)
    sys.exit(0)

from openqasm3.parser import parse

for filename in sys.argv[1:]:
    parse(Path(filename).read_text(encoding='utf-8'))
"#;
    let status = Command::new("python3")
        .args(["-c", python])
        .args(traces.iter().map(|trace| &trace.0))
        .status()
        .expect("run the Python OpenQASM syntax check");
    assert!(status.success(), "OpenQASM parser exited with {status}");
}

#[test]
fn the_applied_circuit_preserves_inputs_and_xors_both_destinations() {
    for (right, expected) in [("0", true), ("1", false)] {
        let trace = Trace::new(&format!("basis-{right}"));
        trace.export(&[FIXTURE, "--entry", "applied", "--arg", right]);
        let text = trace.read();
        let mut bits = HashMap::new();
        for (name, operands) in gates(&text) {
            assert_eq!(name, "x");
            let (target, controls) = operands.split_last().expect("X has a target");
            if controls.iter().all(|wire| bits.get(wire) == Some(&true)) {
                let target = bits.entry(*target).or_insert(false);
                *target = !*target;
            }
        }
        let bit = |wire: &str| bits.get(wire).copied().unwrap_or(false);
        assert!(bit("q0[0]"));
        assert_eq!(bit("q1[0]"), right == "1");
        assert_eq!(bit("q2[0]"), expected);
        assert_eq!(std::array::from_fn(|i| bit(&format!("q3[{i}]"))), [true, false, true, false, false]);
    }
}

#[test]
fn rotations_preserve_angle_bits_and_control_then_target_order() {
    let trace = Trace::new("angles");
    trace.export(&[FIXTURE, "--entry", "rotations"]);
    let text = trace.read();
    let angles: Vec<_> = gates(&text)
        .filter_map(|(name, operands)| {
            let (axis, angle) = name.strip_suffix(')')?.split_once('(')?;
            Some((axis, angle.parse::<f64>().expect("read the angle").to_bits(), operands))
        })
        .collect();
    assert_eq!(
        angles,
        [
            ("rx", (-0.12345678901234568_f64).to_bits(), vec!["q1[0]", "q0[2]"]),
            ("ry", 1, vec!["q0[0]"]),
            ("rz", f64::MAX.to_bits(), vec!["q0[0]", "q0[1]"]),
            ("rz", (-0.0_f64).to_bits(), vec!["q0[2]"]),
        ]
    );
}

#[test]
fn measurements_write_the_outcomes_where_the_controller_received_them() {
    for (seed, first, second) in [("1", "1", "0"), ("42", "0", "1")] {
        let args = [COUNTS, "--entry", "adaptive", "--seed", seed];
        let trace = Trace::new(&format!("adaptive-{seed}"));
        let report = trace.export(&args);
        let text = trace.read();
        let commands: Vec<_> = text.lines().skip_while(|line| *line != "h q0[0];").collect();
        assert!(report.contains(&format!("result: {first}\n")), "{report}");
        assert_eq!(commands[1], format!("measure q0[0]; // outcome {first}"));
        assert_eq!(commands[2], format!("measure q0[0]; // outcome {second}"));
        assert_eq!(commands.last(), Some(&format!("// returned {first} (exit code 0)").as_str()));
        assert_eq!(commands[3..commands.len() - 1], vec!["x q0[0];"; if first == "1" { 10 } else { 0 }]);
        assert_eq!(recount(&text), counted(&args));
    }
}

#[test]
fn discarding_a_register_leaves_it_idle_and_later_allocation_starts_fresh() {
    let trace = Trace::new("discard");
    trace.export(&[COUNTS, "--entry", "dirty_discard"]);
    let text = trace.read();
    let commands: Vec<_> = text.lines().skip_while(|line| *line != "qubit[1] q0;").collect();
    assert_eq!(
        commands,
        [
            "qubit[1] q0;",
            "reset q0;",
            "x q0[0];",
            "// discard q0[0] (partial trace)",
            "qubit[1] q1;",
            "reset q1;",
            "// returned (exit code 0)",
        ]
    );
}

#[test]
fn a_program_fault_preserves_the_commands_reached_and_the_exit_status() {
    let short = Trace::new("return");
    short.export(&[FIXTURE, "--entry", "short"]);
    let fault = Trace::new("budget");
    let output = run("qasm", &[FIXTURE, "--entry", "long", "--budget", "2"], Some(&fault.0));
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("fault: budget (code 1)\nsteps: 2\n"));
    let short = short.read();
    let fault = fault.read();
    let (short_commands, short_end) = short.trim_end().rsplit_once('\n').expect("return trace with footer");
    let (fault_commands, fault_end) = fault.trim_end().rsplit_once('\n').expect("fault trace with footer");
    assert!(short_commands.ends_with("h q0[0];"));
    assert_eq!(short_commands, fault_commands);
    assert_eq!(short_end, "// returned 7 (exit code 0)");
    assert_eq!(fault_end, "// fault budget (code 1, exit code 1)");
}

#[test]
fn a_controller_stop_is_published_with_its_reason_and_exit_status() {
    let trace = Trace::new("stopped");
    let output = run("qasm", &[FIXTURE, "--entry", "stopped"], Some(&trace.0));
    assert_eq!(output.status.code(), Some(2));
    let text = trace.read();
    assert_eq!(recount(&text), [1, 0, 0]);
    let end = text.lines().last().expect("stop footer");
    assert!(end.starts_with("// stopped") && end.contains("udiv") && end.ends_with("(exit code 2)"), "{end}");
}

#[test]
fn a_rejected_entry_does_not_publish_a_trace() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("trace directory");
    let path = dir.path().join("output.qasm");
    let output = run("qasm", &["grover.bn", "--entry", "diffuse", "--arg", "0"], Some(&path));
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("is a register"), "{output:?}");
    assert_eq!(dir.path().read_dir().expect("read trace directory").count(), 0);
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_size_limit_does_not_publish_a_partial_trace() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("trace directory");
    let path = dir.path().join("output.qasm");
    let cases: [(&str, &[&str]); 2] =
        [("0", &[FIXTURE, "--entry", "short"]), ("1", &["grover.bn", "--entry", "searched", "--arg", "2"])];
    for (limit, args) in cases {
        // Ignore SIGXFSZ so the kernel reports EFBIG to the writer instead of terminating the process.
        let output = Command::new("sh")
            .current_dir(EXAMPLES)
            .args(["-c", "ulimit -f \"$1\" && shift && trap '' XFSZ && exec \"$@\"", "sh", limit, BIN])
            .args(["run", "--device", "qasm", "--qasm-output"])
            .arg(&path)
            .args(args)
            .output()
            .expect("run bennett with a file size limit");
        assert_eq!(output.status.code(), Some(2));
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains("cannot write OpenQASM output") && diagnostic.contains("File too large"),
            "{diagnostic}"
        );
        assert_eq!(dir.path().read_dir().expect("read trace directory").count(), 0, "limit {limit}");
    }
}

#[test]
fn a_zero_byte_can_have_impossible_synthetic_outcomes() {
    let args = [FIXTURE, "--entry", "measure_zero_byte", "--seed", "1"];
    assert!(report(run("simulator", &args, None)).starts_with("result: 0\n"));
    let trace = Trace::new("zero-byte");
    assert!(trace.export(&args).starts_with("result: 37\n"));
    let text = trace.read();
    assert!(text.contains("synthetic measurement outcomes (seed 1)"));
    assert!(text.contains("An annotated outcome can have probability zero."));
    assert!(text.contains("A release_zero request assumes a replay with the recorded outcomes."));
    let measured: String =
        text.lines().filter_map(|line| line.split_once("; // outcome ").map(|(_, bit)| bit)).collect();
    assert_eq!(measured, "10100100");
    // Z leaves the initial zero byte unchanged. A physical measurement reads zero, so the three X gates
    // from the synthetic outcomes would leave the byte at 37 when the controller asks to release it in zero.
    assert_eq!(
        gates(&text).collect::<Vec<_>>(),
        [("z", vec!["q0[0]"]), ("x", vec!["q0[0]"]), ("x", vec!["q0[2]"]), ("x", vec!["q0[5]"])]
    );
    assert!(text.lines().any(|line| line.starts_with("// release_zero requested for q0[0]")));
}

#[test]
fn qasm_requires_a_new_output_file_and_disallows_reverse() {
    let trace = Trace::new("output-options");
    for (device, reverse, path, diagnostic) in [
        ("qasm", false, None, "--device qasm requires --qasm-output"),
        ("count", false, Some(trace.0.as_path()), "--qasm-output requires --device qasm"),
        ("qasm", true, Some(trace.0.as_path()), "--reverse requires the simulator"),
    ] {
        let mut args = vec![COUNTS];
        if reverse {
            args.push("--reverse");
        }
        let output = run(device, &args, path);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains(diagnostic), "{output:?}");
        assert!(!trace.0.exists());
    }
    fs::write(&trace.0, "existing output").expect("create an existing output file");
    let output = run("qasm", &[COUNTS], Some(&trace.0));
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot write OpenQASM output"), "{output:?}");
    assert_eq!(trace.read(), "existing output");
}
