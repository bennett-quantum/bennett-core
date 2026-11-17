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

use bennett_exec::Outcome;
use bennett_ir::Fault;
use common::leftover::{HALF, check, entries, machine, strict_unchanged};
use common::{assert_same_state, executor, load};

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
/// keep sets `k`; un-running the gate gives `cell = o`, and un-running the load leaves its wire over holding 0
/// XOR the cell (columns `o`, `k`, cell, wire). It faulted `bad_rewind` for |+> and |1> until 2026-11-16.
#[test]
fn a_memory_qubit_read_that_changed_leaves_its_wire_over() {
    let body = "func @body(q1 %o) -> i8 {
entry:
    %k = qalloc 1
    z [24]
    OUTER_MARK
    %m = mark budget 20
    %v = load i8 24
    x [24:0] ctrl %o, %k
    rewind %m keep (i1 1 -> %k)
    OUTER_REWIND
    ret 7
}";
    let text = entries(&body.replace("OUTER_MARK", "").replace("OUTER_REWIND", ""));
    check(&text, "zero", &[(&[0, 1, 0], 1.0)], 0);
    check(&text, "plus", &[(&[0, 1, 0, 0], HALF), (&[1, 1, 1, 1], HALF)], 8);
    check(&text, "one", &[(&[1, 1, 1, 1], 1.0)], 8);
    strict_unchanged(&text);
    // An outer rewind takes the wire back where the load runs again: the load reads 0 (the wire XOR the cell)
    // without splitting, the wire is un-allocated, `cell ^= o & 1` clears the cell and the keep is XORed out:
    // the state at the outer mark (columns `o`, `k`, cell).
    let outer =
        entries(&body.replace("OUTER_MARK", "%outer = mark budget 60").replace("OUTER_REWIND", "rewind %outer"));
    check(&outer, "zero", &[(&[0, 0, 0], 1.0)], 0);
    check(&outer, "plus", &[(&[0, 0, 0], HALF), (&[1, 0, 0], HALF)], 0);
    check(&outer, "one", &[(&[1, 0, 0], 1.0)], 0);
}

/// A check deferred to the sum of the worlds' parts (the seam's V2, the deferred sum). `%a` (0) is read; a
/// gate carries the keep into `a` where `b` = 1; then `%b` is read, which splits the world where `b` is in
/// superposition. The un-run of the first read is below the split point: in the world `b` = 0 the part is
/// faithful, in the world `b` = 1 it is not. A circuit leaves the read's wire over holding `a` = `b` (columns
/// `b`, `a`, `k`, wire). Until 2026-11-17 both worlds faulted `bad_rewind`, the faithful one too; now every part
/// leaves the wire over (`joint_leftovers.rs`).
#[test]
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

#[test]
fn a_nested_rewind_does_not_defer_an_unsplit_changed_read() {
    // The first outer rewind contains another rewind, but its read of q never split the world. Its
    // changed read is handled locally by leaving a wire over. A spurious deferred note would survive
    // until the later faithful split/join and incorrectly fault that rewind with bad_rewind.
    let module = load(
        "memory 256
func @id(i1 %x) -> i1 {
entry:
    ret %x
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    %k = qalloc 1
    %m = mark budget 20
    %y = call @id(%q)
    %inner = mark
    %v = add i8 17, 25
    rewind %inner
    x %q ctrl %k
    rewind %m keep (i1 1 -> %k)
    %join = mark budget 10
    h %k
    %read = call @id(%k)
    rewind %join
    ret 7
}",
    );
    let mut e = executor(&module, "main", &[], machine());
    let start = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(Some(7)));
    assert_eq!(e.stats().leftover_qubits, 1);
    e.reverse().expect("reverses through the nested rewind and leftover");
    assert_same_state(&e.snapshot(), &start, "nested local inverse returns to entry");
}

/// An inner region run again on a path the outer keep opened, which reaches a measurement (the seam's debt 2,
/// still a refusal until channels). The inner region reads `%r` (0) and skips the measurement; the outer keep
/// sets `r`, so undoing the inner rewind runs its region again reading 1, and the path reaches `measure`, which
/// no rewound region held. Running it as the circuit would makes the measurement a channel; until channels the
/// outer rewind faults `bad_rewind` in both modes, changing nothing (the state before it, `r` = 0), found by
/// running again, never a call's fault code (`Halt::Inspected`); the run reverses to its entry.
#[test]
fn an_inner_region_run_again_that_reaches_a_measurement_stays_a_named_refusal() {
    let module = common::load(
        "memory 256
func @id(i1 %x) -> i1 {
entry:
    ret %x
}
func @main() -> i8 {
entry:
    %c = qalloc 1
    h %c
    %r = qalloc 1
    %outer = mark budget 100
    %inner = mark budget 40
    %v = call @id(%r)
    br %v, measured, done
measured:
    %m = measure %c
    br done
done:
    rewind %inner
    rewind %outer keep (i1 1 -> %r)
    ret 7
}",
    );
    for options in [common::leftover::machine(), common::leftover::strict()] {
        let mut e = common::executor(&module, "main", &[], options);
        let start = e.snapshot();
        let (outcome, before) = loop {
            let before = e.snapshot();
            if let Some(outcome) = e.step() {
                break (outcome, before);
            }
        };
        assert_eq!(outcome, Outcome::Faulted(Fault::BadRewind));
        let mut after = e.snapshot();
        after.classical.outcome = None;
        common::assert_same_state(&after, &before, "the outer rewind changed nothing");
        assert_eq!(e.stats().leftover_qubits, 0);
        assert_eq!(e.stats().observations, 0, "the measurement is refused before it samples");
        e.reverse().expect("reverses to the entry");
        common::assert_same_state(&e.snapshot(), &start, "back to the entry");
    }
}
