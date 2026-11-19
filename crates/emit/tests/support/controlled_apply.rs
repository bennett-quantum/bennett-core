//! Controlled choices, program-proved loops, and their budget boundaries.

use std::fmt::Write;

use super::{
    ApplyError, ApplyInput, ApplyMode, Argument, Construction, CountingDevice, Device, Executor, Options, Outcome,
    RunError, bits, compile_apply, compile_apply_with, gates, load,
};

#[path = "modular_loop.rs"]
mod modular_loop;

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
        stack_available: 0,
        memory: None,
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
fn stores_under_and_after_a_qubit_choice_are_refused_before_apply_gates() {
    for (line, choice, operation) in [
        ("yes:", "br %bit, yes, no", "br"),
        ("latch(i8 %value):", "br %bit, yes, no", "br"),
        ("yes:", "switch i1 %bit, 1: yes, default: no", "switch"),
    ] {
        let source =
            ORDINARY_LOOP.replace(line, &format!("{line}\n    store i8 9, 8")).replace("br %bit, yes, no", choice);
        let module = control_example(&source, 3, ", i3 3");
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare_controls(&module, 0, mode, |_| (0, 1));
            let mut device = CountingDevice::default();
            let mut compiler = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
            let entry = module.function_named("main").expect("main function");
            let run = Executor::run_on_with_apply(
                &module,
                entry,
                &[ORDINARY_LOOP_STEPS + 4],
                None,
                &mut device,
                &mut compiler,
            )
            .expect("run to the compiler refusal");
            assert!(matches!(
                run.outcome,
                Outcome::Stopped(RunError::Apply(ApplyError::Unsupported {
                    operation: actual,
                    route: "private memory in the step machine",
                    ..
                })) if actual == operation
            ));
            let counts = device.counts();
            assert_eq!((counts.one_qubit, counts.two_qubit, counts.toffoli), (6, 1, 0));
            assert_eq!(counts.peak_qubits, 17);
            assert_eq!(counts.zero_released_qubits, 0);
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
fn proved_definite_switches_use_the_selected_target() {
    for (selector, value) in [(2, 3), (3, 5)] {
        let module = control_example(DEFINITE_SWITCH, 1, &format!(", i8 {selector}"));
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare_controls(&module, 3, mode, |bit| (if bit == 0 { 7 } else { value }, 0));
        }
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
        (2, [[44, 10, 22, 26, 52], [78, 19, 53, 35, 81]]),
        (3, [[108, 18, 56, 42, 104], [134, 49, 132, 62, 165]]),
        (4, [[172, 28, 103, 60, 147], [190, 95, 242, 99, 273]]),
        (5, [[260, 40, 165, 83, 227], [254, 157, 383, 146, 405]]),
        (6, [[352, 54, 239, 109, 289], [326, 235, 555, 203, 561]]),
        (8, [[556, 88, 424, 170, 434], [494, 439, 992, 347, 945]]),
    ] {
        count_routes(&format!("shift i{width}"), &shift_loop(width), width, &[], shift_return(width as u64), recorded);
    }
    count_routes(
        "nested shifts",
        NESTED_SHIFTS,
        5,
        &[],
        NESTED_SHIFTS_STEPS,
        [[288, 106, 357, 139, 405], [1468, 2810, 5169, 1675, 1527]],
    );
    count_routes(
        "draining shifts",
        &draining_shifts(DRAINING_SHIFTS_WIDTH, DRAINING_SHIFTS_WIDTH),
        2 * DRAINING_SHIFTS_WIDTH,
        &[],
        DRAINING_SHIFTS_STEPS,
        [[868, 248, 847, 310, 1039], [4744, 9342, 18285, 5614, 4939]],
    );
}

#[test]
fn an_untaken_edge_supplies_constants_for_the_whole_function_check() {
    for jump in ["br 1, done, cold(i8 2)", "br 0, cold(i8 2), done"] {
        let source = format!(
            r#"
memory 1024
func @f(i8 %input) -> i8 {{
entry:
    {jump}
cold(i8 %amount):
    %shifted = shl i8 %input, %amount
    ret %shifted
done:
    ret %input
}}
"#
        );
        let module = control_example(&source, 8, "");
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in [0, 2] {
                compare_controls(&module, budget, mode, |x| if budget == 0 { (0, 1) } else { (x, 0) });
            }
        }
    }
}

fn short_circuit_pairs(pairs: usize) -> String {
    let width = 2 * pairs;
    let mut source = format!("memory 8192\nfunc @f(i{width} %input) -> i8 {{\nentry:\n    br first_0\n");
    for index in 0..pairs {
        let first = 2 * index;
        let second = first + 1;
        let next = index + 1;
        writeln!(
            source,
            "first_{index}:
    %low_{index} = lshr i{width} %input, {first}
    %a_{index} = trunc i{width} %low_{index} to i1
    br %a_{index}, second_{index}, first_{next}
second_{index}:
    %high_{index} = lshr i{width} %input, {second}
    %b_{index} = trunc i{width} %high_{index} to i1
    br %b_{index}, found, first_{next}"
        )
        .expect("write to a string");
    }
    write!(source, "first_{pairs}:\n    ret 7\nfound:\n    ret 3\n}}").expect("write to a string");
    source
}

#[test]
fn shared_short_circuit_suffixes_use_one_step_schedule() {
    let mut circuits = Vec::new();
    for pairs in [3, 8, 9, 10] {
        let width = 2 * pairs;
        let module = load(&short_circuit_pairs(pairs)).expect("valid short-circuit function");
        let args = [Argument::Register(0..width as u64)];
        let input = apply_input(&module, &args, width, (3 + 6 * pairs) as u64);
        let auto = compile_apply(input.clone()).expect("automatic emission");
        let forced = compile_apply_with(input, ApplyMode::StepMachine).expect("forced emission");
        println!(
            "pairs {pairs}: {} automatic, {} step-machine gates",
            auto.block.ops().len() - 2,
            forced.block.ops().len() - 2
        );
        circuits.push((pairs, auto, forced));
    }
    for (pairs, auto, forced) in circuits {
        assert_eq!(auto.reason, "conditional routes would emit shared code more than once", "{pairs} pairs");
        assert_eq!(auto.construction, forced.construction);
        assert_eq!(auto.block.ops(), forced.block.ops());
        let (gates, layout) = gates(&auto, 2 * pairs, 13);
        let inputs = [0, 1, 2].into_iter().chain((0..pairs).map(|pair| 3 << (2 * pair)));
        bits::check_states(&gates, layout, inputs.map(|x| (x, 0x12a5)), |x| {
            if (0..pairs).any(|pair| x >> (2 * pair) & 3 == 3) { 3 } else { 7 }
        })
        .expect("return and clean scratch for each pair");
    }
}

#[test]
fn a_bypassed_predecessor_cannot_change_a_reachable_block_parameter() {
    let source = |decision| {
        format!(
            r#"
memory 1024
func @f(i8 %input) -> i8 {{
entry:
    {decision}
live:
    br join(i8 1)
cold:
    br join(i8 2)
join(i8 %amount):
    %shifted = shl i8 %input, %amount
    ret %shifted
}}
"#
        )
    };
    for jump in ["br 1, live, cold", "br 0, cold, live"] {
        let module = control_example(&source(jump), 8, "");
        for budget in [0, 3, 4] {
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                compare_controls(&module, budget, mode, |x| if budget < 4 { (0, 1) } else { (x << 1 & 255, 0) });
            }
        }
    }

    let source = source("%choose = trunc i8 %input to i1\n    br %choose, live, cold");
    let module = control_example(&source, 8, "");
    for budget in [0, 4, 5] {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare_controls(
                &module,
                budget,
                mode,
                |x| {
                    if budget < 5 { (0, 1) } else { (x << (2 - (x & 1)) & 255, 0) }
                },
            );
        }
    }
}

fn draining_shifts(outer: usize, inner: usize) -> String {
    let width = outer + inner;
    format!(
        r#"
memory 1024
func @f(i{width} %input) -> i8 {{
entry:
    %x = trunc i{width} %input to i{outer}
    %upper = lshr i{width} %input, {outer}
    %y = trunc i{width} %upper to i{inner}
    br outer(%x, i8 0, %y)
outer(i{outer} %i, i8 %acc, i{inner} %remaining):
    %outer_more = icmp ne i{outer} %i, 0
    br %outer_more, start_inner, done
start_inner:
    br inner(%remaining, %acc)
inner(i{inner} %j, i8 %count):
    %inner_more = icmp ne i{inner} %j, 0
    br %inner_more, inner_body, outer_latch
inner_body:
    %next_j = lshr i{inner} %j, 1
    %inc = add i8 %count, 1
    br inner(%next_j, %inc)
outer_latch:
    %next_i = lshr i{outer} %i, 1
    br outer_back
outer_back:
    br outer(%next_i, %count, %j)
done:
    ret %acc
}}
"#
    )
}

const DRAINING_SHIFTS_WIDTH: usize = 6;
const DRAINING_SHIFTS_STEPS: u64 = 7 + 13 * DRAINING_SHIFTS_WIDTH as u64;

#[test]
fn a_drained_inner_word_stays_zero_on_later_outer_passes() {
    let sides = DRAINING_SHIFTS_WIDTH;
    let width = 2 * sides;
    let module = load(&draining_shifts(sides, sides)).expect("valid nested shifts");
    let args = [Argument::Register(0..width as u64)];
    let input = apply_input(&module, &args, width, DRAINING_SHIFTS_STEPS);
    let ample = compile_apply(ApplyInput { budget: 1000, ..input.clone() }).expect("emit at ample budget");
    println!("drained {sides}-bit words: {}, {} gates", ample.construction, ample.block.ops().len() - 2);
    assert_eq!(ample.construction, Construction::Controlled { proved_passes: width });
    let exact = compile_apply(input).expect("emit at the longest return");
    assert_eq!(exact.block.ops(), ample.block.ops());
    let (gates, layout) = gates(&exact, width, 13);
    bits::check_states(&gates, layout, (0..1 << width).map(|x| (x, 0x12a5)), |x| {
        if x & ((1 << sides) - 1) == 0 { 0 } else { u128::from(bit_length((x >> sides) as u64)) }
    })
    .expect("all inputs keep the drained word's result and clear scratch");

    let (outer, inner) = (2, 3);
    let small = control_example(&draining_shifts(outer, inner), outer + inner, "");
    let longest = 7 + 8 * outer as u64 + 5 * inner as u64;
    for budget in [longest - 1, longest, longest + 1] {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let (construction, _) = compare_controls(&small, budget, mode, |x| {
                let outer = bit_length(x & 3);
                let inner = if outer == 0 { 0 } else { bit_length(x >> 2) };
                if budget >= 7 + 8 * outer + 5 * inner { (inner, 0) } else { (0, 1) }
            });
            if budget >= longest && mode == ApplyMode::Automatic {
                assert_eq!(construction, Construction::Controlled { proved_passes: outer + inner });
            }
        }
    }
}

#[test]
fn a_literal_switch_default_can_keep_a_loop_running() {
    let module = control_example(
        r#"
memory 1024
func @f(i2 %x) -> i8 {
entry:
    br header(%x)
header(i2 %word):
    %nonzero = icmp ne i2 %word, 0
    br %nonzero, choose, done
choose:
    switch i2 2, 1: advance, default: stay
advance:
    %half = lshr i2 %word, 1
    br header(%half)
stay:
    br header(%word)
done:
    ret 7
}
"#,
        2,
        "",
    );
    for budget in 0..=17 {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare_controls(&module, budget, mode, |x| if x == 0 && budget >= 4 { (7, 0) } else { (0, 1) });
        }
    }
}

#[test]
fn a_shift_by_zero_keeps_every_nonzero_input_running() {
    let source = shift_loop(2).replace("lshr i2 %x, 1", "lshr i2 %x, 0");
    let module = control_example(&source, 2, "");
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        compare_controls(&module, 20, mode, |x| if x == 0 { (0, 0) } else { (0, 1) });
    }
}

#[path = "operand_analysis.rs"]
mod operand_analysis;

#[test]
fn phantom_overlap() {
    for jump in ["br 1, a, no", "br 0, no, a", "switch i3 1, 1: a, default: no"] {
        let source = format!(
            r#"
memory 1024
func @f(i1 %input) -> i8 {{
entry:
    br %input, yes, no
yes:
    {jump}
a:
    ret 3
no:
    ret 7
}}
"#
        );
        let module = control_example(&source, 1, "");
        for budget in 0..=5 {
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let (construction, _) =
                    compare_controls(&module, budget, mode, |x| if budget < 2 + x { (0, 1) } else { (7 - 4 * x, 0) });
                if budget > 0 && mode == ApplyMode::Automatic {
                    assert_eq!(construction, Construction::Controlled { proved_passes: 0 }, "{jump}, B={budget}");
                }
            }
        }
    }
}

#[path = "calls.rs"]
mod calls;

#[test]
fn a_product_bound_proves_high_bits_for_a_shift() {
    let module = control_example(
        r#"
memory 1024
func @f(i2 %x) -> i8 {
entry:
    %lo = and i2 %x, 1
    %hi = lshr i2 %x, 1
    %a = zext i2 %lo to i8
    %b = zext i2 %hi to i8
    %product = mul i8 %a, %b
    %upper = lshr i8 %product, 1
    %base = shl i8 7, %upper
    %result = add i8 %base, %product
    ret %result
}
"#,
        2,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=10 {
            compare_controls(
                &module,
                budget,
                mode,
                |x| {
                    if budget < 9 { (0, 1) } else { (7 + ((x & 1) * (x >> 1)), 0) }
                },
            );
        }
    }
}

#[test]
fn multiply_two_qubit_words_at_the_return_boundary() {
    let module = control_example(
        r#"
memory 1024
func @f(i8 %input) -> i8 {
entry:
    %left = and i8 %input, 15
    %right = lshr i8 %input, 4
    %product = mul i8 %left, %right
    ret %product
}
"#,
        8,
        "",
    );
    for budget in [3, 4, 5] {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare_controls(&module, budget, mode, |x| if budget < 4 { (0, 1) } else { ((x & 15) * (x >> 4), 0) });
        }
    }
}

#[test]
fn remainder_faults_only_on_reached_zero_divisors() {
    let body = r#"
    %a = trunc i6 %input to i3
    %top = lshr i6 %input, 3
    %d = trunc i6 %top to i3
    %r = urem i3 %a, %d
    %out = zext i3 %r to i8
    ret %out
"#;
    for conditional in [false, true] {
        let split = if conditional {
            "    %odd = trunc i6 %input to i1\n    br %odd, divide, idle\nidle:\n    ret 7\ndivide:\n"
        } else {
            ""
        };
        let source = format!("memory 1024\nfunc @f(i6 %input) -> i8 {{\nentry:\n{split}{body}}}\n");
        let module = control_example(&source, 6, "");
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in 0..=9 {
                compare_controls(&module, budget, mode, |x| {
                    if conditional && x & 1 == 0 {
                        return if budget < 3 { (0, 1) } else { (7, 0) };
                    }
                    let divisor = x >> 3;
                    let prefix = if conditional { 2 } else { 0 };
                    if budget < prefix + 4 {
                        (0, 1)
                    } else if divisor == 0 {
                        (0, 2)
                    } else if budget < prefix + 6 {
                        (0, 1)
                    } else {
                        ((x & 7) % divisor, 0)
                    }
                });
            }
        }
    }

    let inside_call = body.replace("urem i3 %a, %d", "call @residue(%a, %d)");
    let source = format!(
        r#"
memory 1024
func @f(i6 %input) -> i8 {{
entry:
{inside_call}}}
func @residue(i3 %a, i3 %d) -> i3 {{
entry:
    %r = urem i3 %a, %d
    ret %r
}}
"#
    );
    let module = control_example(&source, 6, "");
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=9 {
            compare_controls(&module, budget, mode, |x| {
                let divisor = x >> 3;
                if budget < 5 {
                    (0, 1)
                } else if divisor == 0 {
                    (0, 2)
                } else if budget < 8 {
                    (0, 1)
                } else {
                    ((x & 7) % divisor, 0)
                }
            });
        }
    }

    let module =
        control_example("memory 128\nfunc @f(i8 %input) -> i8 {\nentry:\n %r = urem i8 %input, 0\n ret %r\n}", 8, "");
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in [0, 1, 2] {
            compare_controls(&module, budget, mode, |_| (0, if budget == 0 { 1 } else { 2 }));
        }
    }
}

#[test]
fn a_remainder_bound_proves_high_bits_even_in_a_wide_slot() {
    let module = control_example(
        r#"
memory 1024
func @f(i8 %input) -> i8 {
entry:
    %wide = zext i8 %input to i128
    %r = urem i128 %wide, 11
    %amount = lshr i128 %r, 4
    %shifted = shl i128 %r, %amount
    %out = trunc i128 %shifted to i8
    ret %out
}
"#,
        8,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        compare_controls(&module, 6, mode, |x| (x % 11, 0));
    }
}

#[path = "mulmod.rs"]
mod mulmod;
