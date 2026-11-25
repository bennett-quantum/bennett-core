//! Address resolution and measurement for bytes holding qubits.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use bennett_ir::{Fault, Gate, Operand, Place, Width};

use super::{Device, Event};

impl Device<'_> {
    pub(super) fn memory_range(&self, address: u64, width: Width) -> Result<Range<usize>, Fault> {
        self.byte_range(address, width.bytes())
    }

    fn byte_range(&self, address: u64, bytes: u64) -> Result<Range<usize>, Fault> {
        let end = address.checked_add(bytes).filter(|&end| end <= self.module.memory_size);
        end.map(|end| address as usize..end as usize).ok_or(Fault::OutOfBounds)
    }

    pub(super) fn ordinary_range(&self, address: Operand, width: Width) -> Result<Range<usize>, Fault> {
        self.ordinary_bytes(address, width.bytes())
    }

    pub(super) fn ordinary_bytes(&self, address: Operand, bytes: u64) -> Result<Range<usize>, Fault> {
        let range = self.byte_range(self.read_i64(address), bytes)?;
        let cells = self.applied.as_ref().map_or(&self.quantum.cells, |context| context.cells);
        if cells.range(range.start as u64..range.end as u64).next().is_some() {
            return Err(Fault::QubitMemoryAccess);
        }
        Ok(range)
    }

    fn resolve_place(&self, place: Place) -> Result<ResolvedPlace, Fault> {
        match place {
            Place::Register(qubit) => {
                let (column, bit) = self.qubit(qubit)?;
                Ok(ResolvedPlace::Register(column, bit))
            }
            Place::Memory { address, bit } => {
                let byte = self
                    .read_i64(address)
                    .checked_add(u64::from(bit / 8))
                    .filter(|&byte| byte < self.module.memory_size)
                    .ok_or(Fault::OutOfBounds)?;
                Ok(ResolvedPlace::Memory(byte, bit % 8))
            }
        }
    }

    pub(super) fn gate_at(&mut self, gate: Gate, target: Place, controls: &[Place]) -> Result<(), Fault> {
        let places = std::iter::once(target)
            .chain(controls.iter().copied())
            .map(|place| self.resolve_place(place))
            .collect::<Result<Vec<_>, _>>()?;
        if places.iter().collect::<BTreeSet<_>>().len() != places.len() {
            return Err(Fault::QubitAlias);
        }
        // All operands have been admitted. It is now safe to transfer bytes into cells.
        let mut qubits = places.into_iter().map(|place| match place {
            ResolvedPlace::Register(column, bit) => (column, bit),
            ResolvedPlace::Memory(address, bit) => {
                let column = match self.quantum.cells.get(&address) {
                    Some(&column) => column,
                    None => {
                        let value = std::mem::take(&mut self.memory[address as usize]);
                        self.quantum.promote(address, value)
                    }
                };
                (column, bit)
            }
        });
        let target = qubits.next().expect("gate target");
        let controls = qubits.collect::<Vec<_>>();
        self.quantum.gate(gate, target, &controls);
        Ok(())
    }

    pub(super) fn measure_memory(
        &mut self,
        address: Operand,
        width: Width,
        observed: Option<u128>,
    ) -> Result<(u128, Event), Fault> {
        let range = self.memory_range(self.read_i64(address), width)?;
        let ordinary = little_endian(&self.memory[range.clone()]);
        let cells: Vec<_> = range
            .enumerate()
            .filter_map(|(offset, byte)| {
                self.quantum.cells.get(&(byte as u64)).map(|&column| (byte as u64, offset, column))
            })
            .collect();
        if cells.is_empty() {
            let outcome = ordinary & width.mask();
            assert_eq!(observed, Some(outcome), "ordinary memory measurement at step {}", self.steps);
            return Ok((outcome, Event::Measured { outcome, probabilities: BTreeMap::from([(outcome, 1.0)]) }));
        }
        let (outcome, event) = self.measure(observed, |basis| {
            let word = cells
                .iter()
                .fold(ordinary, |word, &(_, offset, column)| word | (u128::from(basis[column]) << (8 * offset)));
            word & width.mask()
        });
        for (byte, offset, _) in cells {
            if 8 * (offset + 1) <= width.bits() as usize {
                self.quantum.demote(byte);
                self.memory[byte as usize] = (outcome >> (8 * offset)) as u8;
            }
        }
        Ok((outcome, event))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ResolvedPlace {
    Register(usize, u8),
    Memory(u64, u8),
}

pub(super) fn little_endian(bytes: &[u8]) -> u128 {
    let mut word = [0; 16];
    word[..bytes.len()].copy_from_slice(bytes);
    u128::from_le_bytes(word)
}
