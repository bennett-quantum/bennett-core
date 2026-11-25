//! Temporary wire numbering and final placement of an applied circuit.

use std::collections::BTreeSet;
use std::ops::Range;

use bennett_exec::{GateBlock, Wire};

use super::bits::Circuit;
use super::{ApplyError, ApplyInput, Argument};

pub(super) struct Layout {
    interface: Vec<Wire>,
    physical_scratch: Wire,
}

impl Layout {
    pub(super) fn new(input: &ApplyInput<'_>) -> Self {
        let mut interface: BTreeSet<_> = input.out.clone().chain(input.fault.clone()).collect();
        for arg in input.args {
            if let Argument::Register(wires) = arg {
                interface.extend(wires.clone());
            }
        }
        Self { interface: interface.into_iter().collect(), physical_scratch: input.scratch_start }
    }

    pub(super) fn arguments(&self, args: &[Argument]) -> Vec<Argument> {
        args.iter()
            .map(|arg| match arg {
                Argument::Register(wires) => Argument::Register(self.local(wires.clone())),
                Argument::Value(value) => Argument::Value(*value),
            })
            .collect()
    }

    pub(super) fn local(&self, wires: Range<Wire>) -> Range<Wire> {
        // Sorting distinct interface wires preserves the contiguity of each
        // operand range and the overlap between argument slices.
        let start = self.interface.binary_search(&wires.start).expect("validated interface range") as Wire;
        start..start + (wires.end - wires.start)
    }

    pub(super) fn scratch_start(&self) -> Wire {
        self.interface.len() as Wire
    }

    pub(super) fn fits(&self, circuit: &Circuit) -> bool {
        circuit.fits_at(self.physical_scratch)
    }

    pub(super) fn finish(&self, circuit: Circuit, forward: usize, name: &str) -> Result<GateBlock, ApplyError> {
        if !self.fits(&circuit) {
            return Err(ApplyError::WireSpace);
        }
        if self.physical_scratch == self.scratch_start() && self.interface.iter().copied().eq(0..self.scratch_start()) {
            return circuit.finish(forward, name);
        }
        circuit.finish_placed(forward, name, |wire| {
            if wire < self.scratch_start() {
                self.interface[wire as usize]
            } else {
                self.physical_scratch + (wire - self.scratch_start())
            }
        })
    }
}
