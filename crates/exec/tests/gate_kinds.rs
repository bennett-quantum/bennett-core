//! Gate kinds at run time (`docs/SYNTAX.md`, "Gate kinds at run time"): `gate(%k)` with a gate code and
//! `rotate(%k, %a)` with a rotation code. Checked: each code gives the amplitudes of the gate it names
//! (Pauli Y against amplitudes worked out by hand, alone and controlled), `bad_gate` for every other value
//! and before the angle and the qubits are looked at, forward then reverse back to every earlier state, and
//! `quantum_in_region` before the kind is looked at.

mod common;

use std::f64::consts::FRAC_1_SQRT_2;

use bennett_exec::{Options, Outcome};
use bennett_ir::{Fault, GATE_CODES, ROTATION_CODES};
use common::{TOLERANCE, bounded, executor, run};
use num_complex::Complex64;

/// A register of three qubits in superposition and entangled, then `gate` on qubit 1 under a control on
/// qubit 0 and alone on qubit 2.
fn program(gate: &str) -> String {
    format!(
        "memory 256
func @main(i64 %k, i64 %a) {{
entry:
    %q = qalloc 3
    h %q
    ry(0.4) %q[1]
    x %q[2] ctrl %q
    rz(1.3) %q[2]
    {gate} %q[1] ctrl %q
    {gate} %q[2]
    ret
}}"
    )
}

fn state(text: &str, args: &[u64]) -> bennett_exec::QuantumSnapshot {
    let module = common::load(text);
    let (outcome, executor) = run(&module, "main", args, bounded());
    assert_eq!(outcome, Outcome::Returned(None), "{text}");
    executor.snapshot().quantum
}

/// The codes of `docs/HANDLES.md` 5.1 and `front/include/bq.h`, written out here rather than taken from the
/// device's table, so that a table with two codes exchanged fails.
const GATES: [(u64, &str); 8] = [(1, "x"), (2, "y"), (3, "z"), (4, "h"), (5, "s"), (6, "sdg"), (7, "t"), (8, "tdg")];
const ROTATIONS: [(u64, &str); 4] = [(9, "rx"), (10, "ry"), (11, "rz"), (12, "p")];

#[test]
fn the_device_tables_hold_the_interfaces_codes() {
    assert_eq!(GATE_CODES.map(|(code, gate)| (code, gate.mnemonic())), GATES);
    assert_eq!(ROTATION_CODES.map(|(code, axis)| (code, axis.mnemonic())), ROTATIONS);
}

#[test]
fn each_gate_code_gives_the_gate_it_names() {
    let by_code = program("gate(%k)");
    for (code, gate) in GATES {
        let expected = state(&program(gate), &[0, 0]);
        let actual = state(&by_code, &[code, 0]);
        assert_eq!(actual.distance(&expected), Some(0.0), "code {code}");
        assert!(actual.amplitudes.len() > 2, "code {code}: the gates act");
    }
}

#[test]
fn each_rotation_code_gives_the_rotation_it_names() {
    let by_code = program("rotate(%k, %a)");
    for (code, axis) in ROTATIONS {
        for angle in [0.7, -1.89, 7.5] {
            let expected = state(&program(&format!("{axis}(%a)")), &[0, f64::to_bits(angle)]);
            let actual = state(&by_code, &[code, f64::to_bits(angle)]);
            assert_eq!(actual.distance(&expected), Some(0.0), "code {code}, {angle}");
        }
    }
}

/// Code 2 is Pauli Y: on (|0⟩ + |1⟩)/√2 it gives (-i|0⟩ + i|1⟩)/√2; controlled, with the control in
/// (|0⟩ + |1⟩)/√2 and the target at 0, it gives (|00⟩ + i|11⟩)/√2, the control the low qubit.
#[test]
fn gate_code_two_is_pauli_y_alone_and_controlled() {
    let alone =
        state("memory 256\nfunc @main() {\nentry:\n    %q = qalloc 1\n    h %q\n    gate(2) %q\n    ret\n}", &[]);
    let r = FRAC_1_SQRT_2;
    let close = |actual: Complex64, expected: Complex64| assert!((actual - expected).norm() <= TOLERANCE, "{actual}");
    close(alone.amplitude(&[0]), Complex64::new(0.0, -r));
    close(alone.amplitude(&[1]), Complex64::new(0.0, r));
    let controlled = state(
        "memory 256\nfunc @main() {\nentry:\n    %q = qalloc 2\n    h %q\n    gate(2) %q[1] ctrl %q\n    ret\n}",
        &[],
    );
    close(controlled.amplitude(&[0]), Complex64::new(r, 0.0));
    close(controlled.amplitude(&[1]), Complex64::new(0.0, 0.0));
    close(controlled.amplitude(&[3]), Complex64::new(0.0, r));
    assert_eq!(controlled.amplitudes.len(), 2);
}

/// Every value outside a code set is `bad_gate`, before the angle (`bad_angle`), the handle
/// (`dangling_register`) and the position (`bad_range`) are looked at, and before anything changes.
#[test]
fn a_kind_outside_its_codes_is_bad_gate_before_the_other_checks() {
    let module = common::load(
        "memory 256
func @main(i64 %k, i64 %a, i64 %h, i64 %i, i1 %rotate) {
entry:
    %q = qalloc 2
    h %q
    br %rotate, rotation, gate
gate:
    gate(%k) %h[%i] ctrl %q
    ret
rotation:
    rotate(%k, %a) %h[%i] ctrl %q
    ret
}",
    );
    let nan = f64::NAN.to_bits();
    let fault = |k: u64, a: u64, h: u64, i: u64, rotate: u64| {
        let mut executor = executor(&module, "main", &[k, a, h, i, rotate], bounded());
        for _ in 0..3 {
            executor.step();
        }
        let before = executor.snapshot();
        let outcome = executor.run();
        if let Outcome::Faulted(_) = outcome {
            assert_eq!(executor.snapshot().quantum, before.quantum, "{k} {rotate}: nothing changes");
        }
        outcome
    };
    for k in [0, 9, 12, 13, 1 << 32, u64::MAX] {
        assert_eq!(fault(k, nan, 7, 9, 0), Outcome::Faulted(Fault::BadGate), "gate code {k}");
    }
    for k in [0, 1, 2, 8, 13, u64::MAX] {
        assert_eq!(fault(k, nan, 7, 9, 1), Outcome::Faulted(Fault::BadGate), "rotation code {k}");
    }
    assert_eq!(fault(11, nan, 7, 9, 1), Outcome::Faulted(Fault::BadAngle), "then the angle");
    assert_eq!(fault(11, 0, 7, 9, 1), Outcome::Faulted(Fault::DanglingRegister), "then the handle");
    assert_eq!(fault(4, 0, 7, 9, 0), Outcome::Faulted(Fault::DanglingRegister), "the handle after the kind");
    assert_eq!(fault(4, 0, 1, 9, 0), Outcome::Faulted(Fault::BadRange), "then the position");
    assert_eq!(fault(4, 0, 1, 0, 0), Outcome::Faulted(Fault::QubitAlias), "then the alias");
    assert_eq!(fault(4, 0, 1, 1, 0), Outcome::Returned(None));
    assert_eq!(Fault::BadGate.code(), 23);
}

#[test]
fn forward_then_reverse_restores_each_earlier_state() {
    let module = common::load(&program("gate(%k)"));
    let rotations = common::load(&program("rotate(%k, %a)"));
    let cases = GATE_CODES
        .iter()
        .map(|&(code, _)| (&module, code))
        .chain(ROTATION_CODES.iter().map(|&(code, _)| (&rotations, code)));
    for (module, code) in cases {
        let args = [code, 0.7f64.to_bits()];
        let mut executor = executor(module, "main", &args, Options { reversible: true, ..bounded() });
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

/// Inside an applied function both forms are `quantum_in_region` before the kind is looked at.
#[test]
fn a_run_time_kind_inside_an_applied_function_faults_quantum_in_region() {
    for body in ["gate(%a) %a[0]", "rotate(%a, %a) %a[0]"] {
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
        let (outcome, _) = run(&module, "main", &[0], common::allowing_region_faults());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))), "{body}");
    }
}

/// Every operand of the coded forms, and the handle of `qfree`, may be a constant, read as a slot's value
/// is: a constant handle (`1[0]`), a constant position, a constant angle's bits. The same gate as the
/// literal form, and one instruction: inside an applied function with one step of budget left the call
/// gives `quantum_in_region`, never `budget`.
#[test]
fn constant_operands_are_operands_of_the_one_instruction() {
    let half = 0.5f64.to_bits();
    for (constant, literal) in [
        ("gate(4) 1[0] ctrl %q[1]".to_owned(), "h %q[0] ctrl %q[1]".to_owned()),
        (format!("rotate(9, {half}) 1[1]"), "rx(0.5) %q[1]".to_owned()),
        (format!("rotate(12, {half}) %h[%i]"), "p(0.5) %q[1]".to_owned()),
        ("x 1[1] ctrl 1[0]".to_owned(), "x %q[1] ctrl %q[0]".to_owned()),
    ] {
        let text = |gate: &str| {
            format!(
                "memory 64
func @main(i64 %h, i64 %i) {{
entry:
    %q = qalloc 2
    h %q
    ry(0.3) %q[1]
    {gate}
    ret
}}"
            )
        };
        let expected = state(&text(&literal), &[1, 1]);
        assert_eq!(state(&text(&constant), &[1, 1]).distance(&expected), Some(0.0), "{constant}");
    }
    let freed = common::load(
        "memory 64
func @main() {
entry:
    %q = qalloc 1
    qfree 1
    x %q
    ret
}",
    );
    assert_eq!(run(&freed, "main", &[], bounded()).0, Outcome::Faulted(Fault::DanglingRegister));
    for call in [
        "gate(4) 1[0]".to_owned(),
        format!("rotate(9, {half}) 1[0] ctrl 2[3]"),
        "qfree 1".to_owned(),
        "%h = qalloc i64 3".to_owned(),
        "x 1[0]".to_owned(),
    ] {
        let module = common::load(&format!(
            "memory 64
func @f(i64 %a) -> i1 {{
entry:
    {call}
    ret 0
}}
func @main() -> i5 {{
entry:
    %o = qalloc 1
    %f = qalloc 5
    apply @f(i64 0) -> %o, %f budget 1
    %m = measure %f
    ret %m
}}"
        ));
        let (outcome, _) = run(&module, "main", &[], common::allowing_region_faults());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))), "{call}");
    }
}
