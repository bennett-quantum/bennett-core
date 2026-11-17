//! Independent state and amplitude checks; a double apply alone could hide any wrong return mapping.
mod common;

use bennett_exec::{Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load, run};

const COPIED: &str = r#"
memory 16384
data 16 = "abcdefghijklmnopqrstuvwxyzABCDEF"
func @throw(i64 %b, i32 %v, i8 %n) {
entry:
    %pad = alloca 64
    store i8 %n, %pad
    %more = icmp ne i8 %n, 0
    br %more, recurse, throw
recurse:
    %next = sub i8 %n, 1
    call @throw(%b, %v, %next)
    fault unreachable
throw:
    longjmp %b, %v
}
func @jumpy(i3 %x) -> i8 {
entry:
    %buf = alloca 64
    %copy = add i64 %buf, 32
    %r = setjmp %buf
    %first = icmp eq i32 %r, 0
    br %first, go, back
go:
    %hi = add i64 %buf, 16
    %a = load i128 %buf
    %b = load i128 %hi
    store i128 %a, %copy
    %copyhi = add i64 %copy, 16
    store i128 %b, %copyhi
    store i128 0, %buf
    store i128 0, %hi
    %extra = alloca 80
    store i64 123456, %extra
    %n = zext i3 %x to i8
    %v = zext i3 %x to i32
    %sum = add i32 %v, 17
    call @throw(%copy, %sum, %n)
    fault unreachable
back:
    %r8 = trunc i32 %r to i8
    store i8 %r8, 16
    ret %r8
}
func @main(i3 %x) -> i8 {
entry:
    %r = call @jumpy(%x)
    %r2 = call @jumpy(%x)
    %sum = add i8 %r, %r2
    ret %sum
}
func @coherent() {
entry:
    %q = qalloc 3
    %out = qalloc 8
    %fault = qalloc 5
    h %q[0]
    h %q[1]
    h %q[2]
    apply @jumpy(%q) -> %out, %fault budget 500
    ret
}
"#;

fn assert_each_step_reverses(text: &str, name: &str, args: &[u64], expected: Outcome) {
    let module = load(text);
    let mut exec = executor(&module, name, args, Options { reversible: true, ..bounded() });
    let mut states = vec![exec.snapshot()];
    while exec.step().is_none() {
        states.push(exec.snapshot());
    }
    assert_eq!(exec.outcome(), Some(expected));
    let steps = states.len();
    while let Some(state) = states.pop() {
        assert!(exec.step_back().expect("un-runs"));
        assert_same_state(&exec.snapshot(), &state, &format!("{name}{args:?}, step {}", states.len()));
    }
    assert!(!exec.step_back().expect("at entry"));
    assert_eq!(exec.stats().steps, 0);
    assert!(steps > 0);
}

#[test]
fn copied_records_recursive_frames_and_reused_slots_restore_at_every_step() {
    for x in 0..8 {
        assert_each_step_reverses(COPIED, "main", &[x], Outcome::Returned(Some(u128::from(2 * (x + 17)))));
    }
    let parsed = bennett_ir::read(COPIED).expect("parses");
    assert_eq!(bennett_ir::read(&bennett_ir::write(&parsed)).expect("reads back"), parsed);
}

#[test]
fn one_apply_has_the_expected_mapping_phase_and_private_memory_on_all_eight_branches() {
    let module = load(COPIED);
    let (outcome, exec) = run(&module, "coherent", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(None));
    let state = exec.snapshot();
    assert_eq!(state.quantum.amplitudes.len(), 8);
    for x in 0..8 {
        let amplitude = state.quantum.amplitude(&[x, x + 17, 0]);
        assert!((amplitude.re - 1.0 / 8.0_f64.sqrt()).abs() < 1e-12, "x={x}: {amplitude}");
        assert!(amplitude.im.abs() < 1e-12);
    }
    assert_eq!(&state.classical.memory[16..48], b"abcdefghijklmnopqrstuvwxyzABCDEF");
    assert_eq!(exec.stats().region_faults, 0);
    assert_each_step_reverses(COPIED, "coherent", &[], Outcome::Returned(None));
}
#[test]
fn only_the_branch_that_tries_to_escape_its_region_faults_and_strict_apply_is_atomic() {
    let text = r#"
memory 1024
func @escape(i1 %x) -> i8 {
entry:
    br %x, escape, ok
escape:
    longjmp 16, 17
ok:
    ret 42
}
func @main() {
entry:
    %r = setjmp 16
    %q = qalloc 1
    %out = qalloc 8
    %fault = qalloc 5
    h %q
    apply @escape(%q) -> %out, %fault budget 100
    ret
}
"#;
    let module = load(text);
    let mut exec = executor(&module, "main", &[], bounded());
    for _ in 0..5 {
        assert!(exec.step().is_none());
    }
    let before = exec.snapshot();
    assert_eq!(
        exec.step(),
        Some(Outcome::Stopped(RunError::RegionFault {
            callee: common::entry(&module, "escape"),
            fault: Fault::BadLongjmp,
            faulting: 1,
            configurations: 2,
        }))
    );
    let mut after = exec.snapshot();
    after.classical.outcome = before.classical.outcome;
    after.classical.steps = before.classical.steps;
    assert_same_state(&after, &before, "strict apply leaves the pre-apply state");
    let (outcome, exec) = run(&module, "main", &[], common::allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(None));
    let quantum = exec.snapshot().quantum;
    let h = std::f64::consts::FRAC_1_SQRT_2;
    assert!((quantum.amplitude(&[0, 42, 0]).re - h).abs() < 1e-12);
    assert!((quantum.amplitude(&[1, 0, Fault::BadLongjmp.code()]).re - h).abs() < 1e-12);
}
