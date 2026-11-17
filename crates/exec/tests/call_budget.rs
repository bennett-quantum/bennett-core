//! The call with a fault value and a budget, `%y, %c = call @f(args) budget B` (`docs/DESIGN.md`, "The call
//! with a fault value"; the maintainer's decisions of 2026-11-16 [A1] and of the afternoon): the region's
//! rules attached to an ordinary call. Every expected value is worked by hand: `@count(n)` takes 3n + 5
//! steps (the branch into its loop, three per round for n + 1 rounds, the return), so under a budget B it
//! returns 7 exactly when 3n + 5 <= B.

mod common;

use bennett_exec::{Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};

const CALLEES: &str = "memory 256
func @div(i8 %a, i8 %b) -> i8 {
entry:
    %q = udiv i8 %a, %b
    ret %q
}
func @count(i8 %n) -> i8 {
entry:
    br loop(%n)
loop(i8 %k):
    %z = icmp eq i8 %k, 0
    %k1 = sub i8 %k, 1
    br %z, done, loop(%k1)
done:
    ret 7
}
func @talk() -> i8 {
entry:
    effect 65
    ret 1
}
func @quit() -> i8 {
entry:
    exit i16 3
}
func @alloc() -> i8 {
entry:
    %r = qalloc 1
    ret 1
}
func @jump(i64 %buf) -> i8 {
entry:
    longjmp %buf, 1
}
func @own_jump() -> i8 {
entry:
    %buf = alloca 32
    %r = setjmp %buf
    %again = icmp eq i32 %r, 0
    br %again, go, back
go:
    longjmp %buf, 5
back:
    ret 9
}
func @rewinding() -> i8 {
entry:
    br l
l:
    %h = mark
    %x = add i8 1, 1
    rewind %h
    br l
}
func @inner(i8 %n) -> i8 {
entry:
    %y, %c = call @count(%n) budget 10
    %cz = zext i5 %c to i8
    %s = add i8 %y, %cz
    ret %s
}
";

/// `@case()` calls `CALL` after a four-step prologue (alloca, setjmp, compare, branch) and returns
/// `y | c << 8`, five steps after the call.
fn program(call: &str) -> String {
    format!(
        "{CALLEES}func @case() -> i16 {{
entry:
    %buf = alloca 32
    %r = setjmp %buf
    %again = icmp eq i32 %r, 0
    br %again, go, back
back:
    ret 255
go:
    %y, %c = {call}
    %a = zext i8 %y to i16
    %b = zext i5 %c to i16
    %s = shl i16 %b, 8
    %p = or i16 %a, %s
    ret %p
}}"
    )
}

/// Runs `@case` reversibly; returns (y, c), the ticks after the prologue and the tape, and
/// checks that reversal returns to the entry exactly.
fn case(call: &str) -> ((u128, u128), u64, Vec<u8>) {
    let module = load(&program(call));
    let mut e = executor(&module, "case", &[], Options { reversible: true, ..bounded() });
    let entry = e.snapshot();
    let Outcome::Returned(Some(packed)) = e.run() else { panic!("{call}: {:?}", e.outcome()) };
    let result = ((packed & 0xff, packed >> 8), e.stats().steps - 4, e.tape().to_vec());
    e.reverse().unwrap_or_else(|error| panic!("{call}: {error}"));
    assert_same_state(&e.snapshot(), &entry, call);
    result
}

#[test]
fn the_call_form_is_read_written_and_typed() {
    let text = "memory 64
func @f(i8 %x) -> i8 {
entry:
    ret %x
}
func @g() {
entry:
    ret
}
func @main(i64 %b) -> i5 {
entry:
    %y, %c = call @f(i8 1) budget 10
    %d = call @g() budget %b
    %e = call @f(%y) budget 3
    %s = add i5 %c, %d
    ret %s
}
";
    let module = bennett_ir::read(text).expect("reads");
    let written = bennett_ir::write(&module);
    for line in ["%y, %c = call @f(i8 1) budget 10", "%d = call @g() budget %b", "%e = call @f(%y) budget 3"] {
        assert!(written.contains(line), "{line}\n{written}");
    }
    assert_eq!(bennett_ir::read(&written).expect("reads again"), module);
    assert!(bennett_ir::load(text).is_ok());
    // The fault code is an i5; the budget an i64; a value only from a function that returns one; a pair
    // only for a call with a budget.
    for bad in [
        text.replace("%s = add i5 %c, %d", "%s = add i8 %c, %d"),
        text.replace("func @main(i64 %b)", "func @main(i8 %b)"),
        text.replace("%d = call @g() budget %b", "%d, %z = call @g() budget %b"),
        text.replace("%y, %c = call @f(i8 1) budget 10", "%y, %c = call @f(i8 1)"),
        text.replace("%y, %c = call @f(i8 1) budget 10", "%y, %c = add i8 1, 2"),
    ] {
        assert!(bennett_ir::load(&bad).is_err(), "{bad}");
    }
}

#[test]
fn a_return_gives_its_value_and_code_zero_and_a_fault_gives_zero_and_its_code() {
    assert_eq!(case("call @div(i8 7, i8 2) budget 10").0, (3, 0));
    let (values, ticks, _) = case("call @div(i8 7, i8 0) budget 10");
    assert_eq!(values, (0, u128::from(Fault::DivideByZero.code())));
    // The call, then the division faults: the catch is the step that returns to the caller.
    assert_eq!(ticks, 2 + 5);
}

#[test]
fn the_budget_bounds_the_callees_steps() {
    for n in 0..5u64 {
        for budget in 0..25u64 {
            let (values, ticks, _) = case(&format!("call @count(i8 {n}) budget {budget}"));
            if 3 * n + 5 <= budget {
                assert_eq!((values, ticks), ((7, 0), 1 + 3 * n + 5 + 5), "n {n}, budget {budget}");
            } else {
                let expected = ((0, u128::from(Fault::Budget.code())), 1 + budget + 1 + 5);
                assert_eq!((values, ticks), expected, "n {n}, budget {budget}");
            }
        }
    }
}

/// The region's rules: an effect, an `exit`, a quantum instruction and a jump below the callee's frame
/// fault inside, and the program gets their codes, nothing printed; a jump within the callee is allowed.
#[test]
fn the_regions_rules_are_attached() {
    let code = |fault: Fault| u128::from(fault.code());
    let (values, _, tape) = case("call @talk() budget 10");
    assert_eq!((values, tape), ((0, code(Fault::EffectInRegion)), vec![]));
    assert_eq!(case("call @quit() budget 10").0, (0, code(Fault::ExitInRegion)));
    assert_eq!(case("call @alloc() budget 10").0, (0, code(Fault::QuantumInRegion)));
    assert_eq!(case("call @jump(%buf) budget 10").0, (0, code(Fault::BadLongjmp)));
    assert_eq!(case("call @own_jump() budget 20").0, (9, 0));
}

/// Nested calls with budgets: `@inner(2)` gives `@count(2)` (11 steps) a budget of 10, so it gets the
/// code `budget` and returns 0 + 1 = 1 after 15 steps of its own: the call, 10 steps of `@count`, the catch,
/// the extension, the addition, the return. Under an outer budget of 15 the outer call returns (1, 0);
/// under 14 the outer budget is spent first (at `@inner`'s return) and the outer call gets `budget`.
#[test]
fn nested_budgets_each_end_their_own_call() {
    assert_eq!(case("call @inner(i8 2) budget 15").0, (1, 0));
    assert_eq!(case("call @inner(i8 2) budget 14").0, (0, u128::from(Fault::Budget.code())));
    assert_eq!(case("call @inner(i8 1) budget 12").0, (7, 0));
}

/// Every step reverses exactly, the catches included: snapshots after each step, then back step by step.
#[test]
fn every_step_of_a_call_with_a_budget_reverses() {
    for call in [
        "call @inner(i8 2) budget 14",
        "call @inner(i8 2) budget 15",
        "call @own_jump() budget 20",
        "call @rewinding() budget 17",
    ] {
        let module = load(&program(call));
        let mut e = executor(&module, "case", &[], Options { reversible: true, ..bounded() });
        let mut snapshots = vec![e.snapshot()];
        while e.step().is_none() {
            snapshots.push(e.snapshot());
        }
        for (index, expected) in snapshots.iter().enumerate().rev() {
            if index + 1 < snapshots.len() || e.outcome().is_some() {
                assert_eq!(e.step_back(), Ok(true), "{call}, step {index}");
            }
            assert_same_state(&e.snapshot(), expected, &format!("{call}, back to step {index}"));
        }
    }
}

/// `apply @f(%q) -> %out, %fault budget B` and the idiom `%h = mark budget B + 3; %y, %c = call @f(%q)
/// budget B; rewind %h keep (%y -> %out, %c -> %fault)` (the mark's budget counts the call, B steps of the
/// callee, a catch and the rewind) give the same state, on every basis value of the
/// argument and on all at once, with dirty output and fault registers, the callee faulting for some
/// values (a division by the argument, a loop longer than the budget). On the machine both go on; in strict
/// mode both stop where only some branches fault.
#[test]
fn apply_is_mark_call_with_a_budget_and_rewind_keep() {
    let callee = "func @f(i2 %v) -> i2 {
entry:
    %d = udiv i2 3, %v
    br loop(%v)
loop(i2 %k):
    %z = icmp eq i2 %k, 0
    %k1 = sub i2 %k, 1
    br %z, done, loop(%k1)
done:
    ret %d
}";
    // f(v) takes 3v + 6 steps for v > 0: 9, 12, 15; v = 0 faults at once.
    let main = |prepare: &str, body: &str| {
        format!(
            "memory 256\n{callee}\nfunc @main() {{\nentry:\n%q = qalloc 2\n%out = qalloc 2\n%fault = qalloc 5\n\
             x %out[1]\nx %fault[2]\n{prepare}{body}ret\n}}"
        )
    };
    let mut compared = 0;
    for budget in [8u64, 9, 12, 14, 15, 20] {
        let apply = format!("apply @f(%q) -> %out, %fault budget {budget}\n");
        let idiom = format!(
            "%h = mark budget {}\n%y, %c = call @f(%q) budget {budget}\nrewind %h keep (%y -> %out, %c -> %fault)\n",
            budget + 3
        );
        for prepare in ["", "x %q[0]\n", "x %q[1]\n", "x %q[0]\nx %q[1]\n", "h %q[0]\nh %q[1]\n"] {
            let (a, b) = (load(&main(prepare, &apply)), load(&main(prepare, &idiom)));
            let machine = Options { machine_meaning: true, reversible: true, ..bounded() };
            let (outcome_a, ea) = common::run(&a, "main", &[], machine);
            let (outcome_b, eb) = common::run(&b, "main", &[], machine);
            let context = format!("budget {budget}, {prepare:?}");
            assert_eq!((outcome_a, outcome_b), (Outcome::Returned(None), Outcome::Returned(None)), "{context}");
            let distance = ea.snapshot().quantum.distance(&eb.snapshot().quantum);
            assert!(distance.is_some_and(|d| d <= 1e-12), "{context}: {distance:?}");
            let strict = Options { reversible: true, ..bounded() };
            let (strict_a, _) = common::run(&a, "main", &[], strict);
            let (strict_b, _) = common::run(&b, "main", &[], strict);
            let stopped_a = matches!(strict_a, Outcome::Stopped(RunError::RegionFault { .. }));
            let stopped_b = matches!(strict_b, Outcome::Stopped(RunError::WorldFault { .. }));
            // A world faulting alone stops the idiom; all branches faulting alike is one world's fault code.
            let some = prepare.starts_with('h');
            assert_eq!(stopped_b, stopped_a && some, "{context}: {strict_a:?} {strict_b:?}");
            compared += 1;
        }
    }
    assert_eq!(compared, 30);
}

/// A call with a budget inside an applied function: both ways of computing `apply` agree (compare mode stops
/// at any difference), the inner call's fault a value inside the region: `@g(v)` returns `@count(v)`'s value
/// plus its code, so 7 for v = 0, 1 (5 and 8 steps within the budget 8) and 1 (`budget`) for v = 2, 3.
#[test]
fn a_call_with_a_budget_inside_a_region_agrees_both_ways() {
    let text = format!(
        "{CALLEES}func @g(i2 %v) -> i8 {{
entry:
    %n = zext i2 %v to i8
    %y, %c = call @count(%n) budget 8
    %cz = zext i5 %c to i8
    %s = add i8 %y, %cz
    ret %s
}}
func @main() -> i8 {{
entry:
    %q = qalloc 2
    PREPARE
    %out = qalloc 8
    %fault = qalloc 5
    apply @g(%q) -> %out, %fault budget 100
    %m = measure %out
    ret %m
}}"
    );
    for (prepare, expected) in [("", 7), ("x %q[0]", 7), ("x %q[1]", 1), ("x %q[0]\n    x %q[1]", 1)] {
        let module = load(&text.replace("    PREPARE", &format!("    {prepare}")));
        let mut e = executor(&module, "main", &[], bounded());
        assert_eq!(e.run(), Outcome::Returned(Some(expected)), "{prepare}");
    }
}

/// A rewind inside the callee does not give its budget back: `@rewinding` marks, adds, rewinds the addition
/// and loops, four steps a round that never go back, so under a budget B it gets `budget` after exactly B
/// steps, as an applied function would.
#[test]
fn a_rewind_inside_the_callee_gives_no_budget_back() {
    for budget in [1u64, 4, 5, 17, 40] {
        let (values, ticks, _) = case(&format!("call @rewinding() budget {budget}"));
        assert_eq!((values, ticks), ((0, u128::from(Fault::Budget.code())), 1 + budget + 1 + 5), "{budget}");
    }
}
