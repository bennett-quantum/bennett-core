//! Whole-run exit uses the ordinary outcome and preserves the undo path.
mod common;

use bennett_exec::{Options, Outcome, ReverseError, RunError};
use bennett_ir::Fault;
use common::{allowing_region_faults, assert_reverses, bounded, executor, load, run};

#[test]
fn nested_exit_returns_its_status_and_restores_the_full_snapshot() {
    let module = load(include_str!("../../../examples/exit.bn"));
    let (outcome, executor) = run(&module, "main", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(37)));
    assert_eq!(executor.tape(), b"!");
    assert_eq!(executor.snapshot().classical.memory[16], 42);
    assert_eq!(assert_reverses(&module, "main", &[], bounded()), outcome);
}

const REGION: &str = r#"
memory 256
func @stop(i2 %n) -> i2 {
entry:
    store i8 99, 16
    exit %n
}
func @main() -> i5 {
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %out = qalloc 2
    x %out[0]
    %fault = qalloc 5
    apply @stop(%q) -> %out, %fault budget 100
    %code = measure %fault
    ret %code
}
"#;

#[test]
fn exit_faults_on_every_applied_branch_without_leaking_writes() {
    let module = load(REGION);
    let (outcome, executor) = run(&module, "main", &[], bounded());
    assert_eq!(
        outcome,
        Outcome::Stopped(RunError::RegionFault {
            callee: common::entry(&module, "stop"),
            fault: Fault::ExitInRegion,
            faulting: 4,
            configurations: 4,
        })
    );
    assert_eq!(executor.snapshot().classical.memory[16], 0);
    for seed in 0..8 {
        let (outcome, executor) = run(&module, "main", &[], bennett_exec::Options { seed, ..allowing_region_faults() });
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::ExitInRegion.code()))));
        assert_eq!(executor.snapshot().classical.memory[16], 0);
        assert!(executor.snapshot().quantum.amplitudes.iter().all(|(basis, _)| basis[1] == 1));
    }
    assert_reverses(&module, "main", &[], bounded());
}

#[test]
fn exit_checks_the_selected_entry_result_type() {
    let text =
        "memory 128\nfunc @stop() {\nentry:\n exit i32 5\n}\nfunc @main() -> i64 {\nentry:\n call @stop()\n ret 0\n}\n";
    let module = load(text);
    assert_eq!(run(&module, "main", &[], bounded()).0, Outcome::Faulted(Fault::ExitTypeMismatch));
    assert_eq!(run(&module, "stop", &[], bounded()).0, Outcome::Faulted(Fault::ExitTypeMismatch));
    let module = load(&text.replace("-> i64", "-> i32"));
    assert_eq!(run(&module, "main", &[], bounded()).0, Outcome::Returned(Some(5)));
}

// An exit on only one applied branch (n == 2) faults on that branch alone.
const CONDITIONAL_REGION: &str = r#"
memory 256
func @stop(i2 %n) -> i2 {
entry:
    %two = icmp eq i2 %n, 2
    br %two, bail, keep
bail:
    exit %n
keep:
    %r = add i2 %n, 1
    ret %r
}
func @main() -> i4 {
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %out = qalloc 2
    %fault = qalloc 5
    apply @stop(%q) -> %out, %fault budget 100
    ret 0
}
"#;

#[test]
fn conditional_exit_faults_only_on_the_exiting_branch() {
    let module = load(CONDITIONAL_REGION);
    let (outcome, _) = run(&module, "main", &[], bounded());
    assert_eq!(
        outcome,
        Outcome::Stopped(RunError::RegionFault {
            callee: common::entry(&module, "stop"),
            fault: Fault::ExitInRegion,
            faulting: 1,
            configurations: 4,
        })
    );
    let (outcome, executor) = run(&module, "main", &[], allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(0)));
    // Columns: q, out, fault. On q == 2 out keeps its value and fault holds the code.
    let mut configs: Vec<Vec<u64>> = executor.snapshot().quantum.amplitudes.iter().map(|(c, _)| c.clone()).collect();
    configs.sort();
    assert_eq!(configs, [[0, 1, 0], [1, 2, 0], [2, 0, Fault::ExitInRegion.code()], [3, 0, 0]]);
    assert_reverses(&module, "main", &[], allowing_region_faults());
}

#[test]
fn an_exiting_helper_inside_a_region_faults_before_the_entry_type_is_checked() {
    // The helper's i8 exit would also mismatch the i64 entry; inside a region the region fault wins.
    let module = load(
        "memory 256\nfunc @helper() {\nentry:\n exit i8 3\n}\nfunc @callee(i2 %n) -> i2 {\nentry:\n call @helper()\n ret %n\n}\nfunc @main() -> i64 {\nentry:\n %q = qalloc 2\n %out = qalloc 2\n %fault = qalloc 5\n apply @callee(%q) -> %out, %fault budget 100\n %code = measure %fault\n %wide = zext i5 %code to i64\n ret %wide\n}\n",
    );
    assert_eq!(
        run(&module, "main", &[], allowing_region_faults()).0,
        Outcome::Returned(Some(u128::from(Fault::ExitInRegion.code())))
    );
}

#[test]
fn undoing_only_the_finish_redoes_the_exit_without_repeating_effects() {
    let module = load(include_str!("../../../examples/exit.bn"));
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    assert_eq!(executor.run(), Outcome::Returned(Some(37)));
    assert_eq!(executor.step_back(), Ok(true));
    assert_eq!(executor.outcome(), None);
    assert_eq!(executor.tape(), b"!");
    assert_eq!(executor.run(), Outcome::Returned(Some(37)));
    assert_eq!(executor.tape(), b"!", "the effect before the exit ran once");
    assert_eq!(executor.step_back(), Ok(true));
    assert_eq!(executor.step_back(), Ok(true), "now the effect itself is undone");
    assert_eq!(executor.tape(), b"");
    assert_eq!(executor.run(), Outcome::Returned(Some(37)));
    assert_eq!(executor.tape(), b"!");
}

#[test]
fn exit_without_a_journal_keeps_its_outcome_when_reversal_is_refused() {
    let module = load(include_str!("../../../examples/exit.bn"));
    let mut executor = executor(&module, "main", &[], bounded());
    assert_eq!(executor.run(), Outcome::Returned(Some(37)));
    assert_eq!(executor.step_back(), Err(ReverseError::NotReversible));
    assert_eq!(executor.step(), Some(Outcome::Returned(Some(37))), "stepping a finished run changes nothing");
}

#[test]
fn reversal_past_an_exit_still_stops_at_a_measurement() {
    let module = load(
        "memory 256\nfunc @main() -> i32 {\nentry:\n %q = qalloc 1\n x %q\n %m = measure %q\n %w = zext i1 %m to i32\n exit %w\n}\n",
    );
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    assert_eq!(executor.run(), Outcome::Returned(Some(1)));
    assert_eq!(executor.step_back(), Ok(true), "the finish is undone");
    assert_eq!(executor.step_back(), Ok(true), "the extension is undone");
    assert_eq!(executor.step_back(), Err(ReverseError::PastMeasurement));
    assert_eq!(executor.step_back(), Err(ReverseError::PastMeasurement), "and stays refused");
}

#[test]
fn exit_edge_values_match_an_entry_return() {
    for (text, expected) in [
        ("memory 128\nfunc @main() -> i1 {\nentry:\n exit i1 1\n}\n", 1),
        ("memory 128\nfunc @main() -> i8 {\nentry:\n exit i8 -1\n}\n", 255),
        ("memory 128\nfunc @main() -> i8 {\nentry:\n ret -1\n}\n", 255),
    ] {
        assert_eq!(run(&load(text), "main", &[], bounded()).0, Outcome::Returned(Some(expected)));
    }
    // An exit and a return also meet the step budget in the same step.
    for text in [
        "memory 128\nfunc @main() -> i32 {\nentry:\n exit i32 5\n}\n",
        "memory 128\nfunc @main() -> i32 {\nentry:\n ret 5\n}\n",
    ] {
        let module = load(text);
        assert_eq!(
            run(&module, "main", &[], Options { budget: Some(0), ..bounded() }).0,
            Outcome::Faulted(Fault::Budget)
        );
        assert_eq!(run(&module, "main", &[], Options { budget: Some(1), ..bounded() }).0, Outcome::Returned(Some(5)));
    }
    // The widest status travels through a call and reverses.
    let module = load(
        "memory 128\nfunc @f(i128 %v) -> i128 {\nentry:\n exit %v\n}\nfunc @main() -> i128 {\nentry:\n %r = call @f(i128 0xffffffffffffffffffffffffffffffff)\n ret %r\n}\n",
    );
    assert_eq!(assert_reverses(&module, "main", &[], bounded()), Outcome::Returned(Some(u128::MAX)));
    // A slot's type is checked against the entry, not only a constant's.
    let text = "memory 128\nfunc @s(i16 %v) {\nentry:\n exit %v\n}\nfunc @main() -> i32 {\nentry:\n call @s(i16 5)\n ret 0\n}\n";
    assert_eq!(run(&load(text), "main", &[], bounded()).0, Outcome::Faulted(Fault::ExitTypeMismatch));
    assert_eq!(run(&load(&text.replace("-> i32", "-> i16")), "main", &[], bounded()).0, Outcome::Returned(Some(5)));
}

#[test]
fn console_bytes_unwind_with_the_journal_and_redo() {
    let module = load("memory 256\nfunc @main() -> i32 {\nentry:\n effect 0\n effect 255\n effect 10\n ret 3\n}\n");
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let at_entry = executor.snapshot();
    assert_eq!(executor.run(), Outcome::Returned(Some(3)));
    assert_eq!(executor.tape(), [0, 255, 10]);
    assert_eq!(executor.reverse().expect("reverses"), 4);
    assert_eq!(executor.tape(), b"");
    common::assert_same_state(&executor.snapshot(), &at_entry, "after un-running the console output");
    assert_eq!(executor.run(), Outcome::Returned(Some(3)));
    assert_eq!(executor.tape(), [0, 255, 10]);
}

#[test]
fn exit_through_an_indirect_call_keeps_every_frame_for_reversal() {
    let module = load(
        "memory 1024\nfunc @leaf(i32 %v) -> i32 {\nentry:\n effect 76\n exit %v\n}\nfunc @mid(i32 %v) -> i32 {\nentry:\n %f = faddr @leaf\n %r = call_indirect fn(i32) -> i32 %f(%v)\n ret %r\n}\nfunc @main(i32 %v) -> i32 {\nentry:\n %r = call @mid(%v)\n ret %r\n}\n",
    );
    let (outcome, executor) = run(&module, "main", &[200], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(200)));
    assert_eq!(executor.tape(), b"L");
    assert_eq!(executor.snapshot().classical.frames.len(), 3);
    assert_reverses(&module, "main", &[200], bounded());
}
