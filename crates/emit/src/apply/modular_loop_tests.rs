//! Work counts for the speculative parts of the modular proof.

use std::fmt::Write;

use bennett_ir::load;

use crate::{ApplyInput, ApplyMode, Argument, Construction, WorkCounts, compile_apply_with};

fn proof_work(source: &str, budget: u64) -> WorkCounts {
    let module = load(source).expect("valid work-count program");
    let compiled = compile_apply_with(
        ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("applied function"),
            args: &[Argument::Register(0..3)],
            out: 3..11,
            fault: 11..16,
            budget,
            entry_fault: None,
            stack_available: 4096,
            memory: None,
            scratch_start: 16,
        },
        ApplyMode::Automatic,
    )
    .expect("compile work-count program");
    assert_eq!(compiled.construction, Construction::Definite);
    compiled.work
}

#[test]
fn modular_proof_skips_exit_work_before_the_loop_can_run() {
    let mut source = r#"
memory 8192
func @kernel(i3 %input) -> i8 {
entry:
    %none = icmp eq i3 %input, 0
    br %none, done(i8 1), body(i8 1, i8 7, %input)
body(i8 %old, i8 %base, i3 %count):
    %product = mul i8 %old, %base
    %next = urem i8 %product, 15
    br latch(%next)
latch(i8 %result):
    %left = lshr i3 %count, 1
    %square = mul i8 %base, %base
    %factor = urem i8 %square, 15
    %finished = icmp eq i3 %left, 0
    br %finished, done(%result), body(%result, %factor, %left)
done(i8 %value):
    ret %value
}
func @f(i3 %x) -> i8 {
entry:
    %residue = call @kernel(%x)
    %v0 = zext i8 %residue to i32
"#
    .to_owned();
    for power in 0..48 {
        writeln!(&mut source, "    %v{} = mul i32 %v{power}, %v{power}", power + 1).expect("append a square");
    }
    source.push_str("    %out = trunc i32 %v48 to i8\n    ret %out\n}\n");
    // The call and the entry comparison fit. The jump and returns do not.
    let work = proof_work(&source, 2);
    assert_eq!(work.exit_replay_gates, 0);
}

#[test]
fn modular_proof_does_not_copy_slots_for_an_ordinary_countdown() {
    let mut source = "memory 8192\nfunc @f(i3 %x) -> i8 {\nentry:\n".to_owned();
    for index in 0..16 {
        writeln!(&mut source, "    %p{index} = zext i3 %x to i64").expect("append a slot");
    }
    source.push_str(
        r#"
    br check(i8 64)
check(i8 %left):
    %done = icmp eq i8 %left, 0
    br %done, finish, tick
tick:
    %next = sub i8 %left, 1
    br check(%next)
finish:
    %out = trunc i64 %p0 to i8
    ret %out
}
"#,
    );
    assert_eq!(proof_work(&source, 300).snapshot_slots, 0);
}

#[test]
fn known_high_bit_in_a_wide_modular_loop_leaves_no_comparison_gates() {
    let compiled = definite_wide_loop();
    assert_eq!(compiled.definite_steps, 1161);
    assert_eq!(compiled.block.ops().len(), 3);
    assert!(matches!(
        compiled.block.ops(),
        [bennett_exec::BlockOp::Allocate(empty), bennett_exec::BlockOp::Gate(gate), bennett_exec::BlockOp::ReleaseZero(end)]
        if empty.is_empty() && empty == end && gate.target == 128 && gate.controls.is_empty()
    ));
}

fn definite_wide_loop() -> crate::CompiledApply {
    let module = load(
        r#"
memory 8192
func @f(i64 %lower, i63 %upper) -> i8 {
entry:
    %low = zext i64 %lower to i128
    %high = zext i63 %upper to i128
    %offset = shl i128 %high, 64
    %wide = or i128 %low, %offset
    %count = or i128 %wide, 170141183460469231731687303715884105728
    br check(i8 1, i8 2, %count)
check(i8 %residue, i8 %base, i128 %exponent):
    %more = icmp ne i128 %exponent, 0
    br %more, update, exit(%residue)
update:
    %product = mul i8 %residue, %base
    %next = urem i8 %product, 3
    br latch(%next)
latch(i8 %joined):
    %half = lshr i128 %exponent, 1
    %square = mul i8 %base, %base
    %factor = urem i8 %square, 3
    br check(%joined, %factor, %half)
exit(i8 %answer):
    ret %answer
}
"#,
    )
    .expect("wide definite loop");
    compile_apply_with(
        ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("loop"),
            args: &[Argument::Register(0..64), Argument::Register(64..127)],
            out: 127..135,
            fault: 135..140,
            budget: 1161,
            entry_fault: None,
            stack_available: 4096,
            memory: None,
            scratch_start: 140,
        },
        ApplyMode::Automatic,
    )
    .expect("emit the definite loop")
}

#[test]
fn modular_proof_records_the_definite_loop_entry_once() {
    let compiled = definite_wide_loop();
    assert_eq!(compiled.construction, Construction::Definite);
    let work = compiled.work;
    assert_eq!(work.snapshot_slots, 18);
    assert_eq!(work.modular_attempts, 0);
    assert_eq!(work.exit_replay_gates, 0);
}

#[test]
fn a_modular_suffix_requiring_more_scratch_is_skipped() {
    for width in [7, 31, 63] {
        let modulus = (1u64 << width) - 1;
        let source = format!(
            r#"
memory 1024
func @f(i3 %input) -> i{width} {{
entry:
    %bit = and i3 %input, 1
    %exponent = or i3 %bit, 2
    %empty = icmp eq i3 %exponent, 0
    br %empty, finish(i128 99), head(i128 1, i128 2, %exponent)
head(i128 %old, i128 %base, i3 %x):
    %low = trunc i3 %x to i1
    br %low, update, latch(%old)
update:
    %product = mul i128 %old, %base
    %reduced = urem i128 %product, {modulus}
    br latch(%reduced)
latch(i128 %current):
    %next = lshr i3 %x, 1
    %square = mul i128 %base, %base
    %factor = urem i128 %square, {modulus}
    %last = icmp ult i3 %x, 2
    br %last, finish(%current), head(%current, %factor, %next)
finish(i128 %residue):
    %answer = trunc i128 %residue to i{width}
    ret %answer
}}
"#
        );
        let module = load(&source).expect("valid modular loop");
        let result = compile_apply_with(
            ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("loop"),
                args: &[Argument::Register(0..3)],
                out: 3..3 + width,
                fault: 3 + width..8 + width,
                budget: 50,
                entry_fault: None,
                stack_available: 800,
                memory: None,
                scratch_start: 8 + width,
            },
            ApplyMode::Automatic,
        )
        .expect("compile the fitting ordinary circuit");
        assert!(matches!(result.construction, Construction::Controlled { .. }));
        assert_eq!(result.work.gates_generated, 4, "width {width}");
        assert_eq!(result.work.comparison_gates, 0, "width {width}");
        assert_eq!(result.block.ops().len(), 6, "width {width}");
    }
}
