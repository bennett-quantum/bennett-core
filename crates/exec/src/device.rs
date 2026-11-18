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

impl GateOp {
    /// Borrow the controls while sending an owned gate to a device.
    pub fn borrowed(&self) -> GateOp<&[Wire]> {
        GateOp { kind: self.kind, target: self.target, controls: &self.controls }
    }
}

/// A command in a gate block. Allocations are local scratch, released in zero before the block ends.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockOp {
    /// Allocate scratch wires in zero.
    Allocate(Range<Wire>),
    /// Apply a gate to live wires.
    Gate(GateOp),
    /// Release zeroed scratch wires.
    ReleaseZero(Range<Wire>),
}

impl BlockOp {
    /// Expand a command on the device.
    pub fn send(&self, device: &mut (impl Device + ?Sized)) -> Result<(), RunError> {
        match self {
            Self::Allocate(wires) => device.allocate(wires.clone()),
            Self::Gate(gate) => device.gate(gate.borrowed()),
            Self::ReleaseZero(wires) => device.release_zero(wires.clone()),
        }
    }
}

/// A named, reusable sequence without measurements. Scratch lifetimes must fit within one call.
/// Callers define the value once and pass it by reference to `call` and `repeat`.
#[derive(Clone, Debug, PartialEq)]
pub struct GateBlock {
    /// The name for a device or writer that records this block.
    pub name: String,
    /// Commands in execution order.
    pub ops: Vec<BlockOp>,
}

impl GateBlock {
    /// Define a block of gates over wires allocated by the caller.
    pub fn new(name: impl Into<String>, gates: impl IntoIterator<Item = GateOp>) -> Self {
        Self { name: name.into(), ops: gates.into_iter().map(BlockOp::Gate).collect() }
    }
}

/// A destination for quantum commands. Callers supply free wires for allocation and live wires for
/// other commands. Empty allocation and release ranges have no effect. A failed command stops the forward run.
pub trait Device {
    /// Allocate the given consecutive wires in zero.
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError>;
    /// Apply a named gate.
    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError>;
    /// Measure one qubit in the computational basis and return its bit.
    fn measure(&mut self, wire: Wire) -> Result<bool, RunError>;
    /// Release the wires by taking their partial trace.
    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError>;
    /// Release wires that the caller has returned to zero.
    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError>;

    /// Execute a previously defined gate block once.
    fn call(&mut self, block: &GateBlock) -> Result<(), RunError> {
        self.repeat(block, 1)
    }

    /// Execute a gate block `times` times. A device may process the repetition as one command.
    fn repeat(&mut self, block: &GateBlock, times: u64) -> Result<(), RunError> {
        for _ in 0..times {
            for op in &block.ops {
                op.send(self)?;
            }
        }
        Ok(())
    }

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
