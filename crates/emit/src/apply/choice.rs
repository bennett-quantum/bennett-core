//! Compare completed circuits before selecting a newly eligible lowering.

use bennett_exec::GateOp;

use crate::count::isolated_gate_counts;

use super::bits::Circuit;
use super::work::{self, Work};

pub(super) fn cheaper(
    proposed: &Circuit,
    prefix: &[GateOp],
    proposed_forward: usize,
    ordinary: &Circuit,
    ordinary_forward: usize,
) -> bool {
    let ordinary = resources(ordinary, &[], ordinary_forward);
    let proposed = resources(proposed, prefix, proposed_forward);
    proposed != ordinary && proposed.into_iter().zip(ordinary).all(|(new, old)| new <= old)
}

fn resources(circuit: &Circuit, prefix: &[GateOp], forward: usize) -> [u128; 5] {
    let gates = circuit.forward_gates();
    let execution = prefix.iter().chain(gates).chain(gates[..forward].iter().rev()).chain(prefix.iter().rev());
    let count = isolated_gate_counts(execution.inspect(|_| work::add(Work::ComparisonGates, 1)));
    let scratch = circuit.peak_scratch().expect("costs are compared only for fitting circuits");
    [count.one_qubit, count.two_qubit, count.toffoli, count.depth, u128::from(scratch)]
}
