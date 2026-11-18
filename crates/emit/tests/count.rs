//! Gate counts, wire lifetimes, and independent scheduling of disjoint gates.

use bennett_emit::{CountingDevice, Counts, LowerControls, seeded_bits};
use bennett_exec::{Device, GateBlock, GateOp, RunError, Wire};
use bennett_ir::{FixedGate, Gate};

fn op(gate: FixedGate, target: Wire, controls: &[Wire]) -> GateOp<&[Wire]> {
    GateOp { kind: Gate::Fixed(gate), target, controls }
}

#[test]
fn counts_layers_on_live_wires_and_tracks_the_allocation_peak() {
    let mut device = CountingDevice::new([true, false].into_iter());
    device.allocate(0..4).expect("allocated");
    device.gate(op(FixedGate::H, 0, &[])).expect("layer 1");
    device.gate(op(FixedGate::X, 1, &[])).expect("disjoint, also layer 1");
    device.gate(op(FixedGate::X, 2, &[0, 1])).expect("layer 2");
    device.gate(op(FixedGate::X, 3, &[2])).expect("layer 3");
    assert!(device.measure(3).expect("first bit"));
    assert!(!device.measure(3).expect("second bit"));
    assert_eq!(device.measure(3), Err(RunError::Device("measurement source exhausted")));

    device.discard(2..4).expect("released");
    device.allocate(2..3).expect("fresh lifetime");
    device.gate(op(FixedGate::H, 2, &[])).expect("layer 1");
    device.gate(op(FixedGate::X, 2, &[1])).expect("layer 3, from the live control");
    assert_eq!(
        device.counts(),
        Counts {
            one_qubit: 3,
            two_qubit: 2,
            toffoli: 1,
            peak_qubits: 4,
            depth: 3,
            discarded_qubits: 2,
            ..Counts::default()
        }
    );
}

#[test]
fn native_toffoli_and_lowered_gates_have_the_expected_costs() {
    for n in 0..=5 {
        for gate in [FixedGate::X, FixedGate::H] {
            let mut device = LowerControls::new(CountingDevice::default());
            // Include wires at the high end, where the wrapper first looks for scratch.
            let controls: Vec<_> = (Wire::MAX - n..Wire::MAX).collect();
            device.allocate(Wire::MAX - n..Wire::MAX).expect("controls");
            device.allocate(0..1).expect("target");
            let target = op(gate, 0, &controls);
            device.gate(target).expect("lowered");
            device.gate(target).expect("lowered again");

            let scratch = if n <= 1 || (n == 2 && gate == FixedGate::X) { 0 } else { n - 1 };
            let one = u128::from(n == 0);
            let two = u128::from(n == 1 || scratch > 0);
            let toffoli = if scratch > 0 { u128::from(scratch) * 2 } else { u128::from(n == 2) };
            assert_eq!(
                device.inner().counts(),
                Counts {
                    one_qubit: 2 * one,
                    two_qubit: 2 * two,
                    toffoli: 2 * toffoli,
                    peak_qubits: (n + 1 + scratch) as usize,
                    depth: 2 * (one + two + toffoli),
                    zero_released_qubits: u128::from(scratch) * 2,
                    ..Counts::default()
                },
                "{gate:?} with {n} controls"
            );
        }
    }
}

#[test]
fn measurement_sources_are_reproducible() {
    let a: Vec<_> = seeded_bits(42).take(128).collect();
    assert_eq!(a, seeded_bits(42).take(128).collect::<Vec<_>>());
    assert_ne!(a, seeded_bits(17).take(128).collect::<Vec<_>>());
    let mut zero = CountingDevice::default();
    zero.allocate(0..1).expect("allocated");
    assert!(!zero.measure(0).expect("zero bit"));
}

fn owned_x(target: Wire, controls: &[Wire]) -> GateOp {
    GateOp { kind: Gate::Fixed(FixedGate::X), target, controls: controls.to_vec() }
}

#[test]
fn a_billion_repetitions_compose_the_wire_layers() {
    let chain = GateBlock::new("chain", [owned_x(1, &[0]), owned_x(2, &[1]), owned_x(3, &[2])]);
    let mut device = CountingDevice::default();
    device.allocate(0..4).expect("wires");
    device.repeat(&chain, 1_000_000_000).expect("counted");
    // A chain has depth 3 alone. The first gate of the next call can start one layer early.
    assert_eq!(
        device.counts(),
        Counts { two_qubit: 3_000_000_000, peak_qubits: 4, depth: 2_000_000_001, ..Counts::default() }
    );
}

#[test]
fn block_repetition_matches_expansion_with_lowering_and_scratch() {
    let gates = [
        owned_x(1, &[0]),
        owned_x(5, &[4]),
        owned_x(2, &[1]),
        owned_x(3, &[0, 1, 2]),
        GateOp { kind: Gate::Fixed(FixedGate::H), target: 0, controls: vec![4, 5] },
    ];
    let block = GateBlock::new("compute", gates.clone());
    let mut compact = LowerControls::new(CountingDevice::default());
    let mut expanded = LowerControls::new(CountingDevice::default());
    for device in [&mut compact, &mut expanded] {
        device.allocate(0..6).expect("wires");
        device.gate(op(FixedGate::Z, 5, &[])).expect("an existing wire dependency");
    }
    let before = compact.inner().counts();
    compact.repeat(&block, 0).expect("repeat zero");
    assert_eq!(compact.inner().counts(), before);
    compact.repeat(&block, 3).expect("repeat");
    for _ in 0..3 {
        for gate in &gates {
            expanded.gate(gate.borrowed()).expect("ordinary gate");
        }
    }
    assert_eq!(compact.inner().counts(), expanded.inner().counts());
    compact.call(&block).expect("call");
    for gate in &gates {
        expanded.gate(gate.borrowed()).expect("ordinary gate");
    }
    // Check that the retained wire depths also affect gates outside the block.
    for device in [&mut compact, &mut expanded] {
        device.gate(op(FixedGate::X, 4, &[3])).expect("following gate");
        device.discard(0..6).expect("discard");
    }
    assert_eq!(compact.inner().counts(), expanded.inner().counts());
    assert_eq!(compact.inner().counts().peak_qubits, 8);
    assert_eq!(compact.inner().counts().zero_released_qubits, 12);
    assert_eq!(compact.inner().counts().discarded_qubits, 6);
}

#[test]
fn scratch_only_gates_contribute_to_depth_without_carrying_layers_to_the_next_call() {
    use bennett_exec::BlockOp;

    let block = GateBlock {
        name: "scratch only".into(),
        ops: vec![
            BlockOp::Allocate(0..1),
            BlockOp::Gate(owned_x(0, &[])),
            BlockOp::Gate(owned_x(0, &[])),
            BlockOp::ReleaseZero(0..1),
        ],
    };
    let mut device = CountingDevice::default();
    device.repeat(&block, 300).expect("repeat");
    assert_eq!(
        device.counts(),
        Counts { one_qubit: 600, peak_qubits: 1, depth: 2, zero_released_qubits: 300, ..Counts::default() }
    );
}
