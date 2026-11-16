//! The mark's budget (`docs/DESIGN.md`, "The mark's budget"; the maintainer's decisions of the afternoon of
//! 2026-11-16): `%h = mark budget B` bounds each world's steps from the mark to its rewind, the rewind
//! included, by B, and a rewind of it costs B whatever the work since the mark, so that every branch waits
//! at the rewind and the time is the budget, not the work done. Every count below is worked by hand.

mod common;

use bennett_exec::{ApplyMode, Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{bounded, executor, load};

#[test]
fn the_budget_is_read_written_and_typed() {
    let text = "memory 64
func @main(i64 %b) {
entry:
    %h = mark budget 7
    %g = mark budget %b
    %k = mark
    rewind %k
    rewind %g
    rewind %h
    ret
}
";
    let module = bennett_ir::read(text).expect("reads");
    let written = bennett_ir::write(&module);
    assert!(written.contains("%h = mark budget 7") && written.contains("%g = mark budget %b"), "{written}");
    assert!(written.contains("%k = mark\n"), "{written}");
    assert_eq!(bennett_ir::read(&written).expect("reads again"), module);
    assert!(bennett_ir::load(text).is_ok());
    // The budget is an i64, like an `apply`'s.
    let narrow = text.replace("func @main(i64 %b)", "func @main(i8 %b)");
    assert!(bennett_ir::load(&narrow).is_err());
}

/// `%h = mark budget 3` then a loop: the three steps after the mark are taken, the fourth faults `budget`
/// with the run's own budget far away.
#[test]
fn a_world_takes_at_most_the_budget_from_the_mark() {
    for budget in [1u64, 2, 3, 10] {
        let module = load(&format!(
            "memory 64\nfunc @main() {{\nentry:\n%h = mark budget {budget}\nbr loop\nloop:\nbr loop\n}}"
        ));
        let (outcome, e) = common::run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Faulted(Fault::Budget), "budget {budget}");
        assert_eq!(e.stats().steps, 1 + budget, "budget {budget}");
    }
}

/// `@f(v)` takes 3v + 6 steps from the call: the call, the branch into the loop, three per round for v + 1
/// rounds, the return. With `%h = mark budget 20` around `call @f(%q)` the
/// machine's time (`Stats::clock`) is 2 (qalloc, a gate) + 1 (mark) + 20 (to the rewind, included) + 1
/// (ret) = 24 whether `%q` is 0, 1 or both: time does not tell the branches apart. Without the budget it
/// does. The simulator's ticks (`Stats::steps`) count the work of the longest world either way.
#[test]
fn a_rewind_of_a_mark_with_a_budget_costs_the_budget() {
    let program = |prepare: &str, mark: &str| {
        format!(
            "memory 256
func @f(i2 %v) {{
entry:
    br loop(%v)
loop(i2 %n):
    %done = icmp eq i2 %n, 0
    %next = sub i2 %n, 1
    br %done, out, loop(%next)
out:
    ret
}}
func @main() -> i8 {{
entry:
    %q = qalloc 2
    {prepare}
    %h = {mark}
    call @f(%q)
    rewind %h
    ret 0
}}"
        )
    };
    let mut unbudgeted = Vec::new();
    for prepare in ["z %q[0]", "x %q[0]", "h %q[0]", "x %q[1]"] {
        let module = load(&program(prepare, "mark budget 20"));
        let (outcome, e) = common::run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{prepare}");
        assert_eq!(e.stats().clock, 24, "{prepare}");
        let ticks = e.stats().steps;
        let module = load(&program(prepare, "mark"));
        let (outcome, e) = common::run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{prepare}");
        assert_eq!(e.stats().steps, ticks, "{prepare}: the budget changes no tick");
        assert_eq!(e.stats().clock, e.stats().steps, "{prepare}: without a budget the time is the ticks");
        unbudgeted.push(e.stats().steps);
    }
    // Unbudgeted: 2 + 1 + (3v + 6) + 1 rewind + 1 ret = 3v + 11, the longer world for |+>.
    assert_eq!(unbudgeted, [11, 14, 14, 17]);
}

/// The path of one world needs more than the budget: strict mode stops the run where that world faults;
/// with a budget large enough for both the run is the same as for one.
#[test]
fn a_world_over_the_budget_faults_and_strict_mode_stops_the_run() {
    let text = |budget: u64| {
        format!(
            "memory 256
func @f(i1 %v) {{
entry:
    br %v, long, out
long:
    br l1
l1:
    br l2
l2:
    br l3
l3:
    br out
out:
    ret
}}
func @main() -> i8 {{
entry:
    %q = qalloc 1
    h %q
    %h = mark budget {budget}
    call @f(%q)
    rewind %h
    ret 0
}}"
        )
    };
    // The long world: call, br, four brs, ret, rewind = 8 steps from the mark; the short one 4.
    let (outcome, _) = common::run(&load(&text(7)), "main", &[], bounded());
    assert!(matches!(outcome, Outcome::Stopped(RunError::WorldFault { fault: Fault::Budget, .. })), "{outcome:?}");
    let fits = load(&text(8));
    let (outcome, e) = common::run(&fits, "main", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(0)));
    assert_eq!((e.stats().clock, e.stats().worlds), (2 + 1 + 8 + 1, 1));
}

/// A rewind closes its mark's budget: a loop that marks and rewinds each round, every mark staying in the
/// history, runs until the run's budget, each round taking 1 (br) + 1 (mark) + 5 (the budget, the rewind
/// included) of the machine's time and 3 ticks.
#[test]
fn a_rewind_closes_its_marks_budget() {
    let module = load("memory 64\nfunc @main() {\nentry:\nbr loop\nloop:\n%h = mark budget 5\nrewind %h\nbr loop\n}");
    for run_budget in [10u64, 50, 100] {
        let (outcome, e) = common::run(&module, "main", &[], Options { budget: Some(run_budget), ..bounded() });
        assert_eq!(outcome, Outcome::Faulted(Fault::Budget));
        assert_eq!(e.stats().steps, run_budget);
        // By hand: the ticks go br, mark, rewind, br, ...; the time 1, 2, 7, 8, 9, 14, ...
        let rounds = (run_budget - 1) / 3;
        let rest = (run_budget - 1) % 3;
        let clock = 1 + 7 * rounds + [0, 1, 6][rest as usize];
        assert_eq!(e.stats().clock, clock, "run budget {run_budget}");
    }
}

/// A mark with a budget inside an applied function: both ways of computing `apply` agree (compare mode stops
/// at any difference), on every budget from 1 to 12, with the callee's loop length a quarter of the inputs.
#[test]
fn a_mark_budget_inside_a_region_agrees_both_ways() {
    for budget in 1..=12u64 {
        let module = load(&format!(
            "memory 256
func @g(i2 %v) -> i2 {{
entry:
    %h = mark budget {budget}
    br loop(%v)
loop(i2 %n):
    %done = icmp eq i2 %n, 0
    %next = sub i2 %n, 1
    br %done, out, loop(%next)
out:
    rewind %h
    ret %v
}}
func @main() -> i5 {{
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %out = qalloc 2
    %fault = qalloc 5
    apply @g(%q) -> %out, %fault budget 100
    %m = measure %fault
    ret %m
}}"
        ));
        let mut e = executor(&module, "main", &[], Options { allow_region_faults: true, ..bounded() });
        e.set_apply_mode(ApplyMode::Compare);
        let outcome = e.run();
        assert!(matches!(outcome, Outcome::Returned(Some(_))), "budget {budget}: {outcome:?}");
        // The value v takes 1 (br) + 3v + 2 (the last test and the branch out) + 1 (rewind) steps after the
        // mark: 4, 7, 10, 13. Over budget, the region faults `budget` for that value.
        let faulting = (0..4u64).filter(|v| 4 + 3 * v > budget).count() as u64;
        assert_eq!(e.stats().region_faults, u64::from(faulting > 0), "budget {budget}");
    }
}
