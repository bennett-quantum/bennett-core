//! The journal-content measurement hook: it changes nothing when off (and, being a pure observer,
//! nothing when on), and its classification of one program small enough to work by hand.

mod common;

use bennett_exec::{Executor, Options, Outcome, SitePos, Undo, WordClass, WordRole, classify};
use bennett_ir::ValidModule;
use common::{allowing_region_faults, assert_same_state, bounded, load};

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
    %e = qalloc 5
    apply @f(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}

func @main_g() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 5
    apply @g(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}
";

fn measured<'m>(module: &'m ValidModule, name: &str, args: &[u64], keep_raw: bool) -> Executor<'m> {
    let mut executor = common::executor(module, name, args, bounded());
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
    let mut executor = common::executor(&module, "main", &[], bounded());
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
    for (text, name, args) in [
        (THESIS, "main", &[][..]),
        (THESIS, "entangled", &[][..]),
        (GROVER, "searched", &[2][..]),
        (ALIAS, "main", &[][..]),
    ] {
        let module = load(text);
        let mut off = common::executor(&module, name, args, bounded());
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
    let mut off = common::executor(&module, "entangled", &[], options);
    let at_entry = off.snapshot();
    off.run();
    let journal_bytes = off.stats().journal_bytes;
    let mut on = common::executor(&module, "entangled", &[], options);
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
    let mut off = common::executor(&module, "main", &[4], allowing_region_faults());
    let off_outcome = off.run();
    let mut on = common::executor(&module, "main", &[4], allowing_region_faults());
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

/// At the second multiply, the old result is 6/10, the new one is 3/5. Its original
/// operands (2 and 3/5) remain in %saved and %seed. Recompute multiplies those live values.
/// Moving the save before the multiply destroys the only copy of 2: a depth-two chain
/// could recreate it from the first loop edge, but this witness must leave it irreducible.
const ALIAS: &str = "memory 1024
func @f(i1 %b) -> i8 {
entry:
    %seed = select i8 %b, 5, 3
    br loop(i8 2)
loop(i8 %n):
    %v = mul i8 %n, %seed
    %saved = or i8 %n, 0
    %next = sub i8 %n, 1
    %more = icmp ult i8 0, %next
    br %more, loop(%next), done(%v)
done(i8 %out):
    ret %out
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 5
    apply @f(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}
";

#[test]
fn recomputation_uses_live_operand_copies_and_requires_a_common_recipe() {
    use bennett_exec::classify_with_recompute;
    let module = load(ALIAS);
    let mut executor = measured(&module, "main", &[], true);
    let mut measurement = executor.take_journal_measurement().expect("on");
    let copy = classify(&measurement);
    let recompute = classify_with_recompute(&measurement);
    let multiply = |p: &&bennett_exec::ClassifiedPosition| {
        p.kind == Undo::Slot && p.site.block.0 == 1 && p.site.pos == SitePos::Inst(0) && p.k == 1
    };
    for (copy, recompute) in
        [(&copy[0].site_count, &recompute[0].site_count), (&copy[0].call_path, &recompute[0].call_path)]
    {
        let old = copy.iter().find(multiply).expect("second multiply");
        let new = recompute.iter().find(multiply).expect("second multiply");
        assert_eq!(old.class, WordClass::Irreducible);
        assert_eq!(new.class, WordClass::Recomputed);
        assert_eq!(new.distinct_values, 2);
        assert_eq!(new.recompute.as_ref().expect("recipe").inputs.len(), 2);
    }
    // Every instance having *some* recovery is insufficient: one common source per operand
    // is required across branches. Mutate one branch's metadata to model disjoint source identities.
    let witness = measurement.applies[0].branches[1]
        .entries
        .iter_mut()
        .find(|r| r.site.block.0 == 1 && r.site.pos == SitePos::Inst(0) && r.b_k == 1)
        .expect("multiply")
        .words[0]
        .recompute
        .as_mut()
        .expect("instance recipe");
    for source in &mut witness.inputs[0] {
        source.call_path = u32::MAX;
    }
    let different = classify_with_recompute(&measurement);
    assert_eq!(different[0].site_count.iter().find(multiply).expect("multiply").class, WordClass::Irreducible);
    assert_eq!(different[0].call_path.iter().find(multiply).expect("multiply").class, WordClass::Irreducible);

    let no_alias = ALIAS.replace(
        "    %v = mul i8 %n, %seed\n    %saved = or i8 %n, 0",
        "    %saved = or i8 %n, 0\n    %v = mul i8 %n, %seed",
    );
    let module = load(&no_alias);
    let executor = measured(&module, "main", &[], true);
    for branch in &executor.journal_measurement().expect("on").applies[0].branches {
        let old = &branch
            .entries
            .iter()
            .find(|r| r.site.block.0 == 1 && r.site.pos == SitePos::Inst(1) && r.b_k == 1)
            .expect("multiply")
            .words[0];
        assert!(old.recompute.is_none(), "cannot read the vanished operand from observer history");
    }
}

#[test]
fn recomputation_of_a_load_checks_current_memory_and_store_overlap_invalidates_provenance() {
    let program = ALIAS
        .replace(
            "    %seed = select i8 %b, 5, 3",
            "    %seed = select i8 %b, 5, 3\n    %cell = alloca 8\n    store i8 %seed, %cell",
        )
        .replace(
            "    %v = mul i8 %n, %seed\n    %saved = or i8 %n, 0",
            "    %v = load i8 %cell\n    store i8 %n, %cell",
        );
    let module = load(&program);
    let executor = measured(&module, "main", &[], true);
    for branch in &executor.journal_measurement().expect("on").applies[0].branches {
        let old = &branch
            .entries
            .iter()
            .find(|r| r.site.block.0 == 1 && r.site.pos == SitePos::Inst(0) && r.b_k == 1)
            .expect("load")
            .words[0];
        assert!(old.recompute.is_none(), "the same address now holds different bytes");
    }

    let text = HAND
        .replace("    store i8 40, %cell", "    store i16 4660, %cell\n    store i8 7, %cell")
        .replace("    store i8 41, %cell", "    store i16 41, %cell");
    let module = load(&text);
    let executor = measured(&module, "main_f", &[], true);
    let branch = &executor.journal_measurement().expect("on").applies[0].branches[1];
    let overwritten =
        &branch.entries.iter().find(|r| r.kind == Undo::Store && r.site.block.0 == 1).expect("store").words[0];
    assert_eq!(overwritten.value, 0x1207);
    assert!(overwritten.recompute.is_none(), "overlapping stores do not leave one whole-word producer");
}

#[test]
fn the_hand_program_with_recomputation() {
    let module = load(HAND);
    let executor = measured(&module, "main_f", &[], true);
    let measurement = executor.journal_measurement().expect("on");
    let copy = classify(measurement);
    let recompute = bennett_exec::classify_with_recompute(measurement);
    for (old, new) in [(&copy[0].site_count, &recompute[0].site_count), (&copy[0].call_path, &recompute[0].call_path)] {
        assert_eq!(counts(old), (11, 0, 3, 14));
        assert_eq!(counts(new), (11, 0, 2, 14));
        let recovered: Vec<_> = new.iter().filter(|p| p.class == WordClass::Recomputed).collect();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].role, WordRole::Store);
        assert!(recovered[0].recompute.as_ref().expect("store 40 recipe").inputs.is_empty());
        assert!(new.iter().filter(|p| p.class == WordClass::Irreducible).all(|p| p.role == WordRole::JumpSource));
    }
}

#[test]
fn chains_recover_vanished_operands_at_unbounded_depth() {
    // The final multiply overwrites 6/10 with 3/5. Its old operand 2 has no live copy.
    // Follow the literal initial loop argument and every subtract/edge, without a depth cap.
    for n in [2, 40] {
        let text = ALIAS
            .replace(
                "    %v = mul i8 %n, %seed\n    %saved = or i8 %n, 0",
                "    %saved = or i8 %n, 0\n    %v = mul i8 %n, %seed",
            )
            .replace("br loop(i8 2)", &format!("br loop(i8 {n})"))
            .replace("budget 100", "budget 1000");
        let module = load(&text);
        let executor = measured(&module, "main", &[], true);
        let m = executor.journal_measurement().expect("measurement fixture");
        let old = bennett_exec::classify_with_recompute(m);
        let new = bennett_exec::classify_with_chains(m, false);
        for (old, new) in [(&old[0].site_count, &new[0].site_count), (&old[0].call_path, &new[0].call_path)] {
            let find = |p: &&bennett_exec::ClassifiedPosition| {
                p.site.block.0 == 1 && p.site.pos == SitePos::Inst(1) && p.k == n - 1
            };
            assert_eq!(old.iter().find(find).expect("measurement fixture").class, WordClass::Irreducible);
            let recovered = new.iter().find(find).expect("measurement fixture");
            assert_eq!(recovered.class, WordClass::Recomputed);
            let chain = recovered.chain.as_ref().expect("measurement fixture");
            if n == 2 {
                assert_eq!((chain.instructions, chain.depth), (2, 2));
            } else {
                assert!(chain.depth > 32, "{chain:?}");
            }
        }
    }
}

#[test]
fn a_journal_seed_is_counted_once_and_must_precede_the_recovery() {
    // An unsupported call result alternates between 9/25 and 10/26. The first two
    // overwritten results must be retained; the third may read the first retained word.
    let text = ALIAS.replace(
        "    %v = mul i8 %n, %seed",
        "    %v = call @helper(%n, %seed)",
    ).replace("func @f(i1 %b)", "func @helper(i8 %n, i8 %seed) -> i8 {\nentry:\n    %p = and i8 %n, 1\n    %s = mul i8 %seed, %seed\n    %r = add i8 %s, %p\n    ret %r\n}\nfunc @f(i1 %b)")
     .replace("br loop(i8 2)", "br loop(i8 4)").replace("budget 100", "budget 1000");
    let module = load(&text);
    let executor = measured(&module, "main", &[], true);
    let m = executor.journal_measurement().expect("measurement fixture");
    let live = bennett_exec::classify_with_chains(m, false);
    let tape = bennett_exec::classify_with_chains(m, true);
    for (live, tape) in [(&live[0].site_count, &tape[0].site_count), (&live[0].call_path, &tape[0].call_path)] {
        let returns: Vec<_> = live
            .iter()
            .zip(tape)
            .filter(|(p, _)| p.role == WordRole::ReturnResult && p.class == WordClass::Irreducible)
            .collect();
        assert_eq!(returns.len(), 3);
        assert_eq!(returns.iter().filter(|(_, p)| p.class == WordClass::Irreducible).count(), 2);
        let recovered = returns.iter().find(|(_, p)| p.class == WordClass::Recomputed).expect("measurement fixture").1;
        assert_eq!(
            recovered.chain.as_ref().expect("measurement fixture").instructions,
            0,
            "a retained-word copy is a zero-instruction leaf"
        );
    }
}

#[test]
fn chains_do_not_use_historical_load_bytes_as_inputs() {
    let text = ALIAS.replace("func @f(i1 %b) -> i8 {\nentry:\n    %seed = select i8 %b, 5, 3\n    br loop(i8 2)\nloop(i8 %n):\n    %v = mul i8 %n, %seed\n    %saved = or i8 %n, 0", "func @f(i1 %b) -> i8 {\nentry:\n    %cell = alloca 8\n    br %b, hi, lo\nhi:\n    store i8 5, %cell\n    br loop(i8 2)\nlo:\n    store i8 3, %cell\n    br loop(i8 2)\nloop(i8 %n):\n    %a = load i8 %cell\n    store i8 %n, %cell\n    %v = mul i8 %a, 7\n    br clear(%n)\nclear(i8 %discard):");
    // Literal stores leave no live 3/5 alias when the cell is overwritten before the next multiply.
    let module = load(&text);
    let executor = measured(&module, "main", &[], true);
    let m = executor.journal_measurement().expect("measurement fixture");
    let deep = bennett_exec::classify_with_chains(m, false);
    let old = deep[0]
        .site_count
        .iter()
        .find(|p| p.site.pos == SitePos::Inst(2) && p.k == 1 && p.role == WordRole::Slot)
        .expect("measurement fixture");
    assert_eq!(old.class, WordClass::Irreducible);
    assert!(
        old.chain
            .as_ref()
            .expect("measurement fixture")
            .blockers
            .iter()
            .any(|s| s == "current-memory-or-stack differs")
    );
}

#[test]
fn chains_do_not_confuse_numeric_aliases_with_control_presence() {
    let text =
        HAND.replace("    %cell = alloca 8", "    %one = or i8 1, 0\n    %two = or i8 2, 0\n    %cell = alloca 8");
    let module = load(&text);
    let executor = measured(&module, "main_f", &[], true);
    let m = executor.journal_measurement().expect("on");
    for tape in [false, true] {
        let classes = bennett_exec::classify_with_chains(m, tape);
        for positions in [&classes[0].site_count, &classes[0].call_path] {
            assert_eq!(counts(positions), (13, 0, 2, 16));
            assert_eq!(positions.iter().filter(|p| p.class == WordClass::Recomputed).count(), 1);
            for p in positions.iter().filter(|p| p.role == WordRole::JumpSource && p.class == WordClass::Irreducible) {
                assert!(!p.chain.as_ref().expect("control diagnostic").individually_recovered);
            }
        }
    }
}

#[test]
fn setjmp_invalidates_load_chains_in_both_halves_of_its_buffer() {
    for offset in [0, 15, 16, 31] {
        let text = format!(
            "memory 1024
func @f(i1 %b) -> i8 {{
entry:
    %cell = alloca 32
    %at = add i64 %cell, {offset}
    br %b, hi, lo
hi:
    store i8 5, %at
    br loop(i8 2)
lo:
    store i8 3, %at
    br loop(i8 2)
loop(i8 %n):
    %a = load i8 %at
    %v = mul i8 %a, 7
    %r = setjmp %cell
    br clear(%n)
clear(i8 %discard):
    %next = sub i8 %discard, 1
    %more = icmp ult i8 0, %next
    br %more, loop(%next), done(%v)
done(i8 %out):
    ret %out
}}
func @main() -> i8 {{
entry:
    %q = qalloc 1
    h %q
    %out = qalloc 8
    %fault = qalloc 5
    apply @f(%q) -> %out, %fault budget 100
    ret 0
}}"
        );
        let module = load(&text);
        let on = measured(&module, "main", &[], true);
        let measurement = on.journal_measurement().expect("on");
        // With no intervening write the identical load/multiply chain is recoverable.
        let unchanged = load(&text.replace("    %r = setjmp %cell\n", ""));
        let baseline = measured(&unchanged, "main", &[], true);
        let target = |p: &&bennett_exec::ClassifiedPosition| {
            p.site.block == bennett_ir::BlockId(3) && p.site.pos == SitePos::Inst(1) && p.k == 1
        };
        // Retained tape can legitimately supply the old load value; test live memory alone.
        let classes = bennett_exec::classify_with_chains(measurement, false);
        let baseline = bennett_exec::classify_with_chains(baseline.journal_measurement().expect("on"), false);
        for (positions, unchanged) in
            [(&classes[0].site_count, &baseline[0].site_count), (&classes[0].call_path, &baseline[0].call_path)]
        {
            let p = positions.iter().find(target).expect("second multiply");
            assert_eq!(p.class, WordClass::Irreducible, "offset {offset}");
            assert!(
                p.chain
                    .as_ref()
                    .expect("load diagnostic")
                    .blockers
                    .iter()
                    .any(|b| b == "current-memory-or-stack differs")
            );
            assert_ne!(unchanged.iter().find(target).expect("second multiply").class, WordClass::Irreducible);
        }
    }
}

/// The witness of a `longjmp`'s result word names the live frames left after the jump, each by its own
/// call path. `@f` copies 23130 into `%copy`, takes a `setjmp`, jumps back to it with 23130, then again
/// with 99: the second jump overwrites the result slot's 23130, still held by `%copy` in `@f`'s frame,
/// the region's root (call path 0). Once the jump is in `@f` itself (no frame discarded) and once from
/// a callee `@g` (one frame of two discarded); in both the one live copy is `@f`'s slot at path 0.
/// Pairing the live frames with the wrong shadows (for example counting `len / discarded` frames
/// instead of `len - discarded`) names a wrong path or function, or fails outright.
#[test]
fn a_longjmp_result_witness_names_the_frames_left_after_the_jump() {
    for (name, jump_first, jump_second) in [
        ("in the frame", "longjmp %buf, 23130", "longjmp %buf, 99"),
        ("from a callee", "call @g(%buf, i32 23130)\n    ret 0", "call @g(%buf, i32 99)\n    ret 0"),
    ] {
        let text = format!(
            "memory 1024
func @g(i64 %buf, i32 %v) {{
entry:
    longjmp %buf, %v
}}
func @f(i1 %b) -> i32 {{
entry:
    %buf = alloca 32
    %copy = add i32 0, 23130
    %r = setjmp %buf
    %first = icmp eq i32 %r, 0
    br %first, one, after
one:
    {jump_first}
after:
    %again = icmp eq i32 %r, 23130
    br %again, two, done
two:
    {jump_second}
done:
    ret %r
}}
func @main() -> i32 {{
entry:
    %q = qalloc 1
    h %q
    %out = qalloc 32
    %fault = qalloc 5
    apply @f(%q) -> %out, %fault budget 100
    %m = measure %out
    ret %m
}}"
        );
        let module = load(&text);
        let f = common::entry(&module, "f");
        let executor = measured(&module, "main", &[], true);
        let apply = &executor.journal_measurement().expect("on").applies[0];
        assert_eq!(apply.branches.len(), 2, "{name}");
        for branch in &apply.branches {
            assert_eq!(branch.outcome, Outcome::Returned(Some(99)), "{name}");
            let jumps: Vec<_> = branch.entries.iter().filter(|r| r.kind == Undo::Longjmp).collect();
            assert_eq!(jumps.len(), 2, "{name}");
            let result = jumps[1].words.last().expect("the result word");
            assert_eq!((result.role, result.value), (WordRole::LongjmpResult, 23130), "{name}");
            let bennett_exec::WordWitness::LiveCopy(sources) = &result.witness else {
                panic!("{name}: expected a live copy, got {:?}", result.witness);
            };
            assert_eq!(sources.len(), 1, "{name}: {sources:?}");
            assert_eq!(
                (sources[0].call_path, sources[0].func, sources[0].kind),
                (0, Some(f), bennett_exec::SourceKind::Slot),
                "{name}"
            );
        }
    }
}

/// An applied function that rewinds, and one that calls with a budget: both are valid inside a region,
/// and the observer does not model either (a rewind pops records it already classified; a fault caught by
/// the call discards frames). It must say so by name, not panic, and the run must go on unobserved
/// with the outcome it has with the observer off.
const UNSUPPORTED: &str = "memory 1024

func @leaf(i1 %b) -> i8 {
entry:
    %r = zext i1 %b to i8
    ret %r
}

func @rewinds(i1 %b) -> i8 {
entry:
    %h = mark budget -1
    %t = add i1 %b, 1
    rewind %h
    %r = zext i1 %b to i8
    ret %r
}

func @catches(i1 %b) -> i8 {
entry:
    %y, %c = call @leaf(%b) budget 10
    ret %y
}

func @main_rewinds() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 5
    apply @rewinds(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}

func @main_catches() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 5
    apply @catches(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}
";

#[test]
fn a_rewind_or_a_call_with_a_budget_in_a_region_is_named_unsupported() {
    let module = load(UNSUPPORTED);
    for (name, kind) in
        [("main_rewinds", bennett_exec::Unobserved::Rewind), ("main_catches", bennett_exec::Unobserved::CallBudget)]
    {
        let mut off = common::executor(&module, name, &[], bounded());
        let off_outcome = off.run();
        for keep_raw in [false, true] {
            let mut on = measured(&module, name, &[], keep_raw);
            assert_eq!(on.outcome(), Some(off_outcome), "@{name}: the run goes on unobserved");
            assert_eq!(on.stats(), off.stats(), "@{name}: stats");
            assert_same_state(&on.snapshot(), &off.snapshot(), name);
            let measurement = on.take_journal_measurement().expect("on");
            let unsupported = measurement.unsupported.expect("the observer names what it does not follow");
            assert_eq!(unsupported.kind, kind, "@{name}");
            assert!(measurement.applies.is_empty(), "@{name}: the unfinished apply is not reported");
        }
    }
}
