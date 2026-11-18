//! Expand gates beyond the logical device's native controls.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use bennett_exec::{BlockOp, Device, GateBlock, GateBlockKey, GateOp, RunError, Wire};
use bennett_ir::{FixedGate, Gate};

const X: Gate = Gate::Fixed(FixedGate::X);

fn native(gate: GateOp<&[Wire]>) -> bool {
    let count = gate.controls.len();
    count <= 1 || (count == 2 && gate.kind == X)
}

fn scratch_range(live: &BTreeSet<Wire>, count: u64) -> Result<Range<Wire>, RunError> {
    // Search downwards so the usual low-numbered program wires need no remapping.
    let mut end = Wire::MAX;
    loop {
        let start = end.checked_sub(count).ok_or(RunError::Device("no unused wire range for scratch"))?;
        if let Some(&occupied) = live.range(start..end).next_back() {
            end = occupied;
        } else {
            return Ok(start..end);
        }
    }
}

fn lower(gate: GateOp<&[Wire]>, live: &BTreeSet<Wire>, ops: &mut Vec<BlockOp>) -> Result<Range<Wire>, RunError> {
    let scratch = scratch_range(live, (gate.controls.len() - 1) as u64)?;
    let ladder = gate.controls[1..].iter().enumerate().map(|(index, &control)| {
        let target = scratch.start + index as u64;
        let previous = if index == 0 { gate.controls[0] } else { target - 1 };
        BlockOp::Gate(GateOp { kind: X, target, controls: vec![previous, control] })
    });
    ops.push(BlockOp::Allocate(scratch.clone()));
    ops.extend(ladder.clone());
    ops.push(BlockOp::Gate(GateOp { kind: gate.kind, target: gate.target, controls: vec![scratch.end - 1] }));
    ops.extend(ladder.rev());
    ops.push(BlockOp::ReleaseZero(scratch.clone()));
    Ok(scratch)
}

/// A lowered block remains valid until a current allocation occupies its generated scratch.
enum LoweredBlock {
    Native,
    WithScratch { definition: GateBlock, scratch: BTreeSet<(Wire, Wire)> },
}

impl LoweredBlock {
    fn available<'a>(&'a self, original: &'a GateBlock, live: &BTreeSet<Wire>) -> Option<&'a GateBlock> {
        match self {
            Self::Native => Some(original),
            Self::WithScratch { definition, scratch } => {
                scratch.iter().all(|&(start, end)| live.range(start..end).next().is_none()).then_some(definition)
            }
        }
    }

    fn new(block: &GateBlock, live: &BTreeSet<Wire>) -> Result<Self, RunError> {
        let needs_lowering = block.ops().iter().any(|op| match op {
            BlockOp::Gate(gate) => !native(gate.borrowed()),
            BlockOp::Allocate(_) | BlockOp::ReleaseZero(_) => false,
        });
        if !needs_lowering {
            return Ok(Self::Native);
        }

        let mut live = live.clone();
        let mut ops = Vec::new();
        let mut scratch = BTreeSet::new();
        for op in block.ops() {
            match op {
                BlockOp::Gate(gate) if !native(gate.borrowed()) => {
                    let wires = lower(gate.borrowed(), &live, &mut ops)?;
                    scratch.insert((wires.start, wires.end));
                    continue;
                }
                BlockOp::Allocate(wires) => live.extend(wires.clone()),
                BlockOp::Gate(_) => {}
                BlockOp::ReleaseZero(wires) => {
                    for wire in wires.clone() {
                        live.remove(&wire);
                    }
                }
            }
            ops.push(op.clone());
        }
        Ok(Self::WithScratch { definition: GateBlock::from_ops(block.name(), ops), scratch })
    }
}

/// Lowers controlled gates using temporary qubits, forwarding other commands to the inner device.
pub struct LowerControls<D> {
    inner: D,
    live: BTreeSet<Wire>,
    blocks: HashMap<GateBlockKey, LoweredBlock>,
}

impl<D> LowerControls<D> {
    /// Wrap a device before allocating any wires.
    pub fn new(inner: D) -> Self {
        Self { inner, live: BTreeSet::new(), blocks: HashMap::new() }
    }

    fn end_wires(&mut self, wires: Range<Wire>) {
        for wire in wires {
            self.live.remove(&wire);
        }
    }

    /// Inspect the inner device, for example to read its gate counts.
    pub fn inner(&self) -> &D {
        &self.inner
    }
}

impl<D: Device> Device for LowerControls<D> {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.inner.allocate(wires.clone())?;
        self.live.extend(wires);
        Ok(())
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        if native(gate) {
            return self.inner.gate(gate);
        }
        let mut ops = Vec::new();
        lower(gate, &self.live, &mut ops)?;
        for op in ops {
            op.send(&mut self.inner)?;
        }
        Ok(())
    }

    fn measure(&mut self, wire: Wire) -> Result<bool, RunError> {
        self.inner.measure(wire)
    }

    fn measure_many(&mut self, ranges: &[Range<Wire>]) -> Result<Vec<u64>, RunError> {
        self.inner.measure_many(ranges)
    }

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.inner.discard(wires.clone())?;
        self.end_wires(wires);
        Ok(())
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.inner.release_zero(wires.clone())?;
        self.end_wires(wires);
        Ok(())
    }

    fn repeat(&mut self, block: &GateBlock, times: u64) -> Result<(), RunError> {
        if times == 0 {
            return Ok(());
        }
        let key = block.key();
        if let Some(available) = self.blocks.get(&key).and_then(|lowered| lowered.available(block, &self.live)) {
            return self.inner.repeat(available, times);
        }
        // Drop the obsolete definition before issuing the new one, so the inner device can prune it.
        self.blocks.remove(&key);
        self.blocks.retain(|key, _| key.is_live());
        let lowered = LoweredBlock::new(block, &self.live)?;
        let definition = match &lowered {
            LoweredBlock::Native => block,
            LoweredBlock::WithScratch { definition, .. } => definition,
        };
        self.inner.repeat(definition, times)?;
        self.blocks.insert(key, lowered);
        Ok(())
    }
}
