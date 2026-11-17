//! The simulator's strict mode against the machine's meaning (`docs/DESIGN.md`, "Strict mode"; the
//! maintainer's decisions of the afternoon of 2026-11-16). Strict mode, the default, adds diagnostics no
//! device can make: a `qfree` of a register that is not zero faults `dirty_free`, and an `apply` faulting for
//! some argument values stops the run. Under `Options::machine_meaning` the qubits of a dirty `qfree` are
//! left over (kept, with no live handle) and the fault code is the program's to measure. Every expected
//! state below is worked by hand.

mod common;

use bennett_exec::{Options, Outcome, RegisterState, RunError};
use bennett_ir::{Fault, Width};
use common::{assert_same_state, bounded, executor, load};
use num_complex::Complex64 as C;

fn machine() -> Options {
    Options { machine_meaning: true, reversible: true, ..bounded() }
}

fn strict() -> Options {
    Options { reversible: true, ..bounded() }
}

/// `%q` = (|00> + |11>)/sqrt 2 freed: strict mode faults `dirty_free` with the state as before the `qfree`;
/// the machine leaves both qubits over and the run goes on with a new register in a new column. Reversal
/// takes the register back and returns to the entry exactly.
#[test]
fn a_dirty_free_faults_in_strict_mode_and_leaves_its_qubits_over_on_the_machine() {
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
    let (outcome, e) = common::run(&module, "main", &[], strict());
    assert_eq!(outcome, Outcome::Faulted(Fault::DirtyFree));
    assert_eq!(e.stats().steps, 3);
    assert_eq!(e.stats().leftover_qubits, 0);

    let mut e = executor(&module, "main", &[], machine());
    let entry = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(Some(7)));
    let stats = e.stats();
    assert_eq!((stats.steps, stats.leftover_qubits, stats.live_qubits), (7, 2, 3));
    let quantum = e.snapshot().quantum;
    // Register 0: left over, its generation moved on so the handle `%q` (generation 1) is stale.
    assert_eq!(
        quantum.registers,
        [RegisterState { generation: 2, width: Width::new(2) }, RegisterState { generation: 1, width: Width::new(1) }]
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

/// A handle whose register was left over is stale: a gate on it faults `dangling_register`. A clean
/// `qfree` frees as before, so the next allocation takes the same id.
#[test]
fn a_left_over_register_has_no_live_handle_and_a_clean_free_still_frees() {
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
    qfree %q
    %r = qalloc 1
    x %r
    ret
}",
    );
    assert_eq!(common::run(&module, "stale", &[], machine()).0, Outcome::Faulted(Fault::DanglingRegister));
    assert_eq!(common::run(&module, "stale", &[], strict()).0, Outcome::Faulted(Fault::DirtyFree));
    for options in [machine(), strict()] {
        let (outcome, e) = common::run(&module, "clean", &[], options);
        assert_eq!(outcome, Outcome::Returned(None));
        assert_eq!(e.stats().leftover_qubits, 0);
        assert_eq!(e.snapshot().quantum.registers, [RegisterState { generation: 2, width: Width::new(1) }]);
    }
}

/// An `apply` whose callee faults (`examples/fault.bn`: `@triangle(n)` runs out of its budget for n > 3):
/// strict mode stops the run with a region fault; the machine goes on, the fault code in the program's
/// register, exactly as `allow_region_faults` gives.
#[test]
fn a_region_fault_is_a_fault_code_on_the_machine() {
    let module = load(include_str!("../../../examples/fault.bn"));
    for n in 0..16u64 {
        let (outcome, e) = common::run(&module, "main", &[n], machine());
        let allowed = Options { allow_region_faults: true, reversible: true, ..bounded() };
        let (expected, a) = common::run(&module, "main", &[n], allowed);
        assert_eq!(outcome, expected, "n = {n}");
        assert_same_state(&e.snapshot(), &a.snapshot(), &format!("n = {n}"));
        assert_eq!(e.stats().region_faults, u64::from(n > 3));
        let stopped =
            matches!(common::run(&module, "main", &[n], strict()).0, Outcome::Stopped(RunError::RegionFault { .. }));
        assert_eq!(stopped, n > 3, "n = {n}");
        // `out | fault << 8`: the budget's code where it faulted.
        let Outcome::Returned(Some(packed)) = outcome else { panic!("n = {n}: {outcome:?}") };
        assert_eq!(packed >> 8, if n > 3 { Fault::Budget.code().into() } else { 0 }, "n = {n}");
    }
}
