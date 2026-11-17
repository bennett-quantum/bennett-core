//! `apply` computed as the device program's own forms (`docs/DESIGN.md`, "`apply` as the forms"): mark with a
//! budget, the call with a fault value and a budget, rewind-keep, run by the code that runs those instructions
//! (`ApplyMode::Forms`). Compare mode computes every `apply` by the idiom, by branch enumeration and by the forms
//! from the same state and stops at any difference of the forms from the idiom in the result, the machine state
//! and journal, the amplitudes, the history state, the run's state and the other worlds; the whole suites run in
//! that mode (`HANDOFF.md`). Here: the examples, and the differences the decisions allow, each a witness.
//! `apply`, `qalloc` and `qfree` stay first-class forms (the maintainer's decision of the evening of
//! 2026-11-16); this shows what the newer forms express, not a migration.

mod common;

use bennett_exec::{ApplyMode, Options, Outcome};
use bennett_ir::Fault;
use common::{allowing_region_faults, assert_same_state, bounded, executor, load};

const THESIS: &str = include_str!("../../../examples/thesis.bn");
const FAULT: &str = include_str!("../../../examples/fault.bn");
const GROVER: &str = include_str!("../../../examples/grover.bn");

/// Every entry of the three examples that use `apply`, over 8 seeds, strict and allowing region faults: compare
/// mode passes (the forms equal the idiom at every `apply`), and the forms alone give the idiom's outcome and
/// state, and reverse.
#[test]
fn the_forms_equal_the_idiom_on_the_examples() {
    let mut runs = 0;
    for (text, entries) in [
        (THESIS, &[("entangled", None), ("main", None)][..]),
        (GROVER, &[("searched", Some(0)), ("searched", Some(1)), ("searched", Some(2)), ("main", None)][..]),
        (FAULT, &(0..16).map(|n| ("main", Some(n))).collect::<Vec<_>>()[..]),
    ] {
        let module = load(text);
        for &(name, arg) in entries {
            let args: Vec<u64> = arg.into_iter().collect();
            for seed in 0..8 {
                for options in [bounded(), allowing_region_faults()] {
                    let options = Options { seed, reversible: true, ..options };
                    let mut states = Vec::new();
                    for mode in [ApplyMode::Compare, ApplyMode::History, ApplyMode::Forms] {
                        let mut e = executor(&module, name, &args, options);
                        e.set_apply_mode(mode);
                        let at_entry = e.snapshot();
                        let outcome = e.run();
                        states.push((outcome, e.snapshot()));
                        match e.reverse() {
                            Ok(_) => assert_same_state(&e.snapshot(), &at_entry, &format!("@{name} {mode:?}")),
                            Err(bennett_exec::ReverseError::PastMeasurement) => {}
                            Err(error) => panic!("@{name}{args:?} {mode:?}: {error}"),
                        }
                    }
                    let context = format!("@{name}{args:?} seed {seed}");
                    assert_eq!(states[1].0, states[2].0, "{context}: outcome");
                    assert_same_state(&states[2].1, &states[1].1, &context);
                    runs += 1;
                }
            }
        }
    }
    assert_eq!(runs, 2 * 8 * 22);
}

/// The time an `apply` takes. The idiom counts an `apply` as one step of the world's clock; the forms take
/// their own: the mark, the call, `B` steps of the callee at most, a catch, the rewind, and where the
/// arguments read a qubit the rewind costs the mark's budget, `B + 3` after the mark. Time is not machine
/// state ([W1]), so the states agree; the clock differs, as the decisions on the mark's budget make it.
#[test]
fn the_forms_take_the_marks_budget_of_time_where_the_arguments_read_a_qubit() {
    let module = load(
        "memory 64
func @f(i1 %x) -> i1 {
entry:
    ret %x
}
func @main() {
entry:
    %q = qalloc 1
    h %q
    %out = qalloc 1
    %fault = qalloc 5
    apply @f(%q) -> %out, %fault budget 20
    ret
}",
    );
    let mut clocks = Vec::new();
    let mut states = Vec::new();
    for mode in [ApplyMode::History, ApplyMode::Forms] {
        let mut e = executor(&module, "main", &[], bounded());
        e.set_apply_mode(mode);
        assert_eq!(e.run(), Outcome::Returned(None));
        clocks.push(e.stats().clock);
        states.push(e.snapshot());
    }
    assert_same_state(&states[1], &states[0], "the same state");
    // Four steps before the `apply`, the `apply`, the return: 6 by the idiom. By the forms the mark is the
    // `apply`'s step (5), the rewind brings the clock to the mark's plus 23 (28), and the return (29).
    assert_eq!(clocks, [6, 29]);
}

/// An outer mark's budget must cover the inner budgets its region spends ("The mark's budget", Budgets nest).
/// Under `mark budget 6` an `apply ... budget 20` takes one step by the idiom and fits; as the forms it takes
/// the callee's steps under the outer deadline, which passes inside the callee: the call catches `budget`, and
/// the rewind after it is past the deadline too, so the world faults `budget`. A difference the decisions
/// settle for the forms.
#[test]
fn an_outer_marks_budget_bounds_the_forms_of_an_apply() {
    let module = load(
        "memory 256
func @slow(i1 %x) -> i1 {
entry:
    br loop(i8 0)
loop(i8 %i):
    %j = add i8 %i, 1
    %c = icmp ult i8 %j, 5
    br %c, loop(%j), done
done:
    ret %x
}
func @main() -> i5 {
entry:
    %q = qalloc 1
    %out = qalloc 1
    %fault = qalloc 5
    %m = mark budget 6
    apply @slow(%q) -> %out, %fault budget 20
    %c = measure %fault
    ret %c
}",
    );
    let run = |mode| {
        let mut e = executor(&module, "main", &[], bounded());
        e.set_apply_mode(mode);
        e.run()
    };
    assert_eq!(run(ApplyMode::History), Outcome::Returned(Some(0)));
    assert_eq!(run(ApplyMode::Forms), Outcome::Faulted(Fault::Budget));
}
