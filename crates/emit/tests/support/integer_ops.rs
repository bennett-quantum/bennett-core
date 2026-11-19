use bennett_emit::{ApplyInput, ApplyMode, Argument, Construction, CountingDevice, compile_apply_with};
use bennett_exec::{Device, Executor, Options, Outcome};
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
fn or_preserves_complemented_operands() {
    for operands in ["%flipped, %y", "%y, %flipped"] {
        let module = load(&format!(
            "memory 128\nfunc @f(i1 %x, i1 %y) -> i1 {{\nentry:\n    \
             %flipped = xor i1 %x, 1\n    %result = or i1 {operands}\n    ret %result\n}}"
        ))
        .expect("valid OR with a complemented operand");
        let args = [Argument::Register(0..1), Argument::Register(1..2)];
        for budget in 0..=4 {
            let input = ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("callee"),
                args: &args,
                out: 2..3,
                fault: 3..8,
                budget,
                entry_fault: None,
                stack_available: 0,
                scratch_start: 8,
            };
            let compiled = compile_apply_with(input.clone(), ApplyMode::Automatic).expect("emit OR");
            super::check(&compiled, 2, 6, |x| executor_result(&input, 1, &[(x & 1) as u64, (x >> 1) as u64]));
        }
    }
}

#[test]
fn or_in_a_countdown_uses_saved_decisions() {
    let module = load(
        r#"
memory 1024
func @f(i3 %count, i3 %mask) -> i3 {
entry:
    br test(%count, i3 0)
test(i3 %left, i3 %sum):
    %more = icmp ne i3 %left, 0
    br %more, body, done(%sum)
body:
    %value = or i3 %left, %mask
    %next_sum = xor i3 %sum, %value
    %next_left = sub i3 %left, 1
    br test(%next_left, %next_sum)
done(i3 %result):
    ret %result
}
"#,
    )
    .expect("valid countdown using OR");
    let args = [Argument::Register(0..3), Argument::Register(3..6)];
    // Entry and exit cost four steps together. Each of up to seven passes costs six.
    let longest_return = 4 + 7 * 6;
    for budget in [longest_return - 1, longest_return, longest_return + 1] {
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("callee"),
            args: &args,
            out: 6..9,
            fault: 9..14,
            budget,
            entry_fault: None,
            stack_available: 0,
            scratch_start: 14,
        };
        let compiled = compile_apply_with(input.clone(), ApplyMode::Automatic).expect("emit the countdown");
        assert_eq!(compiled.construction, Construction::SavedDecisions { decisions: 1 + (budget - 4) / 6 });
        super::check(&compiled, 6, 8, |x| executor_result(&input, 3, &[(x & 7) as u64, (x >> 3) as u64]));
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

#[test]
fn division_and_remainder_match_the_executor_at_each_budget() {
    for operation in ["udiv", "sdiv", "urem", "srem"] {
        for width in 1..=4 {
            binary_sweep(operation, width);
        }
    }
}

#[test]
fn final_quotient_trial_counts_only_the_borrow() {
    let module = load(
        r#"
memory 1024
func @divide(i8 %x, i8 %y) -> i8 {
entry:
    %answer = udiv i8 %x, %y
    ret %answer
}
"#,
    )
    .expect("valid unsigned quotient");
    let input = ApplyInput {
        module: &module,
        callee: module.function_named("divide").expect("callee"),
        args: &[Argument::Register(0..8), Argument::Register(8..16)],
        out: 16..24,
        fault: 24..29,
        budget: 2,
        entry_fault: None,
        stack_available: 0,
        scratch_start: 29,
    };
    let compiled = compile_apply_with(input, ApplyMode::Automatic).expect("emit quotient");
    let mut counter = CountingDevice::default();
    counter.allocate(0..29).expect("interface qubits");
    compiled.send(&mut counter).expect("count quotient gates");
    let counts = counter.counts();
    assert_eq!((counts.two_qubit, counts.toffoli, counts.peak_qubits), (2_331, 772, 520));
}

#[test]
fn division_faults_only_on_routes_that_attempt_it() {
    for operation in ["udiv", "sdiv", "urem", "srem"] {
        let module = load(&format!(
            r#"
memory 1024
func @f(i4 %x, i3 %d) -> i4 {{
entry:
    %take = trunc i4 %x to i1
    br %take, attempt, done(%x)
attempt:
    %divisor = sext i3 %d to i4
    %answer = {operation} i4 %x, %divisor
    br done(%answer)
done(i4 %result):
    ret %result
}}
"#
        ))
        .expect("valid conditional arithmetic");
        let args = [Argument::Register(0..4), Argument::Register(4..7)];
        for budget in 0..=7 {
            let input = ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("callee"),
                args: &args,
                out: 7..11,
                fault: 11..16,
                budget,
                entry_fault: None,
                stack_available: 0,
                scratch_start: 16,
            };
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let compiled = compile_apply_with(input.clone(), mode).expect("emit conditional division");
                let (gates, layout) = gates(&compiled, 7, 9);
                bits::check_states(&gates, layout, (0..128).map(|x| (x, 0x1a5)), |x| {
                    executor_result(&input, 4, &[(x & 15) as u64, (x >> 4) as u64])
                })
                .unwrap_or_else(|error| panic!("{operation}, {mode:?}, B={budget}: {error}"));
            }
        }
    }
}

#[test]
fn division_checks_the_full_width_and_wraps_signed_overflow() {
    for operation in ["udiv", "sdiv", "urem", "srem"] {
        let module = load(&format!(
            r#"
memory 1024
func @f(i2 %x, i2 %y) -> i8 {{
entry:
    %word = zext i2 %x to i128
    %dividend = shl i128 %word, 126
    %neg = sext i2 %y to i128
    %small = trunc i2 %y to i1
    %signed = zext i1 %small to i128
    %high = shl i128 %signed, 120
    %select = icmp eq i2 %y, 1
    %divisor = select i128 %select, %high, %neg
    %result = {operation} i128 %dividend, %divisor
    %upper = lshr i128 %result, 120
    %top = trunc i128 %upper to i8
    %bottom = trunc i128 %result to i8
    %answer = xor i8 %top, %bottom
    ret %answer
}}
"#
        ))
        .expect("valid wide arithmetic");
        let args = [Argument::Register(0..2), Argument::Register(2..4)];
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("callee"),
            args: &args,
            out: 4..12,
            fault: 12..17,
            budget: 14,
            entry_fault: None,
            stack_available: 0,
            scratch_start: 17,
        };
        let compiled = compile_apply_with(input.clone(), ApplyMode::Automatic).expect("emit wide division");
        let (gates, layout) = gates(&compiled, 4, 13);
        bits::check_states(&gates, layout, (0..16).map(|x| (x, 0x12a5)), |x| {
            executor_result(&input, 8, &[(x & 3) as u64, (x >> 2) as u64])
        })
        .unwrap_or_else(|error| panic!("{operation} i128: {error}"));
    }
}

#[test]
fn wide_signed_remainder_uses_the_dividend_sign_for_both_halves() {
    for offset in [0, 64] {
        let module = load(&format!(
            r#"
memory 1024
func @f(i2 %x, i3 %d) -> i64 {{
entry:
    %wide = sext i2 %x to i128
    %dividend = shl i128 %wide, 64
    %divisor = sext i3 %d to i128
    %remainder = srem i128 %dividend, %divisor
    %half = lshr i128 %remainder, {offset}
    %result = trunc i128 %half to i64
    ret %result
}}
"#
        ))
        .expect("valid wide signed remainder");
        let args = [Argument::Register(0..2), Argument::Register(2..5)];
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("callee"),
            args: &args,
            out: 5..69,
            fault: 69..74,
            budget: 7,
            entry_fault: None,
            stack_available: 0,
            scratch_start: 74,
        };
        let compiled = compile_apply_with(input.clone(), ApplyMode::Automatic).expect("emit wide signed remainder");
        let (gates, layout) = gates(&compiled, 5, 69);
        let cases = (0..32).flat_map(|x| [(x, 0), (x, (1 << 68) | 0x12a5)]);
        bits::check_states(&gates, layout, cases, |x| executor_result(&input, 64, &[(x & 3) as u64, (x >> 2) as u64]))
            .unwrap_or_else(|error| panic!("result half at bit {offset}: {error}"));
    }
}

#[test]
fn switch_cases_share_the_budget_and_transfer_pre_tick_arguments() {
    let module = load(
        r#"
memory 1024
func @f(i3 %n, i2 %x, i2 %y) -> i2 {
entry:
    %test = trunc i2 %x to i1
    br %test, loop(%n, %x, %y), done(%x)
loop(i3 %left, i2 %a, i2 %b):
    switch i3 %left, 0: done(%a), 1: done(%b), 2: next(%a, %b), default: next(%b, %a)
next(i2 %first, i2 %second):
    %count = sub i3 %left, 1
    %adjusted = add i2 %first, 1
    br loop(%count, %second, %adjusted)
done(i2 %result):
    ret %result
}
"#,
    )
    .expect("valid switch loop");
    let args = [Argument::Register(0..3), Argument::Register(3..5), Argument::Register(5..7)];
    for budget in 0..=29 {
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("callee"),
            args: &args,
            out: 7..9,
            fault: 9..14,
            budget,
            entry_fault: None,
            stack_available: 0,
            scratch_start: 14,
        };
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            let compiled = compile_apply_with(input.clone(), mode).expect("emit the switch loop");
            let (gates, layout) = gates(&compiled, 7, 7);
            let cases = (0..128).flat_map(|x| [(x, 0), (x, 0x55)]);
            bits::check_states(&gates, layout, cases, |x| {
                executor_result(&input, 2, &[(x & 7) as u64, (x >> 3 & 3) as u64, (x >> 5) as u64])
            })
            .unwrap_or_else(|error| panic!("{mode:?}, B={budget}: {error}"));
        }
    }
}

#[test]
fn switch_compares_high_bits_and_uses_default_when_no_case_matches() {
    let module = load(r#"
memory 1024
func @f(i3 %x) -> i2 {
entry:
    %sx = sext i3 %x to i128
    switch i128 %sx, 1: done(i2 1), 5: done(i2 2), 340282366920938463463374607431768211453: done(i2 3), default: done(i2 0)
done(i2 %result):
    ret %result
}
"#).expect("valid wide switch");
    let args = [Argument::Register(0..3)];
    for budget in 0..=4 {
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("callee"),
            args: &args,
            out: 3..5,
            fault: 5..10,
            budget,
            entry_fault: None,
            stack_available: 0,
            scratch_start: 10,
        };
        let compiled = compile_apply_with(input.clone(), ApplyMode::Automatic).expect("emit the wide switch");
        let (gates, layout) = gates(&compiled, 3, 7);
        bits::check_states(&gates, layout, (0..8).map(|x| (x, 0x55)), |x| executor_result(&input, 2, &[x as u64]))
            .expect("wide equality, default, XOR destinations and zero scratch");
    }
}

#[test]
fn switch_distinguishes_identical_low_halves() {
    for labels in
        ["0: done(i2 1), default: done(i2 2)", "0: done(i2 1), 18446744073709551616: done(i2 2), default: done(i2 3)"]
    {
        let source = format!(
            r#"
memory 1024
func @f(i1 %x) -> i2 {{
entry:
    %wide = zext i1 %x to i65
    %selector = shl i65 %wide, 64
    switch i65 %selector, {labels}
done(i2 %result):
    ret %result
}}
"#
        );
        let module = load(&source).expect("valid 65-bit switch");
        for budget in 0..=5 {
            let input = ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("callee"),
                args: &[Argument::Register(0..1)],
                out: 1..3,
                fault: 3..8,
                budget,
                entry_fault: None,
                stack_available: 0,
                scratch_start: 8,
            };
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let compiled = compile_apply_with(input.clone(), mode).expect("emit 65-bit switch");
                super::check(&compiled, 1, 7, |x| executor_result(&input, 2, &[x as u64]));
            }
        }
    }
}

#[test]
fn impossible_switch_edges_do_not_plan_indirect_calls() {
    for switch in [
        "switch i1 %bit, 0: done(i8 7), 1: done(i8 8), default: cold",
        "%wide = zext i1 %bit to i128\n    switch i128 %wide, 2: cold, default: done(i8 7)",
        "%wide = sext i1 %bit to i3\n    switch i3 %wide, 0: done(i8 7), 7: done(i8 8), default: cold",
    ] {
        let source = format!(
            r#"
memory 1024
func @f(i1 %bit, i2 %raw, i1 %enter) -> i8 {{
entry:
    br %enter, choose, done(i8 6)
choose:
    {switch}
cold:
    %address = zext i2 %raw to i64
    %value = call_indirect fn() -> i8 %address()
    ret %value
done(i8 %result):
    ret %result
}}
"#
        );
        let module = load(&source).expect("valid switch with an unreachable indirect call");
        for budget in 0..=5 {
            let input = ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("callee"),
                args: &[Argument::Register(0..1), Argument::Register(1..3), Argument::Register(3..4)],
                out: 4..12,
                fault: 12..17,
                budget,
                entry_fault: None,
                stack_available: 0,
                scratch_start: 17,
            };
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let compiled = compile_apply_with(input.clone(), mode).expect("skip the impossible indirect call");
                let (gates, layout) = gates(&compiled, 4, 13);
                let cases = (0..16).flat_map(|x| [(x, 0), (x, 0x12a5)]);
                bits::check_states(&gates, layout, cases, |x| {
                    executor_result(&input, 8, &[(x & 1) as u64, (x >> 1 & 3) as u64, (x >> 3) as u64])
                })
                .expect("result and fault match the executor and scratch is zero");
            }
        }
    }
}

#[test]
fn exhaustive_switch_does_not_schedule_an_impossible_default_loop() {
    let module = load(
        r#"
memory 1024
func @f(i2 %x) -> i2 {
entry:
    switch i2 %x, 0: done, 1: done, 2: done, 3: done, default: spin(i2 0)
done:
    ret %x
spin(i2 %value):
    %next = add i2 %value, %x
    br spin(%next)
}
"#,
    )
    .expect("valid exhaustive switch");
    let input = ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("callee"),
        args: &[Argument::Register(0..2)],
        out: 2..4,
        fault: 4..9,
        budget: 2,
        entry_fault: None,
        stack_available: 0,
        scratch_start: 9,
    };
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let reference = compile_apply_with(input.clone(), mode).expect("emit at the return budget");
        for budget in [2, 10, 1_000] {
            let input = ApplyInput { budget, ..input.clone() };
            let compiled = compile_apply_with(input.clone(), mode).expect("emit without the default loop");
            assert_eq!(compiled.construction, Construction::StepMachine { ticks: 2 });
            assert_eq!(compiled.block.ops(), reference.block.ops());
            super::check(&compiled, 2, 7, |x| executor_result(&input, 2, &[x as u64]));
        }
    }
}

#[test]
fn switch_counts_one_equality_chain_per_case() {
    let cases = 3;
    for width in [2, 3, 8] {
        let module = load(&format!(
            r#"
memory 1024
func @f(i{width} %x) -> i2 {{
entry:
    switch i{width} %x, 0: a, 1: b, 2: c, default: other
a:
    ret 1
b:
    ret 2
c:
    ret 3
other:
    ret 0
}}
"#
        ))
        .expect("valid switch");
        let x = width as u64;
        let args = [Argument::Register(0..x)];
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("callee"),
            args: &args,
            out: x..x + 2,
            fault: x + 2..x + 7,
            budget: 2,
            entry_fault: None,
            stack_available: 0,
            scratch_start: x + 7,
        };
        let compiled = compile_apply_with(input.clone(), ApplyMode::Automatic).expect("emit switch");
        assert_eq!(compiled.construction, Construction::StepMachine { ticks: 2 });
        let (gates, layout) = gates(&compiled, width, 7);
        assert_eq!(gates.iter().filter(|gate| gate.controls.len() == 2).count(), 2 * cases * (width - 1));
        bits::check_states(&gates, layout, (0..1 << width).map(|x| (x, 0x55)), |x| {
            executor_result(&input, 2, &[x as u64])
        })
        .expect("result, XOR destinations and zero scratch");
    }
}
