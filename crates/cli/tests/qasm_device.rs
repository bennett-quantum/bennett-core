//! Read OpenQASM traces independently of the device that writes them.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
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
    run_command(device, args, path).output().expect("run bennett")
}

fn run_command(device: &str, args: &[&str], path: Option<&Path>) -> Command {
    let mut command = Command::new(BIN);
    command.current_dir(EXAMPLES).args(["run", "--device", device]);
    if let Some(path) = path {
        command.arg("--qasm-output").arg(path);
    }
    command.args(args);
    command
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
    let status = Command::new("python3").args(["-c", python]).args(traces.iter().map(|trace| &trace.0)).status();
    match status {
        Ok(status) => assert!(status.success(), "OpenQASM parser exited with {status}"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            writeln!(
                io::stderr().lock(),
                "Skipped OpenQASM syntax check. Install Python 3 on PATH and openqasm3[parser]==1.0.1."
            )
            .expect("report missing python3");
        }
        Err(error) => panic!("run the Python OpenQASM syntax check: {error}"),
    }
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

// A second measurement of the same qubit repeats the first outcome, whichever it was.
#[test]
fn measurements_write_the_outcomes_where_the_controller_received_them() {
    let mut seen = [false; 2];
    for seed in 0..8 {
        let seed = seed.to_string();
        let args = [COUNTS, "--entry", "adaptive", "--seed", &seed];
        let trace = Trace::new(&format!("adaptive-{seed}"));
        let report = trace.export(&args);
        let text = trace.read();
        let commands: Vec<_> = text.lines().skip_while(|line| *line != "h q0[0];").collect();
        let first = commands[1].strip_prefix("measure q0[0]; // outcome ").expect("first measurement");
        assert!(report.contains(&format!("result: {first}\n")), "{report}");
        assert_eq!(commands[2], format!("measure q0[0]; // outcome {first}"), "seed {seed}");
        assert_eq!(commands.last(), Some(&format!("// returned {first} (run status 0)").as_str()));
        let flips = if first == "1" { 10 } else { 0 };
        assert_eq!(commands[3..commands.len() - 1], vec!["x q0[0];"; flips]);
        // The counter takes synthetic bits, so its counts describe another execution of this program.
        assert_eq!(recount(&text), [1 + flips, 0, 0]);
        seen[usize::from(first == "1")] = true;
    }
    assert_eq!(seen, [true, true], "the seeds reach both outcomes");
}

// The outcomes come from the run's own simulation: both halves of a Bell pair agree under every seed,
// both values occur, and the result is the one the simulator device reports for the same seed. Two
// independent coins are sampled jointly, as the simulator samples a register: one draw for both, so
// each seed gives the simulator's value (a draw per qubit would give another value under some seed).
#[test]
fn measured_outcomes_follow_the_simulated_state() {
    let mut values = std::collections::BTreeSet::new();
    for seed in 0..16 {
        let seed = seed.to_string();
        let args = [FIXTURE, "--entry", "two_coins", "--seed", &seed];
        let trace = Trace::new(&format!("coins-{seed}"));
        let exported = trace.export(&args);
        let simulated = report(run("simulator", &args, None));
        let value = simulated.lines().next().expect("a result line");
        assert!(exported.starts_with(&format!("{value}\n")), "seed {seed}: {exported} against {simulated}");
        values.insert(value.to_owned());
    }
    assert_eq!(values.len(), 4, "the seeds reach every value: {values:?}");
    let mut seen = [false; 2];
    for seed in 0..16 {
        let seed = seed.to_string();
        let args = [FIXTURE, "--entry", "bell", "--seed", &seed];
        let trace = Trace::new(&format!("bell-{seed}"));
        let exported = trace.export(&args);
        let outcomes: Vec<_> = trace
            .read()
            .lines()
            .filter_map(|line| line.split_once("; // outcome ").map(|(_, bit)| bit.to_owned()))
            .collect();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0], outcomes[1], "seed {seed}: a Bell pair's outcomes agree");
        let value = if outcomes[0] == "1" { 3 } else { 0 };
        assert!(exported.starts_with(&format!("result: {value}")), "{exported}");
        let simulated = report(run("simulator", &args, None));
        assert!(simulated.starts_with(&format!("result: {value}")), "seed {seed}: {simulated}");
        seen[usize::from(value == 3)] = true;
    }
    assert_eq!(seen, [true, true], "the seeds reach both outcomes");
}

fn outcomes(text: &str) -> String {
    text.lines().filter_map(|line| line.split_once("; // outcome ").map(|(_, bit)| bit)).collect()
}

// By default the file says its outcomes come from the run's simulation; with --qasm-outcomes synthetic it
// says they are synthetic, and they are the counter's bits for the same seed, so a Bell pair's two outcomes
// differ under some seed.
#[test]
fn the_file_names_where_its_outcomes_come_from() {
    let simulated = Trace::new("source-simulated");
    simulated.export(&[FIXTURE, "--entry", "bell", "--seed", "3"]);
    let text = simulated.read();
    assert!(text.contains("// Bennett run; measurement outcomes sampled from the run's own simulation (seed 3)."));
    assert!(!text.contains("synthetic"), "{text}");
    let mut differed = false;
    for seed in 0..16 {
        let seed = seed.to_string();
        let args = [FIXTURE, "--entry", "bell", "--seed", &seed];
        let trace = Trace::new(&format!("source-synthetic-{seed}"));
        let exported = trace.export(&[&args[..], &["--qasm-outcomes", "synthetic"]].concat());
        assert!(exported.contains(&format!("measurement source: synthetic bits (seed {seed})\n")), "{exported}");
        let text = trace.read();
        assert!(text.contains(&format!(
            "// Bennett run; synthetic measurement outcomes (seed {seed}), not from a simulation."
        )));
        assert!(text.contains("// An annotated outcome can have probability zero."));
        let counted = report(run("count", &args, None));
        let bits = counted.lines().find_map(|line| line.strip_prefix("measurement outcomes: ")).expect("counted bits");
        assert_eq!(outcomes(&text), bits, "seed {seed}");
        differed |= bits.as_bytes()[0] != bits.as_bytes()[1];
    }
    assert!(differed, "synthetic bits ignore the state");
}

// A simulation beyond --max-configurations (one configuration here; the Bell pair needs two) is refused by
// name, naming --qasm-outcomes synthetic, and publishes nothing; with that option the same run is written.
#[test]
fn a_state_the_simulation_cannot_hold_is_refused_naming_the_option() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("trace directory");
    let path = dir.path().join("output.qasm");
    let args = [FIXTURE, "--entry", "bell", "--max-configurations", "1"];
    let output = run("qasm", &args, Some(&path));
    assert_eq!(output.status.code(), Some(2));
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("more than 1 basis configurations"), "{diagnostic}");
    assert!(diagnostic.contains("--qasm-outcomes synthetic"), "{diagnostic}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(dir.path().read_dir().expect("read trace directory").count(), 0);
    let written = report(run("qasm", &[&args[..], &["--qasm-outcomes", "synthetic"]].concat(), Some(&path)));
    assert!(written.contains("measurement source: synthetic bits (seed 0)"), "{written}");
    assert!(path.exists());
}

// A device writes an applied function's fault code into the fault register and goes on, as the simulator's
// gate mode does and its direct evaluation does under --machine-meaning or --allow-region-faults. The
// default direct evaluation stops at such an `apply` (a region fault, a simulator's diagnostic). `fault.bn`
// with 5 runs out of the applied function's budget.
#[test]
fn an_applied_fault_is_written_as_under_the_machine_meaning() {
    let args = ["fault.bn", "--arg", "5", "--seed", "0"];
    let trace = Trace::new("applied-fault");
    let exported = trace.export(&args);
    assert!(exported.contains("result: 261\n"), "{exported}");
    let meaning = report(run("simulator", &[&args[..], &["--machine-meaning"]].concat(), None));
    assert!(meaning.starts_with("result: 261\n"), "{meaning}");
    let allowed = report(run("simulator", &[&args[..], &["--allow-region-faults"]].concat(), None));
    assert!(allowed.starts_with("result: 261\n"), "{allowed}");
    let gates = report(run("simulator", &[&args[..], &["--apply-circuit", "auto"]].concat(), None));
    assert!(gates.contains("result: 261\n"), "{gates}");
    let strict = run("simulator", &args, None);
    assert_eq!(strict.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&strict.stdout).starts_with("region fault: apply @triangle faulted (budget)"));
}

// An `apply` the gate compiler refuses (here `retaddr`) stops the OpenQASM run and the simulator's gate
// mode alike with status 2; the trace is published with the stop. Direct evaluation runs it.
#[test]
fn a_refused_apply_stops_as_in_the_simulators_gate_mode() {
    let args = [FIXTURE, "--entry", "refused", "--seed", "0"];
    let trace = Trace::new("refused");
    let output = run("qasm", &args, Some(&trace.0));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stop = "cannot emit retaddr";
    assert!(String::from_utf8_lossy(&output.stdout).starts_with(&format!("stopped: {stop} in @returns_to")));
    let last = trace.read().lines().last().expect("a footer").to_owned();
    assert!(last.starts_with(&format!("// stopped {stop}")) && last.ends_with("(run status 2)"), "{last}");
    let gates = run("simulator", &[&args[..], &["--apply-circuit", "auto"]].concat(), None);
    assert_eq!(gates.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&gates.stdout).starts_with(&format!("stopped: {stop} in @returns_to")));
    assert!(report(run("simulator", &args, None)).starts_with("result: 1\n"));
}

// The initial state already holds one basis configuration, so a limit of zero is refused before any
// gate, naming --qasm-outcomes synthetic, with nothing published.
#[test]
fn a_limit_below_the_initial_state_is_refused() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("trace directory");
    let path = dir.path().join("output.qasm");
    let args = [FIXTURE, "--entry", "measure_fresh", "--max-configurations", "0"];
    let output = run("qasm", &args, Some(&path));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("more than 0 basis configurations"), "{diagnostic}");
    assert!(diagnostic.contains("--qasm-outcomes synthetic"), "{diagnostic}");
    assert_eq!(dir.path().read_dir().expect("read trace directory").count(), 0);
    report(run("qasm", &[FIXTURE, "--entry", "measure_fresh", "--max-configurations", "1"], Some(&path)));
    assert!(path.exists());
}

/// The address space, in KiB, given to a child that must abort while compiling its second `apply`: room for
/// the program and its first, small circuit, none for a step-machine circuit of 10^12 ticks.
const CHILD_ADDRESS_SPACE_KIB: u64 = 32 * 1024;

// The counter and the simulator print the report of an `apply` when they compile it. After one small
// `apply`, this program applies a loop through the step machine with a budget of 10^12 steps, whose circuit
// does not fit in the address space the child is given: the child aborts while compiling it, so a report
// kept until the run ends would never be printed. Reading ends with the child.
#[cfg(target_os = "linux")]
#[test]
fn count_and_simulator_report_an_apply_while_the_run_goes_on() {
    for device in ["count", "simulator"] {
        let output = Command::new("sh")
            .current_dir(EXAMPLES)
            .args(["-c", "ulimit -v \"$1\" && shift && exec \"$@\"", "sh", &CHILD_ADDRESS_SPACE_KIB.to_string(), BIN])
            .args(["run", "--device", device, "--apply-circuit", "step-machine"])
            .args([FIXTURE, "--entry", "apply_then_outgrow"])
            .output()
            .expect("run bennett with a bounded address space");
        assert!(!output.status.success(), "{device}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("memory allocation"), "{device}: {output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("apply @identity uses "), "{device}: {stdout}");
        assert!(!stdout.contains("apply @count_up"), "{device}: {stdout}");
    }
}

// A rotation by 1.75, 19.1875 and -20.9375 radians leaves q[0] at zero, so the register reads 0 or 2. At
// seed 0 the draw and the weight of outcome 0 round to the same number and the sampler takes the next
// outcome, 1, whose weight is rounding error of about 5e-32.
#[test]
#[ignore = "the simulator's sampler selects an outcome of weight about 5e-32 when a draw and the weight of \
            the outcome before it round to the same number (crates/exec/src/quantum.rs, measure_bits)"]
fn a_draw_on_a_rounded_boundary_selects_only_possible_outcomes() {
    let trace = Trace::new("boundary");
    let exported = trace.export(&[FIXTURE, "--entry", "boundary", "--seed", "0"]);
    let result = exported.lines().next().expect("a result line");
    assert!(result == "result: 0" || result.starts_with("result: 2 "), "{exported}");
}

/// One qubit's state under the one-qubit gates of `stdgates.inc`, worked here from their definitions.
fn one_qubit_gate(name: &str, [a, b]: [(f64, f64); 2]) -> [(f64, f64); 2] {
    let r = std::f64::consts::FRAC_1_SQRT_2;
    let times = |(x, y): (f64, f64), (c, d): (f64, f64)| (x * c - y * d, x * d + y * c);
    match name {
        "h" => [((a.0 + b.0) * r, (a.1 + b.1) * r), ((a.0 - b.0) * r, (a.1 - b.1) * r)],
        "x" => [b, a],
        "z" => [a, (-b.0, -b.1)],
        "s" => [a, times(b, (0.0, 1.0))],
        "sdg" => [a, times(b, (0.0, -1.0))],
        "t" => [a, times(b, (r, r))],
        "tdg" => [a, times(b, (r, -r))],
        other => panic!("not a one-qubit gate of this check: {other}"),
    }
}

// `h t t sdg h` is the identity and so is `h tdg tdg s h`; with T and T-dagger exchanged in the file,
// each would be `h z h`, a NOT. Reading the file's own gates, each recorded outcome must have
// probability one.
#[test]
fn t_and_t_dagger_are_written_as_themselves() {
    let trace = Trace::new("phases");
    assert!(trace.export(&[FIXTURE, "--entry", "phases"]).starts_with("result: 0\n"));
    let mut qubits = HashMap::new();
    let mut measured = 0;
    for line in trace.read().lines() {
        let (code, comment) = line.split_once("//").map_or((line, ""), |(code, comment)| (code.trim(), comment.trim()));
        let Some(statement) = code.strip_suffix(';') else { continue };
        let Some((name, operand)) = statement.split_once(' ') else { continue };
        match name {
            "OPENQASM" | "include" | "reset" => {}
            declaration if declaration.starts_with("qubit[") => {}
            "measure" => {
                let [zero, one]: [(f64, f64); 2] = qubits[operand];
                let outcome = comment.strip_prefix("outcome ").expect("an annotated outcome");
                let amplitude = if outcome == "1" { one } else { zero };
                let probability = amplitude.0 * amplitude.0 + amplitude.1 * amplitude.1;
                assert!((probability - 1.0).abs() < 1e-12, "{operand} outcome {outcome}: probability {probability}");
                measured += 1;
            }
            gate => {
                let state = qubits.entry(operand.to_owned()).or_insert([(1.0, 0.0), (0.0, 0.0)]);
                *state = one_qubit_gate(gate, *state);
            }
        }
    }
    assert_eq!(measured, 2);
    let text = trace.read();
    assert_eq!(gates(&text).filter(|(name, _)| *name == "t").count(), 2);
    assert_eq!(gates(&text).filter(|(name, _)| *name == "tdg").count(), 2);
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
            "// returned (run status 0)",
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
    assert_eq!(short_end, "// returned 7 (run status 0)");
    assert_eq!(fault_end, "// fault budget (code 1, run status 1)");
}

#[test]
fn a_controller_stop_is_published_with_its_reason_and_exit_status() {
    let trace = Trace::new("stopped");
    let output = run("qasm", &[FIXTURE, "--entry", "stopped"], Some(&trace.0));
    assert_eq!(output.status.code(), Some(2));
    let text = trace.read();
    assert_eq!(recount(&text), [1, 0, 0]);
    let end = text.lines().last().expect("stop footer");
    assert!(
        end.starts_with("// stopped") && end.contains("cannot emit setjmp") && end.ends_with("(run status 2)"),
        "{end}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_reporting_error_leaves_the_controller_outcome_in_the_trace() {
    use std::process::Stdio;

    let full = fs::OpenOptions::new().write(true).open("/dev/full").expect("open /dev/full");
    let (reader, writer) = std::io::pipe().expect("create pipe");
    drop(reader);
    for (case, sink) in [("full", Stdio::from(full)), ("closed-pipe", Stdio::from(writer))] {
        let trace = Trace::new(case);
        let output = run_command("qasm", &[FIXTURE, "--entry", "short"], Some(&trace.0))
            .stdout(sink)
            .output()
            .expect("run with an unwritable stdout");
        assert!(!output.status.success(), "{case}: {output:?}");
        let text = trace.read();
        assert_eq!(recount(&text), [1, 0, 0], "{case}");
        assert_eq!(text.lines().last(), Some("// returned 7 (run status 0)"), "{case}");
    }
}

#[cfg(unix)]
#[test]
fn published_traces_have_the_mode_of_a_new_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("trace directory");
    for mask in ["022", "002", "077"] {
        let direct = dir.path().join(format!("direct-{mask}"));
        let trace = dir.path().join(format!("trace-{mask}.qasm"));
        let output = Command::new("sh")
            .current_dir(EXAMPLES)
            .args(["-c", "umask \"$1\" && : > \"$2\" && shift 2 && exec \"$@\"", "sh", mask])
            .arg(&direct)
            .arg(BIN)
            .args(["run", "--device", "qasm", "--qasm-output"])
            .arg(&trace)
            .args([FIXTURE, "--entry", "short"])
            .output()
            .expect("run bennett under the test umask");
        report(output);
        let mode = |path| fs::metadata(path).expect("output metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&trace), mode(&direct), "umask {mask}");
    }
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

// Z leaves the initial zero byte unchanged, so every simulated outcome is zero, the controller adds no X
// gates, and its request to release the byte in zero holds in the simulated state.
// The report of each compiled `apply` follows publication: a trace that is never published (here a file
// size limit of zero makes the final write fail) leaves no report of an `apply` it compiled.
#[cfg(target_os = "linux")]
#[test]
fn the_apply_report_follows_publication() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("trace directory");
    let path = dir.path().join("output.qasm");
    let args = [FIXTURE, "--entry", "applied", "--arg", "1"];
    let published = report(run("qasm", &args, Some(&path)));
    assert!(published.contains("apply @both uses "), "{published}");
    fs::remove_file(&path).expect("remove the published trace");
    let output = Command::new("sh")
        .current_dir(EXAMPLES)
        .args(["-c", "ulimit -f 0 && trap '' XFSZ && exec \"$@\"", "sh", BIN])
        .args(["run", "--device", "qasm", "--qasm-output"])
        .arg(&path)
        .args(args)
        .output()
        .expect("run bennett with a file size limit");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("File too large"), "{output:?}");
    assert_eq!(dir.path().read_dir().expect("read trace directory").count(), 0);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("apply @"), "{stdout}");
}

#[test]
fn a_zero_byte_measures_zero_and_is_released_in_zero() {
    let args = [FIXTURE, "--entry", "measure_zero_byte", "--seed", "1"];
    assert!(report(run("simulator", &args, None)).starts_with("result: 0\n"));
    let trace = Trace::new("zero-byte");
    assert!(trace.export(&args).starts_with("result: 0\n"));
    let text = trace.read();
    assert!(text.contains("measurement outcomes sampled from the run's own simulation (seed 1)"));
    assert!(text.contains("A release_zero request assumes a replay with the recorded outcomes."));
    let measured: String =
        text.lines().filter_map(|line| line.split_once("; // outcome ").map(|(_, bit)| bit)).collect();
    assert_eq!(measured, "00000000");
    assert_eq!(gates(&text).collect::<Vec<_>>(), [("z", vec!["q0[0]"])]);
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
    let output = run("count", &[COUNTS, "--qasm-outcomes", "synthetic"], None);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--qasm-outcomes requires --device qasm"), "{output:?}");
    fs::write(&trace.0, "existing output").expect("create an existing output file");
    let output = run("qasm", &[COUNTS], Some(&trace.0));
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot write OpenQASM output"), "{output:?}");
    assert_eq!(trace.read(), "existing output");
}
