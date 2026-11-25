//! Constant-operand proofs and their interaction with untaken control-flow edges.

use super::{ApplyMode, Executor, Options, Outcome, compare_controls, control_example};

#[test]
fn previously_supported_product() {
    let module = control_example(
        r#"
memory 1024
func @f(i3 %input) -> i8 {
entry:
    br 1, live, cold
cold:
    br unused(i3 1)
unused(i3 %cold_n):
    %unused = shl i3 %input, %cold_n
    ret 0
live:
    %odd = trunc i3 %input to i1
    br %odd, prep, join(i3 0, i3 3)
prep:
    br 1, shift(i3 1), shift(i3 2)
shift(i3 %n):
    %v = shl i3 %input, %n
    br join(%v, i3 3)
join(i3 %k, i3 %factor):
    %product = mul i3 %k, %factor
    %out = zext i3 %product to i8
    ret %out
}
"#,
        3,
        "",
    );
    for budget in (0..=12).chain([u64::MAX]) {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare_controls(&module, budget, mode, |x| {
                let odd = x & 1;
                if budget < 6 + 3 * odd { (0, 1) } else { ((6 * x * odd) & 7, 0) }
            });
        }
    }
    check_proved_condition_keeps_dispatch_out_of_cold_code();
}

fn check_proved_condition_keeps_dispatch_out_of_cold_code() {
    let module = control_example(
        r#"
memory 1024
func @f(i2 %input) -> i8 {
entry:
    %low = trunc i2 %input to i1
    br %low, join(i1 1), longer
longer:
    %upper = lshr i2 %input, 1
    %high = trunc i2 %upper to i1
    br %high, join(i1 1), otherwise
join(i1 %good):
    br %good, done, cold(i2 1)
cold(i2 %amount):
    %shifted = shl i2 %input, %amount
    %out = zext i2 %shifted to i8
    ret %out
done:
    ret 3
otherwise:
    ret 7
}
"#,
        2,
        "",
    );
    for mode in [ApplyMode::StepMachine, ApplyMode::Automatic] {
        for budget in 0..=9 {
            compare_controls(&module, budget, mode, |x| {
                let (steps, result) = match x {
                    0 => (6, 7),
                    2 => (7, 3),
                    _ => (4, 3),
                };
                if budget < steps { (0, 1) } else { (result, 0) }
            });
        }
    }
}

#[test]
fn mixed_operand_proofs() {
    for (operation, instruction) in [
        ("shl", "%result = shl i3 1, %k\n    %out = zext i3 %result to i8\n    ret %out"),
        ("mul", "%result = mul i3 %input, %k\n    %out = zext i3 %result to i8\n    ret %out"),
        ("switch", "switch i3 %k, 0: zero, default: other\nzero:\n    ret 3\nother:\n    ret 7"),
    ] {
        let source = format!(
            r#"
memory 1024
func @f(i3 %input) -> i8 {{
entry:
    br 1, live, dead
dead:
    br shift(i3 2)
live:
    %odd = trunc i3 %input to i1
    br %odd, shift(i3 1), join(i3 0)
shift(i3 %n):
    %v = shl i3 %input, %n
    br join(%v)
join(i3 %k):
    {instruction}
}}
"#
        );
        let module = control_example(&source, 3, "");
        let entry = module.function_named("main").expect("main function");
        for budget in (0..=12).chain([u64::MAX]) {
            let options = Options { machine_meaning: true, ..Options::default() };
            let mut direct = Executor::new(&module, entry, &[budget], options).expect("direct simulation");
            assert_eq!(direct.run(), Outcome::Returned(None));
            let expected = |x: u64| {
                let odd = x & 1;
                let k = (2 * x * odd) & 7;
                let (steps, result) = match operation {
                    "shl" => (6 + 2 * odd, (1 << k) & 7),
                    "mul" => (6 + 2 * odd, (x * k) & 7),
                    "switch" => (5 + 2 * odd, if k == 0 { 3 } else { 7 }),
                    _ => unreachable!("test only generates shifts, multiplies and switches"),
                };
                if budget < steps { (0, 1) } else { (result, 0) }
            };
            let state = direct.snapshot();
            assert_eq!(state.quantum.amplitudes.len(), 8);
            for (basis, _) in state.quantum.amplitudes {
                let x = basis[0];
                let (out, fault) = expected(x);
                assert_eq!(basis, [x, x & 1, 4 ^ out, 16 ^ fault], "{operation}, B={budget}");
            }
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                compare_controls(&module, budget, mode, expected);
            }
        }
    }
}

#[test]
fn an_unreachable_shift_with_unknown_amount_does_not_affect_the_result() {
    let module = control_example(
        r#"
memory 1024
func @f(i3 %input) -> i8 {
entry:
    br 1, done, cold(i3 1)
island(i3 %unknown):
    %odd = trunc i3 %input to i1
    br %odd, island(%unknown), cold(%unknown)
cold(i3 %amount):
    %shifted = shl i3 %input, %amount
    ret 0
done:
    ret 7
}
"#,
        3,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        compare_controls(&module, 2, mode, |_| (7, 0));
    }
}

#[test]
fn indirect_dispatch_plans_targets_discovered_on_later_loop_visits() {
    use bennett_emit::{ApplyInput, Argument, Construction, compile_apply_with};
    use bennett_ir::load;

    let module = load(
        r#"
memory 1024
func @f(i1 %change) -> i8 {
entry:
    %first = faddr @first
    %second = faddr @second
    br visit(%first, i1 0)
island(i64 %pending):
    br %change, island(%pending), visit(%pending, i1 1)
visit(i64 %address, i1 %ready):
    br %ready, invoke, later
invoke:
    %answer = call_indirect fn() -> i8 %address()
    ret %answer
later:
    %next = select i64 %change, %second, %first
    br visit(%next, i1 1)
}
func @first() -> i8 {
entry:
    ret 7
}
func @second() -> i8 {
entry:
    ret 9
}
"#,
    )
    .expect("valid loop with a late call target and an unreachable cycle");
    let callee = module.function_named("f").expect("callee");
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in [0, 7, 8, 9, 10, 11] {
            let input = ApplyInput {
                module: &module,
                callee,
                args: &[Argument::Register(0..1)],
                out: 1..9,
                fault: 9..14,
                budget,
                entry_fault: None,
                stack_available: 512,
                memory: None,
                scratch_start: 14,
            };
            let compiled = compile_apply_with(input.clone(), mode).expect("plan both addresses");
            super::super::check(&compiled, 1, 13, |x| if budget < 10 { 1 << 8 } else { 7 + 2 * x });
            let fixed = ApplyInput { args: &[Argument::Value(0)], ..input };
            let compiled = compile_apply_with(fixed, mode).expect("the unreachable cycle cannot change the address");
            assert_eq!(compiled.construction, Construction::Definite);
            super::super::check(&compiled, 1, 13, |_| if budget < 10 { 1 << 8 } else { 7 });
        }
    }
}

#[test]
fn dead_selected() {
    for jump in [
        "br 1, join(i8 1), join(i8 2)",
        "br 0, join(i8 2), join(i8 1)",
        "switch i8 1, 1: join(i8 1), default: join(i8 2)",
    ] {
        let source = format!(
            r#"
memory 1024
func @f(i8 %input) -> i8 {{
entry:
    br 1, done, cold
cold:
    {jump}
join(i8 %amount):
    %shifted = shl i8 %input, %amount
    ret %shifted
done:
    ret %input
}}
"#
        );
        let module = control_example(&source, 8, "");
        for budget in [0, 4] {
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                compare_controls(&module, budget, mode, |x| if budget == 0 { (0, 1) } else { (x, 0) });
            }
        }
    }
}

fn check_computed_choice_in_bypassed_code(prefix: &str, return_at: u64) {
    let source = format!(
        r#"
{prefix}
bypassed:
    %equal = icmp eq i8 %word, %word
    br %equal, shift(i8 1), shift(i8 2)
shift(i8 %distance):
    %shifted = shl i8 %word, %distance
    ret %shifted
done:
    ret 7
}}
"#
    );
    let module = control_example(&source, 1, "");
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=return_at + 1 {
            compare_controls(&module, budget, mode, |_| if budget < return_at { (0, 1) } else { (7, 0) });
        }
    }
}

#[test]
fn a_call_bound_preserves_the_computed_choice_in_a_bypassed_block() {
    check_computed_choice_in_bypassed_code(
        r#"
memory 1024
func @f(i1 %input) -> i8 {
entry:
    %argument = zext i1 %input to i8
    %result = call @helper(%argument)
    ret %result
}
func @helper(i8 %word) -> i8 {
entry:
    %high = lshr i8 %word, 1
    %fits = icmp eq i8 %high, 0
    br %fits, done, bypassed
"#,
        7,
    );
}

#[test]
fn a_product_bound_preserves_the_computed_choice_in_a_bypassed_block() {
    check_computed_choice_in_bypassed_code(
        r#"
memory 1024
func @f(i1 %input) -> i8 {
entry:
    %word = zext i1 %input to i8
    %square = mul i8 %word, %word
    %high = lshr i8 %square, 1
    %fits = icmp eq i8 %high, 0
    br %fits, done, bypassed
"#,
        6,
    );
}
