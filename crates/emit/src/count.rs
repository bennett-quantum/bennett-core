//! Count gates and live wires without keeping a quantum state.
//!
//! The live wires are kept as runs and a gate layer only for the wires gates have acted on, so a register
//! costs host memory for the qubits it uses, not for its width.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::iter::{Repeat, from_fn, repeat};
use std::ops::Range;

use bennett_exec::{BlockOp, Device, GateBlock, GateBlockKey, GateOp, RunError, Wire};
use bennett_ir::{FixedGate, Gate};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rangemap::RangeSet;

use crate::schedule::Schedule;

/// Resources used by one run on the logical device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Gates with only a target.
    pub one_qubit: u128,
    /// Gates with a target and one control.
    pub two_qubit: u128,
    /// X gates with two controls.
    pub toffoli: u128,
    /// The maximum number of simultaneously allocated qubits, in 64 bits on every host: a register can
    /// have 2^32 - 1 qubits, and several can be live.
    pub peak_qubits: u64,
    /// The largest gate layer, counting dependencies between gates on the same live wire.
    pub depth: u128,
    /// Qubits discarded by partial trace.
    pub discarded_qubits: u128,
    /// Qubits released in zero.
    pub zero_released_qubits: u128,
}

impl Counts {
    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        let count = match gate.controls.len() {
            0 => &mut self.one_qubit,
            1 => &mut self.two_qubit,
            2 if gate.kind == Gate::Fixed(FixedGate::X) => &mut self.toffoli,
            _ => return Err(RunError::Device("gate needs control lowering")),
        };
        *count = count.checked_add(1).ok_or(RunError::Device("gate count overflow"))?;
        Ok(())
    }

    fn add_block(&mut self, block: Self, times: u64) -> Result<(), RunError> {
        let add = |total: u128, per_call: u128| {
            per_call
                .checked_mul(u128::from(times))
                .and_then(|delta| total.checked_add(delta))
                .ok_or(RunError::Device("device count overflow"))
        };
        *self = Self {
            one_qubit: add(self.one_qubit, block.one_qubit)?,
            two_qubit: add(self.two_qubit, block.two_qubit)?,
            toffoli: add(self.toffoli, block.toffoli)?,
            zero_released_qubits: add(self.zero_released_qubits, block.zero_released_qubits)?,
            peak_qubits: self.peak_qubits.max(block.peak_qubits),
            ..*self
        };
        Ok(())
    }
}

/// The allocated wires and the layer of each wire a gate has acted on since its allocation; an
/// allocated wire without a layer entry is at layer 0.
#[derive(Default)]
struct LiveWires {
    runs: RangeSet<Wire>,
    /// The number of wires in `runs`.
    count: u64,
    layers: BTreeMap<Wire, u128>,
}

impl LiveWires {
    fn contains(&self, wire: Wire) -> bool {
        self.runs.contains(&wire)
    }

    /// How many of `wires` are live.
    fn live_in(&self, wires: &Range<Wire>) -> u64 {
        covered(&self.runs, wires)
    }

    /// Makes `wires` live at layer 0, as a fresh allocation is.
    fn allocate(&mut self, wires: Range<Wire>) {
        if wires.is_empty() {
            return;
        }
        self.count += (wires.end - wires.start) - self.live_in(&wires);
        self.runs.insert(wires.clone());
        self.forget_layers(wires);
    }

    fn end(&mut self, wires: Range<Wire>) {
        if wires.is_empty() {
            return;
        }
        self.count -= self.live_in(&wires);
        self.runs.remove(wires.clone());
        self.forget_layers(wires);
    }

    fn forget_layers(&mut self, wires: Range<Wire>) {
        let touched: Vec<Wire> = self.layers.range(wires).map(|(&wire, _)| wire).collect();
        for wire in touched {
            self.layers.remove(&wire);
        }
    }

    fn layer(&self, wire: Wire) -> u128 {
        self.layers.get(&wire).copied().unwrap_or(0)
    }
}

/// How many of `wires` lie in `runs`.
fn covered(runs: &RangeSet<Wire>, wires: &Range<Wire>) -> u64 {
    runs.overlapping(wires.clone()).map(|run| run.end.min(wires.end) - run.start.max(wires.start)).sum()
}

/// A block's resources and layer dependencies, independent of the live wires around a call. Its scratch
/// is kept as runs and a schedule row only for scratch wires a gate uses, so a block that allocates a wide
/// register costs host memory for the qubits it uses, as a direct allocation does.
struct BlockSummary {
    schedule: Schedule,
    counts: Counts,
    inputs: HashSet<Wire>,
    /// Every wire the block allocates.
    scratch_wires: RangeSet<Wire>,
}

impl BlockSummary {
    fn new(block: &GateBlock) -> Result<Self, RunError> {
        let mut counts = Counts::default();
        let mut schedule = Schedule::default();
        let mut inputs = HashSet::new();
        // The scratch allocated now, how many wires it has, the part no gate has used since its
        // allocation (its schedule row, at layer 0, is made at its first gate), and the part that has rows.
        let mut scratch = RangeSet::new();
        let mut scratch_count = 0u64;
        let mut fresh = RangeSet::new();
        let mut scheduled = BTreeSet::new();
        let mut scratch_wires = RangeSet::new();
        for op in block.ops() {
            match op {
                BlockOp::Allocate(wires) if wires.is_empty() => {}
                BlockOp::Allocate(wires) => {
                    if scratch.overlaps(wires) {
                        return Err(RunError::Device("block allocates a live wire"));
                    }
                    scratch.insert(wires.clone());
                    fresh.insert(wires.clone());
                    scratch_wires.insert(wires.clone());
                    scratch_count += wires.end - wires.start;
                    counts.peak_qubits = counts.peak_qubits.max(scratch_count);
                }
                BlockOp::Gate(gate) => {
                    for wire in std::iter::once(gate.target).chain(gate.controls.iter().copied()) {
                        if !scratch.contains(&wire) {
                            inputs.insert(wire);
                        } else if fresh.contains(&wire) {
                            fresh.remove(wire..wire + 1);
                            scheduled.insert(wire);
                            schedule.allocate(wire);
                        }
                    }
                    counts.gate(gate.borrowed())?;
                    schedule.gate(gate);
                }
                BlockOp::ReleaseZero(wires) if wires.is_empty() => {}
                BlockOp::ReleaseZero(wires) => {
                    let count = wires.end - wires.start;
                    if covered(&scratch, wires) != count {
                        return Err(RunError::Device("block can only release its own scratch"));
                    }
                    scratch.remove(wires.clone());
                    fresh.remove(wires.clone());
                    let released: Vec<Wire> = scheduled.range(wires.clone()).copied().collect();
                    for wire in released {
                        scheduled.remove(&wire);
                        schedule.release(wire);
                    }
                    scratch_count -= count;
                    counts.zero_released_qubits += u128::from(count);
                }
            }
        }
        if !scratch.is_empty() {
            return Err(RunError::Device("block leaves scratch allocated"));
        }
        Ok(Self { schedule, counts, inputs, scratch_wires })
    }

    fn repeat(&self, times: u64, live: &mut LiveWires, counts: &mut Counts) -> Result<(), RunError> {
        if self.scratch_wires.iter().any(|run| live.runs.overlaps(run)) {
            return Err(RunError::Device("block allocates a live wire"));
        }
        if !self.inputs.iter().all(|&wire| live.contains(wire)) {
            return Err(RunError::Device("block uses an unallocated wire"));
        }
        let mut per_call = self.counts;
        per_call.peak_qubits =
            per_call.peak_qubits.checked_add(live.count).ok_or(RunError::Device("device count overflow"))?;
        let mut total = *counts;
        total.add_block(per_call, times)?;
        self.schedule.apply_repeated(times, &mut live.layers, &mut total.depth)?;
        *counts = total;
        Ok(())
    }
}

/// Counts native gates. Measurement takes one bit from the supplied iterator per qubit.
pub struct CountingDevice<I = Repeat<bool>> {
    bits: I,
    outcomes: Vec<bool>,
    live: LiveWires,
    counts: Counts,
    blocks: HashMap<GateBlockKey, BlockSummary>,
    #[cfg(test)]
    summaries_built: usize,
}

impl<I: Iterator<Item = bool>> CountingDevice<I> {
    /// Use the given measurement source. If it runs out, measurement stops the run.
    pub fn new(bits: I) -> Self {
        Self {
            bits,
            outcomes: Vec::new(),
            live: LiveWires::default(),
            counts: Counts::default(),
            blocks: HashMap::new(),
            #[cfg(test)]
            summaries_built: 0,
        }
    }

    /// Resources counted so far.
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// Bits returned by measurements so far, in the order they were consumed.
    pub fn outcomes(&self) -> &[bool] {
        &self.outcomes
    }

    fn end_wires(&mut self, wires: Range<Wire>) {
        self.live.end(wires);
    }
}

impl Default for CountingDevice {
    /// A counter that always measures zero.
    fn default() -> Self {
        Self::new(repeat(false))
    }
}

/// Native gate counts and depth with every participating wire starting in layer zero.
pub(crate) fn isolated_gate_counts<'a>(gates: impl IntoIterator<Item = &'a GateOp>) -> Counts {
    let mut device = CountingDevice::default();
    for gate in gates {
        for &wire in std::iter::once(&gate.target).chain(&gate.controls) {
            if !device.live.contains(wire) {
                device.live.allocate(wire..wire + 1);
            }
        }
        device.gate(gate.borrowed()).expect("the emitter has lowered the controls");
    }
    device.counts()
}

/// Reproducible synthetic measurement bits, independent of the gates issued.
pub fn seeded_bits(seed: u64) -> impl Iterator<Item = bool> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    from_fn(move || Some(rng.random()))
}

impl<I: Iterator<Item = bool>> Device for CountingDevice<I> {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.live.allocate(wires);
        self.counts.peak_qubits = self.counts.peak_qubits.max(self.live.count);
        Ok(())
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        let wires = || std::iter::once(&gate.target).chain(gate.controls);
        if !wires().all(|&wire| self.live.contains(wire)) {
            return Err(RunError::Device("gate on an unallocated wire"));
        }
        let latest = wires().map(|&wire| self.live.layer(wire)).max().expect("gate has a target");
        let layer = latest.checked_add(1).ok_or(RunError::Device("gate depth overflow"))?;
        self.counts.gate(gate)?;
        for wire in wires() {
            self.live.layers.insert(*wire, layer);
        }
        self.counts.depth = self.counts.depth.max(layer);
        Ok(())
    }

    fn measure(&mut self, _wire: Wire) -> Result<bool, RunError> {
        let outcome = self.bits.next().ok_or(RunError::Device("measurement source exhausted"))?;
        self.outcomes.push(outcome);
        Ok(outcome)
    }

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.counts.discarded_qubits = self
            .counts
            .discarded_qubits
            .checked_add(u128::from(wires.end.saturating_sub(wires.start)))
            .ok_or(RunError::Device("device count overflow"))?;
        self.end_wires(wires);
        Ok(())
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.counts.zero_released_qubits = self
            .counts
            .zero_released_qubits
            .checked_add(u128::from(wires.end.saturating_sub(wires.start)))
            .ok_or(RunError::Device("device count overflow"))?;
        self.end_wires(wires);
        Ok(())
    }

    fn repeat(&mut self, block: &GateBlock, times: u64) -> Result<(), RunError> {
        if times > 0 {
            let key = block.key();
            if !self.blocks.contains_key(&key) {
                self.blocks.retain(|key, _| key.is_live());
            }
            let summary = match self.blocks.entry(key) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let summary = BlockSummary::new(block)?;
                    #[cfg(test)]
                    {
                        self.summaries_built += 1;
                    }
                    entry.insert(summary)
                }
            };
            summary.repeat(times, &mut self.live, &mut self.counts)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LowerControls;

    #[test]
    fn calls_reuse_a_summary_across_layer_and_unrelated_allocation_changes() {
        let mut device = CountingDevice::default();
        device.allocate(0..1).expect("input");
        let block = GateBlock::new("flip", [GateOp { kind: Gate::Fixed(FixedGate::X), target: 0, controls: vec![] }]);
        device.call(&block).expect("first call counts");
        device.allocate(1..3).expect("unrelated wires");
        device.call(&block.clone()).expect("clones share the summary");
        assert_eq!(device.summaries_built, 1);
        assert_eq!(device.counts().depth, 2);
        assert_eq!(device.counts().peak_qubits, 3);
        device.discard(0..1).expect("input gone");
        assert_eq!(device.call(&block), Err(RunError::Device("block uses an unallocated wire")));
        assert_eq!(device.summaries_built, 1);
    }

    #[test]
    fn repeated_lowered_calls_reuse_the_definition_with_the_same_wire_placement() {
        let mut device = LowerControls::new(CountingDevice::default());
        device.allocate(0..3).expect("inputs");
        let block = GateBlock::new(
            "two controls",
            [GateOp { kind: Gate::Fixed(FixedGate::H), target: 2, controls: vec![0, 1] }],
        );
        device.call(&block).expect("first call lowers");
        device.call(&block).expect("call again");
        assert_eq!(device.inner().summaries_built, 1);
        assert_eq!(device.inner().counts().two_qubit, 2);
        assert_eq!(device.inner().counts().toffoli, 4);
    }

    #[test]
    fn unrelated_allocations_do_not_accumulate_summaries_for_one_block() {
        for gate in [
            GateOp { kind: Gate::Fixed(FixedGate::X), target: 2, controls: vec![] },
            GateOp { kind: Gate::Fixed(FixedGate::H), target: 2, controls: vec![0, 1] },
        ] {
            let mut device = LowerControls::new(CountingDevice::default());
            device.allocate(0..3).expect("input wires");
            let is_x = gate.kind == Gate::Fixed(FixedGate::X);
            let block = GateBlock::new("fixed block", [gate]);
            for _ in 0..20_000 {
                device.allocate(3..4).expect("unrelated wire");
                device.call(&block).expect("call the same block");
                device.release_zero(3..4).expect("release the unrelated wire");
            }
            assert_eq!(device.inner().counts().one_qubit, if is_x { 20_000 } else { 0 });
            assert_eq!(device.inner().counts().two_qubit, if is_x { 0 } else { 20_000 });
            assert_eq!(device.inner().blocks.len(), 1);
            assert_eq!(device.inner().summaries_built, 1);
            if is_x {
                assert!(device.inner().blocks.contains_key(&block.key()));
            }
        }
    }

    #[test]
    fn reassigned_scratch_does_not_retain_obsolete_summaries() {
        let mut device = LowerControls::new(CountingDevice::default());
        device.allocate(0..3).expect("input wires");
        let block = GateBlock::new(
            "two controls",
            [GateOp { kind: Gate::Fixed(FixedGate::H), target: 2, controls: vec![0, 1] }],
        );
        device.call(&block).expect("first placement");
        for wire in (Wire::MAX - 32..Wire::MAX).rev() {
            device.allocate(wire..wire + 1).expect("claim the last scratch wire");
            device.call(&block).expect("call with a new scratch location");
            assert_eq!(device.inner().blocks.len(), 1);
        }
        assert_eq!(device.inner().summaries_built, 33);
        assert_eq!(device.inner().counts().two_qubit, 33);
        assert_eq!(device.inner().counts().toffoli, 66);
        assert_eq!(device.inner().counts().peak_qubits, 36);
    }
}
