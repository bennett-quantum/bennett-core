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

fn return_steps(x: u64, fixed_steps: u64, pass_steps: u64, update_steps: u64) -> u64 {
    let passes = u64::from(u64::BITS - x.leading_zeros());
    fixed_steps + pass_steps * passes + update_steps * u64::from(x.count_ones())
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
        let returns = |x| return_steps(x, 3, pass_steps, update_steps);
        let longest = returns(7);
        for budget in 0..=longest + 1 {
            let expected = |x| {
                if budget < returns(x) { (0, Fault::Budget.code()) } else { (reference(x, 1, 7, 15, Update::Odd), 0) }
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
fn zero_exponent_exit_can_read_the_entry_comparison() {
    let longest = return_steps(7, 3, 7, 3);
    for seed in [0, 1] {
        let source = SOURCE.replace("finish(i8 1), head(i8 1,", &format!("zero_result, head(i8 {seed},")).replace(
            "\nfinish(i8 %residue):",
            "\nzero_result:\n    %predicate = zext i1 %empty to i8\n    ret %predicate\nfinish(i8 %residue):",
        );
        let module = control_example(&source, 3, "");
        for budget in 0..=longest + 1 {
            let expected = |x| {
                let steps = if x == 0 { 4 } else { return_steps(x, 3, 7, 3) };
                if budget < steps {
                    (0, Fault::Budget.code())
                } else if x == 0 {
                    (1, 0)
                } else {
                    (reference(x, seed, 7, 15, Update::Odd), 0)
                }
            };
            let (construction, _) = compare_controls(&module, budget, ApplyMode::Automatic, expected);
            assert_eq!(matches!(construction, Construction::ModularInPlace { .. }), seed == 1 && budget >= longest);
            if budget == longest {
                compare_controls(&module, budget, ApplyMode::StepMachine, expected);
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
    // Entry jump, initial test and return cost four steps. Each pass
    // includes the next header test, with three extra steps on an update.
    let longest = return_steps(7, 4, 8, 3);
    for budget in 0..=longest + 1 {
        let (construction, _) = compare_controls(&module, budget, ApplyMode::Automatic, |x| {
            if budget < return_steps(x, 4, 8, 3) {
                (0, Fault::Budget.code())
            } else {
                (reference(x, 1, 7, 15, Update::Odd), 0)
            }
        });
        if budget >= longest {
            assert_eq!(construction, Construction::ModularInPlace { passes: 3 });
        } else {
            assert!(!matches!(construction, Construction::ModularInPlace { .. }));
        }
    }
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

fn private_result(store_at: u64, load_at: u64) -> String {
    let kernel = SOURCE.replace("@f(i3 %exponent)", "@kernel(i3 %exponent, i1 %unused)");
    format!(
        r#"
{kernel}
data 48 = i64 @kernel
func @f(i3 %x) -> i8 {{
entry:
    store i3 %x, 16
    %arg = load i3 16
    %pointer = load i64 48
    %result = call_indirect fn(i3, i1) -> i8 %pointer(%arg, i1 0)
    store i8 %result, {store_at}
    %out = load i8 {load_at}
    ret %out
}}
"#
    )
}

#[test]
fn modular_private_return_uses_the_wrapper_budget() {
    let module = control_example(&private_result(24, 24), 3, "");
    // The wrapper adds four steps before the kernel and three on return.
    let returns = |x| return_steps(x, 3 + 7, 7, 3);
    let longest = returns(7);
    for budget in [longest - 1, longest, longest + 1] {
        let (construction, _) = compare_controls(&module, budget, ApplyMode::Automatic, |x| {
            if budget < returns(x) { (0, Fault::Budget.code()) } else { (reference(x, 1, 7, 15, Update::Odd), 0) }
        });
        assert_eq!(matches!(construction, Construction::ModularInPlace { passes: 3 }), budget >= longest);
    }
}

#[test]
fn modular_private_return_checks_the_last_byte_and_one_past_it() {
    for address in [1023, 1024] {
        let module = control_example(&private_result(address, address), 3, "");
        let inside = address == 1023;
        let (construction, _) = compare_controls(&module, 50, ApplyMode::Automatic, |x| {
            if inside { (reference(x, 1, 7, 15, Update::Odd), 0) } else { (0, Fault::OutOfBounds.code()) }
        });
        assert_eq!(matches!(construction, Construction::ModularInPlace { passes: 3 }), inside);
    }
}

#[test]
fn modular_private_return_requires_the_original_result_bits() {
    let source = private_result(24, 25).replace("store i3 %x, 16", "store i8 1, 25\n    store i3 %x, 16");
    let module = control_example(&source, 3, "");
    let (construction, _) = compare_controls(&module, 50, ApplyMode::Automatic, |_| (1, 0));
    assert!(matches!(construction, Construction::StepMachine { .. }));
}

fn compile_private_loop(module: &bennett_ir::ValidModule, mode: ApplyMode) -> bennett_emit::CompiledApply {
    compile_private_loop_at(module, mode, 16).expect("compile wrapped loop")
}

fn compile_private_loop_at(
    module: &bennett_ir::ValidModule,
    mode: ApplyMode,
    scratch_start: u64,
) -> Result<bennett_emit::CompiledApply, bennett_emit::ApplyError> {
    compile_private_loop_with_budget(module, mode, scratch_start, 50)
}

fn compile_private_loop_with_budget(
    module: &bennett_ir::ValidModule,
    mode: ApplyMode,
    scratch_start: u64,
    budget: u64,
) -> Result<bennett_emit::CompiledApply, bennett_emit::ApplyError> {
    use bennett_emit::{ApplyInput, Argument, compile_apply_with};
    use bennett_exec::ApplyMemory;

    let mut bytes = [0; 1024];
    for data in &module.data {
        let address = data.address as usize;
        bytes[address..address + data.len() as usize].copy_from_slice(&data.encode());
    }
    let held = Default::default();
    compile_apply_with(
        ApplyInput {
            module,
            callee: module.function_named("f").expect("wrapper"),
            args: &[Argument::Register(0..3)],
            out: 3..11,
            fault: 11..16,
            budget,
            entry_fault: None,
            stack_available: 800,
            memory: Some(ApplyMemory::new(&bytes, &held)),
            scratch_start,
        },
        mode,
    )
}

#[test]
fn modular_exit_preserves_the_identity_of_a_reloaded_input() {
    let source = private_result(24, 24).replace(
        "    ret %out",
        "    %equal = icmp eq i3 %arg, %x\n    br %equal, done, mismatch\ndone:\n    ret %out\nmismatch:\n    ret 99",
    );
    let module = control_example(&source, 3, "");
    let (construction, _) =
        compare_controls(&module, 50, ApplyMode::Automatic, |x| (reference(x, 1, 7, 15, Update::Odd), 0));
    assert_eq!(construction, Construction::ModularInPlace { passes: 3 });
    let compiled = compile_private_loop_at(&module, ApplyMode::Automatic, u64::MAX - 32)
        .expect("the in-place circuit fits in 32 scratch wires");
    assert_eq!(compiled.construction, construction);
}

#[test]
fn modular_proof_checks_the_zero_exit_only_when_reachable() {
    for high_bit in [0, 4] {
        let source = private_result(24, 24)
            .replace("store i3 %x, 16", &format!("%exponent = or i3 %x, {high_bit}\n    store i3 %exponent, 16"))
            .replace("finish(i8 1), head", "finish(i8 99), head");
        let module = control_example(&source, 3, "");
        let (construction, _) = compare_controls(&module, 50, ApplyMode::Automatic, |x| {
            let exponent = x | high_bit;
            (if exponent == 0 { 99 } else { reference(exponent, 1, 7, 15, Update::Odd) }, 0)
        });
        assert_eq!(matches!(construction, Construction::ModularInPlace { passes: 3 }), high_bit != 0);
    }
}

#[test]
fn a_winning_modular_wrapper_bounds_both_candidate_walks() {
    let source = private_result(24, 24)
        .replace("store i3 %x, 16", "%exponent = or i3 %x, 4\n    store i3 %exponent, 16")
        .replace("finish(i8 1), head", "finish(i8 99), head");
    let module = control_example(&source, 3, "");
    for (budget, generated, compared) in
        [(41, 37_658, 67_111), (50, 55_105, 101_983), (100, 195_136, 381_853), (200, 489_536, 970_253)]
    {
        let compiled =
            compile_private_loop_with_budget(&module, ApplyMode::Automatic, 16, budget).expect("compile the wrapper");
        assert_eq!(compiled.construction, Construction::ModularInPlace { passes: 3 }, "B={budget}");
        assert_eq!(compiled.block.ops().len(), 5_752, "B={budget}");
        assert!(compiled.work.gates_generated <= generated, "B={budget}: {:?}", compiled.work);
        assert_eq!(compiled.work.comparison_gates, compared, "B={budget}");
    }
}

#[test]
fn a_definitely_nonzero_exponent_keeps_the_controlled_circuit_when_it_fits() {
    for (base, operations, scratch, toffolis, comparison) in [(2, 6, 0, 0, 0), (7, 1482, 164, 240, 7_230)] {
        let source = nonzero_exponent_pair(SOURCE);
        let source = source.replace("finish(i8 1), head(i8 1, i8 7,", &format!("finish(i8 99), head(i8 1, i8 {base},"));
        let module = control_example(&source, 3, "");
        let (construction, _) = compare_controls(&module, 50, ApplyMode::Automatic, |x| {
            (reference(2 | (x & 1), 1, base, 15, Update::Odd), 0)
        });
        let compiled = compile_private_loop(&module, ApplyMode::Automatic);
        let measured = circuit_size(&compiled);
        println!("base {base}, exponent 2 or 3: {construction:?}, {measured:?} operations/scratch/Toffolis");
        assert_eq!(measured, (operations, scratch, toffolis), "base {base}");
        assert_eq!(construction, Construction::Controlled { proved_passes: 0 });
        assert_eq!(compiled.work.comparison_gates, comparison, "base {base}");
        if base == 7 {
            let limited = compile_private_loop_at(&module, ApplyMode::Automatic, u64::MAX - 32)
                .expect("the in-place update fits when the controlled circuit cannot");
            assert_eq!(limited.construction, Construction::ModularInPlace { passes: 2 });
            assert_eq!(limited.work.comparison_gates, 0);
        }
    }
}

#[test]
fn unreachable_product_uses_leave_the_fitting_modular_circuit_eligible() {
    let source = nonzero_exponent_pair(SOURCE).replace("finish(i8 1), head", "finish(i8 99), head");
    let control = control_example(&source, 3, "");
    let expected = compile_private_loop_at(&control, ApplyMode::Automatic, u64::MAX - 32).expect("the control fits");
    assert_eq!(expected.construction, Construction::ModularInPlace { passes: 2 });
    for value in ["0", "%product"] {
        let extended = source.replace("    ret %residue\n}", &format!("    ret %residue\ncold:\n    ret {value}\n}}"));
        let module = control_example(&extended, 3, "");
        let compiled = compile_private_loop_at(&module, ApplyMode::Automatic, u64::MAX - 32)
            .expect("a cold return leaves the in-place circuit eligible");
        assert_eq!(compiled.construction, expected.construction);
        assert_eq!(compiled.block.ops(), expected.block.ops());
    }
}

fn nonzero_exponent_pair(source: &str) -> String {
    source
        .replace("i3 %exponent) ->", "i3 %input) ->")
        .replace("    %empty =", "    %bit = and i3 %input, 1\n    %exponent = or i3 %bit, 2\n    %empty =")
}

fn circuit_size(compiled: &bennett_emit::CompiledApply) -> (usize, u64, usize) {
    use bennett_exec::BlockOp;

    let [BlockOp::Allocate(span), gates @ .., BlockOp::ReleaseZero(released)] = compiled.block.ops() else {
        panic!("expected one scratch lifetime");
    };
    assert_eq!(span, released);
    let toffolis = gates.iter().filter(|op| matches!(op, BlockOp::Gate(gate) if gate.controls.len() == 2)).count();
    (compiled.block.ops().len(), span.end - span.start, toffolis)
}

#[test]
fn a_modular_helper_can_keep_the_cheaper_step_machine_circuit() {
    let source = nonzero_exponent_pair(&through_helper())
        .replace("i8", "i16")
        .replacen("-> i16", "-> i8", 1)
        .replace("finish(i16 1), head(i16 1, i16 7,", "finish(i16 99), head(i16 1, i16 2,")
        .replace("i16 15", "i16 127")
        .replace("    ret %residue", "    %out = trunc i16 %residue to i8\n    ret %out");
    let module = control_example(&source, 3, "");
    let (construction, _) = compare_controls(&module, 50, ApplyMode::Automatic, |x| (4 << (x & 1), 0));
    let compiled = compile_private_loop(&module, ApplyMode::Automatic);
    let measured = circuit_size(&compiled);
    println!("base 2 modulo 127 with helper: {construction:?}, {measured:?} operations/scratch/Toffolis");
    assert_eq!(measured, (22, 11, 1));
    assert!(matches!(construction, Construction::StepMachine { .. }));
}

#[test]
fn modular_proof_counts_the_definite_first_iteration() {
    let source = private_result(24, 24).replace("store i3 %x, 16", "%odd = or i3 %x, 1\n    store i3 %odd, 16");
    let module = control_example(&source, 3, "");
    let automatic = compile_private_loop(&module, ApplyMode::Automatic);
    let dispatch = compile_private_loop(&module, ApplyMode::StepMachine);
    assert_eq!(automatic.construction, Construction::ModularInPlace { passes: 3 });
    assert_eq!(dispatch.definite_steps, 16);
    assert_eq!(automatic.definite_steps, dispatch.definite_steps);
}

#[test]
fn modular_proof_leaves_a_fixed_trip_count_in_the_definite_prefix() {
    let source = private_result(24, 24)
        .replace("    %low = trunc i3 %x to i1\n    br %low, update, latch(%old)\nupdate:\n", "")
        .replace("store i3 %x, 16", "%full = or i3 %x, 4\n    store i3 %full, 16");
    let module = control_example(&source, 3, "");
    let expected = |x| (reference(x | 4, 1, 7, 15, Update::Always), 0);
    let (construction, _) = compare_controls(&module, 50, ApplyMode::Automatic, expected);
    assert_eq!(construction, Construction::Definite);
    let automatic = compile_private_loop(&module, ApplyMode::Automatic);
    let dispatch = compile_private_loop(&module, ApplyMode::StepMachine);
    assert_eq!(automatic.block.ops(), dispatch.block.ops());
    assert!(
        automatic
            .block
            .ops()
            .iter()
            .filter(|op| matches!(op, bennett_exec::BlockOp::Gate(gate) if gate.controls.len() == 2))
            .count()
            <= 2
    );
}

#[test]
fn modular_proof_places_only_the_chosen_circuit_in_the_caller_range() {
    use bennett_emit::{ApplyError, ApplyInput, Argument, compile_apply_with};
    use bennett_exec::BlockOp;

    let source = SOURCE.replace("i3 ", "i12 ").replace("i8 7,", "i8 2,").replace(", 15", ", 3");
    let module = bennett_ir::load(&source).expect("wide exponent loop");
    for interface in [0, u64::MAX - 37] {
        let limit = |room| ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("loop"),
            args: &[],
            out: interface + 12..interface + 20,
            fault: interface + 20..interface + 25,
            budget: 130,
            entry_fault: None,
            stack_available: 800,
            memory: None,
            scratch_start: u64::MAX - room,
        };
        let args = [Argument::Register(interface..interface + 12)];
        for room in [9, 10, 11] {
            let input = ApplyInput { args: &args, ..limit(room) };
            let compiled = compile_apply_with(input, ApplyMode::Automatic);
            if room == 9 {
                assert!(matches!(compiled, Err(ApplyError::WireSpace)));
                continue;
            }
            let compiled = compiled.expect("the in-place update fits");
            assert_eq!(compiled.definite_steps, 1);
            assert_eq!(compiled.construction, Construction::ModularInPlace { passes: 12 });
            let [BlockOp::Allocate(scratch), ops @ .., BlockOp::ReleaseZero(released)] = compiled.block.ops() else {
                panic!("expected one scratch lifetime");
            };
            assert_eq!(scratch, released);
            assert_eq!(scratch.clone(), u64::MAX - room..u64::MAX - room + 10);
            let compact = |wire| {
                if scratch.contains(&wire) {
                    25 + (wire - scratch.start)
                } else {
                    assert!((interface..interface + 25).contains(&wire));
                    wire - interface
                }
            };
            let gates = ops
                .iter()
                .map(|op| {
                    let BlockOp::Gate(gate) = op else { panic!("expected a gate") };
                    let mut gate = gate.clone();
                    gate.target = compact(gate.target);
                    for control in &mut gate.controls {
                        *control = compact(*control);
                    }
                    gate
                })
                .collect::<Vec<_>>();
            crate::bits::check_states(
                &gates,
                crate::bits::Layout { x: 12, y: 13, scratch: 10 },
                (0..4096).flat_map(|x| [(x, 0), (x, 0x11ca)]),
                |x| 1 + (x & 1),
            )
            .expect("preserve the interface and clear scratch");
        }
    }
}

#[test]
fn a_residue_selecting_a_private_byte_needs_its_own_replay_domain() {
    let source = private_result(24, 24).replace("data 48", "data 24 = i16 0\ndata 48").replace(
        "%out = load i8 24",
        "%seven = icmp eq i8 %result, 7\n    %addr = select i64 %seven, 25, 24\n    %out = load i8 %addr",
    );
    let module = control_example(&source, 3, "");
    compare_controls(&module, 50, ApplyMode::Automatic, |x| {
        let residue = reference(x, 1, 7, 15, Update::Odd);
        (if residue == 7 { 0 } else { residue }, 0)
    });
}
