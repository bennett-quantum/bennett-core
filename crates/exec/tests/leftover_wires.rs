//! Running regardless at every check of a rewind's un-run, outside strict mode (the maintainer's decisions of
//! 2026-11-13 and of the afternoon of 2026-11-16: the inverse steps run regardless, leftover qubits are kept;
//! the state-dependent `bad_rewind` is a diagnostic of strict mode). Each check site, as a circuit sees it:
//! the inverse step runs and what it leaves in a wire stays, as a register left over (`docs/DESIGN.md`,
//! "Strict mode"; `crates/exec/src/leftover.rs`). Undoing the rewind (an outer rewind, or reversal) takes the
//! leftovers back, so no later step decides anything by looking at the whole state (the design seam's V1 and
//! V2).
//!
//! Each program runs on three preparations of its control qubit: |0>, |+> and |1>. Every expected state is
//! worked by hand, as the circuit of the program computes it, and written out with its columns.

mod common;

use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};
use num_complex::Complex64 as C;

const HALF: f64 = std::f64::consts::FRAC_1_SQRT_2;

fn machine() -> Options {
    Options { machine_meaning: true, reversible: true, ..bounded() }
}

fn strict() -> Options {
    Options { reversible: true, ..bounded() }
}

/// The three entries: `@zero`, `@plus` and `@one` allocate the control qubit, prepare it and call `@body`.
fn entries(body: &str) -> String {
    let entry = |name: &str, prepare: &str| {
        format!(
            "func @{name}() -> i8 {{\nentry:\n    %c = qalloc 1\n{prepare}    %v = call @body(%c)\n    ret %v\n}}\n"
        )
    };
    format!(
        "memory 256\n\nfunc @id(i1 %x) -> i1 {{\nentry:\n    ret %x\n}}\n\n{body}\n{}{}{}",
        entry("zero", ""),
        entry("plus", "    h %c\n"),
        entry("one", "    x %c\n")
    )
}

/// The final quantum state of a machine run of `entry`, checked against `expected` (configuration, real
/// amplitude) in configuration order, and the leftover qubits; then the run reverses to its entry.
fn check(text: &str, entry: &str, expected: &[(&[u64], f64)], leftover: u32) {
    let module = load(text);
    let mut e = executor(&module, entry, &[], machine());
    let start = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(Some(7)), "@{entry}");
    let quantum = e.snapshot().quantum;
    assert_eq!(quantum.amplitudes.len(), expected.len(), "@{entry}: {quantum:?}");
    for ((config, amplitude), (want, re)) in quantum.amplitudes.iter().zip(expected) {
        assert_eq!(config.as_slice(), *want, "@{entry}: {quantum:?}");
        assert!((amplitude - C::new(*re, 0.0)).norm() < 1e-12, "@{entry}: {quantum:?}");
    }
    assert_eq!(e.stats().leftover_qubits, leftover, "@{entry}");
    e.reverse().unwrap_or_else(|error| panic!("@{entry}: reverses: {error:?}"));
    assert_same_state(&e.snapshot(), &start, &format!("@{entry}: back to the entry"));
}

/// Strict mode keeps each of these a diagnostic: the rewind faults `bad_rewind` where the un-run is not
/// faithful (`plus`, `one`) and returns where it is (`zero`).
fn strict_unchanged(text: &str) {
    let module = load(text);
    assert_eq!(common::run(&module, "zero", &[], strict()).0, Outcome::Returned(Some(7)));
    for entry in ["plus", "one"] {
        assert_eq!(common::run(&module, entry, &[], strict()).0, Outcome::Faulted(Fault::BadRewind), "@{entry}");
    }
}

/// An allocation made since the mark that the keep leaves nonzero (the seam's V1). `%a ^= c & r` forward with
/// `r` = 0 does nothing; the keep sets `r`; un-running the gate gives `a = c`, so un-running `qalloc %a` finds
/// it nonzero where `c` = 1: `%a` is left over holding `c` (columns `c`, `r`, `a`). An outer rewind then undoes
/// the inner one, taking `%a` back where its `qalloc` runs again: `a ^= c & r` with `r` = 1 makes it 0, the keep
/// is XORed out (`r` = 0), and the outer un-run frees `%a` clean: the state at the outer mark, nothing left
/// over. Until 2026-11-16 the leftover was a barrier and the outer rewind faulted `bad_rewind` for `plus` and
/// `one`, though not for `zero`: a fault decided by whether `c` was zero in every configuration.
const ALLOCATION: &str = "func @body(q1 %c) -> i8 {
entry:
    %r = qalloc 1
    %outer = mark
    %m = mark
    %a = qalloc 1
    x %a ctrl %c, %r
    rewind %m keep (i1 1 -> %r)
    OUTER
    ret 7
}";

#[test]
fn an_allocation_left_over_is_taken_back_by_an_outer_rewind() {
    let inner = entries(&ALLOCATION.replace("OUTER", ""));
    check(&inner, "zero", &[(&[0, 1], 1.0)], 0);
    check(&inner, "plus", &[(&[0, 1, 0], HALF), (&[1, 1, 1], HALF)], 1);
    check(&inner, "one", &[(&[1, 1, 1], 1.0)], 1);
    strict_unchanged(&inner);
    let outer = entries(&ALLOCATION.replace("OUTER", "rewind %outer"));
    check(&outer, "zero", &[(&[0, 0], 1.0)], 0);
    check(&outer, "plus", &[(&[0, 0], HALF), (&[1, 0], HALF)], 0);
    check(&outer, "one", &[(&[1, 0], 1.0)], 0);
}

/// A register read since the mark that no longer holds the value read (the seam's V2, a register read). `%q`
/// is 0 when `@id` reads it; `q ^= o & k` forward with `k` = 0 does nothing; the keep sets `k`, so un-running
/// the gate gives `q = o`, and un-running the read leaves its wire over: the value read (0) XOR `q`, that is
/// `o` (columns `o`, `q`, `k`, wire). An outer rewind undoes the inner one: the call runs again reading the
/// value from the wire XOR `q` (0 on every configuration), the wire is un-allocated, `q ^= o & 1` makes `q` 0
/// and the keep is XORed out: the state at the outer mark.
const READ: &str = "func @body(q1 %o) -> i8 {
entry:
    %q = qalloc 1
    %k = qalloc 1
    OUTER_MARK
    %m = mark budget 20
    %y = call @id(%q)
    x %q ctrl %o, %k
    rewind %m keep (i1 1 -> %k)
    OUTER_REWIND
    ret 7
}";

#[test]
fn a_read_that_changed_leaves_its_wire_over() {
    let inner = entries(&READ.replace("OUTER_MARK", "").replace("OUTER_REWIND", ""));
    check(&inner, "zero", &[(&[0, 0, 1], 1.0)], 0);
    check(&inner, "plus", &[(&[0, 0, 1, 0], HALF), (&[1, 1, 1, 1], HALF)], 1);
    check(&inner, "one", &[(&[1, 1, 1, 1], 1.0)], 1);
    strict_unchanged(&inner);
    let outer =
        entries(&READ.replace("OUTER_MARK", "%outer = mark budget 60").replace("OUTER_REWIND", "rewind %outer"));
    check(&outer, "zero", &[(&[0, 0, 0], 1.0)], 0);
    check(&outer, "plus", &[(&[0, 0, 0], HALF), (&[1, 0, 0], HALF)], 0);
    check(&outer, "one", &[(&[1, 0, 0], 1.0)], 0);
}

/// A byte promoted to qubits since the mark whose qubits do not hold its old value when the promotion is
/// un-run (the seam's V2, un-promotion). `x [24:0] ctrl %o, %k` promotes byte 24 (0) and does nothing with
/// `k` = 0; the keep sets `k`; un-running the gate gives bit 0 of the cell `o`, so un-promoting finds it
/// changed where `o` = 1: the old byte is XORed out and the cell's register is left over holding `o` (columns
/// `o`, `k`, cell); the byte is 0 in memory again. An outer rewind takes the register back as the cell where the
/// gate promotes again, then `cell ^= o & 1` clears it, and the outer un-run un-promotes clean.
const PROMOTION: &str = "func @body(q1 %o) -> i8 {
entry:
    %k = qalloc 1
    OUTER_MARK
    %m = mark budget 20
    x [24:0] ctrl %o, %k
    rewind %m keep (i1 1 -> %k)
    OUTER_REWIND
    ret 7
}";

#[test]
fn an_un_promotion_that_meets_changed_qubits_leaves_the_register_over() {
    let inner = entries(&PROMOTION.replace("OUTER_MARK", "").replace("OUTER_REWIND", ""));
    check(&inner, "zero", &[(&[0, 1], 1.0)], 0);
    check(&inner, "plus", &[(&[0, 1, 0], HALF), (&[1, 1, 1], HALF)], 8);
    check(&inner, "one", &[(&[1, 1, 1], 1.0)], 8);
    strict_unchanged(&inner);
    let outer =
        entries(&PROMOTION.replace("OUTER_MARK", "%outer = mark budget 60").replace("OUTER_REWIND", "rewind %outer"));
    check(&outer, "zero", &[(&[0, 0], 1.0)], 0);
    check(&outer, "plus", &[(&[0, 0], HALF), (&[1, 0], HALF)], 0);
    check(&outer, "one", &[(&[1, 0], 1.0)], 0);
}

/// A memory qubit read since the mark that no longer holds the value read (the seam's V2, a cell read). Byte
/// 24 is promoted before the mark (`z`, holding 0); a load reads it; `cell ^= o & k` does nothing forward; the
/// keep sets `k`; un-running the gate gives `cell = o`, and un-running the load should leave its wire over
/// holding 0 XOR the cell (columns `o`, `k`, cell, wire).
#[test]
#[ignore = "defect: running regardless over a memory-qubit read that changed is not built (P0 debt, HANDOFF.md, Known weak spots)"]
fn a_memory_qubit_read_that_changed_leaves_its_wire_over() {
    let text = entries(
        "func @body(q1 %o) -> i8 {
entry:
    %k = qalloc 1
    z [24]
    %m = mark budget 20
    %v = load i8 24
    x [24:0] ctrl %o, %k
    rewind %m keep (i1 1 -> %k)
    ret 7
}",
    );
    check(&text, "zero", &[(&[0, 1, 0], 1.0)], 0);
    check(&text, "plus", &[(&[0, 1, 0, 0], HALF), (&[1, 1, 1, 1], HALF)], 8);
    check(&text, "one", &[(&[1, 1, 1, 1], 1.0)], 8);
}

/// A check deferred to the sum of the worlds' parts (the seam's V2, the deferred sum). `%a` (0) is read; a
/// gate carries the keep into `a` where `b` = 1; then `%b` is read, which splits the world where `b` is in
/// superposition. The un-run of the first read is below the split point: in the world `b` = 0 the part is
/// faithful, in the world `b` = 1 it is not. A circuit leaves the read's wire over holding `a` = `b` (columns
/// `b`, `a`, `k`, wire); today both worlds fault `bad_rewind`, the faithful one too.
#[test]
#[ignore = "defect: a check deferred to the sum of the worlds' parts does not run regardless (P0 debt, HANDOFF.md, Known weak spots)"]
fn a_deferred_check_that_fails_on_one_part_leaves_the_wire_over_in_every_part() {
    let text = entries(
        "func @body(q1 %b) -> i8 {
entry:
    %a = qalloc 1
    %k = qalloc 1
    %m = mark budget 30
    %ya = call @id(%a)
    x %a ctrl %b, %k
    %yb = call @id(%b)
    rewind %m keep (i1 1 -> %k)
    ret 7
}",
    );
    check(&text, "zero", &[(&[0, 0, 1], 1.0)], 0);
    check(&text, "plus", &[(&[0, 0, 1, 0], HALF), (&[1, 1, 1, 1], HALF)], 1);
    check(&text, "one", &[(&[1, 1, 1, 1], 1.0)], 1);
}

/// An inner rewind's region that would fault when an outer rewind runs it again (the seam's V2, the inner
/// region). The inner region allocates `%t`, sets `t ^= c & r` (nothing, `r` = 0) and frees it clean. The outer
/// keep sets `r`; undoing the inner rewind runs its region again, where `t = c` and the `qfree` is dirty where
/// `c` = 1: on the machine a dirty `qfree` leaves the register over, going forward and when the step runs again
/// alike, so the outer rewind goes on. Its un-run then takes `%t` back (undoing the `qfree`), un-runs
/// `t ^= c & r` (`t` = 0) and the allocation clean: `r` = 1 and nothing is left over (columns `c`, `r`). Strict
/// mode refuses the outer rewind where the `qfree` run again is dirty.
#[test]
fn an_inner_region_whose_free_is_dirty_when_run_again_does_not_stop_the_outer_rewind() {
    let text = entries(
        "func @body(q1 %c) -> i8 {
entry:
    %r = qalloc 1
    %outer = mark
    %m = mark
    %t = qalloc 1
    x %t ctrl %c, %r
    qfree %t
    rewind %m
    rewind %outer keep (i1 1 -> %r)
    ret 7
}",
    );
    check(&text, "zero", &[(&[0, 1], 1.0)], 0);
    check(&text, "plus", &[(&[0, 1], HALF), (&[1, 1], HALF)], 0);
    check(&text, "one", &[(&[1, 1], 1.0)], 0);
    strict_unchanged(&text);
}
