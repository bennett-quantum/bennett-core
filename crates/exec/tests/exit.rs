//! Whole-run exit uses the ordinary outcome and preserves the undo path.
mod common;

use bennett_exec::{Outcome, RunError};
use bennett_ir::Fault;
use common::{allowing_region_faults, assert_reverses, bounded, load, run};

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
func @main() -> i4 {
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %out = qalloc 2
    x %out[0]
    %fault = qalloc 4
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
            branches: 4,
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
