//! Qubits in memory (`docs/DESIGN.md`, "Qubits in memory"): gates that take addresses.
//!
//! The memory of a basis state is the classical memory, except at the bytes that hold qubits, the
//! **cells**, whose values are the configuration's. A cell is an 8-qubit register column bound to a byte
//! address (`QuantumState::cells`, part of the register table); the classical memory under a cell holds 0.
//! The first gate at a byte that is not a cell **promotes** it: a fresh column is allocated, the byte's
//! value is XORed into it, and the memory byte becomes 0. The gate's record journals the promotion
//! (address and old byte), so undoing the gate un-promotes: the inverse gate, then the byte XORed out,
//! the column un-allocated, the byte written back.
//!
//! Ordinary code that touches a cell (a load, a store, a jump record) outside `apply` reads its value when
//! every configuration agrees on it; where they disagree the simulator stops
//! (`RunError::SuperposedMemoryRead`), what such a read is to be being undecided. A store XORs the old value
//! out of each cell's column and the new one in (the cell stays a cell). Inside `apply` each configuration's
//! run sees the cells' values in its private memory instead.

use std::ops::Range;

use bennett_ir::{Fault, Gate, Inst, Place, Width};

use crate::executor::{Executor, Halt, RunError};
use crate::gates;
use crate::journal::Undo;
use crate::quantum::QubitRef;

/// A qubit of a gate, resolved: a register qubit, or a bit of a memory byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resolved {
    Register(QubitRef),
    Memory { byte: u64, bit: u8 },
}

impl<'m> Executor<'m> {
    /// Resolves a place. A memory place outside memory is `out_of_bounds`.
    fn resolve_place(&self, place: &Place) -> Result<Resolved, Fault> {
        match *place {
            Place::Register(qubit) => Ok(Resolved::Register(QubitRef {
                column: self.quantum.resolve(self.handle(qubit.register))?,
                bit: qubit.bit,
            })),
            Place::Memory { address, bit } => {
                let byte = self.read_word(&address).checked_add(u64::from(bit / 8));
                let byte = byte.filter(|&byte| byte < self.memory.len() as u64).ok_or(Fault::OutOfBounds)?;
                Ok(Resolved::Memory { byte, bit: bit % 8 })
            }
        }
    }

    /// The qubit a resolved place names; a memory byte must be a cell.
    fn qubit_ref(&self, resolved: Resolved) -> QubitRef {
        match resolved {
            Resolved::Register(qubit) => qubit,
            Resolved::Memory { byte, bit } => {
                QubitRef { column: self.quantum.cell(byte).expect("a gate's memory byte is a cell"), bit }
            }
        }
    }

    /// Makes the memory byte at `address` a cell: a fresh column holding its value, the byte 0. Returns
    /// the byte's old value.
    fn promote(&mut self, address: u64) -> u8 {
        let mut old = [0];
        self.memory.read_into(address as usize..address as usize + 1, &mut old);
        let handle = self.quantum.allocate(Width::BYTE);
        let column = self.quantum.resolve(handle).expect("a fresh register is live");
        self.quantum.xor_register(column, u64::from(old[0]));
        self.memory.write(address as usize, &[0]);
        self.quantum.bind_cell(address, Some(column));
        old[0]
    }

    /// Undoes `promote` of the cell at `address`, whose byte was `old`: the column holds `old` in every
    /// configuration again once every later step is undone.
    fn unpromote(&mut self, address: u64, old: u8) {
        let column = self.quantum.cell(address).expect("an un-promoted byte is a cell");
        self.quantum.xor_register(column, u64::from(old));
        self.quantum.unallocate_column(column);
        self.quantum.bind_cell(address, None);
        self.memory.write(address as usize, &[old]);
    }

    /// `gate target ctrl controls` with qubits in memory, within `limit` basis configurations. Faults
    /// before changing anything; promotes the memory bytes that are not cells yet, journalling each, then
    /// applies the gate.
    pub(crate) fn gate_at(
        &mut self,
        gate: Gate,
        target: &Place,
        controls: &[Place],
        limit: Option<usize>,
    ) -> Result<(), Halt> {
        let places = std::iter::once(target).chain(controls);
        let resolved = places.map(|place| self.resolve_place(place)).collect::<Result<Vec<_>, _>>()?;
        if resolved.iter().enumerate().any(|(k, place)| resolved[..k].contains(place)) {
            return Err(Fault::QubitAlias.into());
        }
        let mut promoted: Vec<(u64, u8)> = Vec::new();
        for place in &resolved {
            if let Resolved::Memory { byte, .. } = *place
                && self.quantum.cell(byte).is_none()
            {
                promoted.push((byte, self.promote(byte)));
            }
        }
        let target = self.qubit_ref(resolved[0]);
        let controls: Vec<QubitRef> = resolved[1..].iter().map(|&place| self.qubit_ref(place)).collect();
        if let Err(error) = self.quantum.apply_gate(&gates::matrix(gate), target, &controls, limit) {
            for &(byte, old) in promoted.iter().rev() {
                self.unpromote(byte, old);
            }
            return Err(error.into());
        }
        if promoted.is_empty() {
            self.record(Undo::GateAt);
        } else {
            for &(byte, old) in &promoted {
                self.record_word(u128::from(byte) << 8 | u128::from(old));
            }
            self.record_word(promoted.len() as u128);
            self.record(Undo::GateAtPromoted);
        }
        self.advance();
        Ok(())
    }

    /// Undoes the `GateAt` or `GateAtPromoted` record on top of the journal: the inverse gate, then the
    /// promotions undone, the latest first.
    pub(crate) fn undo_gate_at(&mut self, promoting: bool) {
        let Inst::GateAt { gate, target, controls } = self.retreat() else { unreachable!("a GateAt record") };
        let places = std::iter::once(target).chain(controls);
        let resolved: Vec<Resolved> =
            places.map(|place| self.resolve_place(place).expect("a gate that ran resolves again")).collect();
        let qubits: Vec<QubitRef> = resolved.iter().map(|&place| self.qubit_ref(place)).collect();
        // Undoing returns to a state the run held, so the configuration limit is not applied.
        let inverse = gates::matrix(gates::inverse(*gate));
        self.quantum.apply_gate(&inverse, qubits[0], &qubits[1..], None).expect("no limit");
        if !promoting {
            return;
        }
        let journal = self.journal.as_mut().expect("undoing needs a journal");
        let count = journal.pop_word() as usize;
        // The words list the latest promotion last, so they pop latest first.
        let promoted: Vec<u128> = (0..count).map(|_| journal.pop_word()).collect();
        for word in promoted {
            self.unpromote((word >> 8) as u64, word as u8);
        }
    }

    /// The cells among the bytes of `range` that an ordinary access outside `apply` touches, each of whose
    /// configurations must agree on its value. Inside `apply` each configuration's run holds the cells'
    /// values in its private memory, so it touches no cell.
    pub(crate) fn read_cells(&self, range: Range<usize>) -> Result<Vec<(u64, usize)>, RunError> {
        if self.materialised || !self.quantum.has_cells() {
            return Ok(Vec::new());
        }
        let cells = self.quantum.cells_in(range.start as u64..range.end as u64);
        if cells.iter().any(|&(_, column)| self.quantum.partition(&[column]).len() > 1) {
            return Err(RunError::SuperposedMemoryRead);
        }
        Ok(cells)
    }

    /// The bytes at `range` as ordinary code sees them, after `read_cells`: memory, with each cell's
    /// value (the same in every configuration) in place of its byte.
    pub(crate) fn read_view(&self, range: Range<usize>, cells: &[(u64, usize)], out: &mut [u8]) {
        self.memory.read_into(range.clone(), out);
        for &(address, column) in cells {
            out[address as usize - range.start] = self.quantum.first_value(column) as u8;
        }
    }

    /// Writes `new` over `old` at `start` as ordinary code sees memory: plain bytes are written, and each
    /// cell's column has `old ^ new` XORed in (its memory byte stays 0). A permutation of configurations
    /// whatever the cells hold, so un-running uses it with the two swapped.
    pub(crate) fn write_view(&mut self, start: usize, new: &[u8], old: &[u8], cells: &[(u64, usize)]) {
        let mut bytes = new.to_vec();
        for &(address, column) in cells {
            let offset = address as usize - start;
            self.quantum.xor_register(column, u64::from(old[offset] ^ new[offset]));
            bytes[offset] = 0;
        }
        self.memory.write(start, &bytes);
    }

    /// Writes each cell's value in `config` into memory, for a coherent region's run for that
    /// configuration, or 0 back (`config` `None`) after it.
    pub(crate) fn materialise_cells(&mut self, cells: &[(u64, usize)], config: Option<&[u64]>) {
        for &(address, column) in cells {
            let byte = config.map_or(0, |config| config[column] as u8);
            self.memory.write(address as usize, &[byte]);
        }
    }

    /// The cells for a region to materialise, outside a region.
    pub(crate) fn region_cells(&self) -> Vec<(u64, usize)> {
        if self.in_region { Vec::new() } else { self.quantum.cells().iter().map(|(&a, &c)| (a, c)).collect() }
    }
}
