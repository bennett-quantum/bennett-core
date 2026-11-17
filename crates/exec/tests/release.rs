//! `qfree` is the partial trace (the maintainer's decision of 2026-11-17; `docs/DESIGN.md`, "Releasing a
//! register"): the register is dropped unexamined, in strict mode and on the machine alike, and no release
//! is a fault. The simulator removes the column of a register that is zero in every configuration, and keeps
//! the column of any other, unnamed and never reused, so the stored state stays the exact purification; it
//! counts those releases and their weight in `Stats`, which no program can read. Every expected state below is
//! worked by hand.

mod common;

use bennett_exec::{Options, Outcome, RegisterState};
use bennett_ir::{Fault, Width};
use common::{assert_same_state, bounded, executor, load};
use num_complex::Complex64 as C;

fn machine() -> Options {
    Options { machine_meaning: true, reversible: true, ..bounded() }
}

fn strict() -> Options {
    Options { reversible: true, ..bounded() }
}

/// `%q` = (|00> + |11>)/sqrt 2 released: the run goes on with a new register in a new column, the pair kept
/// as an unnamed column; one release, of weight 1/2 (the configuration where `%q` is 3). Reversal takes the
/// register back and returns to the entry exactly.
#[test]
fn releasing_half_of_a_bell_pair_keeps_its_column_and_reports_weight_one_half() {
    let module = load(
        "memory 64
func @main() -> i8 {
entry:
    %q = qalloc 2
    h %q[0]
    x %q[1] ctrl %q[0]
    qfree %q
    %r = qalloc 1
    x %r
    ret 7
}",
    );
    for options in [strict(), machine()] {
        let mut e = executor(&module, "main", &[], options);
        let entry = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(Some(7)));
        let stats = e.stats();
        assert_eq!((stats.steps, stats.leftover_qubits, stats.live_qubits), (7, 2, 3));
        assert_eq!(stats.nonzero_releases, 1);
        assert!((stats.nonzero_release_weight - 0.5).abs() < 1e-12, "{}", stats.nonzero_release_weight);
        let quantum = e.snapshot().quantum;
        // Register 0: kept, its generation moved on so the handle `%q` (generation 1) is stale.
        assert_eq!(
            quantum.registers,
            [
                RegisterState { generation: 2, width: Width::new(2) },
                RegisterState { generation: 1, width: Width::new(1) }
            ]
        );
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let expected = [(vec![0, 1], C::new(half, 0.0)), (vec![3, 1], C::new(half, 0.0))];
        assert_eq!(quantum.amplitudes.len(), 2);
        for ((config, amplitude), (want, want_amplitude)) in quantum.amplitudes.iter().zip(&expected) {
            assert_eq!(config, want);
            assert!((amplitude - want_amplitude).norm() < 1e-12);
        }
        e.reverse().expect("reverses");
        assert_same_state(&e.snapshot(), &entry, "back to the entry");
        assert_eq!(e.stats().leftover_qubits, 0);
    }
}

/// A nonzero part of arbitrarily small weight is still a release like any other: `ry(2e-6)` leaves weight
/// sin^2(1e-6) (about 1e-12) on 1. The run returns, the column is kept, and the report gives that weight; a
/// device could not have told this release from a clean one, which is why no release is a fault.
#[test]
fn a_release_with_a_tiny_nonzero_part_is_reported_with_that_weight() {
    let module = load(
        "memory 64
func @main() -> i1 {
entry:
    %q = qalloc 1
    ry(2e-6) %q
    qfree %q
    ret 1
}",
    );
    let (outcome, e) = common::run(&module, "main", &[], strict());
    assert_eq!(outcome, Outcome::Returned(Some(1)));
    let stats = e.stats();
    assert_eq!((stats.nonzero_releases, stats.leftover_qubits), (1, 1));
    let expected = 1e-6f64.sin().powi(2);
    assert!((stats.nonzero_release_weight - expected).abs() < 1e-20, "{}", stats.nonzero_release_weight);
}

/// A handle whose register was released while not zero is stale: a gate on it faults `dangling_register`.
/// A release of a register that is zero in every configuration removes the column, so the next allocation
/// takes the same id, and nothing is reported.
#[test]
fn a_kept_register_has_no_live_handle_and_a_zero_register_is_removed() {
    let module = load(
        "memory 64
func @stale() {
entry:
    %q = qalloc 1
    h %q
    qfree %q
    x %q
    ret
}
func @clean() {
entry:
    %q = qalloc 1
    h %q
    h %q
    qfree %q
    %r = qalloc 1
    x %r
    ret
}",
    );
    for options in [machine(), strict()] {
        assert_eq!(common::run(&module, "stale", &[], options).0, Outcome::Faulted(Fault::DanglingRegister));
        let (outcome, e) = common::run(&module, "clean", &[], options);
        assert_eq!(outcome, Outcome::Returned(None));
        assert_eq!((e.stats().leftover_qubits, e.stats().nonzero_releases), (0, 0));
        assert_eq!(e.snapshot().quantum.registers, [RegisterState { generation: 2, width: Width::new(1) }]);
    }
}
