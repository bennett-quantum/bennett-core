//! Map the controller's register columns to device wires. The simulator keeps its own shortcuts
//! for setting definite values and its own diagnostics and journal support.

use std::collections::BTreeMap;
use std::ops::Range;

use bennett_ir::{FixedGate, Gate, Width};
use rand_chacha::ChaCha8Rng;

use crate::apply_device::ApplyCompiler;
use crate::device::{Device, GateOp, Wire};
use crate::executor::{Executor, RunError};
use crate::quantum::{QuantumState, QubitRef};

pub(crate) fn wire(column: usize, bit: u8) -> Wire {
    column as Wire * 64 + Wire::from(bit)
}

pub(crate) fn wires(column: usize, count: u32) -> Range<Wire> {
    let first = wire(column, 0);
    first..first + Wire::from(count)
}

/// Selects the device for the forward step. Borrowing an external device only for its run keeps
/// the simulator executor independent of that device's lifetime and thread safety.
#[derive(Default)]
pub(crate) struct DeviceContext<'a> {
    pub(crate) external: Option<&'a mut dyn Device>,
    pub(crate) compiler: Option<&'a mut dyn ApplyCompiler>,
    pub(crate) limit: Option<usize>,
}

/// Adapts the sparse simulator's register columns to device commands.
pub(crate) struct Simulator<'a> {
    pub(crate) state: &'a mut QuantumState,
    pub(crate) rng: &'a mut ChaCha8Rng,
    pub(crate) limit: Option<usize>,
    /// The controller reserves a column for a language register before issuing its allocation.
    pub(crate) reserved_column: Option<usize>,
}

/// A masked fragment of one output word in the joint measurement key.
struct MeasurementPart {
    qubit: QubitRef,
    mask: u64,
    word: usize,
    bit: u64,
}

impl Simulator<'_> {
    fn qubit(&self, wire: Wire) -> Result<QubitRef, RunError> {
        self.state.device_wires.qubit(wire).ok_or(RunError::Device("wire is not allocated"))
    }

    fn masks(&self, wires: Range<Wire>) -> Result<BTreeMap<usize, u64>, RunError> {
        let mut masks = BTreeMap::new();
        for wire in wires {
            let qubit = self.qubit(wire)?;
            *masks.entry(qubit.column).or_default() |= 1u64 << qubit.bit;
        }
        Ok(masks)
    }

    fn measurement_parts(&self, ranges: &[Range<Wire>]) -> Result<Vec<MeasurementPart>, RunError> {
        let mut parts = Vec::new();
        for (word, range) in ranges.iter().enumerate() {
            let width = range.end.saturating_sub(range.start);
            if width > 64 {
                return Err(RunError::Device("measurement range exceeds 64 wires"));
            }
            // Read each run of contiguous storage from high to low. The key's fragment order
            // then compares outcomes like the assembled little-endian word.
            let mut remaining = width;
            while remaining != 0 {
                let qubit = self.qubit(range.start + remaining - 1)?;
                let live = self.state.device_wires.mask(qubit.column);
                let count = u64::from((live << (63 - qubit.bit)).leading_ones()).min(remaining);
                let low = qubit.bit + 1 - count as u8;
                let mask = (u64::MAX >> (64 - count)) << low;
                remaining -= count;
                parts.push(MeasurementPart { qubit: QubitRef { bit: low, ..qubit }, mask, word, bit: remaining });
            }
        }
        Ok(parts)
    }
}

impl Device for Simulator<'_> {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        if wires.clone().any(|wire| self.state.device_wires.qubit(wire).is_some()) {
            return Err(RunError::Device("wire is already allocated"));
        }
        let mut start = wires.start;
        while start < wires.end {
            let end = wires.end.min(start.saturating_add(64));
            let column = match self.reserved_column.take() {
                Some(column) => column,
                None => {
                    let width = Width::new((end - start) as u32).expect("width of a storage column");
                    let handle = self.state.reserve(width);
                    self.state.resolve(handle).expect("reserved column")
                }
            };
            self.state.ensure_column(column);
            self.state.device_wires.bind(column, start..end);
            start = end;
        }
        Ok(())
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        let target = self.qubit(gate.target)?;
        let controls = gate.controls.iter().map(|&wire| self.qubit(wire)).collect::<Result<Vec<_>, _>>()?;
        self.state.apply_gate(&crate::gates::matrix(gate.kind), target, &controls, self.limit)
    }

    fn measure(&mut self, wire: Wire) -> Result<bool, RunError> {
        let range = wire..wire.checked_add(1).ok_or(RunError::Device("wire is not allocated"))?;
        Ok(self.measure_many(std::slice::from_ref(&range))?[0] != 0)
    }

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.state.discard(self.masks(wires)?);
        Ok(())
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        let masks = self.masks(wires)?;
        if masks.iter().any(|(&column, &mask)| !self.state.mask_is_zero(column, mask)) {
            return Err(RunError::Device("release_zero requires zero wires"));
        }
        for (column, mask) in masks {
            self.state.release_zero(column, mask);
        }
        Ok(())
    }

    fn measure_many(&mut self, ranges: &[Range<Wire>]) -> Result<Vec<u64>, RunError> {
        let parts = self.measurement_parts(ranges)?;
        let mut words = vec![0; ranges.len()];
        if !parts.is_empty() {
            let picks: Vec<_> = parts.iter().map(|part| (part.qubit.column, part.mask)).collect();
            let values = self.state.measure_bits(&picks, self.rng);
            for (part, value) in parts.into_iter().zip(values) {
                words[part.word] |= (value >> part.qubit.bit) << part.bit;
            }
        }
        Ok(words)
    }
}

impl Executor<'_> {
    fn with_device<T>(
        &mut self,
        context: &mut DeviceContext<'_>,
        reserved_column: Option<usize>,
        command: impl FnOnce(&mut dyn Device) -> Result<T, RunError>,
    ) -> Result<T, RunError> {
        match context.external.as_deref_mut() {
            Some(device) => command(device),
            None => command(&mut Simulator {
                state: &mut self.quantum,
                rng: &mut self.rng,
                limit: context.limit,
                reserved_column,
            }),
        }
    }

    pub(crate) fn allocate_register(&mut self, width: Width, context: &mut DeviceContext<'_>) -> Result<u64, RunError> {
        let handle = self.quantum.reserve(width);
        let column = self.quantum.resolve(handle).expect("fresh register");
        self.with_device(context, Some(column), |device| device.allocate(wires(column, width.bits())))?;
        Ok(handle)
    }

    pub(crate) fn device_gate(
        &mut self,
        kind: Gate,
        target: Wire,
        controls: &[Wire],
        context: &mut DeviceContext<'_>,
    ) -> Result<(), RunError> {
        self.with_device(context, None, |device| device.gate(GateOp { kind, target, controls }))
    }

    pub(crate) fn device_measure(
        &mut self,
        ranges: &[Range<Wire>],
        context: &mut DeviceContext<'_>,
    ) -> Result<Vec<u64>, RunError> {
        self.with_device(context, None, |device| device.measure_many(ranges))
    }

    pub(crate) fn discard_register(
        &mut self,
        column: usize,
        width: Width,
        context: &mut DeviceContext<'_>,
    ) -> Result<(), RunError> {
        self.end_register(column, context, |device| device.discard(wires(column, width.bits())))
    }

    pub(crate) fn release_zero_register(
        &mut self,
        column: usize,
        width: Width,
        context: &mut DeviceContext<'_>,
    ) -> Result<(), RunError> {
        self.end_register(column, context, |device| device.release_zero(wires(column, width.bits())))
    }

    fn end_register(
        &mut self,
        column: usize,
        context: &mut DeviceContext<'_>,
        command: impl FnOnce(&mut dyn Device) -> Result<(), RunError>,
    ) -> Result<(), RunError> {
        self.with_device(context, None, command)?;
        if context.external.is_some() {
            self.quantum.retire(column);
        }
        Ok(())
    }

    /// XOR a known word into a register. The simulator carries out these X gates as one permutation.
    pub(crate) fn xor_register(
        &mut self,
        column: usize,
        value: u64,
        context: &mut DeviceContext<'_>,
    ) -> Result<(), RunError> {
        if context.external.is_none() {
            self.quantum.xor_register(column, value);
        } else {
            let mut pending = value;
            while pending != 0 {
                let bit = pending.trailing_zeros() as u8;
                self.device_gate(Gate::Fixed(FixedGate::X), wire(column, bit), &[], context)?;
                pending &= pending - 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn batches_measure_within_at_and_across_storage_boundaries() {
        let mut state = QuantumState::default();
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let mut sim = Simulator { state: &mut state, rng: &mut rng, limit: None, reserved_column: None };
        sim.allocate(0..64).expect("first column");
        sim.allocate(64..66).expect("second column");
        for target in [2, 63, 64] {
            sim.gate(GateOp { kind: Gate::Fixed(FixedGate::X), target, controls: &[] }).expect("set wire");
        }
        for (range, expected) in [(1..4, 2), (62..64, 2), (64..66, 1), (63..65, 3), (62..66, 6)] {
            assert_eq!(sim.measure_many(std::slice::from_ref(&range)), Ok(vec![expected]), "{range:?}");
        }
    }

    #[test]
    fn subcolumn_release_preserves_a_live_neighbour() {
        let mut state = QuantumState::default();
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let mut sim = Simulator { state: &mut state, rng: &mut rng, limit: None, reserved_column: None };
        sim.allocate(0..3).expect("shared column");
        sim.gate(GateOp { kind: Gate::Fixed(FixedGate::X), target: 0, controls: &[] }).expect("set neighbour");
        sim.release_zero(1..2).expect("only the zero wire is released");
        assert_eq!(sim.measure(0), Ok(true));
        sim.allocate(1..2).expect("reallocated in zero");
        assert_eq!(sim.measure(1), Ok(false));
        assert_eq!(sim.measure(2), Ok(false));
    }

    #[test]
    fn crossing_release_and_reallocation_leave_both_neighbours_live() {
        let mut state = QuantumState::default();
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let mut sim = Simulator { state: &mut state, rng: &mut rng, limit: None, reserved_column: None };
        sim.allocate(0..66).expect("two storage columns");
        let x = Gate::Fixed(FixedGate::X);
        for target in [62, 65] {
            sim.gate(GateOp { kind: x, target, controls: &[] }).expect("set neighbour");
        }
        sim.release_zero(63..65).expect("straddling release");
        assert_eq!(sim.measure(63), Err(RunError::Device("wire is not allocated")));
        sim.allocate(63..65).expect("fresh scratch in zero");
        let range = 62..66;
        assert_eq!(sim.measure_many(std::slice::from_ref(&range)), Ok(vec![9]));
    }

    #[test]
    fn a_block_can_reuse_a_discarded_wire_without_losing_a_correlation() {
        use crate::{BlockOp, GateBlock};

        let mut state = QuantumState::default();
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let mut sim = Simulator { state: &mut state, rng: &mut rng, limit: None, reserved_column: None };
        sim.allocate(0..2).expect("same storage column");
        sim.gate(GateOp { kind: Gate::Fixed(FixedGate::H), target: 0, controls: &[] }).expect("superposition");
        let x = Gate::Fixed(FixedGate::X);
        sim.gate(GateOp { kind: x, target: 1, controls: &[0] }).expect("Bell pair");
        assert_eq!(sim.release_zero(0..1), Err(RunError::Device("release_zero requires zero wires")));
        sim.discard(0..1).expect("keep the partial trace");
        let block = GateBlock::from_ops(
            "reuse released wire",
            [
                BlockOp::Allocate(0..1),
                BlockOp::Gate(GateOp { kind: x, target: 0, controls: vec![1] }),
                BlockOp::Gate(GateOp { kind: x, target: 0, controls: vec![1] }),
                BlockOp::ReleaseZero(0..1),
            ],
        );
        sim.repeat(&block, 2).expect("use clean scratch without disturbing the live neighbour");
        sim.gate(GateOp { kind: Gate::Fixed(FixedGate::H), target: 1, controls: &[] }).expect("rotate live neighbour");
        assert_eq!(state.configurations(), 4, "each of two retained values has both live outcomes");
        assert_eq!(state.leftover_qubits(), 1);
        assert!(state.amplitudes().values().all(|coefficient| (coefficient.norm_sqr() - 0.25).abs() < 1e-12));
    }

    #[test]
    fn one_cross_column_discard_records_one_union_probability() {
        for entangled in [false, true] {
            let mut state = QuantumState::default();
            let mut rng = ChaCha8Rng::seed_from_u64(0);
            let mut sim = Simulator { state: &mut state, rng: &mut rng, limit: None, reserved_column: None };
            sim.allocate(0..65).expect("two storage columns");
            let x = Gate::Fixed(FixedGate::X);
            if entangled {
                sim.gate(GateOp { kind: Gate::Fixed(FixedGate::H), target: 63, controls: &[] })
                    .expect("prepare Bell pair");
                sim.gate(GateOp { kind: x, target: 64, controls: &[63] }).expect("correlate both wires");
            } else {
                sim.gate(GateOp { kind: x, target: 63, controls: &[] }).expect("set bit 63");
                sim.gate(GateOp { kind: x, target: 64, controls: &[] }).expect("set bit 64");
            }
            sim.discard(63..65).expect("one discard");
            let expected = if entangled { 0.5 } else { 1.0 };
            assert!((state.nonzero_release_weight - expected).abs() < 1e-12);
            assert_eq!(state.nonzero_releases, 1);
            assert_eq!(state.leftover_qubits(), 2);
        }
    }

    #[test]
    fn a_whole_64_bit_register_uses_one_word_per_measurement_key() {
        let mut state = QuantumState::default();
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let mut sim = Simulator { state: &mut state, rng: &mut rng, limit: None, reserved_column: None };
        sim.allocate(0..64).expect("one full register");
        let range = 0..64;
        let parts = sim.measurement_parts(std::slice::from_ref(&range)).expect("valid measure");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].mask, u64::MAX);
        assert_eq!(sim.measure_many(std::slice::from_ref(&range)), Ok(vec![0]));
    }
}
