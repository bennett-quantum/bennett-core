//! Control lists in memory (`docs/SYNTAX.md`, "Control lists"): `gate(%k) target ctrl list(%p, %n)` and
//! `rotate(%k, %a) target ctrl list(%p, %n)` read `n` entries of 16 bytes at `p`, each a handle at offset 0
//! and a position at offset 8. Checked: the same amplitudes as the controls written out, for every gate and
//! rotation code; a qubit listed twice as one control; an empty list at any address; the checks in the
//! interface's order (kind, angle, target handle and position, the list readable, each entry's handle and
//! position in list order, then the target among the controls), each before anything changes; forward then
//! reverse back to every earlier state; `quantum_in_region` inside an applied function.

mod common;

use bennett_exec::{Options, Outcome};
use bennett_ir::{Fault, GATE_CODES, ROTATION_CODES};
use common::{bounded, executor, run};

/// Handles: `%q` is 1 (three qubits), `%h` is 2 (two). The list's two entries are written at 64 from the
/// arguments; the gate acts on `%t[%b]` under `controls`.
fn program(gate: &str, controls: &str) -> String {
    format!(
        "memory 512
func @main(i64 %k, i64 %a, i64 %t, i64 %b, i64 %e0h, i64 %e0p, i64 %e1h, i64 %e1p, i64 %p, i64 %n) {{
entry:
    %q = qalloc i64 3
    %h = qalloc i64 2
    h %q[0]
    h %q[1]
    ry(0.6) %q[2]
    h %h[0]
    ry(0.9) %h[1]
    store i64 %e0h, 64
    store i64 %e0p, 72
    store i64 %e1h, 80
    store i64 %e1p, 88
    {gate} %t[%b]{controls}
    ret
}}"
    )
}

/// The steps before the gate.
const BEFORE: usize = 11;

/// Arguments: kind, angle, target handle and position, two entries, list address and count.
struct Args {
    kind: u64,
    angle: f64,
    target: (u64, u64),
    entries: [(u64, u64); 2],
    list: (u64, u64),
}

impl Args {
    fn new(kind: u64) -> Self {
        Args { kind, angle: 0.7, target: (2, 1), entries: [(1, 0), (1, 2)], list: (64, 2) }
    }

    fn values(&self) -> [u64; 10] {
        let [(h0, p0), (h1, p1)] = self.entries;
        [self.kind, self.angle.to_bits(), self.target.0, self.target.1, h0, p0, h1, p1, self.list.0, self.list.1]
    }
}

fn state(text: &str, args: &Args) -> bennett_exec::QuantumSnapshot {
    let module = common::load(text);
    let (outcome, executor) = run(&module, "main", &args.values(), bounded());
    assert_eq!(outcome, Outcome::Returned(None), "{text}");
    executor.snapshot().quantum
}

fn kinds() -> Vec<(&'static str, u64)> {
    let gates = GATE_CODES.iter().map(|&(code, _)| ("gate(%k)", code));
    gates.chain(ROTATION_CODES.iter().map(|&(code, _)| ("rotate(%k, %a)", code))).collect()
}

#[test]
fn a_list_gives_the_amplitudes_of_its_controls_written_out() {
    for (gate, code) in kinds() {
        let args = Args::new(code);
        let listed = state(&program(gate, " ctrl list(%p, %n)"), &args);
        let written = state(&program(gate, " ctrl %q[0], %q[2]"), &args);
        assert_eq!(listed.distance(&written), Some(0.0), "{gate} {code}");
        // The target `%h[1]` holds both values, so every gate, diagonal ones included, acts differently with
        // and without the controls: a list whose controls were lost would fail here.
        let uncontrolled = state(&program(gate, ""), &args);
        assert_ne!(listed.distance(&uncontrolled), Some(0.0), "{gate} {code}: the controls act");
        let one_control = state(&program(gate, " ctrl %q[0]"), &args);
        assert_ne!(listed.distance(&one_control), Some(0.0), "{gate} {code}: the second control acts");
    }
}

#[test]
fn a_qubit_listed_twice_is_one_control_and_an_empty_list_reads_nothing() {
    for (gate, code) in kinds() {
        let twice = Args { entries: [(1, 0), (1, 0)], ..Args::new(code) };
        let listed = state(&program(gate, " ctrl list(%p, %n)"), &twice);
        let once = state(&program(gate, " ctrl %q[0]"), &twice);
        assert_eq!(listed.distance(&once), Some(0.0), "{gate} {code}: twice");
        for address in [0, 64, 505, u64::MAX] {
            let empty = Args { list: (address, 0), ..Args::new(code) };
            let listed = state(&program(gate, " ctrl list(%p, %n)"), &empty);
            let none = state(&program(gate, ""), &empty);
            assert_eq!(listed.distance(&none), Some(0.0), "{gate} {code}: empty at {address}");
        }
    }
}

/// The first failing check is the fault, and nothing has changed.
fn fault(gate: &str, args: &Args) -> Outcome {
    let module = common::load(&program(gate, " ctrl list(%p, %n)"));
    let mut executor = executor(&module, "main", &args.values(), bounded());
    for _ in 0..BEFORE {
        assert_eq!(executor.step(), None);
    }
    let before = executor.snapshot();
    let outcome = executor.run();
    if let Outcome::Faulted(_) = outcome {
        let after = executor.snapshot();
        assert_eq!(after.quantum, before.quantum, "{gate}: the state");
        assert_eq!(after.classical.memory, before.classical.memory, "{gate}: memory");
    }
    outcome
}

#[test]
fn the_checks_run_in_the_interfaces_order() {
    let (gate, rotate) = ("gate(%k)", "rotate(%k, %a)");
    let unreadable = (u64::MAX, 1);
    let faulted = Outcome::Faulted;
    // The kind and the angle before the qubits and the list.
    let bad = Args { list: unreadable, target: (7, 9), ..Args::new(0) };
    assert_eq!(fault(gate, &bad), faulted(Fault::BadGate));
    let nan = Args { angle: f64::NAN, list: unreadable, target: (7, 9), ..Args::new(9) };
    assert_eq!(fault(rotate, &nan), faulted(Fault::BadAngle));
    // The target's handle and position before the list.
    assert_eq!(
        fault(gate, &Args { target: (7, 0), list: unreadable, ..Args::new(1) }),
        faulted(Fault::DanglingRegister)
    );
    assert_eq!(fault(gate, &Args { target: (2, 2), list: unreadable, ..Args::new(1) }), faulted(Fault::BadRange));
    // The list readable as ordinary memory, its byte count as a mathematical integer, before any entry.
    for list in [unreadable, (505, 1), (497, 1), (0, 1 << 60), (16, u64::MAX), (u64::MAX - 7, 2)] {
        let args = Args { list, entries: [(9, 0), (9, 0)], ..Args::new(1) };
        assert_eq!(fault(gate, &args), faulted(Fault::BadArgument), "{list:?}");
    }
    assert_eq!(
        fault(gate, &Args { list: (496, 1), ..Args::new(1) }),
        faulted(Fault::DanglingRegister),
        "the last 16 bytes"
    );
    // Each entry in list order: its handle, then its position.
    for (entries, expected) in [
        ([(1, 0), (9, 0)], Fault::DanglingRegister),
        ([(1, 0), (1, 3)], Fault::BadRange),
        ([(9, 0), (1, 3)], Fault::DanglingRegister),
        ([(1, 3), (9, 0)], Fault::BadRange),
        ([(0, 0), (1, 0)], Fault::DanglingRegister),
        ([(1, 1 << 32), (1, 0)], Fault::BadRange),
    ] {
        assert_eq!(fault(gate, &Args { entries, ..Args::new(1) }), faulted(expected), "{entries:?}");
    }
    // The target among the controls last, after every entry is checked.
    assert_eq!(fault(gate, &Args { entries: [(1, 0), (2, 1)], ..Args::new(1) }), faulted(Fault::QubitAlias));
    assert_eq!(fault(gate, &Args { entries: [(2, 1), (1, 9)], ..Args::new(1) }), faulted(Fault::BadRange));
    assert_eq!(fault(rotate, &Args { entries: [(2, 1), (2, 1)], ..Args::new(12) }), faulted(Fault::QubitAlias));
    assert_eq!(fault(gate, &Args::new(1)), Outcome::Returned(None));
}

/// A list over a byte that holds qubits is not readable as ordinary memory: `bad_argument`.
#[test]
fn a_list_over_a_byte_holding_qubits_is_bad_argument() {
    let module = common::load(
        "memory 256
func @main(i64 %n) {
entry:
    %q = qalloc i64 2
    store i64 1, 64
    store i64 0, 72
    store i64 1, 80
    store i64 0, 88
    h [90:3]
    gate(1) %q[1] ctrl list(64, %n)
    ret
}",
    );
    assert_eq!(run(&module, "main", &[1], bounded()).0, Outcome::Returned(None), "the first entry only");
    assert_eq!(run(&module, "main", &[2], bounded()).0, Outcome::Faulted(Fault::BadArgument));
}

#[test]
fn forward_then_reverse_restores_each_earlier_state() {
    for (gate, code) in kinds() {
        let module = common::load(&program(gate, " ctrl list(%p, %n)"));
        let args = Args::new(code).values();
        let mut executor = executor(&module, "main", &args, Options { reversible: true, ..bounded() });
        let mut before = vec![executor.snapshot()];
        while executor.step().is_none() {
            before.push(executor.snapshot());
        }
        assert_eq!(executor.outcome(), Some(Outcome::Returned(None)), "{code}");
        while let Some(expected) = before.pop() {
            assert!(executor.step_back().expect("reversible"), "{code}");
            common::assert_same_state(&executor.snapshot(), &expected, &format!("code {code}: step {}", before.len()));
        }
    }
}

/// Inside an applied function a gate with a list is a quantum instruction: `quantum_in_region` before its
/// kind or its list is looked at.
#[test]
fn a_listed_gate_inside_an_applied_function_faults_quantum_in_region() {
    for body in ["gate(%a) %a[%a] ctrl list(%a, %a)", "rotate(%a, %a) %a[0] ctrl list(%a, 1)"] {
        let module = common::load(&format!(
            "memory 256
func @f(i64 %a) -> i1 {{
entry:
    {body}
    ret 0
}}
func @main(i64 %a) -> i5 {{
entry:
    %o = qalloc 1
    %f = qalloc 5
    apply @f(%a) -> %o, %f budget 10
    %m = measure %f
    ret %m
}}"
        ));
        let (outcome, _) = run(&module, "main", &[u64::MAX], common::allowing_region_faults());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))), "{body}");
    }
}

/// A listed gate whose target is a byte of its own list: the gate turns the byte into qubits, so the list can
/// no longer be read as ordinary memory. Undoing it uses the controls as the gate read them, and every
/// earlier state comes back.
#[test]
fn undoing_a_gate_on_a_byte_of_its_own_list_restores_each_earlier_state() {
    let module = common::load(
        "memory 128
func @main() {
entry:
    %c = qalloc i64 1
    h %c[0]
    store i64 %c, 64
    gate(1) [64:0] ctrl list(64, 1)
    ret
}",
    );
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let mut before = vec![executor.snapshot()];
    while executor.step().is_none() {
        before.push(executor.snapshot());
    }
    assert_eq!(executor.outcome(), Some(Outcome::Returned(None)));
    assert!(!executor.snapshot().quantum.cells.is_empty(), "the gate made the byte qubits");
    while let Some(expected) = before.pop() {
        assert!(executor.step_back().expect("reversible"));
        common::assert_same_state(&executor.snapshot(), &expected, &format!("step {}", before.len()));
    }
}
