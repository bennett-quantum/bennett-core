//! Measuring a slice of a register (`docs/SYNTAX.md`, "Measuring a slice"): `measure i64 %r[%low, %bits]`
//! measures the qubits `low` to `low + bits - 1` in the computational basis, as the projectors onto each value
//! of the slice define it, and leaves the register allocated. Checked: the outcome and the state after it
//! against states worked out by hand, the qubits outside the slice untouched, the same outcome and state as
//! measuring a copy in a temporary register, a slice across storage columns, the faults `dangling_register`
//! and `bad_range` in that order and before anything changes, no handle taken, and `quantum_in_region`.

mod common;

use std::f64::consts::FRAC_1_SQRT_2;

use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use common::{TOLERANCE, bounded, executor, run};
use num_complex::Complex64;

fn close(actual: Complex64, expected: f64, context: &str) {
    assert!((actual - Complex64::new(expected, 0.0)).norm() <= TOLERANCE, "{context}: {actual} against {expected}");
}

/// Qubits 0 to 2 in (|000⟩ + |111⟩)/√2 and qubit 3 in (|0⟩ + |1⟩)/√2. The slice of qubits 1 and 2 reads 0 or
/// 3, each with probability 1/2, and leaves qubits 0 to 2 at 000 or 111; qubit 3, outside the slice and not
/// entangled with it, keeps its superposition.
#[test]
fn a_slice_measurement_projects_onto_the_value_read() {
    let module = common::load(
        "memory 256
func @main() -> i64 {
entry:
    %q = qalloc 5
    h %q
    x %q[1] ctrl %q
    x %q[2] ctrl %q
    h %q[3]
    %t = measure i64 %q[1, 2]
    ret %t
}",
    );
    let mut seen = [false; 2];
    for seed in 0..32 {
        let (outcome, executor) = run(&module, "main", &[], Options { seed, ..bounded() });
        let Outcome::Returned(Some(t)) = outcome else { panic!("seed {seed}: {outcome:?}") };
        assert!(t == 0 || t == 3, "seed {seed}: {t}");
        seen[usize::from(t == 3)] = true;
        let state = executor.snapshot().quantum;
        let low = if t == 3 { 0b111 } else { 0 };
        let context = format!("seed {seed}");
        close(state.amplitude(&[low]), FRAC_1_SQRT_2, &context);
        close(state.amplitude(&[low | 0b1000]), FRAC_1_SQRT_2, &context);
        assert_eq!(state.amplitudes.len(), 2, "{context}");
    }
    assert_eq!(seen, [true, true]);
}

/// The slice measured directly and the same qubits copied into a temporary register, measured there, the copy
/// undone and the temporary released: the same outcome at every seed and the same state of `%q` after it.
#[test]
fn a_slice_measurement_equals_measuring_a_copy_of_the_slice() {
    let prepare = "%q = qalloc 6
    ry(0.7) %q
    ry(1.9) %q[1] ctrl %q
    ry(2.6) %q[2]
    h %q[3]
    x %q[4] ctrl %q[2]";
    let direct = common::load(&format!(
        "memory 256
func @main() -> i64 {{
entry:
    {prepare}
    %t = measure i64 %q[1, 3]
    ret %t
}}"
    ));
    let copied = common::load(&format!(
        "memory 256
func @main() -> i64 {{
entry:
    {prepare}
    %c = qalloc 3
    x %c[0] ctrl %q[1]
    x %c[1] ctrl %q[2]
    x %c[2] ctrl %q[3]
    %m = measure %c
    x %c[0] ctrl %q[1]
    x %c[1] ctrl %q[2]
    x %c[2] ctrl %q[3]
    qfree %c
    %t = zext i3 %m to i64
    ret %t
}}"
    ));
    for seed in 0..64 {
        let options = Options { seed, ..bounded() };
        let (outcome, slice) = run(&direct, "main", &[], options);
        let (expected, copy) = run(&copied, "main", &[], options);
        assert_eq!(outcome, expected, "seed {seed}");
        // The temporary is released in zero, so both states are of `%q` alone, configuration by configuration.
        let (slice, copy) = (slice.snapshot().quantum, copy.snapshot().quantum);
        assert_eq!(slice.amplitudes.len(), copy.amplitudes.len(), "seed {seed}");
        for (config, amplitude) in &slice.amplitudes {
            assert!((copy.amplitude(config) - amplitude).norm() <= TOLERANCE, "seed {seed}: {config:?}");
        }
    }
}

/// A slice across the boundary of two storage columns of a 200-qubit register, by an `i64` handle and
/// run-time bounds: qubits 62 and 65 set, the slice from 60 of 10 qubits reads 2^2 + 2^5.
#[test]
fn a_slice_across_storage_columns_reads_the_qubits_in_order() {
    let module = common::load(
        "memory 256
func @main(i64 %low, i64 %bits) -> i64 {
entry:
    %h = qalloc i64 200
    x %h[62]
    x %h[65]
    x %h[199]
    %t = measure i64 %h[%low, %bits]
    %u = measure i64 %h[136, 64]
    %s = shl i64 %u, 32
    %r = xor i64 %t, %s
    ret %r
}",
    );
    let (outcome, _) = run(&module, "main", &[60, 10], bounded());
    // %u has qubit 199 at its bit 63: shifted left by 32 it leaves the word, so only %t remains.
    assert_eq!(outcome, Outcome::Returned(Some(0b100100)));
    let (outcome, _) = run(&module, "main", &[0, 64], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(1 << 62)));
}

/// The handle first (`dangling_register`), then the bounds (`bad_range`): `bits` from 1 to 64 and
/// `low + bits` within the width, computed without wrapping; each before anything changes.
#[test]
fn a_bad_handle_or_bound_faults_in_order_before_anything_changes() {
    let module = common::load(
        "memory 256
func @main(i1 %free, i64 %low, i64 %bits) -> i64 {
entry:
    %q = qalloc 70
    h %q[3]
    %h = add i64 0, 1
    %gone = qalloc i64 1
    br %free, freed, measured
freed:
    qfree %gone
    %u = measure i64 %gone[%low, %bits]
    ret %u
measured:
    %t = measure i64 %h[%low, %bits]
    ret %t
}",
    );
    let fault = |free: u64, low: u64, bits: u64| run(&module, "main", &[free, low, bits], bounded()).0;
    assert_eq!(fault(1, 0, 0), Outcome::Faulted(Fault::DanglingRegister), "the handle before the bounds");
    for (low, bits) in [(0, 0), (0, 65), (6, 65), (64, 7), (69, 2), (70, 1), (u64::MAX, 1), (1, u64::MAX)] {
        let mut executor = executor(&module, "main", &[0, low, bits], bounded());
        for _ in 0..5 {
            executor.step();
        }
        let before = executor.snapshot();
        assert_eq!(executor.run(), Outcome::Faulted(Fault::BadRange), "{low}, {bits}");
        assert_eq!(executor.snapshot().quantum, before.quantum, "{low}, {bits}");
    }
    for (low, bits) in [(0, 64), (6, 64), (69, 1), (3, 1)] {
        assert!(matches!(fault(0, low, bits), Outcome::Returned(Some(_))), "{low}, {bits}");
    }
}

/// The register stays allocated and no handle is taken: after the slice the register takes gates and is
/// released, and the next allocation is handle 2.
#[test]
fn a_slice_measurement_takes_no_handle_and_keeps_the_register() {
    let module = "memory 256
func @main() -> i64 {
entry:
    %h = qalloc i64 4
    x %h[2]
    %t = measure i64 %h[1, 3]
    x %h[2]
    %u = measure i64 %h[0, 4]
    qfree %h
    %k = qalloc i64 1
    %a = shl i64 %t, 8
    %b = shl i64 %u, 4
    %c = or i64 %a, %b
    %d = or i64 %c, %k
    ret %d
}";
    let (outcome, _) = run(&common::load(module), "main", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(0b010 << 8 | 2)));
}

/// Inside an applied function a slice measurement is a quantum instruction: `quantum_in_region`.
#[test]
fn a_slice_measurement_inside_an_applied_function_faults_quantum_in_region() {
    let module = common::load(
        "memory 256
func @f(i64 %a) -> i1 {
entry:
    %t = measure i64 %a[0, 1]
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
    let (outcome, _) = run(&module, "main", &[1], common::allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))));
}

/// Reversal stops before a slice measurement, as before any measurement.
#[test]
fn reversal_stops_at_a_slice_measurement() {
    let module = common::load(
        "memory 256
func @main() -> i64 {
entry:
    %q = qalloc 2
    h %q
    %t = measure i64 %q[0, 2]
    ret %t
}",
    );
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    assert!(matches!(executor.run(), Outcome::Returned(Some(_))));
    assert!(executor.reverse().is_err());
}
