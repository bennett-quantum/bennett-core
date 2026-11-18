//! Controlled choices, program-proved loops, and their budget boundaries.

use super::{
    ApplyError, ApplyInput, ApplyMode, Argument, BlockId, Construction, CountingDevice, Device, Executor, Options,
    Outcome, RunError, bits, compile_apply, compile_apply_with, gates, load,
};

const ORDINARY_LOOP: &str = r#"
memory 1024
func @f(i3 %x, i3 %passes) -> i8 {
entry:
    br header(%passes, i8 0)
header(i3 %left, i8 %acc):
    %more = icmp ne i3 %left, 0
    br %more, choose, done
choose:
    %bit = trunc i3 %x to i1
    br %bit, yes, no
yes:
    %plus = zext i3 %x to i8
    %sum = add i8 %acc, %plus
    br latch(%sum)
no:
    %minus = zext i3 %x to i8
    %diff = sub i8 %acc, %minus
    br latch(%diff)
latch(i8 %value):
    %dec = sub i3 %left, 1
    br header(%dec, %value)
done:
    ret %acc
}
"#;
const ORDINARY_LOOP_STEPS: u64 = 31;

const MEETING_ROUTES: &str = r#"
memory 1024
func @f(i1 %input) -> i8 {
entry:
    br %input, longer, done
longer:
    br done
done:
    ret 7
}
"#;

fn shift_loop(width: usize) -> String {
    format!(
        r#"
memory 1024
func @f(i{width} %input) -> i8 {{
entry:
    br test(%input, i8 0)
test(i{width} %x, i8 %count):
    %zero = icmp eq i{width} 0, %x
    br %zero, done, shift
shift:
    %next = lshr i{width} %x, 1
    %inc = add i8 %count, 1
    br test(%next, %inc)
done:
    ret %count
}}
"#
    )
}

const NESTED_SHIFTS: &str = r#"
memory 1024
func @f(i5 %input) -> i8 {
entry:
    %x = trunc i5 %input to i2
    %upper = lshr i5 %input, 2
    %y = trunc i5 %upper to i3
    br outer(%x, i8 0)
outer(i2 %i, i8 %acc):
    %outer_more = icmp ne i2 %i, 0
    br %outer_more, start_inner, done
start_inner:
    br inner(%y, %acc)
inner(i3 %j, i8 %count):
    %inner_more = icmp ne i3 %j, 0
    br %inner_more, inner_body, outer_latch
inner_body:
    %next_j = lshr i3 %j, 1
    %inc = add i8 %count, 1
    br inner(%next_j, %inc)
outer_latch:
    %next_i = lshr i2 %i, 1
    br outer_back
outer_back:
    br outer(%next_i, %count)
done:
    ret %acc
}
"#;
const NESTED_SHIFTS_STEPS: u64 = 53;

fn apply_input<'a>(
    module: &'a bennett_ir::ValidModule,
    args: &'a [Argument],
    width: usize,
    budget: u64,
) -> ApplyInput<'a> {
    let width = width as u64;
    ApplyInput {
        module,
        callee: module.function_named("f").expect("applied function"),
        args,
        out: width..width + 8,
        fault: width + 8..width + 13,
        budget,
        entry_fault: None,
        scratch_start: width + 13,
    }
}

fn control_example(source: &str, width: usize, extra_args: &str) -> bennett_ir::ValidModule {
    let hadamards = (0..width).map(|bit| format!("    h %arg[{bit}]\n")).collect::<String>();
    load(&format!(
        r#"
{source}
func @main(i64 %budget) {{
entry:
    %arg = qalloc {width}
{hadamards}
    %witness = qalloc 1
    x %witness ctrl %arg[0]
    s %witness
    %out = qalloc 8
    %fault = qalloc 5
    x %out[2]
    x %fault[4]
    apply @f(%arg{extra_args}) -> %out, %fault budget %budget
    ret
}}
"#
    ))
    .expect("valid control example")
}

fn compare_controls(
    module: &bennett_ir::ValidModule,
    budget: u64,
    mode: ApplyMode,
    expected: impl Fn(u64) -> (u64, u64),
) -> (Construction, &'static str) {
    let entry = module.function_named("main").expect("main function");
    let options = Options { machine_meaning: true, ..Options::default() };
    let mut direct = Executor::new(module, entry, &[budget], options).expect("direct simulation");
    let mut emitted = Executor::new(module, entry, &[budget], options).expect("gate simulation");
    let mut selected = None;
    let mut compiler = |input: ApplyInput<'_>| {
        let compiled = compile_apply_with(input, mode)?;
        assert!(selected.replace((compiled.construction, compiled.reason)).is_none());
        Ok(compiled.block)
    };
    loop {
        let outcome = direct.step();
        assert_eq!(emitted.step_emitted(&mut compiler), outcome, "B={budget}, {mode:?}");
        assert_eq!(emitted.snapshot(), direct.snapshot(), "B={budget}, {mode:?}");
        if let Some(outcome) = outcome {
            assert_eq!(outcome, Outcome::Returned(None));
            break;
        }
    }
    for (basis, _) in &direct.snapshot().quantum.amplitudes {
        let (out, fault) = expected(basis[0]);
        assert_eq!(*basis, [basis[0], basis[0] & 1, 4 ^ out, 16 ^ fault], "B={budget}");
    }
    selected.expect("one applied circuit")
}

#[test]
fn qubit_choice_in_an_ordinary_loop_uses_control_gates() {
    let module = control_example(ORDINARY_LOOP, 3, ", i3 3");
    for budget in [ORDINARY_LOOP_STEPS - 1, ORDINARY_LOOP_STEPS, ORDINARY_LOOP_STEPS + 1] {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let (construction, _) = compare_controls(&module, budget, mode, |x| {
                if budget < ORDINARY_LOOP_STEPS {
                    return (0, 1);
                }
                let update = if x & 1 == 1 { x } else { 0u64.wrapping_sub(x) };
                (update.wrapping_mul(3) & 255, 0)
            });
            if mode == ApplyMode::Automatic {
                assert_eq!(construction, Construction::Controlled { proved_passes: 0 });
            }
        }
    }
}

#[test]
fn stores_under_and_after_a_qubit_choice_need_memory_in_the_step_machine() {
    for (line, position) in [("yes:", BlockId(3)), ("latch(i8 %value):", BlockId(5))] {
        let source = ORDINARY_LOOP.replace(line, &format!("{line}\n    store i8 9, 8"));
        let module = control_example(&source, 3, ", i3 3");
        let args = [Argument::Register(0..3), Argument::Value(3)];
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in [0, ORDINARY_LOOP_STEPS + 1] {
                let error = compile_apply_with(apply_input(&module, &args, 3, budget), mode)
                    .expect_err("a store needs private memory");
                assert_eq!(
                    error,
                    ApplyError::Unsupported {
                        callee: module.function_named("f").expect("f"),
                        block: position,
                        index: 0,
                        operation: "store",
                        route: "memory in the step machine",
                    },
                );
                assert!(error.to_string().ends_with("needs memory in the step machine"));

                let mut device = CountingDevice::default();
                let mut compiler = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
                let entry = module.function_named("main").expect("main function");
                let run = Executor::run_on_with_apply(&module, entry, &[budget], None, &mut device, &mut compiler)
                    .expect("run to the compiler refusal");
                assert_eq!(run.outcome, Outcome::Stopped(RunError::Apply(error)));
                let counts = device.counts();
                // The caller prepared its registers, and the refused apply contributed no commands.
                assert_eq!((counts.one_qubit, counts.two_qubit, counts.toffoli), (6, 1, 0));
                assert_eq!(counts.peak_qubits, 17);
                assert_eq!(counts.zero_released_qubits, 0);
            }
        }
    }
}

#[test]
fn unequal_routes_fall_back_only_when_the_join_may_exhaust_the_budget() {
    let module = control_example(MEETING_ROUTES, 1, "");
    for budget in 0..=4 {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let (construction, reason) =
                compare_controls(&module, budget, mode, |x| if budget >= 2 + x { (7, 0) } else { (0, 1) });
            if mode == ApplyMode::Automatic {
                match budget {
                    0 => assert_eq!(construction, Construction::Definite),
                    2 => {
                        assert_eq!(construction, Construction::StepMachine { ticks: 2 });
                        assert_eq!(reason, "unequal route lengths can exhaust the budget after a join");
                    }
                    _ => assert_eq!(construction, Construction::Controlled { proved_passes: 0 }),
                }
            }
        }
    }
}

fn bit_length(value: u64) -> u64 {
    u64::from(u64::BITS - value.leading_zeros())
}

fn shift_return(count: u64) -> u64 {
    4 + 5 * count
}

#[test]
fn proved_shift_passes_match_machine_meaning_at_each_return_boundary() {
    for width in 2..=6 {
        let module = control_example(&shift_loop(width), width, "");
        let longest = shift_return(width as u64);
        for budget in (0..=width as u64).flat_map(|count| {
            let steps = shift_return(count);
            [steps - 1, steps, steps + 1]
        }) {
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let (construction, reason) = compare_controls(&module, budget, mode, |x| {
                    let count = bit_length(x);
                    if budget >= shift_return(count) { (count, 0) } else { (0, 1) }
                });
                if mode == ApplyMode::Automatic {
                    if budget >= longest {
                        assert_eq!(construction, Construction::Controlled { proved_passes: width });
                    } else if let Construction::StepMachine { .. } = construction {
                        assert_eq!(reason, "unequal route lengths can exhaust the budget after a join");
                        let args = [Argument::Register(0..width as u64)];
                        let input = apply_input(&module, &args, width, budget);
                        let auto = compile_apply(input.clone()).expect("automatic fallback");
                        let forced = compile_apply_with(input, ApplyMode::StepMachine).expect("forced fallback");
                        assert_eq!(auto.block.ops(), forced.block.ops(), "abandoned gates were removed");
                    } else {
                        assert!(matches!(construction, Construction::Controlled { .. }));
                    }
                }
            }
        }
    }
}

#[test]
fn program_proved_loops_can_nest() {
    let module = control_example(NESTED_SHIFTS, 5, "");
    for budget in [NESTED_SHIFTS_STEPS - 1, NESTED_SHIFTS_STEPS, NESTED_SHIFTS_STEPS + 1] {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let (construction, _) = compare_controls(&module, budget, mode, |x| {
                let outer = bit_length(x & 3);
                let inner = bit_length(x >> 2);
                if budget >= 7 + outer * (8 + 5 * inner) { (outer * inner, 0) } else { (0, 1) }
            });
            if mode == ApplyMode::Automatic && budget >= NESTED_SHIFTS_STEPS {
                assert_eq!(construction, Construction::Controlled { proved_passes: 8 });
            }
        }
    }

    let unproved = control_example(&NESTED_SHIFTS.replace("lshr i3 %j", "ashr i3 %j"), 5, "");
    let (construction, _) = compare_controls(&unproved, NESTED_SHIFTS_STEPS, ApplyMode::Automatic, |x| {
        let outer = bit_length(x & 3);
        let inner = bit_length(x >> 2);
        if outer == 0 || inner < 3 { (outer * inner, 0) } else { (0, 1) }
    });
    assert!(matches!(construction, Construction::StepMachine { .. }));
}

#[test]
fn a_proved_outer_loop_does_not_copy_a_budget_limited_inner_schedule() {
    let source = r#"
memory 1024
func @f(i2 %input) -> i8 {
entry:
    br test(%input)
test(i2 %x):
    %more = icmp ne i2 %x, 0
    br %more, choose, done
choose:
    %odd = trunc i2 %x to i1
    br %odd, spin, shift
spin:
    br 0, shift, spin
shift:
    %next = lshr i2 %x, 1
    br test(%next)
done:
    ret 7
}
"#;
    let module = control_example(source, 2, "");
    let (construction, reason) =
        compare_controls(&module, 35, ApplyMode::Automatic, |x| if x == 0 { (7, 0) } else { (0, 1) });
    assert_eq!(construction, Construction::StepMachine { ticks: 33 });
    assert_eq!(reason, "qubit-dependent loop without a program-proved pass count");
}

#[test]
fn all_eight_bit_shift_inputs_preserve_the_permutation_and_clear_scratch() {
    let width = 8;
    let module = load(&shift_loop(width)).expect("valid shift loop");
    let args = [Argument::Register(0..width as u64)];
    for budget in 0..=shift_return(width as u64) + 1 {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let compiled = compile_apply_with(apply_input(&module, &args, width, budget), mode).expect("shift loop");
            let (gates, layout) = gates(&compiled, width, 13);
            let inputs = (0..256).flat_map(|x| [(x, 0), (x, (x * 151 + 0x12a5) & 0x1fff)]);
            bits::check_states(&gates, layout, inputs, |x| {
                let count = bit_length(x as u64);
                if budget >= shift_return(count) { u128::from(count) } else { 1 << 8 }
            })
            .unwrap_or_else(|err| panic!("B={budget}, {mode:?}: {err}"));
        }
    }
}

const DEFINITE_SWITCH: &str = r#"
memory 128
func @f(i1 %bit, i8 %ordinary) -> i8 {
entry:
    br %bit, case, done(i8 7)
case:
    switch i8 %ordinary, 2: done(i8 3), default: done(i8 5)
done(i8 %out):
    ret %out
}
"#;

#[test]
fn switches_require_a_proved_definite_selector() {
    for (selector, value) in [(2, 3), (3, 5)] {
        let module = control_example(DEFINITE_SWITCH, 1, &format!(", i8 {selector}"));
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare_controls(&module, 3, mode, |bit| (if bit == 0 { 7 } else { value }, 0));
        }
    }
    let dynamic = DEFINITE_SWITCH.replace("switch i8 %ordinary, 2", "switch i1 %bit, 1");
    let module = load(&dynamic).expect("valid qubit switch");
    let args = [Argument::Register(0..1), Argument::Value(2)];
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let error = compile_apply_with(apply_input(&module, &args, 1, 0), mode).expect_err("qubit switch");
        assert_eq!(
            error,
            ApplyError::Unsupported {
                callee: module.function_named("f").expect("f"),
                block: BlockId(1),
                index: 0,
                operation: "switch",
                route: "multiway dispatch in the step machine",
            }
        );
    }
}

fn count_routes(label: &str, source: &str, width: usize, ordinary: &[Argument], budget: u64, recorded: [[u128; 5]; 2]) {
    let module = load(source).expect("valid count example");
    let args = [Argument::Register(0..width as u64)].into_iter().chain(ordinary.iter().cloned()).collect::<Vec<_>>();
    let input = apply_input(&module, &args, width, budget);
    for (mode, expected) in [ApplyMode::Automatic, ApplyMode::StepMachine].into_iter().zip(recorded) {
        let compiled = compile_apply_with(input.clone(), mode).expect("compile count example");
        let mut device = CountingDevice::default();
        device.allocate(0..input.scratch_start).expect("allocate interface");
        compiled.send(&mut device).expect("issue native gates");
        let counts = device.counts();
        let measured = [counts.one_qubit, counts.two_qubit, counts.toffoli, counts.peak_qubits as u128, counts.depth];
        println!("{label}: {}, {measured:?}", compiled.construction);
        assert_eq!(measured, expected, "{label} at B={budget}, {mode:?}");
        if mode == ApplyMode::Automatic {
            assert!(matches!(compiled.construction, Construction::Controlled { .. }));
            let ample =
                compile_apply(ApplyInput { budget: u64::MAX, ..input.clone() }).expect("compile with ample budget");
            assert_eq!(ample.block.ops(), compiled.block.ops(), "{label} schedule is independent of surplus budget");
        }
        assert_eq!(counts.discarded_qubits, 0);
        assert_eq!(counts.zero_released_qubits, counts.peak_qubits as u128 - u128::from(input.scratch_start));
    }
}

#[test]
fn control_and_step_machine_counts() {
    count_routes(
        "ordinary loop",
        ORDINARY_LOOP,
        3,
        &[Argument::Value(3)],
        ORDINARY_LOOP_STEPS,
        [[948, 104, 612, 170, 1265], [460, 326, 354, 181, 639]],
    );
    count_routes("meeting routes", MEETING_ROUTES, 1, &[], 3, [[3, 0, 0, 14, 1], [6, 6, 0, 14, 12]]);
    for (width, recorded) in [
        (2, [[56, 12, 32, 30, 72], [78, 19, 53, 35, 81]]),
        (3, [[132, 24, 80, 51, 142], [134, 49, 132, 62, 165]]),
        (4, [[212, 40, 147, 76, 225], [190, 95, 242, 99, 273]]),
        (5, [[320, 60, 235, 108, 335], [254, 157, 383, 146, 405]]),
        (6, [[436, 84, 341, 145, 457], [326, 235, 555, 203, 561]]),
        (8, [[700, 144, 608, 234, 750], [494, 439, 992, 347, 945]]),
    ] {
        count_routes(&format!("shift i{width}"), &shift_loop(width), width, &[], shift_return(width as u64), recorded);
    }
    count_routes(
        "nested shifts",
        NESTED_SHIFTS,
        5,
        &[],
        NESTED_SHIFTS_STEPS,
        [[348, 126, 433, 167, 539], [1468, 2810, 5169, 1675, 1527]],
    );
}
