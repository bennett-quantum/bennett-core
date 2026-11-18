//! Definite-state and budget checks for emitted applied functions.

#[path = "support/bits.rs"]
mod bits;

use bennett_emit::{
    ApplyInput, ApplyMode, Argument, CompiledApply, Construction, CountingDevice, compile_apply, compile_apply_with,
};
use bennett_exec::BlockOp;
use bennett_exec::Device;
use bennett_ir::{Fault, load};
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
    let module = load(
        r#"
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
"#,
    )
    .expect("valid nested loops");
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
