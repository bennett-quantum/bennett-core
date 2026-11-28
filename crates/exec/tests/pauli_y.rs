//! The fixed gate `y`, Pauli Y `[[0, -i], [i, 0]]` (`docs/SYNTAX.md`), applied where every control is 1.
//! Checked against amplitudes worked out by hand: `y|0⟩ = i|1⟩` and `y|1⟩ = -i|0⟩`, so alone it is `x` up to
//! phases, and under a control the factor `i` is a relative phase that `x` does not give; `y` twice is the
//! identity; forward then reverse restores every earlier state; inside an applied function it is
//! `quantum_in_region`.

mod common;

use std::f64::consts::FRAC_1_SQRT_2;

use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use common::{TOLERANCE, assert_reverses, bounded, executor, run};
use num_complex::Complex64;

fn close(actual: Complex64, expected: Complex64, context: &str) {
    assert!((actual - expected).norm() <= TOLERANCE, "{context}: {actual} against {expected}");
}

fn state(body: &str) -> bennett_exec::QuantumSnapshot {
    let module = common::load(&format!(
        "memory 256
func @main() {{
entry:
    %q = qalloc 2
{body}
    ret
}}"
    ));
    let (outcome, executor) = run(&module, "main", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(None), "{body}");
    executor.snapshot().quantum
}

/// Register values are `q[0] + 2 q[1]`.
#[test]
fn y_maps_zero_to_i_one_and_one_to_minus_i_zero() {
    let (zero, i) = (Complex64::new(0.0, 0.0), Complex64::i());
    let on_zero = state("    y %q[1]");
    close(on_zero.amplitude(&[2]), i, "y|0>");
    close(on_zero.amplitude(&[0]), zero, "y|0>");
    let on_one = state("    x %q[1]\n    y %q[1]");
    close(on_one.amplitude(&[0]), -i, "y|1>");
    close(on_one.amplitude(&[2]), zero, "y|1>");
    // After h: y (|0⟩ + |1⟩)/√2 = (-i|0⟩ + i|1⟩)/√2.
    let r = Complex64::new(FRAC_1_SQRT_2, 0.0);
    let after_h = state("    h %q[1]\n    y %q[1]");
    close(after_h.amplitude(&[0]), -i * r, "y h|0>");
    close(after_h.amplitude(&[2]), i * r, "y h|0>");
}

/// The control `%q[0]` in `(|0⟩ + |1⟩)/√2`, the target `%q[1]` at 0. A controlled `y` leaves
/// `(|0⟩ + i|3⟩)/√2`, a controlled `x` `(|0⟩ + |3⟩)/√2`; a second controlled `y` undoes the first.
#[test]
fn a_controlled_y_gives_the_relative_phase_i_that_a_controlled_x_does_not() {
    let r = Complex64::new(FRAC_1_SQRT_2, 0.0);
    let y = state("    h %q[0]\n    y %q[1] ctrl %q[0]");
    close(y.amplitude(&[0]), r, "ctrl y");
    close(y.amplitude(&[3]), Complex64::i() * r, "ctrl y");
    close(y.amplitude(&[1]), Complex64::new(0.0, 0.0), "ctrl y");
    let x = state("    h %q[0]\n    x %q[1] ctrl %q[0]");
    close(x.amplitude(&[3]), r, "ctrl x");
    let twice = state("    h %q[0]\n    y %q[1] ctrl %q[0]\n    y %q[1] ctrl %q[0]");
    close(twice.amplitude(&[0]), r, "ctrl y twice");
    close(twice.amplitude(&[1]), r, "ctrl y twice");
    assert_eq!(twice.amplitudes.len(), 2, "ctrl y twice");
}

const PROGRAM: &str = "memory 64
func @main() {
entry:
    %q = qalloc 3
    h %q
    h %q[1]
    h [8:5]
    y %q[2]
    y %q[1] ctrl %q
    y %q[0] ctrl %q[2], [8:5]
    y [8:2] ctrl %q[1]
    y [8:5]
    ret
}";

#[test]
fn forward_then_reverse_restores_each_earlier_state() {
    let module = common::load(PROGRAM);
    assert_eq!(assert_reverses(&module, "main", &[], bounded()), Outcome::Returned(None));
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let mut before = vec![executor.snapshot()];
    while executor.step().is_none() {
        before.push(executor.snapshot());
    }
    assert!(executor.snapshot().quantum.amplitudes.len() > 2, "the gates act");
    while let Some(expected) = before.pop() {
        assert!(executor.step_back().expect("reversible"));
        common::assert_same_state(&executor.snapshot(), &expected, &format!("step {}", before.len()));
    }
}

#[test]
fn y_inside_an_applied_function_faults_quantum_in_region() {
    let module = common::load(
        "memory 64
func @f(i1 %a) -> i1 {
entry:
    y [0:0]
    ret 0
}
func @main() -> i5 {
entry:
    %a = qalloc 1
    %o = qalloc 1
    %f = qalloc 5
    apply @f(%a) -> %o, %f budget 10
    %m = measure %f
    ret %m
}",
    );
    let (outcome, _) = run(&module, "main", &[], common::allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))));
}
