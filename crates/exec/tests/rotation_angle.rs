//! Rotations by an angle computed at run time (`docs/DESIGN.md`, "The device program"): `rz(%a)` reads the
//! `i64` in `%a` as the bits of an IEEE 754 binary64. Checked: the same amplitudes as the literal angle, on
//! register and memory qubits, with and without controls; forward then reverse back to the entry; and the
//! fault `bad_angle` for an angle that is not finite, before anything changes.

mod common;

use bennett_exec::Outcome;
use bennett_ir::Fault;
use common::{assert_reverses, bounded, executor, run};

/// Angles of both signs, tiny, and larger than 2 pi.
const ANGLES: [f64; 5] = [0.7, -1.89, 1e-18, std::f64::consts::PI, 7.5];

/// A program that prepares a superposition on a register and a memory byte, then applies `rotation` (the
/// text of an angle in parentheses) about each axis, alone, with a register control and with a memory
/// control. Its parameter `%a` is the angle's bits.
fn program(rotation: &str) -> String {
    format!(
        "memory 64
func @main(i64 %a) {{
entry:
    %q = qalloc 3
    h %q
    h %q[1]
    x [8:2]
    h [8:5]
    rx{rotation} %q[2]
    ry{rotation} %q[2] ctrl %q
    rz{rotation} %q[1] ctrl %q, [8:5]
    ry{rotation} [8:2] ctrl %q[1]
    rz{rotation} [8:5]
    ret
}}"
    )
}

#[test]
fn a_run_time_angle_gives_the_same_amplitudes_as_the_literal() {
    let by_value = common::load(&program("(%a)"));
    for angle in ANGLES {
        let literal = common::load(&program(&format!("({angle:?})")));
        let (expected_outcome, expected) = run(&literal, "main", &[0], bounded());
        let (outcome, actual) = run(&by_value, "main", &[angle.to_bits()], bounded());
        assert_eq!((outcome, expected_outcome), (Outcome::Returned(None), Outcome::Returned(None)), "{angle}");
        let (actual, expected) = (actual.snapshot().quantum, expected.snapshot().quantum);
        // The same gates with the same `f64` angle: the same arithmetic, so the same amplitudes exactly.
        assert_eq!(actual.distance(&expected), Some(0.0), "{angle}");
        assert!(actual.amplitudes.len() > 4, "{angle}: the rotations act");
    }
}

#[test]
fn forward_then_reverse_restores_the_entry_state() {
    let module = common::load(&program("(%a)"));
    for angle in ANGLES {
        assert_eq!(assert_reverses(&module, "main", &[angle.to_bits()], bounded()), Outcome::Returned(None));
    }
}

/// Infinities and NaNs (quiet, signalling, with a payload, negative) fault `bad_angle` with the state as it
/// was before the rotation; zero of either sign, the smallest subnormal and the largest finite angle run.
#[test]
fn an_angle_that_is_not_finite_faults_bad_angle_before_anything_changes() {
    let module = common::load(
        "memory 64
func @main(i64 %a) {
entry:
    %q = qalloc 1
    h %q
    rz(%a) %q
    x %q
    ret
}",
    );
    let not_finite = [
        f64::INFINITY.to_bits(),
        f64::NEG_INFINITY.to_bits(),
        f64::NAN.to_bits(),
        0x7ff0_0000_0000_0001,
        0xfff8_0000_0000_0abc,
    ];
    for bits in not_finite {
        let mut executor = executor(&module, "main", &[bits], bounded());
        for _ in 0..2 {
            executor.step();
        }
        let before = executor.snapshot();
        assert_eq!(executor.run(), Outcome::Faulted(Fault::BadAngle), "{bits:#x}");
        let after = executor.snapshot();
        assert_eq!(after.quantum, before.quantum, "{bits:#x}");
        assert_eq!(after.classical.frames, before.classical.frames, "{bits:#x}: still at the rotation");
    }
    for angle in [0.0, -0.0, f64::from_bits(1), f64::MAX] {
        let (outcome, _) = run(&module, "main", &[angle.to_bits()], bounded());
        assert_eq!(outcome, Outcome::Returned(None), "{angle:e}");
    }
    assert_eq!(Fault::BadAngle.code(), 18);
    assert_eq!(Fault::BadAngle.name(), "bad_angle");
}

/// Inside an applied function a rotation is a quantum instruction like any gate: `quantum_in_region`, before
/// its angle is looked at (here the bits of negative infinity, passed as an ordinary argument).
#[test]
fn a_rotation_inside_an_applied_function_faults_quantum_in_region() {
    let module = common::load(
        "memory 64
func @f(i64 %a) -> i1 {
entry:
    rz(%a) [0:0]
    ret 0
}
func @main(i64 %a) -> i5 {
entry:
    %o = qalloc 1
    %f = qalloc 5
    apply @f(%a) -> %o, %f budget 10
    %m = measure %f
    ret %m
}",
    );
    let (outcome, _) = run(&module, "main", &[f64::NEG_INFINITY.to_bits()], common::allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))));
}
