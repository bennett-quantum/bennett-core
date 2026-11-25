use super::{Reference, Vector, compare, conditional_fault};
use crate::common::{allowing_region_faults, assert_same_state, executor, load};
use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use num_complex::Complex;

#[test]
fn the_signed_i64_minimum_divided_by_minus_one_wraps() {
    let function = "
func @f(i2 %x) -> i8 {
entry:
    %q = sdiv i64 -9223372036854775808, -1
    %r = srem i64 -9223372036854775808, -1
    %upper = lshr i64 %q, 56
    %answer = or i64 %upper, %r
    %low = trunc i64 %answer to i8
    ret %low
}";
    assert_eq!(compare(function, 6), [(0x80, 0); 4]);
}

#[test]
fn a_narrow_load_masks_the_high_bits_of_the_last_byte() {
    let function = "
func @f(i2 %x) -> i8 {
entry:
    %narrow = load i9 0
    %wide = zext i9 %narrow to i16
    %upper = lshr i16 %wide, 8
    %answer = trunc i16 %upper to i8
    ret %answer
}";
    // The static bytes are 9 and 7. An i9 load reads only bit 0 of the second byte.
    assert_eq!(compare(function, 5), [(1, 0); 4]);
}

#[test]
fn a_multibyte_store_replaces_the_whole_last_byte() {
    let function = "
func @f(i2 %x) -> i8 {
entry:
    store i8 255, 1
    %low = zext i2 %x to i12
    %value = add i12 %low, 0xab0
    store i12 %value, 0
    %low_byte = load i8 0
    %high_byte = load i8 1
    %answer = xor i8 %low_byte, %high_byte
    ret %answer
}";
    // The independent byte reads see b0..b3 and 0a, including the zeroed high nibble.
    assert_eq!(compare(function, 8), [(0xba, 0), (0xbb, 0), (0xb8, 0), (0xb9, 0)]);
}

#[test]
fn load_and_store_check_bounds_then_every_held_byte_in_the_live_image() {
    let module = load(
        "
memory 128
data 0 = i8 41
func @f(i3 %x) -> i8 {
entry:
    switch i3 %x, 0: live, 1: load_held, 2: load_bounds, 3: store_held, 4: store_bounds, 5: overflow, default: private
live:
    %v = load i8 0
    ret %v
load_held:
    %held = load i9 126
    ret 1
load_bounds:
    %outside = load i16 127
    ret 2
store_held:
    store i9 7, 126
    ret 3
store_bounds:
    store i16 7, 127
    ret 4
overflow:
    %wrapped = load i16 0xffffffffffffffff
    ret 5
private:
    store i16 0x1234, 125
    %high = load i8 126
    ret %high
}
func @main(i3 %x) {
entry:
    store i8 71, 0
    x [127:0]
    x [127:0]
    %out = qalloc 8
    %fault = qalloc 5
    x %out[6]
    x %fault[4]
    apply @f(%x) -> %out, %fault budget 4
    ret
}",
    );
    let mut caller_memory = [0; 128];
    caller_memory[0] = 71;
    let reference = Reference::new(&module, "f", "main").with_memory(&caller_memory, &[127]);
    let held = (0, Fault::QubitMemoryAccess.code());
    let bounds = (0, Fault::OutOfBounds.code());
    for (input, want) in [(71, 0), held, bounds, held, bounds, bounds, (0x12, 0), (0x12, 0)].into_iter().enumerate() {
        let input = input as u64;
        let basis = [input, 64, 16];
        let (after, outcomes) = reference.apply(
            Vector::from([(basis, Complex::new(1, 0))]),
            4,
            |b| [b[0]],
            |b, (value, fault)| {
                b[1] ^= value;
                b[2] ^= fault;
            },
        );
        assert_eq!(outcomes[&[input]], want, "reference input {input}");
        assert_eq!(reference.finish(&[input], 4).output, want, "definite reference input {input}");
        let options = Options { reversible: true, ..allowing_region_faults() };
        let mut run = executor(&module, "main", &[input], options);
        let entry = run.snapshot();
        for _ in 0..7 {
            assert_eq!(run.step(), None);
        }
        let prepared = run.snapshot();
        assert_eq!(prepared.classical.memory, caller_memory);
        assert_eq!(prepared.quantum.cells, [(127, 0)]);
        assert_eq!(run.step(), None);
        let applied = run.snapshot();
        assert_eq!(applied.classical.memory, caller_memory, "private writes do not escape");
        assert_eq!(applied.quantum.cells, prepared.quantum.cells);
        // The addressed gates allocated column 0, which still holds the byte at value zero.
        let expected = after
            .into_iter()
            .map(|([_, out, fault], _)| (vec![0, out, fault], Complex::new(1.0, 0.0)))
            .collect::<Vec<_>>();
        assert_eq!(applied.quantum.amplitudes, expected, "executor input {input}");
        assert_eq!(run.run(), Outcome::Returned(None));
        run.reverse().expect("reverse the live image fixture");
        assert_same_state(&run.snapshot(), &entry, "restore the original memory and register state");
    }
}

#[test]
fn zero_slot_entry_and_void_callee_fit_at_the_floor() {
    for (budget, want) in [(0, (0, 1)), (1, (0, 1)), (2, (0, 1)), (3, (37, 0))] {
        let module = load(&format!(
            "
memory 17
data 0 = i8 1
func @empty() {{
entry:
    ret
}}
func @f() -> i8 {{
entry:
    call @empty()
    ret 37
}}
func @main() {{
entry:
    %out = qalloc 8
    %fault = qalloc 5
    apply @f() -> %out, %fault budget {budget}
    ret
}}"
        ));
        // The two target slots leave one byte below the caller, exactly the static floor.
        let reference = Reference::new(&module, "f", "main");
        assert_eq!(reference.finish(&[], budget).output, want);
        let (after, _) = reference.apply(
            Vector::from([([0, 0], Complex::new(1, 0))]),
            budget,
            |_| [],
            |b, (value, fault)| {
                b[0] ^= value;
                b[1] ^= fault;
            },
        );
        let mut run = executor(&module, "main", &[], allowing_region_faults());
        assert_eq!(run.run(), Outcome::Returned(None));
        let expected =
            after.into_keys().map(|[out, fault]| (vec![out, fault], Complex::new(1.0, 0.0))).collect::<Vec<_>>();
        assert_eq!(run.snapshot().quantum.amplitudes, expected, "budget {budget}");
    }
}

#[test]
fn reachable_quantum_instructions_fault_before_acting() {
    for instruction in ["%q = qalloc 1\n    ret 0", "x [512:0]\n    ret 0", "%v = measure i8 [512]\n    ret %v"] {
        let function = conditional_fault(instruction);
        assert_eq!(compare(&function, 3)[0], (0, Fault::Budget.code()));
        let quantum = (0, Fault::QuantumInRegion.code());
        assert_eq!(compare(&function, 4), [quantum, (1, 0), (2, 0), (3, 0)]);
    }
}
