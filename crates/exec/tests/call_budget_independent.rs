//! Independent calculations for machine step e. Requirements: HANDOFF's afternoon
//! decisions and design audition of 2026-11-16; DESIGN, "The call with a fault value".

mod common;

use bennett_exec::{ApplyMode, Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};
use num_complex::Complex64 as C;

fn machine() -> Options {
    Options { machine_meaning: true, reversible: true, ..bounded() }
}

/// One bad branch out of four: the code is ordinary data. Independent result is
/// sum_x i^(x&1)|x, x XOR 2, 0>/2 for x!=2, and |2, 2, divide_by_zero>/2
/// for x=2. The input's relative phases and the three success amplitudes survive.
#[test]
fn a_branch_fault_is_coherent_data_and_is_measurable() {
    let callee = "func @f(i2 %x) -> i2 {
entry:
    %bad = icmp eq i2 %x, 2
    br %bad, fail, good
fail:
    fault divide_by_zero
good:
    ret %x
}";
    let main = |body: &str, finish: &str| {
        format!(
            "memory 1024
{callee}
func @main() -> i5 {{
entry:
    %q = qalloc 2
    h %q[0]
    s %q[0]
    h %q[1]
    %out = qalloc 2
    x %out[1]
    %code = qalloc 5
    {body}
    {finish}
}}"
        )
    };
    let idiom = "%m = mark budget 23
    %y, %c = call @f(%q) budget 20
    rewind %m keep (%y -> %out, %c -> %code)";
    let apply = "apply @f(%q) -> %out, %code budget 20";
    for body in [idiom, apply] {
        let module = load(&main(body, "ret 0"));
        for mode in [ApplyMode::History, ApplyMode::Compare] {
            let mut e = executor(&module, "main", &[], machine());
            e.set_apply_mode(mode);
            assert_eq!(e.run(), Outcome::Returned(Some(0)));
            let state = e.snapshot().quantum;
            assert_eq!(state.amplitudes.len(), 4);
            for x in 0..4 {
                let (out, code) = if x == 2 { (2, Fault::DivideByZero.code()) } else { (x ^ 2, 0) };
                let amplitude = if x & 1 == 0 { C::new(0.5, 0.0) } else { C::new(0.0, 0.5) };
                assert!((state.amplitude(&[x, out, code]) - amplitude).norm() < 1e-12, "{body}: x={x}");
            }
        }
        let measured = load(&main(body, "%v = measure %code\n    ret %v"));
        let mut faults = 0;
        for seed in 0..256 {
            match common::run(&measured, "main", &[], Options { seed, ..machine() }).0 {
                Outcome::Returned(Some(0)) => {}
                Outcome::Returned(Some(n)) if n == u128::from(Fault::DivideByZero.code()) => faults += 1,
                other => panic!("{other:?}"),
            }
        }
        // Independent Born weight is 1/4; six standard deviations for the samples.
        assert!((faults as f64 / 256.0 - 0.25).abs() < 6.0 * (0.25f64 * 0.75 / 256.0).sqrt());
        let stopped = common::run(&module, "main", &[], bounded()).0;
        assert!(
            matches!(stopped, Outcome::Stopped(RunError::WorldFault { .. } | RunError::RegionFault { .. })),
            "{stopped:?}"
        );
    }
}

/// Independent instruction counts: a branch-select plus ret (2) for n=0;
/// loop, alloca loop, journal-growing store loop, recursive calls for n=1.
/// Their lengths are respectively 9, 11, 11, 17. B one below the length
/// must produce a coherent budget code only on n=1, never truncate n=0.
#[test]
fn branch_dependent_frames_allocations_history_and_loop_trips_are_bounded() {
    let bodies = [
        (
            "br loop(i2 2)\nloop(i2 %k):\n%next = sub i2 %k, 1\n%more = icmp ne i2 %next, 0\nbr %more, loop(%next), done",
            9,
        ),
        (
            "br loop(i2 2)\nloop(i2 %k):\n%p = alloca 8\n%next = sub i2 %k, 1\n%more = icmp ne i2 %next, 0\nbr %more, loop(%next), done",
            11,
        ),
        (
            "br loop(i2 2)\nloop(i2 %k):\nstore i8 1, 16\n%next = sub i2 %k, 1\n%more = icmp ne i2 %next, 0\nbr %more, loop(%next), done",
            11,
        ),
        ("%r = call @depth(i2 2)\nbr done", 17),
    ];
    for (index, (body, length)) in bodies.iter().enumerate() {
        for budget in [length - 1, *length, length + 1] {
            let module = load(&format!(
                "memory 4096
func @depth(i2 %n) -> i1 {{
entry:
    %zero = icmp eq i2 %n, 0
    br %zero, base, recurse
recurse:
    %next = sub i2 %n, 1
    %r = call @depth(%next)
    ret %r
base:
    ret 1
}}
func @f(i1 %n) -> i1 {{
entry:
    br %n, work, done
work:
    {body}
done:
    ret %n
}}
func @main() {{
entry:
    %q = qalloc 1
    h %q
    %out = qalloc 1
    %code = qalloc 5
    %m = mark budget 100
    %y, %c = call @f(%q) budget {budget}
    rewind %m keep (%y -> %out, %c -> %code)
    ret
}}"
            ));
            let (outcome, e) = common::run(&module, "main", &[], machine());
            assert_eq!(outcome, Outcome::Returned(None), "case {index}, B={budget}");
            let state = e.snapshot().quantum;
            let half = C::new(std::f64::consts::FRAC_1_SQRT_2, 0.0);
            assert_eq!(state.amplitudes.len(), 2);
            assert!((state.amplitude(&[0, 0, 0]) - half).norm() < 1e-12);
            let expected = if budget < *length { [1, 0, Fault::Budget.code()] } else { [1, 1, 0] };
            assert!((state.amplitude(&expected) - half).norm() < 1e-12, "case {index}, B={budget}: {state:?}");
        }
    }
}

/// At the next step both the outer call's guard and the inner mark have expired.
/// The documented outermost-spent-call rule must return Budget to main. Neither
/// a mark deadline nor the nested catch may subtract from an already-zero guard.
#[test]
#[ignore = "defect: simultaneous mark and outer-call expiry underflows the outer guard"]
fn simultaneous_mark_and_outer_call_expiry_returns_budget() {
    let module = load(
        "memory 512
func @g() {
entry:
    %h = mark budget 0
    ret
}
func @f() {
entry:
    %inner = call @g() budget 100
    ret
}
func @main() -> i5 {
entry:
    %code = call @f() budget 2
    ret %code
}",
    );
    assert_eq!(common::run(&module, "main", &[], machine()).0, Outcome::Returned(Some(Fault::Budget.code().into())));
}

/// Budgeted calls inherit the region rule: quantum instructions fault
/// quantum_in_region before any state-dependent diagnostic can be evaluated.
/// A dirty qfree, an apply that would fault on a branch, and a mismatched rewind
/// keep are therefore forbidden operations, not simulator diagnostic codes.
#[test]
fn quantum_operations_under_a_call_cannot_deliver_simulator_diagnostics() {
    for instruction in ["qfree %q", "rewind %m keep (i1 1 -> %q)", "apply @bad() -> %q, %code budget 10"] {
        let module = load(&format!(
            "memory 1024
func @bad() -> i1 {{
entry:
    fault divide_by_zero
}}
func @f(q1 %q, q5 %code) {{
entry:
    %m = mark budget 10
    {instruction}
    ret
}}
func @main() -> i5 {{
entry:
    %q = qalloc 1
    h %q
    %code = qalloc 5
    %c = call @f(%q, %code) budget 20
    ret %c
}}"
        ));
        for options in [bounded(), machine()] {
            assert_eq!(
                common::run(&module, "main", &[], options).0,
                Outcome::Returned(Some(Fault::QuantumInRegion.code().into())),
                "{instruction}"
            );
        }
    }
}

/// A longjmp may cross ordinary frames inside the guarded activation. Hand count:
/// alloca/setjmp/test/branch, ordinary call, longjmp, test/branch, ret = 9.
/// It must preserve the guard through the jump and reverse every recorded step.
#[test]
fn longjmp_across_an_ordinary_frame_keeps_its_call_budget() {
    for budget in 7..=11 {
        let module = load(&format!(
            "memory 1024
func @jump(i64 %buf) {{
entry:
    longjmp %buf, 7
}}
func @f() -> i8 {{
entry:
    %buf = alloca 32
    %r = setjmp %buf
    %first = icmp eq i32 %r, 0
    br %first, go, back
go:
    call @jump(%buf)
    fault unreachable
back:
    ret 9
}}
func @main() -> i8 {{
entry:
    %v, %c = call @f() budget {budget}
    %cz = zext i5 %c to i8
    %sum = add i8 %v, %cz
    ret %sum
}}"
        ));
        let mut e = executor(&module, "main", &[], machine());
        let start = e.snapshot();
        let expected = if budget >= 9 { 9 } else { u128::from(Fault::Budget.code()) };
        assert_eq!(e.run(), Outcome::Returned(Some(expected)), "B={budget}");
        e.reverse().expect("all call, longjmp and catch records reverse");
        assert_same_state(&e.snapshot(), &start, &format!("B={budget}"));
    }
}

/// An apply's budget covers an unbudgeted inner mark that reads a memory qubit
/// (DESIGN, "The mark's budget"). Its documented expansion must cover it too:
/// mark budget B+3; call budget B; rewind keep. Expected: byte 16 in |+>,
/// output 7, fault 0, calculated by copy/uncompute with no changed read.
#[test]
#[ignore = "defect: call budget does not cover an inner unbudgeted quantum read as apply does"]
fn call_budget_covers_an_inner_unbudgeted_quantum_read_like_apply() {
    let wrapper = |body: &str| {
        format!(
            "memory 1024
func @f() -> i8 {{
entry:
    %h = mark
    %v = load i8 16
    rewind %h
    ret 7
}}
func @main() {{
entry:
    h [16:0]
    %out = qalloc 8
    %fault = qalloc 5
    {body}
    ret
}}"
        )
    };
    for body in [
        "apply @f() -> %out, %fault budget 20",
        "%m = mark budget 23\n    %y, %c = call @f() budget 20\n    rewind %m keep (%y -> %out, %c -> %fault)",
    ] {
        let module = load(&wrapper(body));
        let (outcome, e) = common::run(&module, "main", &[], machine());
        assert_eq!(outcome, Outcome::Returned(None), "{body}");
        let q = e.snapshot().quantum;
        assert_eq!(q.amplitudes.len(), 2);
        for bit in 0..2 {
            assert!(
                (q.amplitude(&[bit, 7, 0]) - C::new(std::f64::consts::FRAC_1_SQRT_2, 0.0)).norm() < 1e-12,
                "{body}: {q:?}"
            );
        }
    }
}

/// All branches execute the same call site and reserve identical caller slots.
/// The callee's frame cannot fit; apply must XOR stack_overflow into fault, and
/// so must its call-budget expansion (DESIGN, "The call with a fault value").
#[test]
#[ignore = "defect: call-budget entry stack overflow ends the world instead of returning its code"]
fn a_callee_frame_that_does_not_fit_returns_a_call_fault() {
    let module = load(
        "memory 128
func @f() -> i8 {
entry:
    %a = add i8 1, 0
    %b = add i8 %a, 0
    %c = add i8 %b, 0
    %d = add i8 %c, 0
    %e = add i8 %d, 0
    %f = add i8 %e, 0
    %g = add i8 %f, 0
    %h = add i8 %g, 0
    %i = add i8 %h, 0
    %j = add i8 %i, 0
    ret %j
}
func @main(i1 %idiom) -> i5 {
entry:
    %out = qalloc 8
    %fault = qalloc 5
    br %idiom, expanded, direct
expanded:
    %m = mark budget 23
    %y, %c = call @f() budget 20
    rewind %m keep (%y -> %out, %c -> %fault)
    br done
direct:
    apply @f() -> %out, %fault budget 20
    br done
done:
    %v = measure %fault
    ret %v
}",
    );
    for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
        for path in [0, 1] {
            let mut e = executor(&module, "main", &[path], machine());
            e.set_apply_mode(mode);
            assert_eq!(e.run(), Outcome::Returned(Some(Fault::StackOverflow.code().into())), "{mode:?}, path {path}");
        }
    }
}

/// Reusing one mark makes replay exponential (HANDOFF audition, "Exponential
/// costs"). A guarded call still has a finite bound. For any finite B this
/// non-returning callee must deliver budget, then reverse exactly; no formula
/// for its implementation's work is assumed.
#[test]
fn repeated_rewind_replay_inside_a_call_is_bounded_and_reverses() {
    for budget in 0..64 {
        let module = load(&format!(
            "memory 512
func @f() {{
entry:
    %m = mark
    br loop
loop:
    rewind %m
    br loop
}}
func @main() -> i5 {{
entry:
    %c = call @f() budget {budget}
    ret %c
}}"
        ));
        let mut e = executor(&module, "main", &[], Options { budget: Some(500), ..machine() });
        let before = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(Some(Fault::Budget.code().into())), "B={budget}");
        e.reverse().expect("replay and catches reverse");
        assert_same_state(&e.snapshot(), &before, &format!("B={budget}"));
    }
}
