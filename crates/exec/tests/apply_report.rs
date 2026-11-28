//! The step report of a measuring run (`docs/DESIGN.md`, "The simulator's own stops and reports"): for each
//! `apply` the simulator evaluates directly, its position, its function, the inputs evaluated, the largest
//! step count among inputs that finished and how many reached the budget. Checked against each input run on
//! its own as the entry of a run, whose step count the executor reports.

mod common;

use bennett_exec::{ApplyReport, Options, Outcome};
use common::{bounded, run};

/// `@count` counts down from its input and returns after a number of steps that grows with it; `@main`
/// applies it to a register of three qubits in uniform superposition, then to the register at 5, then with a
/// budget that only inputs below 4 meet.
const PROGRAM: &str = "memory 256
func @count(i3 %x) -> i3 {
entry:
    br loop(%x)
loop(i3 %n):
    %done = icmp eq i3 %n, 0
    br %done, out, next
next:
    %m = sub i3 %n, 1
    br loop(%m)
out:
    ret %x
}
func @main(i64 %b) -> i5 {
entry:
    %x = qalloc 3
    h %x
    h %x[1]
    h %x[2]
    %o = qalloc 3
    %f = qalloc 5
    apply @count(%x) -> %o, %f budget 100
    %y = qalloc 3
    x %y
    x %y[2]
    %p = qalloc 3
    apply @count(%y) -> %p, %f budget 100
    %q = qalloc 3
    apply @count(%x) -> %q, %f budget %b
    %m = measure %f
    ret %m
}";

/// The steps `@count(x)` takes when it is the entry of a run.
fn steps_alone(module: &bennett_ir::ValidModule, x: u64) -> u64 {
    let (outcome, executor) = run(module, "count", &[x], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(u128::from(x))));
    executor.stats().steps
}

#[test]
fn each_apply_reports_its_inputs_and_their_largest_step_count() {
    let module = common::load(PROGRAM);
    let alone: Vec<u64> = (0..8).map(|x| steps_alone(&module, x)).collect();
    assert!(alone.windows(2).all(|pair| pair[0] < pair[1]), "{alone:?}");
    let count = module.function_named("count").expect("count");
    // A budget that inputs 0 to 3 meet and 4 to 7 do not.
    let budget = alone[3];
    let options = Options { allow_region_faults: true, ..bounded() };
    let (outcome, executor) = run(&module, "main", &[budget], options);
    assert!(matches!(outcome, Outcome::Returned(Some(_))), "{outcome:?}");
    assert_eq!(
        executor.apply_reports(),
        [
            ApplyReport { position: 1, callee: count, inputs: 8, largest: Some(alone[7]), at_budget: 0 },
            ApplyReport { position: 2, callee: count, inputs: 8, largest: Some(alone[5]), at_budget: 0 },
            ApplyReport { position: 3, callee: count, inputs: 8, largest: Some(alone[3]), at_budget: 4 },
        ]
    );
}

/// Undoing an `apply` runs it again, which adds no report.
#[test]
fn reversing_a_run_adds_no_report() {
    let module = common::load(PROGRAM);
    let options = Options { allow_region_faults: true, reversible: true, ..bounded() };
    let mut executor = common::executor(&module, "main", &[1000], options);
    while executor.step().is_none() {
        if executor.apply_reports().len() == 2 {
            break;
        }
    }
    let before = executor.apply_reports().to_vec();
    assert_eq!(before.len(), 2);
    while executor.step_back().expect("reversible") {}
    assert_eq!(executor.apply_reports(), before);
}
