//! Definite-state and budget checks for emitted applied functions.

#[path = "support/bits.rs"]
mod bits;

use bennett_emit::{
    ApplyError, ApplyInput, ApplyMode, Argument, CompiledApply, Construction, CountingDevice, compile_apply,
    compile_apply_with,
};
use bennett_exec::{BlockOp, Device, Executor, GateBlock, GateOp, Options, Outcome, RunError};
use bennett_ir::{BlockId, Fault, FixedGate, Gate, load};
use bits::Layout;

fn gates(compiled: &CompiledApply, x: usize, y: usize) -> (Vec<bennett_exec::GateOp>, Layout) {
    let ops = compiled.block.ops();
    let [BlockOp::Allocate(scratch), gates @ .., BlockOp::ReleaseZero(released)] = ops else {
        panic!("expected a single scratch lifetime");
    };
    assert_eq!(scratch, released);
    assert_eq!(scratch.start as usize, x + y);
    let gates = gates
        .iter()
        .map(|op| match op {
            BlockOp::Gate(gate) => gate.clone(),
            BlockOp::Allocate(_) | BlockOp::ReleaseZero(_) => panic!("unexpected lifetime"),
        })
        .collect::<Vec<_>>();
    let layout = Layout { x, y, scratch: (scratch.end - scratch.start) as usize };
    (gates, layout)
}

fn check(compiled: &CompiledApply, x: usize, y: usize, reference: impl Fn(u128) -> u128) {
    let (gates, layout) = gates(compiled, x, y);
    bits::exhaustive(&gates, layout, reference).expect("input, XOR destinations and scratch");
}

#[test]
fn symbolic_arithmetic_preserves_inputs_and_clears_scratch() {
    let module = load(
        r#"
memory 1024
func @f(i3 %x, i3 %y, i8 %fixed) -> i4 {
entry:
    %sx = sext i3 %x to i8
    %uy = zext i3 %y to i8
    %p = mul i8 %sx, %fixed
    %sum = add i8 %p, %uy
    %diff = sub i8 %sum, 3
    %shift = ashr i8 %diff, 2
    %test = icmp slt i3 %x, %y
    %selected = select i8 %test, %shift, %diff
    %mask = and i8 %selected, 15
    %flip = xor i8 %mask, 5
    %result = trunc i8 %flip to i4
    ret %result
}
"#,
    )
    .expect("valid test module");
    let input = ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("f"),
        args: &[Argument::Register(0..3), Argument::Register(3..6), Argument::Value(3)],
        out: 6..10,
        fault: 10..15,
        budget: 12,
        entry_fault: None,
        scratch_start: 15,
    };
    let compiled = compile_apply(input).expect("arithmetic on qubit words");
    assert_eq!(compiled.definite_steps, 12);
    check(&compiled, 6, 9, |input| {
        let signed = |bits: u128| ((bits as i8) << 5) >> 5;
        let x = signed(input);
        let y = (input >> 3) as i8;
        let diff = 3 * x + y - 3;
        let selected = if x < signed(y as u128) { diff >> 2 } else { diff };
        u128::from((selected as u8 & 15) ^ 5)
    });
}

#[test]
fn definite_control_charges_parallel_transfer_and_return() {
    let module = load(
        r#"
memory 128
func @f() -> i4 {
entry:
    br loop(i4 1, i4 2, i1 0)
loop(i4 %a, i4 %b, i1 %again):
    br %again, done, loop(%b, %a, i1 1)
done:
    %left = shl i4 %a, 2
    %answer = xor i4 %left, %b
    ret %answer
}
"#,
    )
    .expect("valid test module");
    let input = ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("f"),
        args: &[],
        out: 0..4,
        fault: 4..9,
        budget: 0,
        entry_fault: None,
        scratch_start: 9,
    };
    for (budget, steps, result) in [(0, 0, 16), (5, 5, 16), (6, 6, 9), (7, 6, 9)] {
        let compiled = compile_apply(ApplyInput { budget, ..input.clone() }).expect("definite back edge");
        assert_eq!(compiled.definite_steps, steps);
        check(&compiled, 0, 9, |_| result);
    }
    let compiled = compile_apply(ApplyInput { entry_fault: Some(Fault::StackOverflow), ..input }).expect("entry fault");
    assert_eq!(compiled.definite_steps, 0);
    check(&compiled, 0, 9, |_| 6 << 4);
}

#[test]
fn wide_sign_extension_carries_and_large_shifts_use_the_declared_width() {
    let module = load(
        r#"
memory 128
func @f(i64 %a, i64 %b) -> i64 {
entry:
    %wide_a = sext i64 %a to i128
    %wide_b = sext i64 %b to i128
    %sum = add i128 %wide_a, %wide_b
    %shifted = lshr i128 %sum, 64
    %too_far = ashr i128 %wide_a, 128
    %result = xor i128 %shifted, %too_far
    %word = trunc i128 %result to i64
    ret %word
}
"#,
    )
    .expect("valid test module");
    let compiled = compile_apply(ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("f"),
        args: &[Argument::Register(0..64), Argument::Register(64..128)],
        out: 128..192,
        fault: 192..197,
        budget: 8,
        entry_fault: None,
        scratch_start: 197,
    })
    .expect("wide arithmetic");
    let (gates, layout) = gates(&compiled, 128, 69);
    bits::sampled(&gates, layout, 256, 19, |bits| {
        let a = i128::from(bits as i64);
        let b = i128::from((bits >> 64) as i64);
        (a + b).cast_unsigned() >> 64
    })
    .expect("wide arithmetic, XOR destinations and scratch");
}

#[test]
fn nested_loops_share_the_clock_after_the_definite_prefix() {
    let source = r#"
memory 1024
func @f(i2 %x, i2 %y) -> i1 {
entry:
    %first = icmp ne i2 %x, 0
    br %first, inner(%x, %y, i1 0), done(i1 0)
inner(i2 %i, i2 %j, i1 %z):
    %more = icmp ne i2 %j, 0
    br %more, flip, leave
flip:
    %toggled = xor i1 %z, 1
    %next_j = sub i2 %j, 1
    br inner(%i, %next_j, %toggled)
leave:
    %next_i = sub i2 %i, 1
    br outer(%next_i, %z)
outer(i2 %left, i1 %parity):
    %again = icmp ne i2 %left, 0
    br %again, inner(%left, %y, %parity), done(%parity)
done(i1 %result):
    ret %result
}
"#;
    let module = load(source).expect("valid nested loops");
    for budget in [1, 2, 3, 13, 14, 15, 35] {
        let compiled = compile_apply(ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("f"),
            args: &[Argument::Register(0..2), Argument::Register(2..4)],
            out: 4..5,
            fault: 5..10,
            budget,
            entry_fault: None,
            scratch_start: 10,
        })
        .expect("emit both loops under one clock");
        assert_eq!(compiled.definite_steps, 1);
        let expected =
            if budget == 1 { Construction::Definite } else { Construction::StepMachine { ticks: budget - 1 } };
        assert_eq!(compiled.construction, expected);
        check(&compiled, 4, 6, |input| {
            let (x, y) = (input & 3, input >> 2);
            let steps = 3 + x * (6 + 5 * y);
            if steps <= u128::from(budget) { (x * y) & 1 } else { 2 }
        });
    }
    let main = r#"
func @main(i64 %budget) {
entry:
    %x = qalloc 2
    %y = qalloc 2
    x %x[0]
    x %y[0]
    %out = qalloc 1
    %fault = qalloc 5
    apply @f(%x, %y) -> %out, %fault budget %budget
    ret
}
"#;
    let module = load(&format!("{source}{main}")).expect("nested-loop program");
    let entry = module.function_named("main").expect("entry");
    for (budget, out, fault) in [(13, 0, 1), (14, 1, 0)] {
        let options = Options { machine_meaning: true, ..Options::default() };
        let mut reference = Executor::new(&module, entry, &[budget], options).expect("reference");
        let mut emitted = Executor::new(&module, entry, &[budget], options).expect("simulator");
        let mut compiler = |input: ApplyInput<'_>| {
            let compiled = compile_apply(input)?;
            assert_eq!(compiled.construction, Construction::StepMachine { ticks: budget - 1 });
            Ok(compiled.block)
        };
        assert_eq!(reference.run(), Outcome::Returned(None));
        assert_eq!(emitted.run_emitted(&mut compiler), Outcome::Returned(None));
        let expected = reference.snapshot();
        assert_eq!(expected.quantum.amplitudes[0].0, [1, 1, out, fault]);
        assert_eq!(emitted.snapshot(), expected);
    }
}

#[test]
fn coincident_successors_keep_their_own_parallel_arguments() {
    let module = load(
        r#"
memory 128
func @f(i1 %c, i3 %x, i3 %y) -> i3 {
entry:
    br %c, join(%x, %y), join(%y, %x)
join(i3 %a, i3 %b):
    %result = sub i3 %a, %b
    ret %result
}
"#,
    )
    .expect("valid join");
    let compiled = compile_apply(ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("f"),
        args: &[Argument::Register(0..1), Argument::Register(1..4), Argument::Register(4..7)],
        out: 7..10,
        fault: 10..15,
        budget: 10,
        entry_fault: None,
        scratch_start: 15,
    })
    .expect("emit two edges to one successor");
    assert_eq!(compiled.construction, Construction::StepMachine { ticks: 3 });
    check(&compiled, 7, 8, |input| {
        let x = (input >> 1) & 7;
        let y = input >> 4;
        if input & 1 != 0 { x.wrapping_sub(y) & 7 } else { y.wrapping_sub(x) & 7 }
    });
}

const COLLATZ_FIRST_RETURN: u64 = 5;
const COLLATZ_CONTINUATION: u64 = 9;
const COLLATZ_PEAK_VALUE: u128 = 13_120;
const COLLATZ_LONGEST: u64 = 127;
const COLLATZ_BUDGET: u64 = COLLATZ_FIRST_RETURN + COLLATZ_CONTINUATION * COLLATZ_LONGEST;

fn collatz_source(width: usize) -> String {
    format!(
        r#"
memory 1024
func @collatz(i{width} %input) -> i8 {{
entry:
    %wide = zext i{width} %input to i14
    br test(%wide, i8 0)
test(i14 %n, i8 %steps):
    %done = icmp eq i14 %n, 1
    br %done, finish, more
more:
    %odd = trunc i14 %n to i1
    %half = lshr i14 %n, 1
    %triple = mul i14 %n, 3
    %odd_next = add i14 %triple, 1
    %next = select i14 %odd, %odd_next, %half
    %inc = add i8 %steps, 1
    br test(%next, %inc)
finish:
    ret %steps
}}
"#,
    )
}

fn collatz(mut n: u128) -> (u64, u128) {
    assert_ne!(n, 0, "zero never returns");
    let mut updates = 0;
    let mut peak = n;
    while n != 1 {
        n = if n.is_multiple_of(2) { n / 2 } else { 3 * n + 1 };
        peak = peak.max(n);
        updates += 1;
    }
    (updates, peak)
}

fn collatz_return(updates: u64) -> u64 {
    COLLATZ_FIRST_RETURN + COLLATZ_CONTINUATION * updates
}

fn compile_collatz(module: &bennett_ir::ValidModule, width: u64, budget: u64, mode: ApplyMode) -> CompiledApply {
    compile_apply_with(
        ApplyInput {
            module,
            callee: module.function_named("collatz").expect("Collatz function"),
            args: &[Argument::Register(0..width)],
            out: width..width + 8,
            fault: width + 8..width + 13,
            budget,
            entry_fault: None,
            scratch_start: width + 13,
        },
        mode,
    )
    .expect("compile Collatz")
}

fn check_collatz(compiled: &CompiledApply, width: usize, budget: u64, updates: &[u64]) {
    let (gates, layout) = gates(compiled, width, 13);
    // Every input, with both zero and patterned nonzero initial destinations.
    let inputs = (0..1u128 << width).flat_map(|input| [(input, 0), (input, (input * 151 + 0x12a5) & 0x1fff)]);
    bits::check_states(&gates, layout, inputs, |x| {
        if x != 0 && collatz_return(updates[x as usize]) <= budget { u128::from(updates[x as usize]) } else { 1 << 8 }
    })
    .unwrap_or_else(|error| panic!("budget {budget}, {:?}: {error}", compiled.construction));
}

#[test]
fn saved_decisions_preserve_first_returns_at_exact_collatz_cutoffs() {
    let module = load(&collatz_source(3)).expect("valid Collatz function");
    let updates = std::iter::once(0).chain((1..8).map(|x| collatz(x).0)).collect::<Vec<_>>();
    for budget in [4, 5, 13, 14, 15, 31, 32, 33] {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let compiled = compile_collatz(&module, 3, budget, mode);
            assert_eq!(compiled.definite_steps, 3);
            match mode {
                ApplyMode::Automatic => assert_eq!(
                    compiled.construction,
                    Construction::SavedDecisions { decisions: budget.checked_sub(5).map_or(0, |rest| rest / 9 + 1) },
                ),
                ApplyMode::StepMachine => {
                    assert_eq!(compiled.construction, Construction::StepMachine { ticks: budget - 3 })
                }
            }
            check_collatz(&compiled, 3, budget, &updates);
        }
    }
}

#[test]
// Run with: cargo test -p bennett-emit --test apply collatz_eight_bit_budget_sweep_and_counts -- --ignored --exact --nocapture
#[ignore = "compiles both circuits at every distinct return boundary and runs all 256 definite inputs"]
fn collatz_eight_bit_budget_sweep_and_counts() {
    let module = load(&collatz_source(8)).expect("valid Collatz function");
    let trajectories = (1..256).map(collatz).collect::<Vec<_>>();
    let peak = trajectories.iter().map(|&(_, peak)| peak).max();
    let longest = trajectories.iter().map(|&(updates, _)| updates).max();
    assert_eq!(peak, Some(COLLATZ_PEAK_VALUE));
    assert_eq!(longest, Some(COLLATZ_LONGEST));
    let updates = std::iter::once(0).chain(trajectories.iter().map(|&(updates, _)| updates)).collect::<Vec<_>>();
    let budgets = [0, 1, 2, 3]
        .into_iter()
        .chain(updates[1..].iter().flat_map(|&updates| {
            let end = collatz_return(updates);
            [end - 1, end, end + 1]
        }))
        .collect::<std::collections::BTreeSet<_>>();
    println!("{} distinct budgets, 256 inputs, two destination words per input", budgets.len());
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for &budget in &budgets {
            let compiled = compile_collatz(&module, 8, budget, mode);
            check_collatz(&compiled, 8, budget, &updates);
            if budget == COLLATZ_BUDGET {
                let mut device = CountingDevice::default();
                device.allocate(0..21).expect("interface wires");
                compiled.send(&mut device).expect("count native gates");
                let counts = device.counts();
                println!("{:?}: {counts:?}", compiled.construction);
                let measured =
                    [counts.one_qubit, counts.two_qubit, counts.toffoli, counts.peak_qubits as u128, counts.depth];
                let recorded = match mode {
                    ApplyMode::Automatic => [7_136, 28_513, 19_258, 14_330, 3_729],
                    ApplyMode::StepMachine => [8_410, 53_269, 65_688, 26_888, 44_745],
                };
                assert_eq!(measured, recorded);
                assert_eq!(counts.zero_released_qubits, counts.peak_qubits as u128 - 21);
                assert_eq!(counts.discarded_qubits, 0);
            }
        }
    }
}

#[test]
fn collatz_circuits_match_machine_meaning_after_each_apply() {
    for width in 2..=5 {
        let hadamards = (0..width).map(|bit| format!("    h %arg[{bit}]\n")).collect::<String>();
        let module = load(&format!(
            "{}\nfunc @main(i64 %budget) {{\nentry:\n    %arg = qalloc {width}\n{hadamards}{}",
            collatz_source(width),
            r#"
    %witness = qalloc 1
    x %witness ctrl %arg[0]
    s %witness
    %out = qalloc 8
    %fault = qalloc 5
    x %out[2]
    x %fault[4]
    apply @collatz(%arg) -> %out, %fault budget %budget
    apply @collatz(%arg) -> %out, %fault budget %budget
    ret
}
"#,
        ))
        .expect("valid superposition program");
        let entry = module.function_named("main").expect("entry");
        let longest = (1..1 << width).map(|x| collatz(x).0).max().expect("terminating inputs");
        for budget in [4, 5, 14, collatz_return(longest)] {
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let machine = Options { machine_meaning: true, ..Options::default() };
                let mut reference = Executor::new(&module, entry, &[budget], machine).expect("reference");
                let options = Options { reversible: true, ..Options::default() };
                let mut emitted = Executor::new(&module, entry, &[budget], options).expect("gate simulator");
                let initial = emitted.snapshot();
                let mut selections = Vec::new();
                let mut compiler = |input: ApplyInput<'_>| {
                    let compiled = compile_apply_with(input, mode)?;
                    selections.push(compiled.construction);
                    Ok(compiled.block)
                };
                loop {
                    let expected = reference.step();
                    let actual = emitted.step_emitted(&mut compiler);
                    let detail = (width, budget, mode, reference.stats().steps);
                    assert_eq!(actual, expected, "{detail:?}");
                    assert_eq!(emitted.snapshot(), reference.snapshot(), "{detail:?}");
                    if let Some(outcome) = actual {
                        assert_eq!(outcome, Outcome::Returned(None));
                        break;
                    }
                }
                let expected = match mode {
                    ApplyMode::Automatic => {
                        Construction::SavedDecisions { decisions: budget.checked_sub(5).map_or(0, |rest| rest / 9 + 1) }
                    }
                    ApplyMode::StepMachine => Construction::StepMachine { ticks: budget - 3 },
                };
                assert_eq!(selections, [expected, expected]);
                emitted.reverse().expect("the two applied gate operations can be undone");
                let after_reverse = emitted.snapshot();
                assert_eq!(after_reverse.classical, initial.classical);
                let distance = after_reverse.quantum.distance(&initial.quantum).expect("registers restored");
                assert!(distance < 1e-15, "reversing Hadamards rounds by {distance:e}");
            }
        }
    }
}

const DEVICE_PROGRAM: &str = r#"
memory 512
func @f(i4 %x) -> i4 {
entry:
    %sum = add i4 %x, 3
    INSTRUCTION
    ret %answer
}
func @main() {
entry:
    %arg = qalloc 4
    %out = qalloc 4
    %fault = qalloc 5
    apply @f(%arg) -> %out, %fault budget 3
    ret
}
"#;

#[test]
fn qubit_shift_and_multiplier_refusals_do_not_depend_on_budget() {
    for (operation, route) in [
        ("mul", "multiplication of two qubit words"),
        ("shl", "shifts by a qubit word"),
        ("lshr", "shifts by a qubit word"),
        ("ashr", "shifts by a qubit word"),
    ] {
        let source = DEVICE_PROGRAM
            .replace("func @main()", "func @main(i64 %budget)")
            .replace("budget 3", "budget %budget")
            .replace("INSTRUCTION", &format!("%answer = {operation} i4 %sum, %x"));
        let module = load(&source).expect("valid program");
        let entry = module.function_named("main").expect("entry");
        let expected = Outcome::Stopped(RunError::Apply(ApplyError::Unsupported {
            callee: module.function_named("f").expect("callee"),
            block: BlockId(0),
            index: 1,
            operation,
            route,
        }));
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in [0, 1, 2, 3, u64::MAX] {
                let mut counter = CountingDevice::default();
                let mut compiler = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
                let run = Executor::run_on_with_apply(&module, entry, &[budget], None, &mut counter, &mut compiler)
                    .expect("device run");
                assert_eq!(run.outcome, expected, "{operation}, {mode:?}, B={budget}");
                assert_eq!(run.steps, 3);
                assert_eq!(counter.counts().peak_qubits, 13);
                assert_eq!(counter.counts().depth, 0);
                assert_eq!(counter.counts().zero_released_qubits, 0);

                let mut simulator = Executor::new(&module, entry, &[budget], Options::default()).expect("simulator");
                for _ in 0..3 {
                    assert_eq!(simulator.step_emitted(&mut compiler), None);
                }
                let mut before = simulator.snapshot();
                assert_eq!(simulator.step_emitted(&mut compiler), Some(expected));
                before.classical.outcome = Some(expected);
                assert_eq!(simulator.snapshot(), before);
            }
        }
    }
}

#[test]
fn ordinary_factors_stay_definite_across_conditional_block_transfers() {
    let module = load(
        r#"
memory 512
func @f(i3 %x, i3 %factor) -> i3 {
entry:
    %cond = trunc i3 %x to i1
    %fixed = add i3 %factor, 1
    br %cond, scale(%fixed), done(%x)
scale(i3 %k):
    %product = mul i3 %x, %k
    %shifted = shl i3 %product, %factor
    br done(%shifted)
done(i3 %result):
    ret %result
}
"#,
    )
    .expect("valid program");
    for budget in [0, 1, 6, 7, 8] {
        let compiled = compile_apply(ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("callee"),
            args: &[Argument::Register(0..3), Argument::Value(1)],
            out: 3..6,
            fault: 6..11,
            budget,
            entry_fault: None,
            scratch_start: 11,
        })
        .expect("the shift and factor are definite");
        check(&compiled, 3, 8, |x| {
            if x & 1 == 0 && budget >= 4 {
                x
            } else if x & 1 != 0 && budget >= 7 {
                (4 * x) & 7
            } else {
                1 << 3
            }
        });
    }
}

#[test]
fn rejects_a_late_invalid_gate_before_issuing_the_circuit() {
    let module = load(&DEVICE_PROGRAM.replace("INSTRUCTION", "%answer = xor i4 %sum, 6")).expect("valid program");
    let entry = module.function_named("main").expect("entry");
    let mut counter = CountingDevice::default();
    let mut compiler = |input: ApplyInput<'_>| {
        let target = input.out.start;
        let valid = compile_apply(input)?;
        let mut ops = valid.block.ops().to_vec();
        ops.insert(
            ops.len() - 1,
            BlockOp::Gate(GateOp { kind: Gate::Fixed(FixedGate::X), target, controls: vec![target] }),
        );
        Ok(GateBlock::from_ops("invalid circuit", ops))
    };
    let run = Executor::run_on_with_apply(&module, entry, &[], None, &mut counter, &mut compiler).expect("device run");
    assert_eq!(run.outcome, Outcome::Stopped(RunError::Device("apply circuit repeats a wire in one gate")));
    assert_eq!(run.steps, 3);
    assert_eq!(counter.counts().peak_qubits, 13);
    assert_eq!(counter.counts().depth, 0);
    assert_eq!(counter.counts().zero_released_qubits, 0);
}

#[test]
fn emitted_fault_statistics_use_the_change_to_a_nonzero_destination() {
    let module = load(
        r#"
memory 256
func @f(i1 %x) -> i1 {
entry:
    br %x, done, spin
done:
    ret 1
spin:
    br spin
}
func @main() {
entry:
    %x = qalloc 1
    h %x
    s %x
    %out = qalloc 1
    %fault = qalloc 5
    x %fault[0]
    apply @f(%x) -> %out, %fault budget 2
    apply @f(%x) -> %out, %fault budget 0
    ret
}
"#,
    )
    .expect("valid fault program");
    let entry = module.function_named("main").expect("entry");
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let options = Options { machine_meaning: true, ..Options::default() };
        let mut direct = Executor::new(&module, entry, &[], options).expect("direct evaluator");
        let mut emitted = Executor::new(&module, entry, &[], options).expect("gate simulator");
        let mut compiler = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
        for (step, count, total, maximum) in [(7, 1, 0.5, 0.5), (8, 2, 1.5, 1.0)] {
            while direct.stats().steps < step {
                assert_eq!(direct.step(), None);
                assert_eq!(emitted.step_emitted(&mut compiler), None);
            }
            assert_eq!(emitted.snapshot(), direct.snapshot());
            let expected = direct.stats();
            assert_eq!(expected.region_faults, count);
            assert!((expected.region_fault_weight - total).abs() < 1e-12);
            assert!((expected.region_fault_max_weight - maximum).abs() < 1e-12);
            let actual = emitted.stats();
            assert_eq!(actual.region_faults, expected.region_faults, "{mode:?}");
            assert_eq!(actual.region_fault_weight, expected.region_fault_weight, "{mode:?}");
            assert_eq!(actual.region_fault_max_weight, expected.region_fault_max_weight, "{mode:?}");
        }
        assert_eq!(emitted.step_emitted(&mut compiler), Some(Outcome::Returned(None)));
    }
}

#[test]
fn scratch_range_can_end_at_the_maximum_wire() {
    let module = load(
        r#"
memory 128
func @f(i1 %x, i1 %y) -> i1 {
entry:
    %both = and i1 %x, %y
    ret %both
}
"#,
    )
    .expect("valid AND");
    let input = ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("callee"),
        args: &[Argument::Register(0..1), Argument::Register(1..2)],
        out: 2..3,
        fault: 3..8,
        budget: 2,
        entry_fault: None,
        scratch_start: u64::MAX - 1,
    };
    let compiled = compile_apply(input.clone()).expect("last addressable scratch wire");
    let scratch = u64::MAX - 1..u64::MAX;
    assert_eq!(compiled.block.ops().first(), Some(&BlockOp::Allocate(scratch.clone())));
    assert_eq!(compiled.block.ops().last(), Some(&BlockOp::ReleaseZero(scratch)));
    let mut counter = CountingDevice::default();
    counter.allocate(0..8).expect("allocate interface");
    compiled.send(&mut counter).expect("use the last addressable wire");
    assert_eq!(counter.counts().peak_qubits, 9);
    assert_eq!(counter.counts().zero_released_qubits, 1);

    let boundary = ApplyInput { scratch_start: u64::MAX, ..input };
    assert_eq!(compile_apply(boundary.clone()).expect_err("a further wire cannot fit"), ApplyError::WireSpace);
    let empty = compile_apply(ApplyInput { budget: 0, ..boundary }).expect("zero scratch at the boundary");
    empty.send(&mut counter).expect("issue a budget fault with zero scratch");
    assert_eq!(counter.counts().zero_released_qubits, 1);
}
