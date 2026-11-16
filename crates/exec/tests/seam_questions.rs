//! Witnesses of the design seam's questions for the maintainer (`HANDOFF.md`, "Questions for the maintainer"):
//! rules of the machine's meaning whose result depends on other worlds or on a run-wide counter, where a
//! device decides per branch. Each test states what a per-branch rule gives; each is ignored until the
//! maintainer decides, and fails on today's executor.

mod common;

use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use common::{bounded, load};

fn machine(budget: Option<u64>, seed: u64) -> Options {
    Options { machine_meaning: true, budget, seed, ..bounded() }
}

/// V3. `@work(x)` returns 5 after 3 steps for x = 0 and after about 120 for x = 1. Under a run budget of 30
/// with x = |+>, the world x = 0 has returned when the budget runs out; the run reports `budget` for the whole
/// run on every seed. Per branch (the council's preferred option (i)): the branch that returned keeps its
/// result, so the host reading the result sees 5 with probability 1/2.
#[test]
#[ignore = "awaits the maintainer's decision (V3: the run's budget with several worlds)"]
fn the_run_budget_does_not_end_a_world_that_returned() {
    let module = load(
        "memory 64
func @work(i1 %x) -> i8 {
entry:
    br %x, loop(i8 0), done
loop(i8 %i):
    %n = add i8 %i, 1
    %c = icmp ult i8 %n, 40
    br %c, loop(%n), done
done:
    ret 5
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = call @work(%q)
    ret %y
}",
    );
    let outcomes: Vec<Outcome> =
        (0..32).map(|seed| common::run(&module, "main", &[], machine(Some(30), seed)).0).collect();
    assert!(outcomes.iter().all(|o| matches!(o, Outcome::Returned(Some(5)) | Outcome::Faulted(Fault::Budget))));
    assert!(outcomes.contains(&Outcome::Returned(Some(5))), "{outcomes:?}");
}

/// V3'. `@f` rewinds one mark in a loop, so each rewind runs the previous ones again; the replay charge spends
/// the run's budget, and the rewind that would pass it faults `budget`, which the call with a budget catches:
/// the program reads the run-wide counter as its code 1. Under either option for V3 the run's budget is never
/// a call's code.
#[test]
#[ignore = "awaits the maintainer's decision (V3': the replay charge delivered as a call's code)"]
fn the_run_budget_spent_by_running_again_is_not_a_call_code() {
    let module = load(
        "memory 256
func @f() -> i8 {
entry:
    %h = mark
    br loop
loop:
    rewind %h
    br loop
}
func @main() -> i5 {
entry:
    %y, %c = call @f() budget 100000
    ret %c
}",
    );
    for budget in [10, 11, 12, 16, 17, 18, 19, 20, 30] {
        let outcome = common::run(&module, "main", &[], machine(Some(budget), 0)).0;
        assert_ne!(outcome, Outcome::Returned(Some(Fault::Budget.code().into())), "run budget {budget}");
    }
}

/// V4. A `longjmp` through a record held in a superposition of two valid records: each record alone lands the
/// jump at the `setjmp`, which then gives 1. Per branch (all seats): each branch jumps on its own record, so the
/// program returns 1; today the world faults `bad_longjmp`.
#[test]
#[ignore = "awaits the maintainer's decision (V4: a longjmp over a record in superposition)"]
fn a_longjmp_over_a_record_in_superposition_jumps_on_each_branch() {
    let module = load(
        "memory 256
func @body() -> i32 {
entry:
    %buf = alloca 32
    %r = setjmp %buf
    %first = icmp eq i32 %r, 0
    br %first, go, back
go:
    %pad = alloca 16
    %p = add i64 %buf, 24
    h [%p:3]
    longjmp %buf, 1
back:
    ret %r
}
func @main() -> i32 {
entry:
    %v = call @body()
    ret %v
}",
    );
    for options in [machine(None, 0), bounded()] {
        assert_eq!(common::run(&module, "main", &[], options).0, Outcome::Returned(Some(1)));
    }
}

/// N1. A mark rewound twice after reads of `q` whose paths take different times (`@delay` is 2 steps for
/// x = 0, about 14 for x = 1), under an outer mark of budget 50: the inner mark's deadline is cleared at its
/// first rewind, so the second pass ends with clocks that differ by configuration; merging takes the larger,
/// and the outer deadline then faults `budget` for `q` = |1> and |+> but not |0>, with no measurement. Whatever
/// the rule chosen, the three preparations must agree on the outcome.
#[test]
#[ignore = "awaits the maintainer's decision (N1: a second rewind of one budgeted mark)"]
fn a_second_rewind_of_a_budgeted_mark_does_not_tell_inputs_apart() {
    let probe = "memory 512
func @delay(i1 %x) -> i1 {
entry:
    br %x, slow, done
slow:
    br loop(i8 0)
loop(i8 %i):
    %j = add i8 %i, 1
    %c = icmp ult i8 %j, 4
    br %c, loop(%j), done
done:
    ret %x
}
func @probe(q1 %q) -> i8 {
entry:
    %outer = mark budget 50
    %inner = mark budget 25
    %a = call @delay(%q)
    rewind %inner
    %b = call @delay(%q)
    rewind %inner
    %p0 = add i8 0, 0
    %p1 = add i8 0, 0
    %p2 = add i8 0, 0
    %p3 = add i8 0, 0
    %p4 = add i8 0, 0
    %p5 = add i8 0, 0
    %p6 = add i8 0, 0
    %p7 = add i8 0, 0
    %p8 = add i8 0, 0
    %p9 = add i8 0, 0
    ret 7
}
func @main(i8 %prepare) -> i8 {
entry:
    %q = qalloc 1
    switch i8 %prepare, 0: run, 1: one, default: plus
one:
    x %q
    br run
plus:
    h %q
    br run
run:
    %r = call @probe(%q)
    ret %r
}";
    let module = load(probe);
    let outcomes: Vec<Outcome> =
        (0..3).map(|prepare| common::run(&module, "main", &[prepare], machine(None, 0)).0).collect();
    assert!(outcomes.iter().all(|o| *o == outcomes[0]), "{outcomes:?}");
}
