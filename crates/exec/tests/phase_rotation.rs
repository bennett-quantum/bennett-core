//! The phase rotation `p(θ)` (`docs/DESIGN.md`, "The model"): `diag(1, e^{iθ})` on the target, applied where
//! every control is 1. Checked against amplitudes worked out by hand: alone it is `rz(θ)` times the global
//! phase `e^{iθ/2}`; under a control the two differ by a relative phase that `h` turns into a measurable
//! difference; the run-time angle `p(%a)` gives the literal's amplitudes exactly; forward then reverse
//! restores every earlier state; an angle that is not finite is `bad_angle`.

mod common;

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use common::{TOLERANCE, assert_reverses, bounded, executor, run};
use num_complex::Complex64;

/// Angles of both signs, a half turn, tiny, larger than 2 pi, and far larger, where a reduction of the angle
/// modulo a rounded 2 pi would give a wrong phase.
const ANGLES: [f64; 7] = [0.7, -1.89, PI, 1e-18, 7.5, -PI / 2.0, 1e18];

fn close(actual: Complex64, expected: Complex64, context: &str) {
    assert!((actual - expected).norm() <= TOLERANCE, "{context}: {actual} against {expected}");
}

/// After `h`, `p(θ)` leaves `(|0⟩ + e^{iθ}|1⟩)/√2`, and `rz(θ)` leaves `(e^{-iθ/2}|0⟩ + e^{iθ/2}|1⟩)/√2`:
/// the same state times the global phase `e^{iθ/2}`.
#[test]
fn the_phase_rotation_alone_is_rz_up_to_a_global_phase() {
    let state = |gate: &str, angle: f64| {
        let module = common::load(&format!(
            "memory 256
func @main() {{
entry:
    %q = qalloc 1
    h %q
    {gate}({angle:?}) %q
    ret
}}"
        ));
        let (outcome, executor) = run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(None));
        executor.snapshot().quantum
    };
    for angle in ANGLES {
        let (p, rz) = (state("p", angle), state("rz", angle));
        let context = format!("p({angle})");
        let r = Complex64::new(FRAC_1_SQRT_2, 0.0);
        close(p.amplitude(&[0]), r, &context);
        close(p.amplitude(&[1]), r * Complex64::from_polar(1.0, angle), &context);
        let global = Complex64::from_polar(1.0, angle / 2.0);
        for value in [0, 1] {
            close(p.amplitude(&[value]), global * rz.amplitude(&[value]), &format!("{context}: against rz, {value}"));
        }
    }
}

/// The control `%q[0]` in `(|0⟩ + |1⟩)/√2`, the target `%q[1]` at 1. A controlled `p(θ)` gives the control's
/// 1 the phase `e^{iθ}`, a controlled `rz(θ)` only `e^{iθ/2}` (`rz` on 1 is `e^{iθ/2}`); then `h` on the
/// control leaves the amplitudes `(1 ± e^{iφ})/2` of the register values 2 and 3, with φ = θ or θ/2.
/// With `measured`, the program measures the register and returns it; without, it ends before the measurement.
fn controlled(gate: &str, angle: &str, measured: bool) -> String {
    let end = if measured { "%m = measure %q\n    ret %m" } else { "ret 0" };
    format!(
        "memory 256
func @main(i64 %a) -> i2 {{
entry:
    %q = qalloc 2
    h %q[0]
    x %q[1]
    {gate}({angle}) %q[1] ctrl %q[0]
    h %q[0]
    {end}
}}"
    )
}

#[test]
fn controlled_phase_and_controlled_rz_differ_by_a_relative_phase() {
    for angle in ANGLES {
        for (gate, phase) in [("p", angle), ("rz", angle / 2.0)] {
            let module = common::load(&controlled(gate, &format!("{angle:?}"), false));
            let (outcome, executor) = run(&module, "main", &[0], bounded());
            assert_eq!(outcome, Outcome::Returned(Some(0)));
            let state = executor.snapshot().quantum;
            let turn = Complex64::from_polar(1.0, phase);
            let half = Complex64::new(0.5, 0.0);
            let context = format!("{gate}({angle}) ctrl");
            close(state.amplitude(&[2]), half * (1.0 + turn), &context);
            close(state.amplitude(&[3]), half * (1.0 - turn), &context);
            close(state.amplitude(&[0]), Complex64::new(0.0, 0.0), &context);
        }
    }
    // A half turn: the controlled phase rotation is a controlled Z, so the control reads 1 at every seed; the
    // controlled rz gives it the phase i only, and both outcomes occur.
    let read = |gate: &str| {
        let module = common::load(&controlled(gate, &format!("{PI:?}"), true));
        (0..32)
            .map(|seed| match run(&module, "main", &[0], Options { seed, ..bounded() }).0 {
                Outcome::Returned(Some(value)) => value,
                other => panic!("{gate}, seed {seed}: {other:?}"),
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(read("p"), [3].into());
    assert_eq!(read("rz"), [2, 3].into());
}

/// The phase rotation on a register and on a memory byte, alone and under register and memory controls,
/// with the angle as a literal or as the program's `i64` argument.
fn program(angle: &str) -> String {
    format!(
        "memory 64
func @main(i64 %a) {{
entry:
    %q = qalloc 3
    h %q
    h %q[1]
    h %q[2]
    x [8:2]
    h [8:5]
    p{angle} %q[2]
    p{angle} %q[1] ctrl %q
    p{angle} %q[0] ctrl %q[2], [8:5]
    p{angle} [8:2] ctrl %q[1]
    p{angle} [8:5]
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
fn forward_then_reverse_restores_each_earlier_state() {
    for text in [program("(%a)"), program("(0.7)")] {
        let module = common::load(&text);
        for angle in ANGLES {
            assert_eq!(assert_reverses(&module, "main", &[angle.to_bits()], bounded()), Outcome::Returned(None));
            let mut executor = executor(&module, "main", &[angle.to_bits()], Options { reversible: true, ..bounded() });
            let mut before = vec![executor.snapshot()];
            while executor.step().is_none() {
                before.push(executor.snapshot());
            }
            while let Some(expected) = before.pop() {
                assert!(executor.step_back().expect("reversible"), "{angle}");
                common::assert_same_state(&executor.snapshot(), &expected, &format!("{angle}: step {}", before.len()));
            }
        }
    }
}

/// An infinity or a NaN as the angle of `p(%a)` faults `bad_angle` with the state as it was before.
#[test]
fn an_angle_that_is_not_finite_faults_bad_angle_before_anything_changes() {
    let module = common::load(
        "memory 64
func @main(i64 %a) {
entry:
    %q = qalloc 2
    h %q
    p(%a) %q[1] ctrl %q
    ret
}",
    );
    for bits in [f64::INFINITY.to_bits(), f64::NEG_INFINITY.to_bits(), f64::NAN.to_bits(), 0x7ff0_0000_0000_0001] {
        let mut executor = executor(&module, "main", &[bits], bounded());
        for _ in 0..2 {
            executor.step();
        }
        let before = executor.snapshot();
        assert_eq!(executor.run(), Outcome::Faulted(Fault::BadAngle), "{bits:#x}");
        assert_eq!(executor.snapshot().quantum, before.quantum, "{bits:#x}");
    }
}

/// Inside an applied function the phase rotation is a quantum instruction like any gate, in both forms:
/// `quantum_in_region`, before its angle is looked at.
#[test]
fn a_phase_rotation_inside_an_applied_function_faults_quantum_in_region() {
    for rotation in ["p(%a) [0:0]", "p(0.5) [0:0]"] {
        let module = common::load(&format!(
            "memory 64
func @f(i64 %a) -> i1 {{
entry:
    {rotation}
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
        let (outcome, _) = run(&module, "main", &[f64::NAN.to_bits()], common::allowing_region_faults());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))), "{rotation}");
    }
}
