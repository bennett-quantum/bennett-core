//! Running regardless where worlds join (outside strict mode; `docs/DESIGN.md`, "Strict mode"): a rewind whose
//! un-run meets a state its history does not describe on one world's part of a state several worlds share, or in
//! a world that other worlds join at the rewind, or while worlds split off during the un-run. Every check site
//! there leaves its wire over in every world, faithful or not, so that the worlds' register tables agree and
//! their sum is the circuit's; once they have merged, the wires zero in every configuration are discarded
//! (`crates/exec/src/leftover.rs`). And a register left over whose id a later step of the same un-run revives
//! moves out of its way.
//!
//! Each program runs on |0>, |+> and |1> of its control qubit `%c`; every expected state is worked by hand as
//! the program's circuit computes it (each read copies the register into a wire, each un-read XORs the register
//! into that wire again), with its columns named, then the run reverses to its entry.

mod common;

use bennett_exec::{Outcome, RunError};
use bennett_ir::Fault;
use common::leftover::{HALF, check, entries, strict};

/// The strict outcomes of `@zero`, `@plus` and `@one`.
fn strict_outcomes(text: &str) -> [Outcome; 3] {
    let module = common::load(text);
    ["zero", "plus", "one"].map(|entry| common::run(&module, entry, &[], strict()).0)
}

/// A wire left over at an id that a later step of the same un-run revives. `%t` is allocated and freed since
/// the mark, so its id is free when the un-run reaches the read of `%a` (0), which the keep changed through the
/// gate (`a = c`): the read's wire takes that id, holding `c`. Un-running `qfree %t` then revives `%t` there: the
/// wire moves to the next free id first (columns `c`, `a`, `k`, `%t`'s, free again, and the wire). Until
/// 2026-11-17 the revival took the wire's id and un-running `qalloc %t` panicked (machine meaning, |+> and |1>).
/// An outer rewind takes the wire back where the call runs again: the state at the outer mark.
#[test]
fn a_wire_whose_id_a_later_step_revives_moves_out_of_its_way() {
    let body = "func @body(q1 %c) -> i8 {
entry:
    %a = qalloc 1
    %k = qalloc 1
    OUTER_MARK
    %m = mark budget 40
    %t = qalloc 1
    qfree %t
    %y = call @id(%a)
    x %a ctrl %c, %k
    rewind %m keep (i1 1 -> %k)
    OUTER_REWIND
    ret 7
}";
    let inner = entries(&body.replace("OUTER_MARK", "").replace("OUTER_REWIND", ""));
    check(&inner, "zero", &[(&[0, 0, 1], 1.0)], 0);
    check(&inner, "plus", &[(&[0, 0, 1, 0, 0], HALF), (&[1, 1, 1, 0, 1], HALF)], 1);
    check(&inner, "one", &[(&[1, 1, 1, 0, 1], 1.0)], 1);
    let faulted = Outcome::Faulted(Fault::BadRewind);
    assert_eq!(strict_outcomes(&inner), [Outcome::Returned(Some(7)), faulted, faulted]);
    let outer =
        entries(&body.replace("OUTER_MARK", "%outer = mark budget 90").replace("OUTER_REWIND", "rewind %outer"));
    check(&outer, "zero", &[(&[0, 0, 0], 1.0)], 0);
    check(&outer, "plus", &[(&[0, 0, 0], HALF), (&[1, 0, 0], HALF)], 0);
    check(&outer, "one", &[(&[1, 0, 0], 1.0)], 0);
}

/// Worlds that join at the rewind, a check above their split point. `%c` is read (the split: worlds c = 0 and
/// c = 1), then `%r` (0, both worlds); `h %r ctrl %c, %k` does nothing forward (`k` = 0); the keep sets `k`, so
/// un-running the gate gives `r` = |+> where `c` = 1. Un-reading `%r` leaves its wire over in both worlds: 0 in
/// c = 0, `r` in c = 1; un-reading `%c` leaves a wire holding 0 in both. The worlds are one machine state again
/// and merge; the zero wire is discarded (columns `c`, `r`, `k`, the wire of `%r`): (|0,0,1,0> + (|1,0,1,0> +
/// |1,1,1,1>)/sqrt 2)/sqrt 2. Until 2026-11-17 only the world c = 1 left a wire over, so the two never merged and
/// the host's reading observed one of them (this state's interference lost).
#[test]
fn worlds_that_join_at_the_rewind_leave_the_wire_over_in_each() {
    let text = entries(
        "func @body(q1 %c) -> i8 {
entry:
    %r = qalloc 1
    %k = qalloc 1
    %m = mark budget 40
    %a = call @id(%c)
    %b = call @id(%r)
    h %r ctrl %c, %k
    rewind %m keep (i1 1 -> %k)
    ret 7
}",
    );
    check(&text, "zero", &[(&[0, 0, 1], 1.0)], 0);
    check(&text, "plus", &[(&[0, 0, 1, 0], HALF), (&[1, 0, 1, 0], 0.5), (&[1, 1, 1, 1], 0.5)], 1);
    check(&text, "one", &[(&[1, 0, 1, 0], HALF), (&[1, 1, 1, 1], HALF)], 1);
    let [zero, plus, one] = strict_outcomes(&text);
    assert_eq!((zero, one), (Outcome::Returned(Some(7)), Outcome::Faulted(Fault::BadRewind)));
    assert!(matches!(plus, Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })), "{plus:?}");
}

/// The same with a memory qubit in place of `%r`: byte 24 is promoted before the mark (a cell holding 0), the
/// load reads it in both worlds, and `h [24:0] ctrl %c, %k` makes its bit 0 |+> where `c` = 1 once the keep sets
/// `k`. Un-loading leaves the access's byte wire over in both worlds (0 XOR the cell), the zero wire of `%c`'s
/// read is discarded (columns `c`, `k`, the cell, the load's wire).
#[test]
fn worlds_that_join_at_the_rewind_leave_a_memory_access_wire_over_in_each() {
    let text = entries(
        "func @body(q1 %c) -> i8 {
entry:
    %k = qalloc 1
    z [24:0]
    %m = mark budget 40
    %a = call @id(%c)
    %v = load i8 24
    h [24:0] ctrl %c, %k
    rewind %m keep (i1 1 -> %k)
    ret 7
}",
    );
    check(&text, "zero", &[(&[0, 1, 0], 1.0)], 0);
    check(&text, "plus", &[(&[0, 1, 0, 0], HALF), (&[1, 1, 0, 0], 0.5), (&[1, 1, 1, 1], 0.5)], 8);
    check(&text, "one", &[(&[1, 1, 0, 0], HALF), (&[1, 1, 1, 1], HALF)], 8);
}

/// Worlds that split off during the un-run (the seam's debt 3, first half). `%a` (0) is read; an inner
/// apply-like region reads `%c` and rewinds, merging its worlds; the gate does nothing forward. The outer keep
/// sets `k`: un-running the gate gives `a = c`; undoing the inner rewind runs its region again, where reading
/// `%c` splits the world; each world un-runs the inner region (its read of `%c` leaves a zero wire) and then,
/// below that split point, the read of `%a`, whose wire holds `c`. Both worlds carry what was left over, merge,
/// and the zero wire is discarded (columns `c`, `a`, `k`, the wire of `%a`). Until 2026-11-17 a rewind that left
/// something over while worlds split off faulted `bad_rewind` on the machine too.
#[test]
fn worlds_that_split_off_during_the_un_run_carry_its_leftovers() {
    let text = entries(
        "func @body(q1 %c) -> i8 {
entry:
    %a = qalloc 1
    %k = qalloc 1
    %outer = mark budget 100
    %y = call @id(%a)
    %inner = mark budget 20
    %z = call @id(%c)
    rewind %inner
    x %a ctrl %c, %k
    rewind %outer keep (i1 1 -> %k)
    ret 7
}",
    );
    check(&text, "zero", &[(&[0, 0, 1], 1.0)], 0);
    check(&text, "plus", &[(&[0, 0, 1, 0], HALF), (&[1, 1, 1, 1], HALF)], 1);
    check(&text, "one", &[(&[1, 1, 1, 1], 1.0)], 1);
    let [zero, plus, one] = strict_outcomes(&text);
    assert_eq!((zero, one), (Outcome::Returned(Some(7)), Outcome::Faulted(Fault::BadRewind)));
    assert_ne!(plus, Outcome::Returned(Some(7)), "strict mode diagnoses the changed read");
}

/// A call run again whose wire is taken back while another of its reads splits the world (the seam's debt 3,
/// second half). `@pair` reads `%a` (0) and `%c` (the split); the gate `a ^= k` does nothing forward; the inner
/// keep sets `k`, so un-running the gate makes `a` = 1: un-reading `%a` leaves its wire over holding 1 in both
/// worlds, un-reading `%c` a zero wire, discarded once they merge (columns `c`, `a`, `k`, the wire of `%a`). An
/// outer rewind undoes the inner one: the call runs again, the world splits by `%c` first and each world takes
/// back its own copy of the wire (`a` XOR wire = 0, the value read); `a ^= k` clears `a`, the keep is XORed
/// out, and the outer un-run is faithful: the state at the outer mark (columns `c`, `a`, `k`). Until 2026-11-17
/// running the call again refused there, so the outer rewind faulted `bad_rewind`.
#[test]
fn a_call_run_again_takes_its_wire_back_in_every_world_its_other_read_splits() {
    let body = "func @pair(i1 %x, i1 %y) -> i1 {
entry:
    ret %x
}

func @body(q1 %c) -> i8 {
entry:
    %a = qalloc 1
    %k = qalloc 1
    OUTER_MARK
    %inner = mark budget 40
    %y = call @pair(%a, %c)
    x %a ctrl %k
    rewind %inner keep (i1 1 -> %k)
    OUTER_REWIND
    ret 7
}";
    let inner = entries(&body.replace("OUTER_MARK", "").replace("OUTER_REWIND", ""));
    check(&inner, "zero", &[(&[0, 1, 1, 1], 1.0)], 1);
    check(&inner, "plus", &[(&[0, 1, 1, 1], HALF), (&[1, 1, 1, 1], HALF)], 1);
    check(&inner, "one", &[(&[1, 1, 1, 1], 1.0)], 1);
    let outer =
        entries(&body.replace("OUTER_MARK", "%outer = mark budget 100").replace("OUTER_REWIND", "rewind %outer"));
    check(&outer, "zero", &[(&[0, 0, 0], 1.0)], 0);
    check(&outer, "plus", &[(&[0, 0, 0], HALF), (&[1, 0, 0], HALF)], 0);
    check(&outer, "one", &[(&[1, 0, 0], 1.0)], 0);
}
