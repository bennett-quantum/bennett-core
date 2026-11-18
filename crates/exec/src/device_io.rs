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

/// Selects the device for the forward step. Borrowing an external device only for its run keeps
/// the simulator executor independent of that device's lifetime and thread safety.
#[derive(Default)]
pub(crate) struct DeviceContext<'a> {
    pub(crate) external: Option<&'a mut dyn Device>,
    pub(crate) limit: Option<usize>,
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

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.state.discard(qubit(wires.start).column);
        Ok(())
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.state.free(qubit(wires.start).column);
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
        context: &mut DeviceContext<'_>,
        command: impl FnOnce(&mut dyn Device) -> Result<T, RunError>,
    ) -> Result<T, RunError> {
        match context.external.as_deref_mut() {
            Some(device) => command(device),
            None => command(&mut Simulator { state: &mut self.quantum, rng: &mut self.rng, limit: context.limit }),
        }
    }

    pub(crate) fn allocate_register(&mut self, width: Width, context: &mut DeviceContext<'_>) -> Result<u64, RunError> {
        let handle = self.quantum.reserve(width);
        let column = self.quantum.resolve(handle).expect("fresh register");
        self.with_device(context, |device| device.allocate(wires(column, width.bits())))?;
        Ok(handle)
    }

    pub(crate) fn device_gate(
        &mut self,
        kind: Gate,
        target: Wire,
        controls: &[Wire],
        context: &mut DeviceContext<'_>,
    ) -> Result<(), RunError> {
        self.with_device(context, |device| device.gate(GateOp { kind, target, controls }))
    }

    pub(crate) fn device_measure(
        &mut self,
        ranges: &[Range<Wire>],
        context: &mut DeviceContext<'_>,
    ) -> Result<Vec<u64>, RunError> {
        self.with_device(context, |device| device.measure_many(ranges))
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
        self.with_device(context, command)?;
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
