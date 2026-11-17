//! Requirements from HANDOFF, "Design audition of 2026-11-16" and
//! "Decisions the maintainer took on 2026-11-16, in the afternoon".
//! Expectations are hand calculations or independent dense contractions.

mod common;

use bennett_exec::{Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{bounded, executor, load};
use num_complex::Complex64 as C;

fn machine() -> Options {
    Options { machine_meaning: true, reversible: true, ..bounded() }
}

/// The arms do one and three instructions after their branch. Compute/copy/uncompute
/// copies q to out; CX clears that copy. Starting in |->, the last H gives |1>
/// with certainty. Orthogonal parking records would instead give P(1)=1/2.
#[test]
fn unequal_arms_wait_at_the_rewind_and_preserve_interference() {
    for budget in [7, 8, 17, 40] {
        let text = format!(
            "memory 512
func @work(i1 %v) -> i1 {{
entry:
    br %v, long, short
short:
    ret %v
long:
    %a = add i1 %v, 0
    %b = xor i1 %a, 0
    ret %b
}}
func @main() -> i1 {{
entry:
    %q = qalloc 1
    h %q
    z %q
    %out = qalloc 1
    %m = mark budget {budget}
    %v = call @work(%q)
    rewind %m keep (%v -> %out)
    x %out ctrl %q
    h %q
    %bit = measure %q
    ret %bit
}}"
        );
        let module = load(&text);
        for seed in 0..8 {
            let mut e = executor(&module, "main", &[], Options { seed, ..machine() });
            assert_eq!(e.run(), Outcome::Returned(Some(1)), "B={budget}, seed={seed}");
            assert_eq!(e.stats().clock, budget + 9);
            assert_eq!(e.stats().observations, 0, "worlds must rejoin before measurement");
            assert!((e.snapshot().quantum.amplitude(&[1, 0]) - C::new(1.0, 0.0)).norm() < 1e-12);
        }
        // Five prologue steps; call+branch+1/3+rewind under the budget;
        // then CX, H, measure, return. Two basis preparations keep this count.
        for prepare in ["z %q\n    z %q", "x %q\n    z %q"] {
            let basis = load(&text.replace("h %q\n    z %q", prepare));
            let (_, e) = common::run(&basis, "main", &[], machine());
            assert_eq!(e.stats().clock, budget + 9);
        }
    }
}

/// Missing budget after a read is a classical bad_rewind in either mode.
/// Worlds meeting a state already left must still stop as Unaligned.
#[test]
fn classical_missing_budget_and_unaligned_still_stop() {
    let module = load(
        "memory 512
func @id(i1 %v) -> i1 {
entry:
    ret %v
}
func @main() {
entry:
    %q = qalloc 1
    h %q
    %m = mark
    %v = call @id(%q)
    rewind %m
    ret
}
func @unaligned() {
entry:
    %q = qalloc 1
    h %q
    %m = mark budget 20
    %v = call @id(%q)
    br %v, one, zero
zero:
    rewind %m
    ret
one:
    rewind %m
    br zero
}",
    );
    let strict = common::run(&module, "main", &[], bounded()).0;
    assert!(matches!(strict, Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })), "{strict:?}");
    assert_eq!(common::run(&module, "main", &[], machine()).0, Outcome::Faulted(Fault::BadRewind));
    for options in [bounded(), machine()] {
        assert_eq!(common::run(&module, "unaligned", &[], options).0, Outcome::Stopped(RunError::Unaligned));
    }
}

/// Dense state: Ry(a), CX to the released wire, S, then Ry(b) on the retained
/// wire. Trace by summing |amplitude|^2 separately for each released value.
/// Release must retain this purification and measurements follow its marginal.
#[test]
fn dirty_release_keeps_a_purification_with_the_dense_partial_trace() {
    let (a, b) = (0.91f64, -0.63f64);
    let (ca, sa) = ((a / 2.0).cos(), (a / 2.0).sin());
    let (cb, sb) = ((b / 2.0).cos(), (b / 2.0).sin());
    let dense = [C::new(ca * cb, 0.0), C::new(ca * sb, 0.0), C::new(0.0, -sa * sb), C::new(0.0, sa * cb)];
    let p1 = dense[1].norm_sqr() + dense[3].norm_sqr();
    let module = load(&format!(
        "memory 256
func @main() -> i1 {{
entry:
    %keep = qalloc 1
    %drop = qalloc 1
    ry({a}) %keep
    x %drop ctrl %keep
    s %keep
    qfree %drop
    ry({b}) %keep
    %m = measure %keep
    ret %m
}}"
    ));
    assert_eq!(common::run(&module, "main", &[], bounded()).0, Outcome::Faulted(Fault::DirtyFree));
    let mut e = executor(&module, "main", &[], machine());
    for _ in 0..7 {
        assert_eq!(e.step(), None);
    }
    assert_eq!(e.stats().leftover_qubits, 1);
    let state = e.snapshot().quantum;
    for drop in 0..2 {
        for keep in 0..2 {
            assert!((state.amplitude(&[keep, drop]) - dense[(keep + 2 * drop) as usize]).norm() < 1e-12);
        }
    }
    let mut ones = 0;
    const SAMPLES: u64 = 1024;
    for seed in 0..SAMPLES {
        match common::run(&module, "main", &[], Options { seed, ..machine() }).0 {
            Outcome::Returned(Some(1)) => ones += 1,
            Outcome::Returned(Some(0)) => {}
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
    // Six binomial standard deviations; exact amplitudes above are the primary oracle.
    let tolerance = 6.0 * (p1 * (1.0 - p1) / SAMPLES as f64).sqrt();
    assert!((ones as f64 / SAMPLES as f64 - p1).abs() < tolerance, "{ones}/{SAMPLES}, dense P(1)={p1}");
}

/// Quantum allocation count is bounded by a *mark*, since the call form forbids
/// quantum instructions. The n=0 arm waits to measure, then returns 7 within five steps from the mark;
/// n=1 allocates forever and must fault budget. These orthogonal worlds have
/// hand-calculated weights 1/2, so machine mode may observe either, never success
/// for the allocating arm; strict mode stops at that arm's budget fault.
#[test]
fn a_quantum_dependent_allocation_count_is_bounded_on_every_branch() {
    for budget in [5, 8, 12] {
        let module = load(&format!(
            "memory 4096
func @f(i1 %n) -> i8 {{
entry:
    br %n, loop, done
loop:
    %q = qalloc 1
    br loop
done:
    ret 7
}}
func @main() -> i8 {{
entry:
    %q = qalloc 1
    h %q
    %m = mark budget {budget}
    %r = call @f(%q)
    %observed = measure %q
    ret %r
}}"
        ));
        let strict = common::run(&module, "main", &[], bounded()).0;
        assert!(matches!(strict, Outcome::Stopped(RunError::WorldFault { fault: Fault::Budget, .. })), "{strict:?}");
        let mut returned = 0;
        let mut faulted = 0;
        for seed in 0..64 {
            match common::run(&module, "main", &[], Options { seed, ..machine() }).0 {
                Outcome::Returned(Some(7)) => returned += 1,
                Outcome::Faulted(Fault::Budget) => faulted += 1,
                other => panic!("B={budget}, {other:?}"),
            }
        }
        assert!(returned > 0 && faulted > 0);
        // Six standard deviations for the hand-computed Bernoulli(1/2) weight.
        assert!((returned as f64 / 64.0 - 0.5).abs() < 6.0 * (0.25f64 / 64.0).sqrt());
    }
}

/// Was a debt until 2026-11-17 (HANDOFF, "Known weak spots", running regardless, cause 1).
/// Keeping the value into the register just read changes that read on q=1.
/// The afternoon ruling requires the inverse to run regardless outside strict
/// mode. Its resulting state is unspecified, so only continuation is asserted;
/// strict mode must still diagnose the mismatch.
#[test]
fn a_changed_read_is_a_strict_diagnostic_and_machine_execution_continues() {
    let module = load(
        "memory 512
func @id(i1 %v) -> i1 {
entry:
    ret %v
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %m = mark budget 20
    %v = call @id(%q)
    rewind %m keep (%v -> %q)
    ret 7
}",
    );
    let strict = common::run(&module, "main", &[], bounded()).0;
    assert!(matches!(strict, Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })), "{strict:?}");
    assert_eq!(common::run(&module, "main", &[], machine()).0, Outcome::Returned(Some(7)));
}
