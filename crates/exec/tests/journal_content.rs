//! The journal-content measurement hook: it changes nothing when off (and, being a pure observer,
//! nothing when on), and its classification of one program small enough to work by hand.

mod common;

use bennett_exec::{Executor, Options, Outcome, SitePos, Undo, WordClass, WordRole, classify};
use bennett_ir::ValidModule;
use common::{allowing_region_faults, assert_same_state, bounded, entry, load};

const THESIS: &str = include_str!("../../../examples/thesis.bn");
const GROVER: &str = include_str!("../../../examples/grover.bn");
const FAULT: &str = include_str!("../../../examples/fault.bn");

/// A program small enough to enumerate by hand. `@f(b)` stores 40 in a cell, stores 41 instead when
/// `b`, loads the cell and returns `@inc` of it: 41 when `b` is 0, 42 when `b` is 1. `@g(b)` runs a
/// two-iteration loop carrying an accumulator that starts at 10 (20 when `b`) and a counter at 2.
/// Functions in order: `@inc` 0, `@f` 1, `@g` 2. Blocks of `@f`: entry 0, then 1, else 2, join 3.
/// Blocks of `@g`: entry 0, hi 1, lo 2, loop 3, done 4. The wrappers apply each once to one qubit
/// in uniform superposition, so every measurement below has exactly the two branches `b` = 0 and 1,
/// and the slot a first write overwrites holds the 0 the slot stack was grown with (the first
/// branch's unwind restores it for the second).
const HAND: &str = "memory 1024

func @inc(i8 %x) -> i8 {
entry:
    %y = add i8 %x, 1
    ret %y
}

func @f(i1 %b) -> i8 {
entry:
    %cell = alloca 8
    store i8 40, %cell
    br %b, then, else
then:
    store i8 41, %cell
    br join
else:
    br join
join:
    %v = load i8 %cell
    %r = call @inc(%v)
    ret %r
}

func @g(i1 %b) -> i8 {
entry:
    br %b, hi, lo
hi:
    br loop(i8 20, i8 2)
lo:
    br loop(i8 10, i8 2)
loop(i8 %acc, i8 %n):
    %acc.next = add i8 %acc, 1
    %n.next = sub i8 %n, 1
    %more = icmp ult i8 0, %n.next
    br %more, loop(%acc.next, %n.next), done(%acc.next)
done(i8 %out):
    ret %out
}

func @main_f() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 4
    apply @f(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}

func @main_g() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 4
    apply @g(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}
";

fn measured<'m>(module: &'m ValidModule, name: &str, args: &[u64], keep_raw: bool) -> Executor<'m> {
    let mut executor = Executor::new(module, entry(module, name), args, bounded()).expect("starts");
    executor.measure_journal_content(keep_raw);
    executor.run();
    executor
}

/// (same, recoverable, irreducible, positions) of a classified list.
fn counts(positions: &[bennett_exec::ClassifiedPosition]) -> (usize, usize, usize, usize) {
    let of = |class: WordClass| positions.iter().filter(|p| p.class == class).count();
    (of(WordClass::Same), of(WordClass::Recoverable), of(WordClass::Irreducible), positions.len())
}

#[test]
fn the_measurement_is_off_by_default() {
    let module = load(THESIS);
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], bounded()).expect("starts");
    assert!(executor.journal_measurement().is_none());
    executor.run();
    assert!(executor.journal_measurement().is_none());
    assert!(executor.take_journal_measurement().is_none());
}

/// The argument, in addition to this test: the observer's only executor writes are none. It is fed
/// from `step` after a step already ran, from `evaluate` around a region run, and it reads slots,
/// memory, frames and the journal without writing any of them (see `crates/exec/src/measure.rs`).
/// Here the same runs with it off and on must agree on outcome, snapshot, run statistics, and
/// reversal — with and without raw records, on an interference program and on Grover.
#[test]
fn turning_the_measurement_on_changes_nothing() {
    for (text, name, args) in
        [(THESIS, "main", &[][..]), (THESIS, "entangled", &[][..]), (GROVER, "searched", &[2][..])]
    {
        let module = load(text);
        let mut off = Executor::new(&module, entry(&module, name), args, bounded()).expect("starts");
        let off_outcome = off.run();
        for keep_raw in [false, true] {
            let on = measured(&module, name, args, keep_raw);
            assert_eq!(on.outcome(), Some(off_outcome), "@{name} keep_raw={keep_raw}: outcome");
            assert_eq!(on.stats(), off.stats(), "@{name} keep_raw={keep_raw}: stats");
            assert_same_state(&on.snapshot(), &off.snapshot(), &format!("@{name} keep_raw={keep_raw}"));
        }
    }
    // Reversal of a measured run returns to the recorded entry state, and the outer (program)
    // journal is the size it is without the observer.
    let module = load(THESIS);
    let options = Options { reversible: true, ..bounded() };
    let mut off = Executor::new(&module, entry(&module, "entangled"), &[], options).expect("starts");
    let at_entry = off.snapshot();
    off.run();
    let journal_bytes = off.stats().journal_bytes;
    let mut on = Executor::new(&module, entry(&module, "entangled"), &[], options).expect("starts");
    on.measure_journal_content(true);
    on.run();
    assert_eq!(on.stats().journal_bytes, journal_bytes, "the program journal is untouched");
    let undone = on.reverse().expect("a measured run reverses");
    assert_eq!(undone, off.stats().steps);
    assert_same_state(&on.snapshot(), &at_entry, "measured, then reversed, is back at entry");
}

#[test]
fn counts_without_raw_records_match_counts_with_them() {
    let module = load(HAND);
    let raw = measured(&module, "main_f", &[], true);
    let counted = measured(&module, "main_f", &[], false);
    let (raw, counted) = (raw.journal_measurement().expect("on"), counted.journal_measurement().expect("on"));
    assert_eq!(raw.applies.len(), 1);
    for (raw, counted) in raw.applies[0].branches.iter().zip(&counted.applies[0].branches) {
        assert_eq!(
            (&raw.args, raw.outcome, raw.steps, raw.records, raw.words),
            (&counted.args, counted.outcome, counted.steps, counted.records, counted.words)
        );
        assert_eq!(raw.entries.len(), raw.records, "one copied record per journal record");
        assert_eq!(raw.entries.iter().map(|r| r.words.len()).sum::<usize>(), raw.words);
        assert!(counted.entries.is_empty());
        // Every copied word is one word of the branch journal the run itself counts.
        assert_eq!(raw.words, raw.entries.iter().flat_map(|r| &r.words).count());
    }
}

/// `@f` by hand. Branch `b = 0` journals, in order: entry Call [old %b = 0]; Alloca [0, old stack
/// pointer 960 = 1024 - 32 bytes of wrapper slots - 32 of `@f` slots]; Store [old cell byte 0];
/// Jump from entry [source 0]; Jump from else [source 2]; Slot (the load) [0]; Call [old %x = 0];
/// Slot in `@inc` [0]; Return [packed `@inc`/block 0, stack pointer 936 = 960 - 8 alloca bytes -
/// 16 `@inc` slot bytes, old %r = 0]; Finish []. Branch `b = 1` replaces the else Jump with the
/// then block's Store [old cell byte 40] and Jump [source 1]. Aligned, the shared words all agree
/// (Same: 11), the three branch-only words have no witness (Irreducible: the overwritten 40 is in
/// no live slot — it was a constant operand — and a Jump source is control history), and no word
/// needs the Recoverable class because every first write here overwrites the same 0.
#[test]
fn the_hand_program_store_and_call() {
    let module = load(HAND);
    let executor = measured(&module, "main_f", &[], true);
    let measurement = executor.journal_measurement().expect("on");
    let apply = &measurement.applies[0];
    assert_eq!(apply.branches.len(), 2);
    assert_eq!(apply.branches[0].args, [0]);
    assert_eq!(apply.branches[0].outcome, Outcome::Returned(Some(41)));
    assert_eq!((apply.branches[0].steps, apply.branches[0].records, apply.branches[0].words), (9, 10, 12));
    assert_eq!(apply.branches[1].args, [1]);
    assert_eq!(apply.branches[1].outcome, Outcome::Returned(Some(42)));
    assert_eq!((apply.branches[1].steps, apply.branches[1].records, apply.branches[1].words), (10, 11, 13));
    let (kinds0, values0): (Vec<_>, Vec<_>) =
        apply.branches[0].entries.iter().map(|r| (r.kind, r.words.iter().map(|w| w.value).collect::<Vec<_>>())).unzip();
    assert_eq!(
        kinds0,
        [
            Undo::Call,
            Undo::Alloca,
            Undo::Store,
            Undo::Jump,
            Undo::Jump,
            Undo::Slot,
            Undo::Call,
            Undo::Slot,
            Undo::Return,
            Undo::Finish
        ]
    );
    assert_eq!(
        values0,
        [vec![0], vec![0, 960], vec![0], vec![0], vec![2], vec![0], vec![0], vec![0], vec![0, 936, 0], vec![]]
    );
    let classes = classify(measurement);
    assert_eq!(counts(&classes[0].site_count), (11, 0, 3, 14), "alignment A");
    assert_eq!(counts(&classes[0].call_path), (11, 0, 3, 14), "alignment B: both branches take the same call path");
    let then_store = classes[0]
        .site_count
        .iter()
        .find(|p| p.kind == Undo::Store && p.site.block.0 == 1)
        .expect("the then store position");
    assert_eq!((then_store.branches_present, then_store.class), (1, WordClass::Irreducible));
    assert!(classes[0].site_count.iter().all(|p| p.class != WordClass::Recoverable));
}

/// `@g` by hand. The interesting aligned positions (both alignments; there are no calls, so they
/// coincide): the first loop entry's Jump params come from hi on one branch and lo on the other, so
/// they are branch-only positions, but each is a first write (Recoverable x 4), while the hi/lo Jump
/// sources are Irreducible; the loop back-edge's `%acc` Jump param journals 10 or 20, which after
/// the jump is in no live slot (Irreducible — the loop-carried history); the second write of
/// `%acc.next` journals 11 or 21, which the back-edge had just copied into `%acc`, still live
/// (Recoverable by that one common source). Everything else agrees on both branches. Totals:
/// 19 positions = 11 Same + 5 Recoverable + 3 Irreducible.
#[test]
fn the_hand_program_loop() {
    let module = load(HAND);
    let executor = measured(&module, "main_g", &[], true);
    let measurement = executor.journal_measurement().expect("on");
    let apply = &measurement.applies[0];
    assert_eq!(
        apply.branches.iter().map(|b| (b.steps, b.records, b.words)).collect::<Vec<_>>(),
        [(11, 12, 16), (11, 12, 16)]
    );
    let classes = classify(measurement);
    for positions in [&classes[0].site_count, &classes[0].call_path] {
        assert_eq!(counts(positions), (11, 5, 3, 19));
        let acc_next_second = positions
            .iter()
            .find(|p| p.kind == Undo::Slot && p.site.pos == SitePos::Inst(0) && p.k == 1)
            .expect("the second %acc.next write");
        assert_eq!(
            (acc_next_second.class, acc_next_second.distinct_values, acc_next_second.branches_present),
            (WordClass::Recoverable, 2, 2)
        );
        let acc_back_edge = positions
            .iter()
            .find(|p| p.kind == Undo::Jump && p.role == WordRole::JumpParam(0) && p.site.block.0 == 3 && p.k == 0)
            .expect("the back-edge %acc");
        assert_eq!((acc_back_edge.class, acc_back_edge.distinct_values), (WordClass::Irreducible, 2));
        // The classification accounts for every word instance of both branch journals.
        assert_eq!(positions.iter().map(|p| p.instances).sum::<usize>(), 32);
    }
}

/// A branch that faults is measured up to its fault: `@triangle(4)` runs out of its budget of 20 steps,
/// and the observer has copied every record the branch journalled before the fault (the counts
/// `measure_end_branch` checks against the journal). Off and on agree here too.
#[test]
fn a_faulting_branch_is_measured_up_to_its_fault() {
    let module = load(FAULT);
    let mut off = Executor::new(&module, entry(&module, "main"), &[4], allowing_region_faults()).expect("starts");
    let off_outcome = off.run();
    let mut on = Executor::new(&module, entry(&module, "main"), &[4], allowing_region_faults()).expect("starts");
    on.measure_journal_content(true);
    assert_eq!(on.run(), off_outcome);
    assert_eq!(on.stats(), off.stats());
    assert_same_state(&on.snapshot(), &off.snapshot(), "fault.bn");
    let measurement = on.journal_measurement().expect("on");
    let branch = &measurement.applies[0].branches[0];
    assert_eq!(branch.outcome, Outcome::Faulted(bennett_ir::Fault::Budget));
    assert_eq!(branch.steps, 20);
    assert_eq!(branch.entries.len(), branch.records);
    assert_eq!(branch.entries.iter().map(|r| r.words.len()).sum::<usize>(), branch.words);
}
