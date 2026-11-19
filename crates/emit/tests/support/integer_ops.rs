use bennett_emit::{ApplyInput, ApplyMode, Argument, compile_apply_with};
use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::load;

use super::{bits, gates};

fn executor_result(input: &ApplyInput<'_>, out_width: usize, args: &[u64]) -> u128 {
    let options = Options { budget: Some(input.budget), ..Options::default() };
    let mut executor = Executor::new(input.module, input.callee, args, options).expect("executor entry");
    match executor.run() {
        Outcome::Returned(Some(value)) => value,
        Outcome::Faulted(fault) => u128::from(fault.code()) << out_width,
        outcome => panic!("unexpected executor outcome: {outcome:?}"),
    }
}

fn binary_sweep(operation: &str, width: usize) {
    let module = load(&format!(
        "memory 1024\nfunc @f(i{width} %x, i{width} %y) -> i{width} {{\nentry:\n    \
         %result = {operation} i{width} %x, %y\n    ret %result\n}}"
    ))
    .expect("valid integer test");
    let x = width as u64;
    let fault = 3 * x;
    let args = [Argument::Register(0..x), Argument::Register(x..2 * x)];
    let mask = (1 << width) - 1;
    for budget in 0..=3 {
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("callee"),
            args: &args,
            out: 2 * x..fault,
            fault: fault..fault + 5,
            budget,
            entry_fault: None,
            stack_available: 0,
            scratch_start: fault + 5,
        };
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let compiled = compile_apply_with(input.clone(), mode).expect("emit the binary operation");
            let (gates, layout) = gates(&compiled, 2 * width, width + 5);
            let cases = (0..1 << (2 * width)).flat_map(|x| [(x, 0), (x, 0x5555 & ((1 << (width + 5)) - 1))]);
            bits::check_states(&gates, layout, cases, |x| {
                executor_result(&input, width, &[(x & mask) as u64, (x >> width) as u64])
            })
            .unwrap_or_else(|error| panic!("{operation} i{width}, {mode:?}, B={budget}: {error}"));
        }
    }
}

#[test]
fn or_matches_the_executor_at_each_budget() {
    for width in 1..=4 {
        binary_sweep("or", width);
    }
}

#[test]
fn variable_shifts_match_the_executor_at_each_budget() {
    for operation in ["shl", "lshr", "ashr"] {
        for width in 1..=4 {
            binary_sweep(operation, width);
        }
    }
}

#[test]
fn variable_shifts_see_amount_bits_above_sixty_four() {
    let source = |operation| {
        format!(
            r#"
memory 1024
func @f(i3 %x, i3 %distance, i1 %high) -> i8 {{
entry:
    %word = sext i3 %x to i128
    %low = zext i3 %distance to i128
    %flag = zext i1 %high to i128
    %far = shl i128 %flag, 100
    %amount = or i128 %low, %far
    %shifted = {operation} i128 %word, %amount
    %answer = trunc i128 %shifted to i8
    ret %answer
}}
"#
        )
    };
    for operation in ["shl", "lshr", "ashr"] {
        let module = load(&source(operation)).expect("valid wide shift");
        let args = [Argument::Register(0..3), Argument::Register(3..6), Argument::Register(6..7)];
        for budget in 5..=9 {
            let input = ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("callee"),
                args: &args,
                out: 7..15,
                fault: 15..20,
                budget,
                entry_fault: None,
                stack_available: 0,
                scratch_start: 20,
            };
            let compiled = compile_apply_with(input.clone(), ApplyMode::Automatic).expect("emit the wide shift");
            let (gates, layout) = gates(&compiled, 7, 13);
            bits::check_states(&gates, layout, (0..128).map(|x| (x, 0x12a5)), |x| {
                executor_result(&input, 8, &[(x & 7) as u64, (x >> 3 & 7) as u64, (x >> 6) as u64])
            })
            .expect("wide shift, XOR destinations and zero scratch");
        }
    }
}
