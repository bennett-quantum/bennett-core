//! Map the controller's register columns to device wires. The simulator keeps its own shortcuts
//! for setting definite values and its own diagnostics and journal support.

use std::ops::Range;

use bennett_ir::{FixedGate, Gate, Width};
use rand_chacha::ChaCha8Rng;

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

pub(crate) fn qubit(wire: Wire) -> QubitRef {
    QubitRef { column: (wire / 64) as usize, bit: (wire % 64) as u8 }
}

/// Adapts the sparse simulator's register columns to device commands.
struct Simulator<'a> {
    state: &'a mut QuantumState,
    rng: &'a mut ChaCha8Rng,
    limit: Option<usize>,
}

impl Device for Simulator<'_> {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.state.ensure_column(qubit(wires.start).column);
        Ok(())
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        self.state.apply_gate(&crate::gates::matrix(gate.kind), qubit(gate.target), gate.controls, self.limit)
    }

    fn measure(&mut self, wire: Wire) -> Result<bool, RunError> {
        let range = wire..wire + 1;
        Ok(self.measure_many(std::slice::from_ref(&range))?[0] != 0)
    }

    fn release(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.state.discard(qubit(wires.start).column);
        Ok(())
    }

    fn measure_many(&mut self, ranges: &[Range<Wire>]) -> Result<Vec<u64>, RunError> {
        let picks: Vec<_> = ranges
            .iter()
            .map(|range| {
                let first = qubit(range.start);
                let mask = u64::MAX >> (64 - (range.end - range.start));
                (first.column, mask << first.bit)
            })
            .collect();
        let values = self.state.measure_bits(&picks, self.rng);
        Ok(values.into_iter().zip(ranges).map(|(value, range)| value >> qubit(range.start).bit).collect())
    }
}

impl Executor<'_> {
    fn with_device<T>(
        &mut self,
        limit: Option<usize>,
        command: impl FnOnce(&mut dyn Device) -> Result<T, RunError>,
    ) -> Result<T, RunError> {
        match self.device.as_deref_mut() {
            Some(device) => command(device),
            None => command(&mut Simulator { state: &mut self.quantum, rng: &mut self.rng, limit }),
        }
    }

    pub(crate) fn allocate_register(&mut self, width: Width) -> Result<u64, RunError> {
        let handle = self.quantum.reserve(width);
        let column = self.quantum.resolve(handle).expect("fresh register");
        self.with_device(None, |device| device.allocate(wires(column, width.bits())))?;
        Ok(handle)
    }

    pub(crate) fn device_gate(
        &mut self,
        kind: Gate,
        target: Wire,
        controls: &[Wire],
        limit: Option<usize>,
    ) -> Result<(), RunError> {
        self.with_device(limit, |device| device.gate(GateOp { kind, target, controls }))
    }

    pub(crate) fn device_measure(&mut self, ranges: &[Range<Wire>]) -> Result<Vec<u64>, RunError> {
        self.with_device(None, |device| device.measure_many(ranges))
    }

    pub(crate) fn release_register(&mut self, column: usize, width: Width) -> Result<(), RunError> {
        self.with_device(None, |device| device.release(wires(column, width.bits())))?;
        if self.device.is_some() {
            self.quantum.retire(column);
        }
        Ok(())
    }

    /// XOR a known word into a register. The simulator carries out these X gates as one permutation.
    pub(crate) fn xor_register(&mut self, column: usize, value: u64) -> Result<(), RunError> {
        if self.device.is_none() {
            self.quantum.xor_register(column, value);
        } else {
            let mut pending = value;
            while pending != 0 {
                let bit = pending.trailing_zeros() as u8;
                self.device_gate(Gate::Fixed(FixedGate::X), wire(column, bit), &[], None)?;
                pending &= pending - 1;
            }
        }
        Ok(())
    }
}
