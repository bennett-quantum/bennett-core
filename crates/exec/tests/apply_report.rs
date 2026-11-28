//! The step report of a measuring run (`docs/DESIGN.md`, "The step report"): for each `apply` the simulator
//! evaluates directly, its position among every `apply` the run executed, its function, its budget, the
//! inputs evaluated, the largest step count among inputs that returned or faulted (the faulting step
//! counted) and how many the budget ended. Checked against each input run on its own as the entry of a run,
//! whose step count the executor reports, and by replaying with the reported count as the budget.

mod common;

use bennett_emit::{ApplyInput, ApplyMode, compile_apply_with};
use bennett_exec::{ApplyReport, Executor, Options, Outcome};
use bennett_ir::{Fault, ValidModule};
use common::{bounded, executor, run};

/// Runs `main(args)` keeping the step reports.
fn reported<'m>(module: &'m ValidModule, args: &[u64], options: Options) -> (Outcome, Executor<'m>) {
    let mut executor = executor(module, "main", args, options);
    executor.keep_apply_reports();
    (executor.run(), executor)
}

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
    let (outcome, executor) = reported(&module, &[budget], options);
    assert!(matches!(outcome, Outcome::Returned(Some(_))), "{outcome:?}");
    let report = |position, budget, largest, at_budget| ApplyReport {
        position,
        callee: count,
        budget,
        inputs: 8,
        largest: Some(largest),
        at_budget,
    };
    assert_eq!(
        executor.apply_reports(),
        [report(1, 100, alone[7], 0), report(2, 100, alone[5], 0), report(3, budget, alone[3], 4)]
    );
    // Without asking for them, a run keeps none.
    assert_eq!(run(&module, "main", &[budget], options).1.apply_reports(), []);
}

/// `@f` faults on its third step for input 0 (a division by zero, or an explicit `fault budget`) and returns
/// on its second for input 1. A fault counts the step that faulted: the reported count, as the budget,
/// gives the same fault code, and one step less gives `budget`. An explicit `fault budget` is a fault the
/// budget did not cause: it has a count, and no input is at the budget.
#[test]
fn a_faulting_input_counts_its_faulting_step_and_an_explicit_budget_fault_is_a_fault() {
    for (fault, code) in [("udiv", Fault::DivideByZero), ("fault budget", Fault::Budget)] {
        let (body, end) = if fault == "udiv" {
            ("%a = add i8 1, 1\n    %z = udiv i8 %a, 0", "ret %x")
        } else {
            ("%a = add i8 1, 1", "fault budget")
        };
        let module = common::load(&format!(
            "memory 256
func @f(i1 %x) -> i1 {{
entry:
    br %x, done, zero
zero:
    {body}
    {end}
done:
    ret %x
}}
func @main(i64 %b) -> i5 {{
entry:
    %x = qalloc 1
    h %x
    %o = qalloc 1
    %f = qalloc 5
    apply @f(%x) -> %o, %f budget %b
    %m = measure %f
    ret %m
}}"
        ));
        let options = Options { allow_region_faults: true, seed: 1, ..bounded() };
        let (_, executor) = reported(&module, &[100], options);
        let [report] = executor.apply_reports() else { panic!("{fault}: one report") };
        // Input 0: the branch, the addition and the faulting step; input 1: the branch and the return.
        assert_eq!((report.largest, report.at_budget), (Some(3), 0), "{fault}");
        // Replayed on the input that faulted, with the reported count as the budget: the same code.
        let input = |budget: u64| {
            let text = at_zero(body, end);
            let module = common::load(&text);
            run(&module, "main", &[budget], options).0
        };
        assert_eq!(input(3), Outcome::Returned(Some(u128::from(code.code()))), "{fault}");
        assert_eq!(input(2), Outcome::Returned(Some(u128::from(Fault::Budget.code()))), "{fault}");
    }
}

/// The program of the test above with the register at 0 instead of in superposition.
fn at_zero(body: &str, end: &str) -> String {
    format!(
        "memory 256
func @f(i1 %x) -> i1 {{
entry:
    br %x, done, zero
zero:
    {body}
    {end}
done:
    ret %x
}}
func @main(i64 %b) -> i5 {{
entry:
    %x = qalloc 1
    %o = qalloc 1
    %f = qalloc 5
    apply @f(%x) -> %o, %f budget %b
    %m = measure %f
    ret %m
}}"
    )
}

/// A report's position counts every `apply` the run executed, those compiled to gates included: the first
/// `apply` runs compiled, the second evaluated directly, and its report is the second.
#[test]
fn a_reports_position_counts_compiled_applies_too() {
    let module = common::load(PROGRAM);
    let options = Options { allow_region_faults: true, ..bounded() };
    let mut executor = executor(&module, "main", &[1000], options);
    executor.keep_apply_reports();
    let mut compiler = |input: ApplyInput<'_>| Ok(compile_apply_with(input, ApplyMode::Automatic)?.block);
    while executor.apply_reports().is_empty() && executor.outcome().is_none() {
        // Compile the first `apply`, the seventh step, and evaluate the rest.
        let compiled_so_far = executor.stats().steps;
        if compiled_so_far < 7 {
            executor.step_emitted(&mut compiler);
        } else {
            executor.step();
        }
    }
    assert_eq!(executor.apply_reports()[0].position, 2, "{:?}", executor.apply_reports());
}

/// Undoing an `apply` runs it again, which adds no report.
#[test]
fn reversing_a_run_adds_no_report() {
    let module = common::load(PROGRAM);
    let options = Options { allow_region_faults: true, reversible: true, ..bounded() };
    let mut executor = common::executor(&module, "main", &[1000], options);
    executor.keep_apply_reports();
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
