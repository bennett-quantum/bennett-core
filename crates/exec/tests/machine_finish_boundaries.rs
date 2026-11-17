//! Independent boundary probes for machine-finish: observer completeness, nested fault delivery,
//! and finite guarded calls compared with the whole-world reference where it models the form.

mod common;

use bennett_exec::{Options, Outcome, Snapshot, Unobserved};
use bennett_ir::Fault;
use common::{bounded, executor, load};

fn options(machine: bool) -> Options {
    Options { reversible: true, machine_meaning: machine, ..bounded() }
}

fn same(actual: &Snapshot, expected: &Snapshot) {
    common::assert_same_state(actual, expected, "full snapshot");
    assert_eq!(actual.worlds.len(), expected.worlds.len());
    for (a, b) in actual.worlds.iter().zip(&expected.worlds) {
        same(a, b);
    }
}

#[test]
fn observer_keeps_completed_applies_and_names_the_first_unsupported_step() {
    for (operation, reason) in [
        ("%m = mark\n%v = add i8 %n, 1\nrewind %m", Unobserved::Rewind),
        ("%v, %c = call @id(%n) budget 10", Unobserved::CallBudget),
        // A caught fault discards frames: observation must stop before inspecting their old layout.
        ("%v, %c = call @bad(%n) budget 10", Unobserved::CallBudget),
    ] {
        let text = format!(
            "memory 2048
func @id(i8 %n) -> i8 {{
entry:
    ret %n
}}
func @bad(i8 %n) -> i8 {{
entry:
    fault divide_by_zero
}}
func @nested(i8 %n) -> i8 {{
entry:
    {operation}
    ret %n
}}
func @middle(i8 %n) -> i8 {{
entry:
    %v = call @nested(%n)
    ret %v
}}
func @main() -> i8 {{
entry:
    %q = qalloc 8
    h %q[0]
    s %q[0]
    %out = qalloc 8
    %fault = qalloc 5
    apply @id(%q) -> %out, %fault budget 30
    apply @middle(%q) -> %out, %fault budget 30
    apply @id(%q) -> %out, %fault budget 30
    ret 7
}}"
        );
        let p = load(&text);
        let mut off = executor(&p, "main", &[], options(true));
        let start = off.snapshot();
        assert_eq!(off.run(), Outcome::Returned(Some(7)));
        for raw in [false, true] {
            let mut on = executor(&p, "main", &[], options(true));
            on.measure_journal_content(raw);
            assert_eq!(on.run(), Outcome::Returned(Some(7)));
            same(&on.snapshot(), &off.snapshot());
            assert_eq!(on.stats(), off.stats());
            let measurement = on.journal_measurement().expect("observer enabled");
            assert_eq!(measurement.applies.len(), 1, "only the completed first apply is measured");
            let unsupported = measurement.unsupported.expect("named unsupported operation");
            assert_eq!(unsupported.kind, reason);
            assert_eq!(unsupported.site.func, p.function_named("nested").expect("nested"));
            on.reverse().expect("observer run reverses");
            same(&on.snapshot(), &start);
        }
    }
}

/// No handler, including nested handlers, can deliver a whole-state inspection to the program.
/// Each basis record is malformed and may give code 16; their superposition must stop outside the
/// handlers. This tests the code boundary only, without prescribing how non-local jumps should lower.
#[test]
fn nested_guards_never_turn_disagreeing_jump_cells_into_a_program_code() {
    for prepare in ["z [16]", "x [16]", "h [16]", "h [16]\ns [16]"] {
        let text = format!(
            "memory 1024
func @jump() -> i8 {{
entry:
    longjmp 16, 1
}}
func @inner() -> i8 {{
entry:
    %y, %c = call @jump() budget 20
    %r = zext i5 %c to i8
    ret %r
}}
func @outer() -> i8 {{
entry:
    %y, %c = call @inner() budget 40
    ret %y
}}
func @main() -> i8 {{
entry:
    {prepare}
    %y, %c = call @outer() budget 60
    ret %y
}}"
        );
        let p = load(&text);
        for machine in [false, true] {
            let mut e = executor(&p, "main", &[], options(machine));
            let start = e.snapshot();
            let expected = if prepare.starts_with('h') {
                Outcome::Faulted(Fault::BadLongjmp)
            } else {
                Outcome::Returned(Some(u128::from(Fault::BadLongjmp.code())))
            };
            assert_eq!(e.run(), expected, "{prepare}, machine {machine}");
            e.reverse().expect("nested inspection reverses");
            same(&e.snapshot(), &start);
        }
    }
}

/// Callee steps: mark, load, ordinary call, id return, rewind, return = six. An unbudgeted
/// inner rewind is bounded by the enclosing call, including a memory-qubit read and its split.
#[test]
fn a_guard_covers_an_inner_unbudgeted_memory_read_rewind_in_compare_mode() {
    for budget in [5, 6, 7] {
        let text = format!(
            "memory 1024
func @id(i8 %n) -> i8 {{
entry:
    ret %n
}}
func @f() -> i8 {{
entry:
    %m = mark
    %v = load i8 16
    %y = call @id(%v)
    rewind %m
    ret 3
}}
func @main() -> i8 {{
entry:
    h [16]
    s [16]
    %out = qalloc 8
    x %out[2]
    %fault = qalloc 5
    x %fault[1]
    apply @f() -> %out, %fault budget {budget}
    ret 7
}}"
        );
        let p = load(&text);
        let mut e = executor(&p, "main", &[], options(true));
        let start = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(Some(7)));
        let state = e.snapshot().quantum;
        let (out, code) = if budget < 6 { (4, 2 ^ Fault::Budget.code()) } else { (4 ^ 3, 2) };
        for cell in 0..2 {
            let expected = if cell == 0 {
                num_complex::Complex64::new(std::f64::consts::FRAC_1_SQRT_2, 0.0)
            } else {
                num_complex::Complex64::new(0.0, std::f64::consts::FRAC_1_SQRT_2)
            };
            assert!((state.amplitude(&[cell, out, code]) - expected).norm() < 1e-12);
        }
        assert_eq!(state.amplitudes.len(), 2);
        e.reverse().expect("guarded region reverses");
        same(&e.snapshot(), &start);
        common::oracle_support::worlds_executor::compare(&p, "main", &[], 64, false);
    }
}
