//! Copied call paths, their continuations and their shared budget.

use std::fmt::Write;

use super::{
    ApplyError, ApplyInput, ApplyMode, Construction, CountingDevice, Executor, Options, Outcome, RunError,
    compare_controls, compile_apply_with, control_example, load,
};

#[test]
fn saved_loop_after_a_copied_call_and_early_exit_shares_one_budget() {
    let module = control_example(
        r#"
memory 1024
func @f(i3 %x) -> i8 {
entry:
    %r = call @count(%x)
    %result = add i8 %r, 4
    ret %result
}
func @count(i3 %x) -> i8 {
entry:
    %large = icmp ugt i3 %x, 1
    br %large, loop(%x, i8 0), finish(i8 0)
loop(i3 %remaining, i8 %n):
    %next = sub i3 %remaining, 1
    %inc = add i8 %n, 1
    %more = icmp ugt i3 %next, 1
    br %more, loop(%next, %inc), finish(%inc)
finish(i8 %out):
    ret %out
}
"#,
        3,
        "",
    );
    for budget in 0..=31 {
        let (construction, _) = compare_controls(&module, budget, ApplyMode::Automatic, |x| {
            let passes = x.saturating_sub(1);
            if budget < 6 + 4 * passes { (0, 1) } else { (4 + passes, 0) }
        });
        if budget >= 3 {
            assert!(matches!(construction, Construction::SavedDecisions { .. }), "{construction}, B={budget}");
        }
    }
}

#[test]
fn copied_arguments_keep_proved_high_bits() {
    let module = control_example(
        r#"
memory 4096
func @f(i1 %x) -> i8 {
entry:
    %wide = zext i1 %x to i64
    %result = call @relay(%wide)
    ret %result
}
func @relay(i64 %wide) -> i8 {
entry:
    %high = lshr i64 %wide, 1
    %base = faddr @leaf
    %address = add i64 %base, %high
    %result = call_indirect fn(i64) -> i8 %address(%wide)
    ret %result
}
func @leaf(i64 %wide) -> i8 {
entry:
    %low = trunc i64 %wide to i8
    %result = add i8 %low, 7
    ret %result
}
"#,
        1,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=12 {
            compare_controls(&module, budget, mode, |x| if budget < 11 { (0, 1) } else { (7 + x, 0) });
        }
    }
}

#[test]
fn bypassed_indirect_calls_need_no_target_proof() {
    for condition in ["1", "%same"] {
        let source = format!(
            r#"
memory 4096
func @f(i1 %x) -> i8 {{
entry:
    br %x, choose, early
choose:
    %same = icmp eq i1 %x, %x
    br {condition}, done, cold
cold:
    %address = faddr @leaf
    %result = call_indirect fn() -> i8 %address()
    ret %result
early:
    ret 7
done:
    ret 8
}}
func @leaf() -> i8 {{
entry:
    ret 9
}}
"#
        );
        let module = control_example(&source, 1, "");
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in 0..=6 {
                compare_controls(&module, budget, mode, |x| if budget < 2 + 2 * x { (0, 1) } else { (7 + x, 0) });
            }
        }
    }
}

#[test]
fn two_call_sites_keep_distinct_continuations() {
    let module = control_example(
        r#"
memory 1024
func @f(i3 %x) -> i8 {
entry:
    %odd = trunc i3 %x to i1
    br %odd, left, right
left:
    %l = call @leaf(%x)
    %a = add i8 %l, 1
    ret %a
right:
    %addr = faddr @leaf
    %r = call @invoke(%addr, %x)
    %b = add i8 %r, 2
    ret %b
}
func @invoke(i64 %addr, i3 %x) -> i8 {
entry:
    %value = call_indirect fn(i3) -> i8 %addr(%x)
    ret %value
}
func @leaf(i3 %x) -> i8 {
entry:
    %value = zext i3 %x to i8
    ret %value
}
"#,
        3,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=11 {
            compare_controls(&module, budget, mode, |x| {
                let (steps, offset) = if x & 1 == 1 { (7, 1) } else { (10, 2) };
                if budget < steps { (0, 1) } else { (x + offset, 0) }
            });
        }
    }
}

#[test]
fn two_calls_at_one_site_share_the_clock_and_refresh_the_frame() {
    let module = control_example(
        r#"
memory 1024
func @f(i3 %x) -> i8 {
entry:
    br again(%x, i2 0)
again(i3 %arg, i2 %count):
    %value = call @update(%arg)
    %next = add i2 %count, 1
    %done = icmp eq i2 %next, 2
    br %done, finish, again(%value, %next)
finish:
    %out = zext i3 %value to i8
    ret %out
}
func @update(i3 %x) -> i3 {
entry:
    %odd = trunc i3 %x to i1
    br %odd, odd, even
odd:
    %plus_two = add i3 %x, 2
    ret %plus_two
even:
    %plus_one = add i3 %x, 1
    ret %plus_one
}
"#,
        3,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in [0, 8, 18, 19, 20] {
            let (construction, _) =
                compare_controls(
                    &module,
                    budget,
                    mode,
                    |x| {
                        if budget < 19 { (0, 1) } else { ((x + 3 + (x & 1)) & 7, 0) }
                    },
                );
            if budget >= 19 {
                match mode {
                    ApplyMode::Automatic => assert_eq!(construction, Construction::Controlled { proved_passes: 0 }),
                    ApplyMode::StepMachine => assert!(matches!(construction, Construction::StepMachine { .. })),
                }
            }
        }
    }
}

#[test]
fn frame_reservation_distinguishes_seven_and_eight_stack_bytes() {
    for (function, available, budget, out, fault) in [
        ("    %value = add i8 3, 4\n    ret %value", 7, 0, 0, 6),
        ("    %value = add i8 3, 4\n    ret %value", 8, 0, 0, 1),
        ("    %value = call @leaf(i8 7)\n    ret %value", 15, 0, 0, 1),
        ("    %value = call @leaf(i8 7)\n    ret %value", 15, 1, 0, 6),
        ("    %value = call @leaf(i8 7)\n    ret %value", 16, 2, 0, 1),
        ("    %value = call @leaf(i8 7)\n    ret %value", 16, 3, 7, 0),
        ("    call @noop()\n    ret 7", 0, 3, 7, 0),
        ("    %value = call_indirect fn(i8) -> i8 0(i8 7)\n    ret %value", 8, 1, 0, 4),
        ("    %addr = faddr @leaf\n    %value = call_indirect fn(i1) -> i8 %addr(i1 0)\n    ret %value", 16, 2, 0, 4),
    ] {
        let memory = 25 + available; // The caller uses 24 bytes and the static floor is one.
        let module = load(&format!(
            r#"
memory {memory}
data 0 = i8 0
func @f() -> i8 {{
entry:
{function}
}}
func @leaf(i8 %value) -> i8 {{
entry:
    ret %value
}}
func @noop() {{
entry:
    ret
}}
func @main(i64 %budget) {{
entry:
    %out = qalloc 8
    %fault = qalloc 5
    apply @f() -> %out, %fault budget %budget
    ret
}}
"#
        ))
        .expect("valid call example");
        let entry = module.function_named("main").expect("main");
        let options = Options { machine_meaning: true, ..Options::default() };
        let mut direct = Executor::new(&module, entry, &[budget], options).expect("direct run");
        assert_eq!(direct.run(), Outcome::Returned(None));
        let state = direct.snapshot();
        assert_eq!(state.quantum.amplitudes[0].0, [out, fault], "{function}, {available} bytes, B={budget}");
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let mut emitted = Executor::new(&module, entry, &[budget], options).expect("gate run");
            let mut compile = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
            assert_eq!(emitted.run_emitted(&mut compile), Outcome::Returned(None));
            assert_eq!(emitted.snapshot(), state, "{function}, {available} bytes, B={budget}, {mode:?}");
        }
    }
}

#[test]
fn a_grandchild_reserves_stack_below_its_parent() {
    for memory in [40, 48] {
        let module = load(&format!(
            r#"
memory {memory}
func @main(i64 %budget) {{
entry:
    %out = qalloc 8
    %fault = qalloc 5
    apply @f() -> %out, %fault budget %budget
    ret
}}
func @f() -> i8 {{
entry:
    %result = call @middle()
    ret %result
}}
func @middle() -> i8 {{
entry:
    %result = call @leaf(i8 7)
    ret %result
}}
func @leaf(i8 %value) -> i8 {{
entry:
    ret %value
}}
"#
        ))
        .expect("valid nested calls");
        let entry = module.function_named("main").expect("main function");
        let options = Options { machine_meaning: true, ..Options::default() };
        for budget in 0..=6 {
            // Main reserves 24 bytes. Each of f, middle and leaf reserves eight more.
            // The grandchild first attempts to enter at budget two.
            let expected = if memory == 40 && budget >= 2 {
                [0, 6]
            } else if budget < 5 {
                [0, 1]
            } else {
                [7, 0]
            };
            let mut direct = Executor::new(&module, entry, &[budget], options).expect("direct run");
            assert_eq!(direct.run(), Outcome::Returned(None));
            let state = direct.snapshot();
            assert_eq!(state.quantum.amplitudes[0].0, expected, "{memory} bytes, B={budget}");
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let mut emitted = Executor::new(&module, entry, &[budget], options).expect("gate run");
                let mut compile = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
                assert_eq!(emitted.run_emitted(&mut compile), Outcome::Returned(None));
                let actual = emitted.snapshot();
                assert_eq!(actual.quantum.amplitudes[0].0, expected, "{memory} bytes, B={budget}, {mode:?}");
                assert_eq!(actual, state);
            }
        }
    }
}

#[test]
fn call_refusals_precede_every_gate_of_the_apply() {
    let mut tree = String::new();
    let levels = 14;
    for level in 0..levels {
        writeln!(tree, "func @level{level}() {{\nentry:").expect("write string");
        if level + 1 != levels {
            let next = level + 1;
            writeln!(tree, "    call @level{next}()\n    call @level{next}()").expect("write string");
        }
        tree.push_str("    ret\n}\n");
    }
    for (body, extra, operation, route) in [
        ("%r = call @f(%x)", "", "call", "recursive call frames"),
        (
            "%addr = faddr @f\n    %r = call_indirect fn(i1) -> i8 %addr(%x)",
            "",
            "call_indirect",
            "recursive call frames",
        ),
        (
            "%addr = zext i1 %x to i64\n    %r = call_indirect fn(i1) -> i8 %addr(%x)",
            "",
            "call_indirect",
            "a proved definite function address",
        ),
        (
            "call @level0()\n    %r = add i8 1, 2",
            &tree,
            "call",
            "a smaller copy expansion (limit: 16384 slots and program points)",
        ),
    ] {
        let source = format!("memory 4096\nfunc @f(i1 %x) -> i8 {{\nentry:\n    {body}\n    ret %r\n}}\n{extra}");
        let module = control_example(&source, 1, "");
        let entry = module.function_named("main").expect("main");
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in [0, 40] {
                let mut device = CountingDevice::default();
                let mut compile = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
                let run = Executor::run_on_with_apply(&module, entry, &[budget], None, &mut device, &mut compile)
                    .expect("call refusal is a run outcome");
                assert!(
                    matches!(run.outcome, Outcome::Stopped(RunError::Apply(ApplyError::Unsupported {
                        operation: actual_operation, route: actual_route, ..
                    })) if actual_operation == operation && actual_route == route),
                    "{body}, {mode:?}, B={budget}: {:?}",
                    run.outcome
                );
                let counts = device.counts();
                assert_eq!((counts.one_qubit, counts.two_qubit, counts.toffoli, counts.peak_qubits), (4, 1, 0, 15));
                assert_eq!(counts.zero_released_qubits, 0);
            }
        }
    }
}
