//! The simulator's strict mode against the machine's meaning (`docs/DESIGN.md`, "Strict mode"; the
//! maintainer's decisions of the afternoon of 2026-11-16). Strict mode, the default, adds diagnostics no
//! device can make: a `qfree` of a register that is not zero faults `dirty_free`, and a fault on some
//! branches or worlds stops the run. Under `Options::machine_meaning` the qubits of a dirty `qfree` are left
//! over (kept, with no live handle) and a fault on some branches is the program's to measure. Every
//! expected state below is worked by hand.

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

/// A rewind un-runs a `qfree` that left qubits over (taking the register back), then the gate and the
/// allocation, which finds the register zero again: the machine is at the mark, nothing left over.
/// Reversal runs the region again, leaving the qubits over again, and returns to the entry.
#[test]
fn a_rewind_takes_back_what_a_dirty_free_left_over() {
    let module = load(
        "memory 64
func @main() {
entry:
    %h = mark
    %t = qalloc 1
    h %t
    qfree %t
    rewind %h
    ret
}",
    );
    assert_eq!(common::run(&module, "main", &[], strict()).0, Outcome::Faulted(Fault::DirtyFree));
    let mut e = executor(&module, "main", &[], machine());
    let entry = e.snapshot();
    for _ in 0..4 {
        assert_eq!(e.step(), None);
    }
    assert_eq!(e.stats().leftover_qubits, 1);
    assert_eq!(e.step(), None);
    let stats = e.stats();
    assert_eq!((stats.leftover_qubits, stats.live_qubits, stats.rewound_steps), (0, 0, 3));
    let amplitudes = e.snapshot().quantum.amplitudes;
    assert_eq!(amplitudes.len(), 1);
    assert!(amplitudes[0].0.is_empty() && (amplitudes[0].1 - C::new(1.0, 0.0)).norm() < 1e-12);
    assert_eq!(e.run(), Outcome::Returned(None));
    e.reverse().expect("reverses");
    assert_same_state(&e.snapshot(), &entry, "back to the entry");
}

/// A world that faults while another goes on: `@f` divides by the qubit read as an integer. Strict mode
/// stops the run; on the machine the faulting world ends and the host observes one world at the end,
/// either the fault or the result, by weight (a half each). The machine's meaning gives what
/// `allow_region_faults` gives, seed by seed.
#[test]
fn a_fault_in_some_worlds_stops_strict_mode_and_is_the_programs_on_the_machine() {
    let module = load(
        "memory 64
func @f(i1 %b) -> i8 {
entry:
    %d = zext i1 %b to i8
    %r = udiv i8 1, %d
    ret %r
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %v = call @f(%q)
    ret %v
}",
    );
    assert!(matches!(
        common::run(&module, "main", &[], strict()).0,
        Outcome::Stopped(RunError::WorldFault { fault: Fault::DivideByZero, .. })
    ));
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..16 {
        let (outcome, e) = common::run(&module, "main", &[], Options { seed, ..machine() });
        let allowed = Options { seed, allow_region_faults: true, reversible: true, ..bounded() };
        let (expected, a) = common::run(&module, "main", &[], allowed);
        assert_eq!(outcome, expected, "seed {seed}");
        assert_same_state(&e.snapshot(), &a.snapshot(), &format!("seed {seed}"));
        assert!(matches!(outcome, Outcome::Returned(Some(1)) | Outcome::Faulted(Fault::DivideByZero)));
        seen.insert(format!("{outcome:?}"));
    }
    assert_eq!(seen.len(), 2);
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

/// A rewind whose keep changes what the region read, through a gate, so that its un-run meets a temporary
/// that is not zero. `%q` = |+>, `%t` = `%q` copied, keep 1 into `%q`: (|00> + |11>)/sqrt 2 becomes
/// (|10> + |01>)/sqrt 2 (columns `%q`, `%t`); un-running `x %t ctrl %q` gives (|11> + |01>)/sqrt 2, `%t` = 1
/// in both. Strict mode faults `bad_rewind`, changing nothing; the machine runs the un-run regardless and
/// leaves `%t` over holding 1, `%q` = |+> again. Undoing the rewind (reversal, or a rewind of an outer mark)
/// takes `%t` back where its `qalloc` runs again.
#[test]
fn an_un_run_that_meets_a_nonzero_allocation_runs_regardless_on_the_machine() {
    let text = "memory 64
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %outer = mark
    %h = mark
    %t = qalloc 1
    x %t ctrl %q
    rewind %h keep (i1 1 -> %q)
    OUTER
    ret 5
}";
    let module = load(&text.replace("OUTER", ""));
    let mut e = executor(&module, "main", &[], strict());
    for _ in 0..6 {
        assert_eq!(e.step(), None);
    }
    let before = e.snapshot();
    assert_eq!(e.step(), Some(Outcome::Faulted(Fault::BadRewind)));
    let mut after = e.snapshot();
    after.classical.outcome = None;
    assert_same_state(&after, &before, "strict: nothing changed");

    let mut e = executor(&module, "main", &[], machine());
    let at_entry = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(Some(5)));
    let stats = e.stats();
    assert_eq!((stats.leftover_qubits, stats.live_qubits, stats.rewound_steps), (1, 2, 2));
    let quantum = e.snapshot().quantum;
    assert_eq!(
        quantum.registers,
        [RegisterState { generation: 1, width: Width::new(1) }, RegisterState { generation: 2, width: Width::new(1) }]
    );
    let half = std::f64::consts::FRAC_1_SQRT_2;
    let expected = [(vec![0, 1], half), (vec![1, 1], half)];
    assert_eq!(quantum.amplitudes.len(), 2);
    for ((config, amplitude), (want, re)) in quantum.amplitudes.iter().zip(&expected) {
        assert_eq!(config, want);
        assert!((amplitude - C::new(*re, 0.0)).norm() < 1e-12);
    }
    // Reversal takes the leftover back where the rewind's region runs again: back to the entry.
    e.reverse().expect("reverses through the rewind that left a register over");
    assert_same_state(&e.snapshot(), &at_entry, "back to the entry");

    // An outer rewind undoes the inner one, taking `%t` back (holding 1): its region runs again, `t ^= q` gives
    // (|01> + |10>)/sqrt 2 and the keep XORed out (|11> + |00>)/sqrt 2, the state before the inner rewind; then
    // the outer un-run gives `t ^= q` = 0 and frees `%t` clean: `%q` = |+>, nothing left over. Until the design seam's V1 was fixed the leftover was a
    // barrier and this rewind faulted `bad_rewind`.
    let module = load(&text.replace("OUTER", "rewind %outer"));
    let mut e = executor(&module, "main", &[], machine());
    assert_eq!(e.run(), Outcome::Returned(Some(5)));
    assert_eq!(e.stats().leftover_qubits, 0);
    let quantum = e.snapshot().quantum;
    let expected = [(vec![0], half), (vec![1], half)];
    assert_eq!(quantum.amplitudes.len(), 2);
    for ((config, amplitude), (want, re)) in quantum.amplitudes.iter().zip(&expected) {
        assert_eq!(config, want);
        assert!((amplitude - C::new(*re, 0.0)).norm() < 1e-12);
    }
}

/// With the keep 0 nothing changes what the region read: the un-run is faithful in both modes, nothing is
/// left over, and the run reverses to its entry.
#[test]
fn a_faithful_un_run_is_the_same_in_both_modes() {
    let module = load(
        "memory 64
func @main() {
entry:
    %q = qalloc 1
    h %q
    %h = mark
    %t = qalloc 1
    x %t ctrl %q
    rewind %h keep (i1 0 -> %q)
    ret
}",
    );
    let (mut s, mut m) = (executor(&module, "main", &[], strict()), executor(&module, "main", &[], machine()));
    let entry = s.snapshot();
    assert_eq!((s.run(), m.run()), (Outcome::Returned(None), Outcome::Returned(None)));
    assert_same_state(&m.snapshot(), &s.snapshot(), "same state");
    assert_eq!(m.stats().leftover_qubits, 0);
    for e in [&mut s, &mut m] {
        e.reverse().expect("reverses");
        assert_same_state(&e.snapshot(), &entry, "back to the entry");
    }
}
