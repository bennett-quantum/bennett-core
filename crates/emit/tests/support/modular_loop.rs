//! Prove the update and its whole-function budget, or use ordinary lowering.

use super::{ApplyMode, Construction, compare_controls, control_example};
use bennett_ir::Fault;

const SOURCE: &str = r#"
memory 1024
func @f(i3 %exponent) -> i8 {
entry:
    %empty = icmp eq i3 %exponent, 0
    br %empty, finish(i8 1), head(i8 1, i8 7, %exponent)
head(i8 %old, i8 %base, i3 %x):
    %low = trunc i3 %x to i1
    br %low, update, latch(%old)
update:
    %product = mul i8 %old, %base
    %reduced = urem i8 %product, 15
    br latch(%reduced)
latch(i8 %current):
    %next = lshr i3 %x, 1
    %square = mul i8 %base, %base
    %factor = urem i8 %square, 15
    %last = icmp ult i3 %x, 2
    br %last, finish(%current), head(%current, %factor, %next)
finish(i8 %residue):
    ret %residue
}
"#;

#[derive(Clone, Copy)]
enum Update {
    Odd,
    Even,
    Always,
}

fn reference(mut x: u64, mut residue: u8, mut base: u8, modulus: u8, update: Update) -> u64 {
    while x != 0 {
        let multiply = match update {
            Update::Odd => x & 1 != 0,
            Update::Even => x & 1 == 0,
            Update::Always => true,
        };
        if multiply {
            residue = residue.wrapping_mul(base) % modulus;
        }
        base = base.wrapping_mul(base) % modulus;
        x >>= 1;
    }
    u64::from(residue)
}

fn through_helper() -> String {
    const HELPER: &str = r#"
func @multiply(i8 %a, i8 %b, i8 %modulus) -> i8 {
entry:
    %product = mul i8 %a, %b
    %reduced = urem i8 %product, %modulus
    ret %reduced
}
"#;
    SOURCE
        .replace(
            "%product = mul i8 %old, %base\n    %reduced = urem i8 %product, 15",
            "%reduced = call @multiply(%old, %base, i8 15)",
        )
        .replace(
            "%square = mul i8 %base, %base\n    %factor = urem i8 %square, 15",
            "%factor = call @multiply(%base, %base, i8 15)",
        )
        + HELPER
}

#[test]
fn direct_and_helper_updates_use_one_budget_at_all_return_boundaries() {
    let helper = through_helper();
    // The direct body costs seven steps plus three for a taken update.
    // The two helper sites add a call and a return to each product.
    for (source, pass_steps, update_steps) in [(SOURCE, 7, 3), (helper.as_str(), 9, 5)] {
        let module = control_example(source, 3, "");
        let returns = |x: u64| 3 + pass_steps * u64::from(x.ilog2() + 1) + update_steps * u64::from(x.count_ones());
        let longest = returns(7);
        for budget in 0..=longest + 1 {
            let expected = |x| {
                let steps = if x == 0 { 3 } else { returns(x) };
                if budget < steps { (0, Fault::Budget.code()) } else { (reference(x, 1, 7, 15, Update::Odd), 0) }
            };
            let (construction, _) = compare_controls(&module, budget, ApplyMode::Automatic, expected);
            if budget >= longest {
                assert_eq!(construction, Construction::ModularInPlace { passes: 3 });
            } else {
                assert!(!matches!(construction, Construction::ModularInPlace { .. }));
            }
            if budget == longest {
                let (forced, _) = compare_controls(&module, budget, ApplyMode::StepMachine, expected);
                assert!(matches!(forced, Construction::StepMachine { .. }));
            }
        }
    }
}

#[test]
fn a_helper_without_stack_space_keeps_its_fault() {
    let source = through_helper();
    let roomy = control_example(&source, 3, "");
    let frame = |name| {
        let function = roomy.function_named(name).expect("function");
        bennett_exec::frame_bytes(roomy.function(function).slot_count()).expect("frame size")
    };
    let capacity = frame("main") + frame("f");
    let tight = source.replacen("memory 1024", &format!("memory {capacity}"), 1);
    let module = control_example(&tight, 3, "");
    let (construction, _) = compare_controls(&module, 45, ApplyMode::Automatic, |x| {
        if x == 0 { (1, 0) } else { (0, Fault::StackOverflow.code()) }
    });
    assert!(!matches!(construction, Construction::ModularInPlace { .. }));
}

fn unguarded_source() -> String {
    SOURCE.replace("    %low = trunc i3 %x to i1\n    br %low, update, latch(%old)\nupdate:\n", "")
}

#[test]
fn inactive_rounds_preserve_residue_for_even_and_unconditional_updates() {
    let even = SOURCE.replace("br %low, update, latch(%old)", "br %low, latch(%old), update");
    for (source, update) in [(even, Update::Even), (unguarded_source(), Update::Always)] {
        let module = control_example(&source, 3, "");
        let (construction, _) =
            compare_controls(&module, 40, ApplyMode::Automatic, |x| (reference(x, 1, 7, 15, update), 0));
        assert_eq!(construction, Construction::ModularInPlace { passes: 3 });
    }
}

#[test]
fn a_value_forwarded_to_an_exit_prevents_reuse() {
    let unguarded = unguarded_source();
    for live in ["%product", "%old"] {
        let source =
            unguarded.replace("%last, finish(%current)", &format!("%last, with_copy(%current, {live})")).replace(
                "\nfinish(i8 %residue):",
                "\nwith_copy(i8 %answer, i8 %also):\n    ret %answer\nfinish(i8 %residue):",
            );
        let module = control_example(&source, 3, "");
        let (construction, _) =
            compare_controls(&module, 40, ApplyMode::Automatic, |x| (reference(x, 1, 7, 15, Update::Always), 0));
        assert!(!matches!(construction, Construction::ModularInPlace { .. }), "{live}");
    }
}

#[test]
fn header_test_and_constant_argument_keep_the_same_residue_update() {
    let source = SOURCE
        .replace("func @f(i3 %exponent)", "func @f(i3 %exponent, i8 %modulus)")
        .replace(
            "    %empty = icmp eq i3 %exponent, 0\n    br %empty, finish(i8 1), head(i8 1, i8 7, %exponent)\nhead(i8 %old, i8 %base, i3 %x):",
            "    br head(i8 1, i8 7, %exponent)\nhead(i8 %old, i8 %base, i3 %x):\n    %empty = icmp eq i3 %x, 0\n    br %empty, finish(%old), choose\nchoose:",
        )
        .replace(", 15", ", %modulus")
        .replace(
            "    %last = icmp ult i3 %x, 2\n    br %last, finish(%current), head(%current, %factor, %next)",
            "    br head(%current, %factor, %next)",
        );
    let module = control_example(&source, 3, ", i8 15");
    const WHOLE_BUDGET: u64 = 37; // Entry jump, initial test, three eleven-step passes, and return.
    let (construction, _) =
        compare_controls(&module, WHOLE_BUDGET, ApplyMode::Automatic, |x| (reference(x, 1, 7, 15, Update::Odd), 0));
    assert_eq!(construction, Construction::ModularInPlace { passes: 3 });
}

#[test]
fn unproved_updates_preserve_the_ordinary_result_or_fault() {
    let nonunit = SOURCE.replace(", 15", ", 21");
    let out_of_range = SOURCE.replace("i8 1)", "i8 15)").replace("head(i8 1,", "head(i8 15,");
    let overflow = SOURCE.replace("i8 7,", "i8 17,").replace(", 15", ", 20");
    let product_used = SOURCE.replace("    %reduced = urem", "    %extra = add i8 %product, 1\n    %reduced = urem");
    let old_used = SOURCE.replace("    %next = lshr", "    %extra = add i8 %old, 1\n    %next = lshr");
    for (source, seed, base, modulus) in [
        (nonunit, 1, 7, 21),
        (out_of_range, 15, 7, 15),
        (overflow, 1, 17, 20),
        (product_used, 1, 7, 15),
        (old_used, 1, 7, 15),
    ] {
        let module = control_example(&source, 3, "");
        let (construction, _) = compare_controls(&module, 40, ApplyMode::Automatic, |x| {
            (reference(x, seed, base, modulus, Update::Odd), 0)
        });
        assert!(!matches!(construction, Construction::ModularInPlace { .. }));
    }

    let source = SOURCE.replace(", 15", ", 0");
    let module = control_example(&source, 3, "");
    let (construction, _) = compare_controls(&module, 40, ApplyMode::Automatic, |x| {
        if x == 0 { (1, 0) } else { (0, Fault::DivideByZero.code()) }
    });
    assert!(!matches!(construction, Construction::ModularInPlace { .. }));

    let variable = SOURCE
        .replace("    %empty =", "    %variable = zext i3 %exponent to i8\n    %empty =")
        .replace("head(i8 1, i8 7,", "head(i8 1, %variable,");
    let module = control_example(&variable, 3, "");
    let (construction, _) =
        compare_controls(&module, 40, ApplyMode::Automatic, |x| (reference(x, 1, x as u8, 15, Update::Odd), 0));
    assert!(!matches!(construction, Construction::ModularInPlace { .. }));

    let truncation = SOURCE
        .replace("i8", "i32")
        .replace("-> i32", "-> i8")
        .replace("i32 7,", "i32 256,")
        .replace(", 15", ", 257")
        .replace("    ret %residue", "    %narrow = trunc i32 %residue to i8\n    ret %narrow");
    let module = control_example(&truncation, 3, "");
    let (construction, _) = compare_controls(&module, 40, ApplyMode::Automatic, |x| (u64::from(x & 1 == 0), 0));
    assert!(!matches!(construction, Construction::ModularInPlace { .. }));
}
