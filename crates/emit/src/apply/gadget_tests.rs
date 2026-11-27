//! Check the work wires at the end of one native gadget, before undo of the applied function.

use bennett_exec::{BlockOp, GateBlock};

use super::bits::Word;

pub(super) fn check(block: &GateBlock, result: &Word, inputs: &[(usize, u128)], expected: u128, toffolis: usize) {
    let [BlockOp::Allocate(scratch), gates @ .., BlockOp::ReleaseZero(_)] = block.ops() else {
        panic!("expected one scratch range");
    };
    let measured = gates.iter().filter(|op| matches!(op, BlockOp::Gate(gate) if gate.controls.len() == 2)).count();
    assert_eq!(measured, toffolis);

    let mut wires = vec![false; scratch.end as usize];
    let args =
        inputs.iter().flat_map(|&(width, value)| (0..width).map(move |i| value >> i & 1 != 0)).collect::<Vec<_>>();
    assert_eq!(scratch.start as usize, args.len());
    wires[..args.len()].copy_from_slice(&args);
    for op in gates {
        let BlockOp::Gate(gate) = op else {
            panic!("unexpected allocation");
        };
        if gate.controls.iter().all(|&control| wires[control as usize]) {
            wires[gate.target as usize] ^= true;
        }
    }
    let mut work_start = args.len();
    let found = result.iter().enumerate().fold(0, |value, (i, bit)| {
        let actual = match bit.wire() {
            None => bit.literal().expect("a literal bit"),
            Some((wire, inverted)) => {
                work_start = work_start.max(wire as usize + 1);
                wires[wire as usize] ^ inverted
            }
        };
        value | (u128::from(actual) << i)
    });
    assert_eq!(found, expected);
    assert_eq!(&wires[..args.len()], &args);
    assert!(wires[work_start..].iter().all(|&bit| !bit), "gadget work wires must return to zero");
}
