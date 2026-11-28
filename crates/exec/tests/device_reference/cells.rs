//! Address resolution and measurement for bytes holding qubits.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use bennett_ir::{CONTROL_ENTRY_BYTES, Fault, Gate, Operand, Place, Width};

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

    /// Reads an ordinary byte range for an interface list or image: length 0 needs no readable byte
    /// whatever `address` is; otherwise overflow, out-of-bounds or any held-qubit byte is `bad_argument`.
    pub(super) fn readable_ordinary_bytes(&self, address: u64, bytes: u64) -> Result<Range<usize>, Fault> {
        if bytes == 0 {
            return Ok(0..0);
        }
        let end = address.checked_add(bytes).filter(|&end| end <= self.module.memory_size).ok_or(Fault::BadArgument)?;
        let cells = self.applied.as_ref().map_or(&self.quantum.cells, |context| context.cells);
        if cells.range(address..end).next().is_some() {
            return Err(Fault::BadArgument);
        }
        Ok(address as usize..end as usize)
    }

    pub(super) fn read_control_list(&self, address: Operand, count: Operand) -> Result<Vec<(usize, u32)>, Fault> {
        let count = self.read_i64(count);
        let bytes = count.checked_mul(CONTROL_ENTRY_BYTES).ok_or(Fault::BadArgument)?;
        let range = self.readable_ordinary_bytes(self.read_i64(address), bytes)?;
        let mut controls = Vec::with_capacity(count as usize);
        for chunk in self.memory[range].as_chunks::<{ CONTROL_ENTRY_BYTES as usize }>().0 {
            let handle = u64::from_le_bytes(chunk[0..8].try_into().expect("control handle"));
            let position = u64::from_le_bytes(chunk[8..16].try_into().expect("control position"));
            let register = self.quantum.position(handle).ok_or(Fault::DanglingRegister)?;
            let width = u64::from(self.quantum.live[register].1.count());
            if position >= width {
                return Err(Fault::BadRange);
            }
            controls.push((register, position as u32));
        }
        Ok(controls)
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
            Place::Indexed { register, index } => {
                let (column, bit) = self.qubit_operand(register, self.read_i64(index))?;
                Ok(ResolvedPlace::Register(column, bit))
            }
        }
    }

    fn promote_place(&mut self, place: ResolvedPlace) -> (usize, u32) {
        match place {
            ResolvedPlace::Register(column, bit) => (column, bit),
            ResolvedPlace::Memory(address, bit) => {
                let column = match self.quantum.cells.get(&address) {
                    Some(&column) => column,
                    None => {
                        let value = std::mem::take(&mut self.memory[address as usize]);
                        self.quantum.promote(address, value)
                    }
                };
                (column, u32::from(bit))
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
        let mut qubits = places.into_iter().map(|place| self.promote_place(place));
        let target = qubits.next().expect("gate target");
        let controls = qubits.collect::<Vec<_>>();
        self.quantum.gate(gate, target, &controls);
        Ok(())
    }

    pub(super) fn gate_with_control_list(
        &mut self,
        gate: Gate,
        target: Place,
        address: Operand,
        count: Operand,
    ) -> Result<(), Fault> {
        let target = self.resolve_place(target)?;
        let raw_controls = self.read_control_list(address, count)?;
        if let ResolvedPlace::Register(column, bit) = target
            && raw_controls.contains(&(column, bit))
        {
            return Err(Fault::QubitAlias);
        }
        let controls: Vec<_> = raw_controls.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
        let target_qubit = self.promote_place(target);
        self.quantum.gate(gate, target_qubit, &controls);
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
                .fold(ordinary, |word, &(_, offset, column)| word | (basis[column].integer() << (8 * offset)));
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
    Register(usize, u32),
    Memory(u64, u8),
}

pub(super) fn little_endian(bytes: &[u8]) -> u128 {
    let mut word = [0; 16];
    word[..bytes.len()].copy_from_slice(bytes);
    u128::from_le_bytes(word)
}
