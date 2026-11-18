//! Definite-state checks of reversible arithmetic circuits.

#[path = "support/bits.rs"]
mod bits;

use bennett_exec::{GateOp, Wire};
use bennett_ir::{FixedGate, Gate};
use bits::Layout;

fn x(target: Wire, controls: &[Wire]) -> GateOp {
    GateOp { kind: Gate::Fixed(FixedGate::X), target, controls: controls.to_vec() }
}

#[test]
fn a_ripple_carry_adder_xors_the_sum_into_every_initial_output() {
    // x holds a and b, both three bits. Compute the carry out of each bit into wires 10..13.
    let carries = [
        x(10, &[0, 3]),
        x(11, &[1, 4]),
        x(11, &[1, 10]),
        x(11, &[4, 10]),
        x(12, &[2, 5]),
        x(12, &[2, 11]),
        x(12, &[5, 11]),
    ];
    // Each sum bit is a XOR b XOR carry-in. The high output bit is the final carry.
    let sum =
        [x(6, &[0]), x(6, &[3]), x(7, &[1]), x(7, &[4]), x(7, &[10]), x(8, &[2]), x(8, &[5]), x(8, &[11]), x(9, &[12])];
    let gates = carries.iter().chain(&sum).chain(carries.iter().rev()).cloned().collect::<Vec<_>>();
    let layout = Layout { x: 6, y: 4, scratch: 3 };
    let add = |x| (x & 7) + (x >> 3);
    bits::exhaustive(&gates, layout, add).expect("all 1024 pairs");

    let mut missing_carry = gates.clone();
    missing_carry.remove(carries.len() + sum.len() - 1);
    let error = bits::exhaustive(&missing_carry, layout, add).expect_err("the sum loses its high bit");
    assert!(error.contains("output wire 9"), "{error}");

    let mut dirty = gates;
    dirty.pop();
    let error = bits::exhaustive(&dirty, layout, add).expect_err("carry scratch was left behind");
    assert!(error.contains("scratch wire 10"), "{error}");
}

/// Compute the borrow of a - b into scratch using majority of !a, b, and the incoming borrow.
fn comparator(width: Wire) -> Vec<GateOp> {
    let first = 2 * width + 1;
    let mut borrow = Vec::new();
    for a in 0..width {
        let b = a + width;
        let out = first + a;
        borrow.extend([x(a, &[]), x(out, &[a, b])]);
        if a != 0 {
            borrow.extend([x(out, &[a, out - 1]), x(out, &[b, out - 1])]);
        }
        borrow.push(x(a, &[]));
    }
    let result = x(2 * width, &[first + width - 1]);
    borrow.iter().cloned().chain([result]).chain(borrow.iter().rev().cloned()).collect()
}

#[test]
fn comparator_is_exhaustive_for_small_words_and_seeded_for_wide_words() {
    let less = |width: u32| move |x: u128| u128::from((x & ((1u128 << width) - 1u128)) < (x >> width));
    bits::exhaustive(&comparator(2), Layout { x: 4, y: 1, scratch: 2 }, less(2)).expect("all 32 pairs");
    bits::sampled(&comparator(64), Layout { x: 128, y: 1, scratch: 64 }, 1_003, 87, less(64))
        .expect("64-bit comparison");
}

#[test]
fn the_runner_returns_wire_words_and_rejects_unsupported_gates() {
    let mut words = [0xaaaa_aaaa_aaaa_aaaa, 0xcccc_cccc_cccc_cccc, 0];
    bits::run(&[x(2, &[0, 1]), x(0, &[]), x(1, &[0])], &mut words).expect("ran supported gates");
    assert_eq!(words, [0x5555_5555_5555_5555, 0x9999_9999_9999_9999, 0x8888_8888_8888_8888]);
    assert!(bits::run(&[x(2, &[9])], &mut words).is_err());
    let h = GateOp { kind: Gate::Fixed(FixedGate::H), ..x(0, &[]) };
    assert!(bits::run(&[h], &mut words).is_err());
}
