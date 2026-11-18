//! Commands issued by the classical controller to a quantum device.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Weak};

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
/// Clones refer to the same immutable definition, so devices can cache it by identity.
#[derive(Clone, Debug)]
pub struct GateBlock(Arc<BlockDefinition>);

#[derive(Debug)]
struct BlockDefinition {
    name: String,
    ops: Vec<BlockOp>,
}

/// A cache key for a gate block. It does not keep the block's commands alive.
#[derive(Debug)]
pub struct GateBlockKey(Weak<BlockDefinition>);

impl GateBlockKey {
    /// Whether an owner still holds the block definition.
    pub fn is_live(&self) -> bool {
        self.0.strong_count() != 0
    }
}

impl PartialEq for GateBlockKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.ptr_eq(&other.0)
    }
}

impl Eq for GateBlockKey {}

impl Hash for GateBlockKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_ptr().hash(state);
    }
}

impl GateBlock {
    /// Define a block of gates over wires allocated by the caller.
    pub fn new(name: impl Into<String>, gates: impl IntoIterator<Item = GateOp>) -> Self {
        Self::from_ops(name, gates.into_iter().map(BlockOp::Gate))
    }

    /// Define a block that can allocate and release local scratch.
    pub fn from_ops(name: impl Into<String>, ops: impl IntoIterator<Item = BlockOp>) -> Self {
        Self(Arc::new(BlockDefinition { name: name.into(), ops: ops.into_iter().collect() }))
    }

    /// The name for a device or writer that records this block.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// Get a cache key that stops being live when the last definition owner goes away.
    pub fn key(&self) -> GateBlockKey {
        GateBlockKey(Arc::downgrade(&self.0))
    }

    /// Commands in execution order.
    pub fn ops(&self) -> &[BlockOp] {
        &self.0.ops
    }
}

impl PartialEq for GateBlock {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for GateBlock {}

impl Hash for GateBlock {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
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
            for op in block.ops() {
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
