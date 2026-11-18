//! Count gates and live wires without keeping a quantum state.

use std::collections::{HashMap, HashSet};
use std::iter::{Repeat, from_fn, repeat};
use std::ops::Range;

use bennett_exec::{BlockOp, Device, GateBlock, GateOp, RunError, Wire};
use bennett_ir::{FixedGate, Gate};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

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
    /// The maximum number of simultaneously allocated qubits.
    pub peak_qubits: usize,
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

/// Counts native gates. Measurement takes one bit from the supplied iterator per qubit.
pub struct CountingDevice<I = Repeat<bool>> {
    bits: I,
    layers: HashMap<Wire, u128>,
    counts: Counts,
}

impl<I: Iterator<Item = bool>> CountingDevice<I> {
    /// Use the given measurement source. If it runs out, measurement stops the run.
    pub fn new(bits: I) -> Self {
        Self { bits, layers: HashMap::new(), counts: Counts::default() }
    }

    /// Resources counted so far.
    pub fn counts(&self) -> Counts {
        self.counts
    }

    fn summarize(&self, block: &GateBlock) -> Result<(Schedule, Counts), RunError> {
        let mut counts = Counts::default();
        let mut schedule = Schedule::default();
        let mut scratch = HashSet::new();
        for op in &block.ops {
            match op {
                BlockOp::Allocate(wires) => {
                    for wire in wires.clone() {
                        if self.layers.contains_key(&wire) || !scratch.insert(wire) {
                            return Err(RunError::Device("block allocates a live wire"));
                        }
                        schedule.allocate(wire);
                    }
                    counts.peak_qubits = counts.peak_qubits.max(self.layers.len() + scratch.len());
                }
                BlockOp::Gate(gate) => {
                    let live = |wire: &Wire| self.layers.contains_key(wire) || scratch.contains(wire);
                    if !std::iter::once(&gate.target).chain(&gate.controls).all(live) {
                        return Err(RunError::Device("block uses an unallocated wire"));
                    }
                    counts.gate(gate.borrowed())?;
                    schedule.gate(gate);
                }
                BlockOp::ReleaseZero(wires) => {
                    for wire in wires.clone() {
                        if !scratch.remove(&wire) {
                            return Err(RunError::Device("block can only release its own scratch"));
                        }
                        schedule.release(wire);
                        counts.zero_released_qubits += 1;
                    }
                }
            }
        }
        if !scratch.is_empty() {
            return Err(RunError::Device("block leaves scratch allocated"));
        }
        Ok((schedule, counts))
    }

    fn end_wires(&mut self, wires: Range<Wire>) {
        for wire in wires {
            self.layers.remove(&wire);
        }
    }
}

impl Default for CountingDevice {
    /// A counter that always measures zero.
    fn default() -> Self {
        Self::new(repeat(false))
    }
}

/// Reproducible synthetic measurement bits, independent of the gates issued.
pub fn seeded_bits(seed: u64) -> impl Iterator<Item = bool> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    from_fn(move || Some(rng.random()))
}

impl<I: Iterator<Item = bool>> Device for CountingDevice<I> {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.layers.extend(wires.map(|wire| (wire, 0)));
        self.counts.peak_qubits = self.counts.peak_qubits.max(self.layers.len());
        Ok(())
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        let wires = || std::iter::once(&gate.target).chain(gate.controls);
        let latest = wires().map(|wire| self.layers[wire]).max().expect("gate has a target");
        let layer = latest.checked_add(1).ok_or(RunError::Device("gate depth overflow"))?;
        self.counts.gate(gate)?;
        for wire in wires() {
            self.layers.insert(*wire, layer);
        }
        self.counts.depth = self.counts.depth.max(layer);
        Ok(())
    }

    fn measure(&mut self, _wire: Wire) -> Result<bool, RunError> {
        self.bits.next().ok_or(RunError::Device("measurement source exhausted"))
    }

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.counts.discarded_qubits = self
            .counts
            .discarded_qubits
            .checked_add(u128::from(wires.end - wires.start))
            .ok_or(RunError::Device("device count overflow"))?;
        self.end_wires(wires);
        Ok(())
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.counts.zero_released_qubits = self
            .counts
            .zero_released_qubits
            .checked_add(u128::from(wires.end - wires.start))
            .ok_or(RunError::Device("device count overflow"))?;
        self.end_wires(wires);
        Ok(())
    }

    fn repeat(&mut self, block: &GateBlock, times: u64) -> Result<(), RunError> {
        if times > 0 {
            let (schedule, block_counts) = self.summarize(block)?;
            let mut counts = self.counts;
            counts.add_block(block_counts, times)?;
            schedule.apply_repeated(times, &mut self.layers, &mut counts.depth)?;
            self.counts = counts;
        }
        Ok(())
    }
}
