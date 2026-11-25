//! Worked non-local jumps, including the record's layout and the endpoints of its checks.

use std::f64::consts::FRAC_1_SQRT_2;

use bennett_ir::Fault;
use num_complex::Complex64 as C;

use super::{BUDGET, Finish, assert_density, module, pure, run};

#[test]
fn outer_unwind_restores_the_stack_and_preserves_the_live_state() {
    let program = module(
        "memory 512
func @main(i32 %value) -> i32 {
entry:
 %q = qalloc 1
 h %q
 s %q
 x [160] ctrl %q
 %result = call @target(%value, %q)
 ret %result
}
func @target(i32 %value, q1 %q) -> i32 {
entry:
 %before = alloca 13
 br capture
skipped:
 fault unreachable
capture:
 store i8 73, %before
 %result = setjmp 32
 %marker = load i1 0
 br %marker, verify, travel
travel:
 store i1 1, 0
 %after = alloca 11
 call @middle(%value, %q, %before, %after)
 fault unreachable
verify:
 %saved = stacksave
 %slot_ok = icmp eq i64 %saved, %before
 %touched = load i8 %before
 %mem_ok = icmp eq i8 %touched, 74
 %kept_ok = icmp eq i32 %result, %value
 %state_ok = and i1 %slot_ok, %mem_ok
 %all = and i1 %state_ok, %kept_ok
 br %all, finish, skipped
finish:
 z %q
 ret %result
}
func @middle(i32 %value, q1 %q, i64 %before, i64 %after) {
entry:
 call @leaf(%value, %q, %before, %after)
 fault unreachable
}
func @leaf(i32 %value, q1 %q, i64 %before, i64 %after) {
entry:
 x [160:1] ctrl %q
 store i8 74, %before
 store i8 0xe9, %after
 longjmp 32, %value
}",
    );
    for value in [0, 1, -7i32 as u32] {
        let result = run(&program, "main", &[u64::from(value)], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Returned(Some(u128::from(value)))));
        assert_eq!(result.reference.steps, 32);
        // Depth 1, function 1, block 2, instruction 1. The caller reserves 3 slots; the
        // target reserves 13, and its first 13-byte allocation rounds down to 368.
        let record: Vec<_> =
            [0x1_0000_0001u64, 0x1_0000_0002, 488, 368].into_iter().flat_map(u64::to_le_bytes).collect();
        let memory = &result.reference.memory;
        assert_eq!(&memory[32..64], record);
        assert_eq!((memory[0], memory[368], memory[352]), (1, 74, 0xe9));
        assert_eq!(result.reference.quantum.cells.keys().copied().collect::<Vec<_>>(), [160]);
        let expected = pure(&[(vec![0, 0], C::new(FRAC_1_SQRT_2, 0.0)), (vec![1, 3], C::new(0.0, -FRAC_1_SQRT_2))]);
        assert_density(&result.reference.quantum.density, &expected, "outer unwind");
    }
}

#[test]
fn applied_unwind_counts_suspended_frames_and_discards_private_records() {
    let program = module(
        "memory 768
data 0 = i8 42
data 64 = i64 0x11223344556677ab, i64 0x12345678, i64 42, i64 1729
func @main() {
entry:
 %q = qalloc 2
 h %q[0]
 h %q[1]
 s %q[0]
 z %q[1]
 call @bridge(%q)
 ret
}
func @bridge(q2 %q) {
entry:
 %r = setjmp 32
 switch i32 %r, 0: invoke, default: finish
invoke:
 %out = qalloc 64
 %fault = qalloc 5
 x %out[7]
 x %fault[2]
 apply @applied(%q) -> %out, %fault budget 32
 br finish
finish:
 ret
}
func @applied(i2 %choice) -> i64 {
entry:
 store i8 99, 0
 switch i2 %choice, 0: save, 1: direct, 2: escape, default: waiting
save:
 %before = alloca 11
 %r = setjmp 64
 %written = load i8 0
 %resumed = icmp eq i8 %written, 77
 br %resumed, verify, travel
travel:
 %after = alloca 17
 call @middle(i64 64, i32 0, %after)
 fault unreachable
verify:
 %saved = stacksave
 %stack_ok = icmp eq i64 %saved, %before
 %value_ok = icmp eq i32 %r, 0
 %ok = and i1 %stack_ok, %value_ok
 %encoded = load i64 64
 %answer = select i64 %ok, %encoded, 255
 ret %answer
direct:
 %original = load i8 64
 %wide = zext i8 %original to i64
 ret %wide
escape:
 call @middle(i64 32, i32 7, i64 120)
 fault unreachable
waiting:
 br waiting
}
func @middle(i64 %record, i32 %value, i64 %scratch) {
entry:
 call @leaf(%record, %value, %scratch)
 fault unreachable
}
func @leaf(i64 %record, i32 %value, i64 %scratch) {
entry:
 store i8 77, 0
 store i8 9, %scratch
 longjmp %record, %value
}",
    );
    let result = run(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
    let memory = &result.reference.memory;
    assert_eq!(memory[0], 42);
    assert_eq!(memory[120], 0);
    let original: Vec<_> =
        [0x1122_3344_5566_77abu64, 0x12345678, 42, 1729].into_iter().flat_map(u64::to_le_bytes).collect();
    assert_eq!(&memory[64..96], original);
    let outer: Vec<_> = [0x1_0000_0001u64, 0, 760, 728].into_iter().flat_map(u64::to_le_bytes).collect();
    assert_eq!(&memory[32..64], outer);
    assert_eq!(&memory[121..], vec![0; 768 - 121]);
    let expected = pure(&[
        (vec![0, 128 ^ 0x2_0000_0002, 4], C::new(0.5, 0.0)),
        (vec![1, 128 ^ 0xab, 4], C::new(0.0, 0.5)),
        (vec![2, 128, 4 ^ Fault::BadLongjmp.code()], C::new(-0.5, 0.0)),
        (vec![3, 128, 4 ^ Fault::Budget.code()], C::new(0.0, -0.5)),
    ]);
    assert_density(&result.reference.quantum.density, &expected, "private unwind, return, escape and timeout");
}

#[test]
fn corrupt_records_fault_without_unwinding_and_allocation_limits_are_inclusive() {
    for (label, address, word, expected) in [
        ("maximum depth", 32, u64::from(u32::MAX), Finish::Faulted(Fault::BadLongjmp)),
        ("wrong function", 32, 1 << 32, Finish::Faulted(Fault::BadLongjmp)),
        ("wrong entry pointer", 48, 511, Finish::Faulted(Fault::BadLongjmp)),
        ("missing block", 40, 100, Finish::Faulted(Fault::BadLongjmp)),
        ("missing instruction", 40, 100 << 32, Finish::Faulted(Fault::BadLongjmp)),
        ("ordinary instruction", 40, 1, Finish::Faulted(Fault::BadLongjmp)),
        ("below lower limit", 56, 479, Finish::Faulted(Fault::BadLongjmp)),
        ("at lower limit", 56, 480, Finish::Returned(Some(17))),
        ("unaligned within limits", 56, 481, Finish::Returned(Some(17))),
        ("at upper limit", 56, 496, Finish::Returned(Some(17))),
        ("above upper limit", 56, 497, Finish::Faulted(Fault::BadLongjmp)),
    ] {
        for jump in ["longjmp 32, 17", "call @leaf()\n fault unreachable"] {
            let program = module(&format!(
                "memory 512
func @main() -> i32 {{
entry:
 %r = setjmp 32
 switch i32 %r, 0: travel, default: resumed
travel:
 %scratch = alloca 16
 store i64 {word}, {address}
 {jump}
resumed:
 ret %r
}}
func @leaf() {{
entry:
 %scratch = alloca 19
 longjmp 32, 17
}}"
            ));
            let result = run(&program, "main", &[], 0, BUDGET);
            assert_eq!(result.reference.finish, Some(expected), "{label}, {jump}");
            assert_eq!(&result.reference.memory[address..address + 8], &word.to_le_bytes(), "{label}");
            if matches!(expected, Finish::Faulted(_)) {
                let called = jump.starts_with("call");
                assert_eq!(result.reference.frames.len(), if called { 2 } else { 1 }, "{label}");
                assert_eq!(result.reference.stack, if called { 448 } else { 480 }, "{label}");
                assert_eq!(result.reference.steps, if called { 6 } else { 4 }, "{label}");
            }
        }
    }
}

#[test]
fn a_record_for_a_returned_frame_is_not_live() {
    let program = module(
        "memory 512
func @main() {
entry:
 call @expired()
 longjmp 32, 5
}
func @expired() {
entry:
 %r = setjmp 32
 ret
}",
    );
    let result = run(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Faulted(Fault::BadLongjmp)));
    assert_eq!(result.reference.steps, 3);
    assert_eq!(result.reference.frames.len(), 1);
    assert_eq!(result.reference.stack, 512);
    assert_eq!(&result.reference.memory[32..40], &0x1_0000_0001u64.to_le_bytes());
}

#[test]
fn a_valid_record_can_resume_an_instruction_that_has_never_run() {
    let program = module(
        "memory 256
data 32 = i64 0, i64 1, i64 256, i64 248
func @main() -> i32 {
entry:
 longjmp 32, -7
saved:
 %r = setjmp 96
 ret %r
}",
    );
    let result = run(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(Some(u128::from(-7i32 as u32)))));
    assert_eq!(result.reference.steps, 2);
    assert_eq!(&result.reference.memory[96..128], &[0; 32]);
}

#[test]
fn jump_buffers_check_the_full_range_before_held_bytes_in_both_runs() {
    for (operation, ending) in [("%r = setjmp %buf", "ret 1"), ("longjmp %buf, 1", "")] {
        let saving = operation.starts_with("%r");
        for (address, fault) in [
            (223, (!saving).then_some(Fault::BadLongjmp)),
            (224, Some(Fault::QubitMemoryAccess)),
            (225, Some(Fault::OutOfBounds)),
            (u64::MAX, Some(Fault::OutOfBounds)),
        ] {
            let program = module(&format!(
                "memory 256
func @main(i64 %buf, i1 %private) -> i1 {{
entry:
 store i8 0x49, 223
 store i8 0x75, 224
 %q = qalloc 1
 h %q
 x [255] ctrl %q
 %out = qalloc 1
 %flag = qalloc 5
 br %private, applied, outer
applied:
 apply @access(%buf) -> %out, %flag budget 4
 ret 0
outer:
 {operation}
 {ending}
}}
func @access(i64 %buf) -> i1 {{
entry:
 {operation}
 {ending}
}}"
            ));
            let outer = run(&program, "main", &[address, 0], 0, BUDGET);
            assert_eq!(
                outer.reference.finish,
                Some(fault.map_or(Finish::Returned(Some(1)), Finish::Faulted)),
                "{operation} at {address}",
            );
            if fault.is_some() {
                assert_eq!(&outer.reference.memory[223..225], &[0x49, 0x75]);
                assert_eq!(outer.reference.steps, 8);
            }
            let applied = run(&program, "main", &[address, 1], 0, BUDGET);
            assert_eq!(applied.reference.finish, Some(Finish::Returned(Some(0))));
            assert_eq!(&applied.reference.memory[223..225], &[0x49, 0x75]);
            let amplitude = C::new(FRAC_1_SQRT_2, 0.0);
            let out = u64::from(fault.is_none());
            let code = fault.map_or(0, Fault::code);
            let expected = pure(&[(vec![0, out, code, 0], amplitude), (vec![1, out, code, 1], amplitude)]);
            assert_density(&applied.reference.quantum.density, &expected, operation);
        }
    }
}
