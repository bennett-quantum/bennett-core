//! Work counts for the speculative parts of the modular proof.

use std::cell::Cell;
use std::fmt::Write;

use bennett_ir::load;

use crate::{ApplyInput, ApplyMode, Argument, Construction, compile_apply_with};

#[derive(Clone, Copy, Default)]
struct Work {
    attempts: usize,
    slot_words: usize,
    replay_gates: usize,
}

thread_local! {
    static WORK: Cell<Work> = const { Cell::new(Work { attempts: 0, slot_words: 0, replay_gates: 0 }) };
}

pub(super) fn record_attempt() {
    let work = WORK.get();
    WORK.set(Work { attempts: work.attempts + 1, ..work });
}

pub(super) fn record_slot_words(words: usize) {
    let work = WORK.get();
    WORK.set(Work { slot_words: work.slot_words + words, ..work });
}

pub(super) fn record_replay_gates(gates: usize) {
    let work = WORK.get();
    WORK.set(Work { replay_gates: work.replay_gates + gates, ..work });
}

fn proof_work(source: &str, budget: u64) -> Work {
    let module = load(source).expect("valid work-count program");
    WORK.set(Work::default());
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
    WORK.get()
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
    assert_eq!(work.replay_gates, 0);
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
    assert_eq!(proof_work(&source, 300).slot_words, 0);
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
    WORK.set(Work::default());
    let compiled = definite_wide_loop();
    assert_eq!(compiled.construction, Construction::Definite);
    let work = WORK.get();
    assert_eq!(work.slot_words, 18);
    assert_eq!(work.attempts, 0);
    assert_eq!(work.replay_gates, 0);
}
