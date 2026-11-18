//! Constant-operand proofs and their interaction with untaken control-flow edges.

use super::{
    ApplyError, ApplyMode, Argument, Executor, Options, Outcome, apply_input, compare_controls, compile_apply_with,
    control_example, load,
};

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
    let mut incorrectly_accepted = Vec::new();
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
        let args = [Argument::Register(0..3)];
        for budget in (0..=12).chain([u64::MAX]) {
            let options = Options { machine_meaning: true, ..Options::default() };
            let mut direct = Executor::new(&module, entry, &[budget], options).expect("direct simulation");
            assert_eq!(direct.run(), Outcome::Returned(None));
            let state = direct.snapshot();
            assert_eq!(state.quantum.amplitudes.len(), 8);
            for (basis, _) in state.quantum.amplitudes {
                let x = basis[0];
                let odd = x & 1;
                let k = (2 * x * odd) & 7;
                let (steps, result) = match operation {
                    "shl" => (6 + 2 * odd, (1 << k) & 7),
                    "mul" => (6 + 2 * odd, (x * k) & 7),
                    "switch" => (5 + 2 * odd, if k == 0 { 3 } else { 7 }),
                    _ => unreachable!("test only generates shifts, multiplies and switches"),
                };
                let (out, fault) = if budget < steps { (0, 1) } else { (result, 0) };
                assert_eq!(basis, [x, odd, 4 ^ out, 16 ^ fault], "{operation}, B={budget}");
            }

            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                if operation == "mul" {
                    compare_controls(&module, budget, mode, |x| {
                        let odd = x & 1;
                        let k = (2 * x * odd) & 7;
                        if budget < 6 + 2 * odd { (0, 1) } else { ((x * k) & 7, 0) }
                    });
                    continue;
                }
                match compile_apply_with(apply_input(&module, &args, 3, budget), mode) {
                    Ok(circuit) => incorrectly_accepted.push((operation, budget, mode, circuit.construction)),
                    Err(ApplyError::Unsupported { operation: actual, .. }) => assert_eq!(actual, operation),
                    Err(error) => panic!("{operation}, B={budget}, {mode:?}: {error}"),
                }
            }
        }
    }
    assert!(incorrectly_accepted.is_empty(), "accepted unproved operands: {incorrectly_accepted:?}");
}

#[test]
fn a_pending_cold_predecessor_does_not_prove_a_constant() {
    let module = load(
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
    )
    .expect("valid function with an unreachable cycle");
    let args = [Argument::Register(0..3)];
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let error = compile_apply_with(apply_input(&module, &args, 3, 2), mode).expect_err("unproved cold operand");
        assert!(matches!(error, ApplyError::Unsupported { operation: "shl", .. }));
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
