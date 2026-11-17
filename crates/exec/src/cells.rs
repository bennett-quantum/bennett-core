//! Qubits in memory (`docs/DESIGN.md`, "Qubits in memory"): gates that take addresses.
//!
//! The memory of a basis state is the world's memory, except at the bytes that hold qubits, the
//! **cells**, whose values are the configuration's. A cell is an 8-qubit register column bound to a byte
//! address (`QuantumState::cells`, part of the register table); the classical memory under a cell holds 0.
//! The first gate at a byte that is not a cell **promotes** it: a fresh column is allocated, the byte's
//! value is XORed into it, and the memory byte becomes 0. The gate's record journals the promotion
//! (address and old byte), so un-running the gate un-promotes: the inverse gate, then the byte XORed out,
//! the column un-allocated, the byte written back. That is the un-run of an allocation, with the same check
//! as for a register allocated since a mark: the column must hold the promoted byte in every configuration,
//! else the un-run would discard configurations (`Mismatch`, so a rewind faults `bad_rewind`); below a
//! world's split point the check is off and the un-allocation keeps the part where it holds (its adjoint).
//!
//! Ordinary code that touches a cell (a load, a store, a jump record) reads it as an integer, as a call
//! reads a register: where the configurations disagree on the cells the world splits first, one world per
//! value. Then a load gives the cells' values, and a store XORs the old value out of each cell's column and
//! the new one in (the cell stays a cell). The journal notes each cell touched with the byte it holds after
//! the step; un-running the step checks it is still there, as for a register read. A classical base byte
//! is never quantum: promotion writes 0 into this world's memory, and a cell is a column.

use std::ops::Range;

use bennett_ir::{Fault, Gate, Inst, Place, Width};

use crate::executor::{Executor, Halt};
use crate::gates;
use crate::history::Mismatch;
use crate::journal::{Journal, Undo};
use crate::leftover::Leftover;
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
        // Running again a promotion whose un-run left the byte's register over takes the register back.
        let record = self.next_record();
        let handle = match self
            .take_back_at(record, |kind| matches!(kind, Leftover::Promotion { address: a, .. } if a == address))
        {
            Some(take) => self.quantum.reclaim_allocation(take.id),
            None => self.quantum.allocate(Width::BYTE),
        };
        let column = self.quantum.resolve(handle).expect("a fresh register is live");
        self.quantum.xor_register(column, u64::from(old[0]));
        self.memory.write(address as usize, &[0]);
        self.quantum.bind_cell(address, Some(column));
        old[0]
    }

    /// Undoes `promote` of the cell at `address`, whose byte was `old`. The column must hold `old` in
    /// every configuration for this to invert the promotion; elsewhere only that part is kept.
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

    /// Un-runs the `GateAt` or `GateAtPromoted` record on top of the journal, unless a byte it promoted no
    /// longer holds its old value in every configuration (`checked`): then it changes nothing. Below a
    /// world's split point (`deferred`) that check is made on the sum of the worlds' parts instead: this
    /// part notes its failing configurations (`Executor::defer`) and keeps the others (the adjoint).
    pub(crate) fn unrun_gate_at(&mut self, checked: bool, deferred: bool, promoting: bool) -> Result<(), Mismatch> {
        let Inst::GateAt { gate, target, controls } = self.retreat() else { unreachable!("a GateAt record") };
        let places = std::iter::once(target).chain(controls);
        let resolved: Vec<Resolved> =
            places.map(|place| self.resolve_place(place).expect("a gate that ran resolves again")).collect();
        let qubits: Vec<QubitRef> = resolved.iter().map(|&place| self.qubit_ref(place)).collect();
        // Undoing returns to a state the run held, so the configuration limit is not applied.
        let inverse = gates::matrix(gates::inverse(*gate));
        self.quantum.apply_gate(&inverse, qubits[0], &qubits[1..], None).expect("no limit");
        let journal = self.journal.as_ref().expect("undoing needs a journal");
        let count = if promoting { journal.word_from_top(0) as usize } else { 0 };
        let promoted: Vec<(u64, u8)> = (1..=count)
            .map(|depth| journal.word_from_top(depth))
            .map(|word| ((word >> 8) as u64, word as u8))
            .collect();
        let record = journal.records() - 1;
        let mut regardless = false;
        if (checked || deferred) && !promoted.is_empty() {
            let expected: Vec<(usize, u64)> = promoted
                .iter()
                .map(|&(byte, old)| (self.quantum.cell(byte).expect("a promoted byte is a cell"), u64::from(old)))
                .collect();
            let failing = self.quantum.failing(|config| expected.iter().any(|&(column, old)| config[column] != old));
            // Outside strict mode a rewind's own un-run runs regardless: a byte whose qubits do not hold its old
            // value leaves its register over (`leftover.rs`).
            regardless = checked && !failing.is_empty() && self.runs_regardless();
            if checked && !failing.is_empty() && !regardless {
                let forward = gates::matrix(*gate);
                self.quantum.apply_gate(&forward, qubits[0], &qubits[1..], None).expect("no limit");
                self.advance();
                return Err(Mismatch);
            }
            if deferred {
                self.defer(Undo::GateAtPromoted, failing);
            }
        }
        let journal = self.journal.as_mut().expect("undoing needs a journal");
        journal.pop();
        if promoting {
            journal.pop_word();
        }
        for _ in 0..count {
            journal.pop_word();
        }
        // `promoted` lists the latest promotion first.
        for &(byte, old) in &promoted {
            let column = self.quantum.cell(byte).expect("a promoted byte is a cell");
            if regardless && !self.quantum.failing(|config| config[column] != u64::from(old)).is_empty() {
                self.leave_promotion_over(record, byte, old);
            } else {
                self.unpromote(byte, old);
            }
        }
        Ok(())
    }

    /// Reads the cells among the bytes of `range` before an ordinary access: splits the world by their
    /// values if its configurations disagree, every new world running `step` itself. Returns the cells
    /// (address, column) and whether this world split from others here. A coherent region's branch
    /// enumerated on private memory sees the cells' values there instead, so it touches no cell.
    pub(crate) fn read_cells(
        &mut self,
        range: Range<usize>,
        step: &dyn Fn(&mut Self) -> Result<(), Fault>,
    ) -> (Vec<(u64, usize)>, bool) {
        if self.materialised || !self.quantum.has_cells() {
            return (Vec::new(), false);
        }
        let cells = self.quantum.cells_in(range.start as u64..range.end as u64);
        if cells.is_empty() {
            return (cells, false);
        }
        // Running again an access whose un-run left wires over: those cells are read from their wires.
        let taken = self.take_cells_back(&cells);
        let columns: Vec<usize> =
            cells.iter().map(|&(_, column)| column).filter(|column| !taken.contains(column)).collect();
        let split = self.split(&columns, step) || self.splitting;
        (cells, split)
    }

    /// The bytes at `range` as ordinary code sees them, after `read_cells`: memory, with each cell's
    /// value (the same in every configuration of this world) in place of its byte.
    pub(crate) fn read_view(&self, range: Range<usize>, cells: &[(u64, usize)], out: &mut [u8]) {
        self.memory.read_into(range.clone(), out);
        for &(address, column) in cells {
            let taken = self.cell_reads.iter().find(|&&(taken, _)| taken == column).map(|&(_, byte)| byte);
            out[address as usize - range.start] = taken.unwrap_or(self.quantum.first_value(column) as u8);
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

    /// Journals the cells a step touched, each with the byte at its offset in `bytes` (from `start`):
    /// what un-running the step must find there.
    pub(crate) fn note_cells(&mut self, cells: &[(u64, usize)], start: usize, bytes: &[u8], split: bool) {
        // The bytes read from wires taken back served this access only.
        self.cell_reads.clear();
        if let Some(journal) = &mut self.journal {
            for &(address, column) in cells {
                journal.push_cell(column, bytes[address as usize - start], split);
            }
        }
    }

    /// The configurations where a cell the last record's step touched no longer holds the byte noted.
    pub(crate) fn cells_no_longer_held(&self) -> crate::quantum::Amplitudes {
        let journal = self.journal.as_ref().expect("undoing needs a journal");
        let entries = journal.cells_of(journal.records() - 1);
        self.quantum.failing(|config| entries.iter().any(|entry| config[entry.column] != u64::from(entry.value)))
    }

    /// Drops the cell entries of the record just popped (now at index `records()`); below an access that
    /// split the world, a rewind's un-run is of this world's part of a shared state.
    pub(crate) fn pop_cell_entries(&mut self) {
        let journal = self.journal.as_mut().expect("undoing needs a journal");
        let record = journal.records();
        while let Some(entry) = journal.pop_cell_of(record) {
            if entry.split && self.rewinding > 0 {
                self.partial = true;
                self.pending |= self.deferring;
            }
        }
    }

    /// Writes each cell's value in `config` into memory, for a coherent region's branch on that
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

impl Journal {
    /// Whether the top record has cell entries.
    pub(crate) fn top_has_cells(&self) -> bool {
        self.records() > 0 && !self.cells_of(self.records() - 1).is_empty()
    }
}
