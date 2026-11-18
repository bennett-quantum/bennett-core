//! Commands issued by the classical controller to a quantum device.

use std::ops::Range;

use bennett_ir::Gate;

use crate::RunError;

/// A caller-assigned qubit identity, live from allocation to release.
pub type Wire = u64;

/// A named single-target gate enabled when every control wire is one.
/// Owned gates can be kept in a gate list, and devices receive borrowed controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateOp<C = Vec<Wire>> {
    /// The gate, including the angle of a rotation.
    pub kind: Gate,
    /// The qubit changed by the gate.
    pub target: Wire,
    /// Distinct control qubits, different from the target.
    pub controls: C,
}

/// A destination for quantum commands. Callers supply free wires for allocation and live wires for
/// other commands. A failed command stops the forward run.
pub trait Device {
    /// Allocate the given consecutive wires in zero.
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError>;
    /// Apply a named gate.
    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError>;
    /// Measure one qubit in the computational basis and return its bit.
    fn measure(&mut self, wire: Wire) -> Result<bool, RunError>;
    /// Release the wires by taking their partial trace.
    fn release(&mut self, wires: Range<Wire>) -> Result<(), RunError>;

    /// Measure disjoint nonempty ranges of at most 64 wires each, returning one little-endian word per range.
    /// The sparse simulator overrides this to preserve its seeded joint sampling.
    fn measure_many(&mut self, ranges: &[Range<Wire>]) -> Result<Vec<u64>, RunError> {
        ranges
            .iter()
            .map(|range| {
                range
                    .clone()
                    .enumerate()
                    .try_fold(0, |word, (bit, wire)| Ok(word | (u64::from(self.measure(wire)?) << bit)))
            })
            .collect()
    }
}
