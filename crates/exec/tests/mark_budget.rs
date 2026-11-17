//! The mark's budget (`docs/DESIGN.md`, "The mark's budget"; the maintainer's decisions of the afternoon of
//! 2026-11-16): `%h = mark budget B` bounds each world's steps from the mark to its rewind, the rewind
//! included, by B, and a rewind of it costs B whatever the work since the mark, so that every branch waits
//! at the rewind and the time is the budget, not the work done. Every count below is worked by hand.

mod common;

use bennett_exec::{Options, Outcome, RunError};
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
/// rounds, the return. With `%h = mark budget 20` around `call @f(%q)` the machine's time (`Stats::clock`)
/// is 2 (qalloc, a gate) + 1 (mark) + 20 (to the rewind, included) + 1 (ret) = 24 whether `%q` is 0, 1 or
/// both: time does not tell the branches apart. The simulator's ticks (`Stats::steps`) count the work of
/// the longest world: 3v + 11. Without the budget the rewind follows a read of a qubit with none, which is
/// refused (`bad_rewind`, nothing changed).
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
    let mut ticks = Vec::new();
    for prepare in ["z %q[0]", "x %q[0]", "h %q[0]", "x %q[1]"] {
        let module = load(&program(prepare, "mark budget 20"));
        let (outcome, e) = common::run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(0)), "{prepare}");
        assert_eq!(e.stats().clock, 24, "{prepare}");
        ticks.push(e.stats().steps);
        // Unbudgeted: refused at the rewind with the state as it was there; with two worlds the first to
        // reach it is refused while the other runs, so strict mode stops the run as a world fault.
        let module = load(&program(prepare, "mark"));
        let mut e = executor(&module, "main", &[], bounded());
        let mut before = e.snapshot();
        let outcome = loop {
            if let Some(outcome) = e.step() {
                break outcome;
            }
            before = e.snapshot();
        };
        if prepare.starts_with('h') {
            assert!(matches!(outcome, Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })));
        } else {
            assert_eq!(outcome, Outcome::Faulted(Fault::BadRewind), "{prepare}");
            let mut after = e.snapshot();
            after.classical.outcome = None;
            common::assert_same_state(&after, &before, prepare);
        }
    }
    // 2 + 1 + (3v + 6) + 1 rewind + 1 ret = 3v + 11, the longer world for |+>.
    assert_eq!(ticks, [11, 14, 14, 17]);
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

/// A rewind closes its mark's budget: a loop that marks, reads a qubit and rewinds each round, every mark
/// staying in the history, runs until the run's budget. Each round takes 5 ticks (mark, call, return,
/// rewind, br) and 7 of the machine's time (mark, the budget 5 from the call to the rewind, br). Where no
/// qubit is read since the mark, the rewind costs its work.
#[test]
fn a_rewind_closes_its_marks_budget() {
    let text = "memory 64
func @id(i1 %x) -> i1 {
entry:
    ret %x
}
func @main() {
entry:
    %q = qalloc 1
    br loop
loop:
    %h = mark budget 5
    %v = call @id(ARG)
    rewind %h
    br loop
}";
    for (arg, read) in [("%q", true), ("i1 0", false)] {
        let module = load(&text.replace("ARG", arg));
        for run_budget in [10u64, 50, 100] {
            let (outcome, e) = common::run(&module, "main", &[], Options { budget: Some(run_budget), ..bounded() });
            assert_eq!(outcome, Outcome::Faulted(Fault::Budget));
            assert_eq!(e.stats().steps, run_budget);
            // By hand, step by step: qalloc and br, then mark, call, return, rewind, br per round.
            let (mut ticks, mut clock, mut deadline) = (2u64, 2u64, 0u64);
            'run: loop {
                for step in ["mark", "call", "ret", "rewind", "br"] {
                    if ticks >= run_budget {
                        break 'run;
                    }
                    ticks += 1;
                    clock += 1;
                    match step {
                        "mark" => deadline = clock + 5,
                        "rewind" if read => clock = clock.max(deadline),
                        _ => {}
                    }
                }
            }
            assert_eq!(e.stats().clock, clock, "{arg}, run budget {run_budget}");
        }
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
        let outcome = e.run();
        assert!(matches!(outcome, Outcome::Returned(Some(_))), "budget {budget}: {outcome:?}");
        // The value v takes 1 (br) + 3v + 2 (the last test and the branch out) + 1 (rewind) steps after the
        // mark: 4, 7, 10, 13. Over budget, the region faults `budget` for that value.
        let faulting = (0..4u64).filter(|v| 4 + 3 * v > budget).count() as u64;
        assert_eq!(e.stats().region_faults, u64::from(faulting > 0), "budget {budget}");
    }
}

/// A rewind whose history since an unbudgeted mark read a qubit faults `bad_rewind` (a classical cause,
/// nothing changed): here a load of a byte that holds qubits. With a budget it runs. Inside an applied
/// function no budget is required, as the `apply` has one: both ways of computing it agree there.
#[test]
fn a_rewind_after_a_read_of_a_qubit_needs_a_budget_outside_a_region() {
    let outside = |mark: &str| {
        format!(
            "memory 64\nfunc @main() -> i8 {{\nentry:\nh [16:0]\n%h = {mark}\n%v = load i8 16\nrewind %h\nret 3\n}}"
        )
    };
    let module = load(&outside("mark"));
    let (outcome, e) = common::run(&module, "main", &[], bounded());
    assert!(
        matches!(
            outcome,
            Outcome::Faulted(Fault::BadRewind) | Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })
        ),
        "{outcome:?}"
    );
    assert_eq!(e.stats().rewound_steps, 0);
    let module = load(&outside("mark budget 10"));
    assert_eq!(common::run(&module, "main", &[], bounded()).0, Outcome::Returned(Some(3)));
    // Without any read: no budget needed.
    let module = load("memory 64\nfunc @main() -> i8 {\nentry:\n%h = mark\n%v = load i8 16\nrewind %h\nret 3\n}");
    assert_eq!(common::run(&module, "main", &[], bounded()).0, Outcome::Returned(Some(3)));

    let module = load(
        "memory 64
func @g() -> i8 {
entry:
    %h = mark
    %v = load i8 16
    rewind %h
    ret 3
}
func @main() -> i8 {
entry:
    h [16:0]
    %out = qalloc 8
    %fault = qalloc 5
    apply @g() -> %out, %fault budget 10
    %m = measure %out
    ret %m
}",
    );
    let mut e = executor(&module, "main", &[], bounded());
    assert_eq!(e.run(), Outcome::Returned(Some(3)));
}
