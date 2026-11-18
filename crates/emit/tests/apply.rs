//! Definite-state and budget checks for emitted applied functions.

#[path = "support/bits.rs"]
mod bits;

use bennett_emit::{ApplyInput, Argument, CompiledApply, compile_apply};
use bennett_exec::BlockOp;
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
