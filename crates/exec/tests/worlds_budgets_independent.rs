//! Second implementation from DESIGN's History, mark budget, budgeted call, Strict mode and SYNTAX.
//! No production interpreter internals are used. Every comparison also checks reversal.
mod common;
use bennett_exec::{Executor, Options, Outcome, RunError};
use bennett_ir::Fault;
use common::oracle_support::worlds::{Finish, Model, basis};
use common::oracle_support::worlds_executor::{assert_observed, assert_snapshot, observed_reference};

fn exact(text: &str, entry: &str, args: &[u64], machine: bool) {
    let p = common::load(text);
    let mut model = Model::new(&p, entry);
    model.machine_meaning = machine;
    let mut state = basis(model.initial(entry, &args.iter().map(|&x| x.into()).collect::<Vec<_>>()));
    let mut e = Executor::new(
        &p,
        p.function_named(entry).expect("valid fixture"),
        args,
        Options { reversible: true, machine_meaning: machine, budget: Some(1000), ..common::bounded() },
    )
    .expect("valid fixture");
    let mut states = vec![state.clone()];
    for _ in 0..1000 {
        assert_snapshot(&model, &state, &e);
        if e.outcome().is_none() || matches!(e.outcome(), Some(Outcome::Returned(_))) {
            assert_eq!(model.clock(&state), e.stats().clock, "forward clock, {entry}{args:?}");
        }
        if e.outcome().is_some() {
            break;
        }
        state = model.step(&state);
        e.step();
        states.push(state.clone());
    }
    assert!(e.outcome().is_some(), "bounded fixture finishes");
    // A fault without a catch changes no state and has no journal record.
    if matches!(e.outcome(), Some(Outcome::Faulted(_))) {
        states.pop();
    }
    for previous in states.iter().rev().skip(1) {
        assert_eq!(e.step_back(), Ok(true));
        assert_snapshot(&model, previous, &e);
    }
    e.reverse().expect("complete executor reversal");
    assert_snapshot(&model, &states[0], &e);
    assert_eq!(e.stats().clock, 0);
    // Independent inversion, including catches and all dead slots/history.
    while state.keys().any(|w| !w.history.is_empty()) {
        state = model.undo_state(&state).expect("reference inverse");
    }
    assert_eq!(state.keys().collect::<Vec<_>>(), states[0].keys().collect::<Vec<_>>());
    for (w, a) in state {
        assert!((a - states[0][&w]).norm() < 1e-12);
    }
}

const COUNT: &str = "func @count(i8 %n) -> i8 {
entry:
 br loop(%n)
loop(i8 %k):
 %z = icmp eq i8 %k, 0
 %next = sub i8 %k, 1
 br %z, done, loop(%next)
done:
 ret 7
}
";

#[test]
fn finite_call_budgets_exhaustive_small_inputs_and_boundaries() {
    for budget in 0..20 {
        let text = format!(
            "memory 512\n{COUNT}func @main(i8 %n) -> i16 {{\nentry:\n%y, %c = call @count(%n) budget {budget}\n%a = zext i8 %y to i16\n%b = zext i5 %c to i16\n%c8 = shl i16 %b, 8\n%r = or i16 %a, %c8\nret %r\n}}"
        );
        for n in 0..4 {
            exact(&text, "main", &[n], false);
        }
    }
}

#[test]
fn finite_mark_budgets_read_and_no_read_and_nested_deadlines() {
    for budget in 0..16 {
        for arg in ["%q", "i8 2"] {
            let text = format!(
                "memory 512\n{COUNT}func @main() {{\nentry:\n%q = qalloc 8\nx %q[1]\n%h = mark budget {budget}\n%r = call @count({arg})\nrewind %h\nret\n}}"
            );
            exact(&text, "main", &[], false);
        }
    }
    for outer in 1..10 {
        exact(
            &format!(
                "memory 256\nfunc @id(i1 %v) {{\nentry:\nret\n}}\nfunc @main() {{\nentry:\n%q = qalloc 1\n%a = mark budget {outer}\n%b = mark budget 5\ncall @id(%q)\nrewind %b\nrewind %a\nret\n}}"
            ),
            "main",
            &[],
            false,
        );
    }
}

#[test]
fn faults_nested_calls_longjmp_and_rewinds() {
    let callees = format!(
        "{COUNT}
func @inner(i8 %n) -> i8 {{
entry:
 %y, %c = call @count(%n) budget 8
 %cz = zext i5 %c to i8
 %r = add i8 %y, %cz
 ret %r
}}
func @rewind() -> i8 {{
entry:
 br loop
loop:
 %h = mark
 %v = add i8 1, 2
 rewind %h
 br loop
}}
func @jump(i64 %p) -> i8 {{
entry:
 longjmp %p, 1
}}
func @own() -> i8 {{
entry:
 %p = alloca 32
 %j = setjmp %p
 %z = icmp eq i32 %j, 0
 br %z, jump, done
jump:
 call @jump(%p)
 ret 0
done:
 ret 9
}}"
    );
    for call in [
        "call @inner(i8 2) budget 12",
        "call @inner(i8 2) budget 13",
        "call @own() budget 20",
        "call @rewind() budget 17",
        "call @jump(%p) budget 10",
    ] {
        let text = format!(
            "memory 1024\n{callees}\nfunc @main() -> i5 {{\nentry:\n%p = alloca 32\n%j = setjmp %p\n%y, %c = {call}\nret %c\n}}"
        );
        exact(&text, "main", &[], false);
    }
    for body in
        ["%r = udiv i8 1, 0\nret %r", "effect 65\nret 0", "exit i8 3", "%q = qalloc 1\nret 0", "fault unreachable"]
    {
        exact(
            &format!(
                "memory 256\nfunc @f() -> i8 {{\nentry:\n{body}\n}}\nfunc @main() -> i5 {{\nentry:\n%y, %c = call @f() budget 10\nret %c\n}}"
            ),
            "main",
            &[],
            false,
        );
    }
}

#[test]
fn branch_local_budget_faults_and_codes_match_born_support() {
    for budget in 0..16 {
        for mark in [false, true] {
            let body = if mark {
                format!("%h = mark budget {budget}\n%v = call @count(%q)\nrewind %h\nret i8 7")
            } else {
                format!("%v, %c = call @count(%q) budget {budget}\n%cz = zext i5 %c to i8\n%r = add i8 %v, %cz\nret %r")
            };
            let text = format!("memory 512\n{COUNT}func @main() -> i8 {{\nentry:\n%q = qalloc 8\nh %q[0]\n{body}\n}}")
                .replace("ret i8 7", "ret 7");
            let p = common::load(&text);
            let model = Model::new(&p, "main");
            let state = observed_reference(&model, basis(model.initial("main", &[])), 100);
            for seed in 0..8 {
                let mut e = Executor::new(
                    &p,
                    p.function_named("main").expect("valid fixture"),
                    &[],
                    Options { seed, reversible: true, machine_meaning: true, ..common::bounded() },
                )
                .expect("valid fixture");
                e.run();
                assert_observed(&model, &state, &e);
                // Host selection of a world is a Born boundary, like measurement.
                match e.reverse() {
                    Ok(_) => assert_snapshot(&model, &basis(model.initial("main", &[])), &e),
                    Err(error) => assert_eq!(error, bennett_exec::ReverseError::PastMeasurement),
                }
            }
            let mut strict = Executor::new(
                &p,
                p.function_named("main").expect("valid fixture"),
                &[],
                Options { reversible: true, ..common::bounded() },
            )
            .expect("valid fixture");
            let outcome = strict.run();
            let mut reference = basis(model.initial("main", &[]));
            let mut diagnostic = None;
            for _ in 0..100 {
                let next = model.step(&reference);
                diagnostic = model.strict_stop(&reference, &next);
                reference = next;
                if diagnostic.is_some() || reference.keys().all(|w| w.classical.finish.is_some()) {
                    break;
                }
            }
            // A rewind waits for the other path. Ordinary caught calls need no such barrier;
            // the B=7 catch and sibling return share a round, whose order is not specified.
            if mark || budget != 7 {
                let stop = if mark {
                    budget > 0
                        && state.keys().any(|w| {
                            w.classical.finish == Some(common::oracle_support::worlds::Finish::Faulted(Fault::Budget))
                        })
                } else {
                    diagnostic.is_some()
                };
                assert_eq!(
                    matches!(outcome, Outcome::Stopped(RunError::WorldFault { fault: Fault::Budget, .. })),
                    stop,
                    "{budget}, {mark}: {outcome:?}, {diagnostic:?}"
                );
            }
        }
    }
}

#[test]
fn strict_and_machine_meaning_dirty_frees_and_stale_handles() {
    for end in ["%r = qalloc 1\nx %r\nret", "x %q\nret"] {
        let text = format!(
            "memory 256\nfunc @main() {{\nentry:\n%q = qalloc 2\nh %q[0]\nx %q[1] ctrl %q[0]\nqfree %q\n{end}\n}}"
        );
        for machine in [false, true] {
            exact(&text, "main", &[], machine);
        }
    }
    exact(
        "memory 256\nfunc @main() {\nentry:\n%h = mark\n%t = qalloc 1\nh %t\nqfree %t\nrewind %h\nret\n}",
        "main",
        &[],
        true,
    );
}

/// Since the design seam's V1 was fixed (2026-11-16, machine-finish 0d) the leftover is no barrier on the
/// machine: undoing the rewind takes it back (`crates/exec/tests/leftover_wires.rs`, each state worked by
/// hand). This reference still models the earlier barrier, so on the machine it is compared up to the
/// inner rewind only; the outer rewind and reversal are checked against the hand-worked states there. Route:
/// model the take-back in `oracle_support::worlds`.
#[test]
fn nonzero_allocation_left_over_and_the_outer_rewind() {
    for outer in ["", "rewind %outer"] {
        let text = format!(
            "memory 256\nfunc @main() {{\nentry:\n%q = qalloc 1\nh %q\n%outer = mark\n%h = mark\n%t = qalloc 1\nx %t ctrl %q\nrewind %h keep (i1 1 -> %q)\n{outer}\nret\n}}"
        );
        let p = common::load(&text);
        for machine in [false, true] {
            let mut m = Model::new(&p, "main");
            m.machine_meaning = machine;
            let state = m.run(basis(m.initial("main", &[])), 20);
            let mut e = Executor::new(
                &p,
                p.function_named("main").expect("entry"),
                &[],
                Options { machine_meaning: machine, reversible: true, ..common::bounded() },
            )
            .expect("starts");
            let entry = e.snapshot();
            e.run();
            if machine {
                // The unfaithful inverse's quantum result is unspecified. Compare only the promised
                // leftover and completion, without pinning amplitudes; then the run reverses to its entry.
                let expected_leftovers = if outer.is_empty() { 1 } else { 0 };
                assert_eq!(e.stats().leftover_qubits, expected_leftovers);
                assert_eq!(e.outcome(), Some(Outcome::Returned(None)));
                if outer.is_empty() {
                    assert!(state.keys().all(|w| w.classical.time.leftovers.len() == 1));
                }
                e.reverse().expect("the leftover is taken back on the way");
                common::assert_same_state(&e.snapshot(), &entry, "back to the entry");
            } else {
                assert_snapshot(&m, &state, &e);
                e.reverse().expect("strict refusal reverses");
            }
        }
    }
}

#[test]
fn register_and_memory_reads_require_a_budget_and_wait_only_when_read() {
    for mark in ["mark", "mark budget 2", "mark budget 10", "mark budget 32"] {
        for read in ["%v = load i8 16", "%v = add i8 1, 1"] {
            exact(
                &format!("memory 256\nfunc @main() {{\nentry:\nx [16:0]\n%h = {mark}\n{read}\nrewind %h\nret\n}}"),
                "main",
                &[],
                false,
            );
        }
    }
}

#[test]
fn replay_is_charged_to_the_run_and_budget_failure_is_atomic() {
    let p =
        common::load("memory 256\nfunc @main() {\nentry:\n%h = mark budget 256\nbr loop\nloop:\nrewind %h\nbr loop\n}");
    for budget in 0..64 {
        let m = Model::new(&p, "main");
        let initial = basis(m.initial("main", &[]));
        let expected = m.run_budgeted(initial.clone(), budget);
        let mut e = Executor::new(
            &p,
            p.function_named("main").expect("entry"),
            &[],
            Options { reversible: true, budget: Some(budget as u64), ..common::bounded() },
        )
        .expect("starts");
        assert_eq!(e.run(), Outcome::Faulted(Fault::Budget));
        assert_snapshot(&m, &expected, &e);
        assert_eq!(m.replayed(), e.stats().replay_charged, "budget {budget}");
        e.reverse().expect("finite prefix reverses");
        assert_snapshot(&m, &initial, &e);
    }
}

#[test]
fn guarded_call_fault_codes_interfere_after_a_budgeted_rewind() {
    for budget in 0..17 {
        for prepare in ["x %q[0]", "h %q[0]", "h %q[0]\ns %q[0]\nh %q[1]"] {
            let text = format!(
                "memory 512\n{COUNT}func @main() {{\nentry:\n%q = qalloc 8\n%out = qalloc 8\n%fault = qalloc 5\nx %out[2]\nx %fault[3]\n{prepare}\n%h = mark budget {}\n%v, %c = call @count(%q) budget {budget}\nrewind %h keep (%v -> %out, %c -> %fault)\nh %q[0]\nret\n}}",
                budget + 3
            );
            let p = common::load(&text);
            let m = Model::new(&p, "main");
            let initial = basis(m.initial("main", &[]));
            let expected = m.run_joined(initial.clone(), 100);
            let mut e = Executor::new(
                &p,
                p.function_named("main").expect("entry"),
                &[],
                Options { reversible: true, machine_meaning: true, ..common::bounded() },
            )
            .expect("starts");
            assert_eq!(e.run(), Outcome::Returned(None), "B={budget}, {prepare}");
            assert_snapshot(&m, &expected, &e);
            assert_eq!(m.clock(&expected), e.stats().clock, "B={budget}, {prepare}");
            e.reverse().expect("coherent fault/value uncomputation reverses");
            assert_snapshot(&m, &initial, &e);
            let reversed = m
                .reverse_joined(expected)
                .unwrap_or_else(|f| panic!("reference reverses, B={budget}, {prepare}: {f:?}"));
            assert_eq!(reversed.keys().collect::<Vec<_>>(), initial.keys().collect::<Vec<_>>());
        }
    }
}

#[test]
fn finite_marks_and_guarded_calls_inside_apply_use_the_same_reference_rules() {
    for budget in 0..15 {
        let text = format!(
            "memory 512\n{COUNT}func @f(i8 %n) -> i8 {{\nentry:\n%h = mark budget {budget}\n%v, %c = call @count(%n) budget 8\nrewind %h\nret %n\n}}\nfunc @main() {{\nentry:\n%q = qalloc 8\nh %q[0]\n%out = qalloc 8\n%fault = qalloc 5\napply @f(%q) -> %out, %fault budget 50\nret\n}}"
        );
        // Reference meaning permits faults as values; strict region diagnostics are tested separately.
        exact(&text, "main", &[], true);
    }
}

#[test]
fn replay_that_newly_enters_an_endless_path_exhausts_the_run_budget() {
    let p = common::load(
        "memory 256
func @id(i1 %v) -> i1 {
entry:
 ret %v
}
func @main() {
entry:
 %q = qalloc 1
 %outer = mark budget 32
 %inner = mark budget 8
 %v = call @id(%q)
 br %v, spin, done
spin:
 br spin
done:
 rewind %inner
 rewind %outer keep (i1 1 -> %q)
 ret
}",
    );
    for budget in [32, 64, 96] {
        let m = Model::new(&p, "main");
        let initial = basis(m.initial("main", &[]));
        let expected = m.run_budgeted(initial.clone(), budget);
        let mut e = Executor::new(
            &p,
            p.function_named("main").expect("entry"),
            &[],
            Options { reversible: true, budget: Some(budget as u64), ..common::bounded() },
        )
        .expect("starts");
        assert_eq!(e.run(), Outcome::Faulted(Fault::Budget));
        assert_snapshot(&m, &expected, &e);
        e.reverse().expect("failed replay is atomic and reversible");
        assert_snapshot(&m, &initial, &e);
    }
}
#[test]
fn finite_mark_stops_before_a_write_past_its_deadline() {
    let p = bennett_ir::load(
        "memory 256\nfunc @main() {\nentry:\n%h = mark budget 1\n%v = add i8 1, 1\nstore i8 %v, 0\nret\n}\n",
    )
    .expect("valid");
    let m = Model::new(&p, "main");
    let s = m.run(basis(m.initial("main", &[])), 10);
    assert!(s.keys().all(|w| w.classical.finish == Some(Finish::Faulted(Fault::Budget)) && m.memory(w)[0] == 0));
}
#[test]
fn budgeted_call_delivers_a_fault_as_an_integer() {
    let p = bennett_ir::load("memory 256\nfunc @f() {\nentry:\nfault unreachable\n}\nfunc @main() -> i5 {\nentry:\n%c = call @f() budget 1\nret %c\n}\n").expect("valid");
    let m = Model::new(&p, "main");
    let s = m.run(basis(m.initial("main", &[])), 10);
    assert!(s.keys().all(|w| w.classical.finish == Some(Finish::Returned(Some(Fault::Unreachable.code().into())))));
}

#[test]
fn simultaneous_nested_exhaustion_catches_the_outermost_spent_guard() {
    for inner in 0..6 {
        for outer in 0..9 {
            let text = format!(
                "memory 512\n{COUNT}func @middle() -> i5 {{\nentry:\n%y, %c = call @count(i8 2) budget {inner}\nret %c\n}}\nfunc @main() -> i5 {{\nentry:\n%y, %c = call @middle() budget {outer}\n%r = or i5 %y, %c\nret %r\n}}"
            );
            exact(&text, "main", &[], false);
        }
    }
    for machine in [false, true] {
        exact(
            "memory 256\nfunc @f(q1 %q) {\nentry:\nqfree %q\nret\n}\nfunc @main() -> i5 {\nentry:\n%q = qalloc 1\nx %q\n%c = call @f(%q) budget 10\nret %c\n}",
            "main",
            &[],
            machine,
        );
    }
}

#[test]
fn strict_region_diagnostics_are_separate_from_program_fault_values() {
    for body in ["%r = udiv i8 1, %v\nret %r", "effect 65\nret 0", "%r = qalloc 1\nret 0"] {
        let text = format!(
            "memory 256\nfunc @f(i8 %v) -> i8 {{\nentry:\n{body}\n}}\nfunc @main() {{\nentry:\n%q = qalloc 8\nh %q[0]\n%out = qalloc 8\n%fault = qalloc 5\napply @f(%q) -> %out, %fault budget 10\nret\n}}"
        );
        let p = common::load(&text);
        let m = Model::new(&p, "main");
        let initial = basis(m.initial("main", &[]));
        let mut state = initial.clone();
        let mut e = Executor::new(
            &p,
            p.function_named("main").expect("entry"),
            &[],
            Options { reversible: true, ..common::bounded() },
        )
        .expect("starts");
        loop {
            let next = m.step(&state);
            let diagnostic = m.strict_stop(&state, &next);
            let outcome = e.step();
            if let Some(common::oracle_support::worlds::Diagnostic::Region(_)) = diagnostic {
                assert!(matches!(outcome, Some(Outcome::Stopped(RunError::RegionFault { .. }))));
                let mut snapshot = e.snapshot();
                snapshot.classical.outcome = None;
                common::oracle_support::worlds_executor::assert_worlds(&m, &state, &snapshot);
                e.reverse().expect("strict diagnostic reverses");
                assert_snapshot(&m, &initial, &e);
                break;
            }
            assert!(outcome.is_none(), "diagnostic must precede return");
            state = next;
        }
        exact(&text, "main", &[], true);
    }
}

#[test]
fn an_in_callee_rewind_does_not_erase_branch_local_guard_consumption() {
    for budget in 5..12 {
        let text = internal_rewind_program(budget, 64);
        let p = common::load(&text);
        let m = Model::new(&p, "main");
        let initial = basis(m.initial("main", &[]));
        let expected = m.run_joined(initial.clone(), 100);
        let mut e = Executor::new(
            &p,
            p.function_named("main").expect("entry"),
            &[],
            Options { reversible: true, machine_meaning: true, ..common::bounded() },
        )
        .expect("starts");
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_eq!(m.clock(&expected), e.stats().clock, "inner rewind, budget {budget}");
        assert_eq!(m.replayed(), e.stats().replay_charged, "inner rewind replay, budget {budget}");
        assert_snapshot(&m, &expected, &e);
        let reversed = m.reverse_joined(expected.clone()).expect("reference branch budgets reverse");
        assert_eq!(reversed.keys().collect::<Vec<_>>(), initial.keys().collect::<Vec<_>>());
        e.reverse().expect("branch budgets reverse");
        assert_snapshot(&m, &initial, &e);
    }
}

fn internal_rewind_program(budget: u64, mark_budget: u64) -> String {
    format!(
        "memory 512
func @work() -> i8 {{
entry:
 %h = mark budget 10
 %v = load i8 0
 %z = icmp eq i8 %v, 0
 br %z, done, long
long:
 %w = add i8 1, 1
 br done
done:
 rewind %h
 %a = add i8 1, 1
 ret 7
}}
func @main() {{
entry:
 h [0:0]
 %out = qalloc 8
 %fault = qalloc 5
 %outer = mark budget {}
 %y, %c = call @work() budget {budget}
 rewind %outer keep (%y -> %out, %c -> %fault)
 ret
}}",
        mark_budget
    )
}

/// This records a composition the documents do not settle, without declaring an executor defect.
#[test]
#[ignore = "specification: composition of a callee's padded clock with its caller's mark is unclear"]
fn caller_mark_and_padded_callee_clock_need_a_composition_rule() {
    let p = common::load(&internal_rewind_program(5, 8));
    let m = Model::new(&p, "main");
    let expected = m.run_joined(basis(m.initial("main", &[])), 100);
    let mut e = Executor::new(
        &p,
        p.function_named("main").expect("entry"),
        &[],
        Options { reversible: true, machine_meaning: true, ..common::bounded() },
    )
    .expect("starts");
    e.run();
    assert_snapshot(&m, &expected, &e);
}

#[test]
fn replay_charges_a_guard_budget_catch_as_a_step() {
    let p = common::load(
        "memory 256\nfunc @f() {\nentry:\nret\n}\nfunc @main() {\nentry:\n%outer = mark budget 40\n%inner = mark budget 20\n%c = call @f() budget 0\nrewind %inner\nrewind %outer\nret\n}\n",
    );
    for budget in 0..13 {
        let m = Model::new(&p, "main");
        let initial = basis(m.initial("main", &[]));
        let expected = m.run_budgeted(initial.clone(), budget);
        let mut e = Executor::new(
            &p,
            p.function_named("main").expect("entry"),
            &[],
            Options { reversible: true, budget: Some(budget as u64), ..common::bounded() },
        )
        .expect("starts");
        e.run();
        assert_snapshot(&m, &expected, &e);
        assert_eq!(m.replayed(), e.stats().replay_charged, "catch replay, budget {budget}");
        e.reverse().expect("catch replay reverses");
        assert_snapshot(&m, &initial, &e);
    }
}

const COLLECTIVE_JUMP: &str = "memory 512
func @save(i1 %prepare) -> i5 {
entry:
 br %prepare, save, jump
save:
 %r = setjmp 0
 ret 0
jump:
 longjmp 0, 1
}
func @main() -> i5 {
entry:
 call @save(i1 1)
 h [0]
 %value, %code = call @save(i1 0) budget 8
 ret %code
}
";

#[test]
fn a_collective_jump_diagnostic_never_becomes_a_program_fault_value() {
    // The old record can match this activation (the documented stale-record behaviour).
    // Depth is 0 or 1: one record names the caller, the other the guarded callee itself.
    // M3 keeps the collective refusal for now; the afternoon's e6 never delivers it as a code.
    for (gate, code) in [("z [0]", 0), ("x [0]", Fault::BadLongjmp.code())] {
        let text = COLLECTIVE_JUMP.replace("h [0]", gate);
        let p = common::load(&text);
        let m = Model::new(&p, "main");
        let result = m.run(basis(m.initial("main", &[])), 32);
        assert!(result.keys().all(|w| w.classical.finish == Some(Finish::Returned(Some(code.into())))));
        exact(&text, "main", &[], false);
    }
    let p = common::load(COLLECTIVE_JUMP);
    let m = Model::new(&p, "main");
    let initial = basis(m.initial("main", &[]));
    let expected = m.run(initial.clone(), 32);
    assert!(expected.keys().all(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadLongjmp))));
    let reversed = m.reverse_joined(expected).expect("diagnostic boundary reverses");
    assert_eq!(reversed.keys().collect::<Vec<_>>(), initial.keys().collect::<Vec<_>>());
    for (w, a) in reversed {
        assert!((a - initial[&w]).norm() < 1e-12);
    }
}

/// The collective check inspects all configurations. Named bad_longjmp faults may be caught;
/// this simulator diagnostic must not become code 16, even in strict mode (HANDOFF, V4b).
/// cargo test -p bennett-exec --test worlds_budgets_independent collective_jump -- --ignored --nocapture
#[test]
#[ignore = "defect: a collective longjmp-record check becomes a budgeted call's fault value"]
fn collective_jump_diagnostic_is_not_delivered_by_a_budgeted_call() {
    let p = common::load(COLLECTIVE_JUMP);
    let m = Model::new(&p, "main");
    let expected = m.run(basis(m.initial("main", &[])), 32);
    assert!(expected.keys().all(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadLongjmp))));
    let mut e = Executor::new(
        &p,
        p.function_named("main").expect("entry"),
        &[],
        Options { reversible: true, ..common::bounded() },
    )
    .expect("starts");
    let initial = e.snapshot();
    let actual = e.run();
    e.reverse().expect("the diagnostic's precursor reverses");
    common::assert_same_state(&e.snapshot(), &initial, "collective check reverse");
    assert!(!matches!(actual, Outcome::Returned(_)), "collective diagnostic became a program value: {actual:?}");
}
