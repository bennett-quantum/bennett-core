//! Leftovers: what a rewind's un-run keeps where it runs regardless, outside strict mode, and how undoing the
//! rewind takes them back (`docs/DESIGN.md`, "Strict mode").
//!
//! An un-run that meets a state its history does not describe runs regardless on the machine (the
//! maintainer's decision of the afternoon of 2026-11-16): every check of the un-run becomes the inverse step
//! of a circuit, and what the inverse leaves in a wire is kept as a register left over, never released.
//!
//! - **An allocation** whose register is not zero when its `qalloc` is un-run: the register is left over.
//! - **A read** of a register as an integer that no longer holds the value read: reading copied the register
//!   into the call's argument; un-running it XORs the register into that copy, which then holds the value
//!   read XOR the register's value now, per configuration. That wire is left over.
//! - **An un-promotion** of a memory byte whose qubits do not hold the byte's old value: the old value is XORed
//!   out as always and the register that held the byte is left over; the byte goes back to memory.
//!
//! No check of the whole state decides what happens next: the leftovers are journalled with the rewind (its
//! `LeftBehind` record), and undoing the rewind (reversal, or an outer rewind) runs its region again taking
//! each one back where its step runs again: the `qalloc` gets the left-over register, the read gets its value
//! from the wire (the wire XOR the register) and the wire is un-allocated, the promotion gets its register
//! back. So undoing the rewind restores the state before it exactly, whatever the configurations hold.

use crate::executor::Executor;
use crate::journal::{Journal, Undo};

/// One register a rewind's un-run left over, and the step whose un-run left it: what running the step again
/// takes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TakeBack {
    /// The index of the step's record in the journal.
    pub(crate) record: usize,
    /// The register left over.
    pub(crate) id: usize,
    pub(crate) kind: Leftover,
}

/// The step a leftover came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Leftover {
    /// A `qalloc`: the register itself.
    Allocation,
    /// A read of the register in `column` as an integer by a call: the wire holding the value read XOR it.
    Read { column: usize },
    /// An ordinary access (a load, a store, a `setjmp`'s record) to the memory qubits of the cell in `column`:
    /// the wire holding the byte noted (read, or written) XOR the cell.
    Cell { column: usize },
    /// A promotion of the memory byte at `address`, whose value was `old`: the register that held it.
    Promotion { address: u64, old: u8 },
}

impl Leftover {
    fn encode(self) -> (u128, u128) {
        match self {
            Leftover::Allocation => (0, 0),
            Leftover::Read { column } => (1, column as u128),
            Leftover::Promotion { address, old } => (2, u128::from(address) << 8 | u128::from(old)),
            Leftover::Cell { column } => (3, column as u128),
        }
    }

    fn decode(kind: u128, extra: u128) -> Self {
        match kind {
            0 => Leftover::Allocation,
            1 => Leftover::Read { column: extra as usize },
            2 => Leftover::Promotion { address: (extra >> 8) as u64, old: extra as u8 },
            _ => Leftover::Cell { column: extra as usize },
        }
    }
}

impl Executor<'_> {
    /// Journals the leftovers the rewind just finished made, after its `Rewind` record: their words (for each,
    /// in the order made: the record, the register, the kind and its datum), their number, then a `LeftBehind`
    /// record.
    pub(crate) fn record_leftovers(&mut self, made: &[TakeBack]) {
        for take in made {
            let (kind, extra) = take.kind.encode();
            self.record_word(take.record as u128);
            self.record_word(take.id as u128);
            self.record_word(kind);
            self.record_word(extra);
        }
        self.record_word(made.len() as u128);
        self.record(Undo::LeftBehind);
    }

    /// Pops the `LeftBehind` record on top of the journal and returns its leftovers, in the order made.
    pub(crate) fn pop_leftovers(&mut self) -> Vec<TakeBack> {
        let journal = self.journal.as_mut().expect("undoing needs a journal");
        assert_eq!(journal.pop(), Some(Undo::LeftBehind));
        let count = journal.pop_word() as usize;
        let mut made = Vec::with_capacity(count);
        for _ in 0..count {
            let extra = journal.pop_word();
            let kind = journal.pop_word();
            let id = journal.pop_word() as usize;
            let record = journal.pop_word() as usize;
            made.push(TakeBack { record, id, kind: Leftover::decode(kind, extra) });
        }
        made.reverse();
        made
    }

    /// The leftover to take back for the step whose record will be at `record`, of the kind `matches`
    /// accepts, if undoing a rewind left one there.
    pub(crate) fn take_back_at(&mut self, record: usize, matches: impl Fn(Leftover) -> bool) -> Option<TakeBack> {
        let index = self.taking_back.iter().position(|take| take.record == record && matches(take.kind))?;
        Some(self.taking_back.remove(index))
    }

    /// The index the next record of the journal will have.
    pub(crate) fn next_record(&self) -> usize {
        self.journal.as_ref().map_or(0, Journal::records)
    }

    /// Un-runs, regardless, the reads of the `Call` record on top of the journal whose registers no longer
    /// hold the value read: each leaves its wire over, holding the value read XOR the register, per
    /// configuration. The top frame is the callee's.
    pub(crate) fn leave_reads_over(&mut self) {
        let journal = self.journal.as_ref().expect("undoing needs a journal");
        let record = journal.records() - 1;
        let base = self.top().base;
        let reads: Vec<(usize, u64)> = journal
            .reads_from(record)
            .iter()
            .map(|read| (read.column, self.slots[base + read.slot.index()] as u64))
            .collect();
        for (column, value) in reads {
            if self.quantum.failing(|config| config[column] != value).is_empty() {
                continue;
            }
            let wire = self.quantum.allocate_wire(self.quantum.width_of(column));
            self.quantum.xor_column(wire, column, value);
            self.quantum.leave(wire);
            self.leftovers_made.push(TakeBack { record, id: wire, kind: Leftover::Read { column } });
        }
    }

    /// Un-runs, regardless, the access of the record on top of the journal to memory qubits that no longer hold
    /// the byte noted: each such cell leaves the access's wire over, holding the byte noted XOR the cell, per
    /// configuration (an access copied the cell's byte into the classical step; un-running XORs it back).
    pub(crate) fn leave_cells_over(&mut self) {
        let journal = self.journal.as_ref().expect("undoing needs a journal");
        let record = journal.records() - 1;
        let entries: Vec<(usize, u64)> =
            journal.cells_of(record).iter().map(|entry| (entry.column, u64::from(entry.value))).collect();
        for (column, value) in entries {
            if self.quantum.failing(|config| config[column] != value).is_empty() {
                continue;
            }
            let wire = self.quantum.allocate_wire(self.quantum.width_of(column));
            self.quantum.xor_column(wire, column, value);
            self.quantum.leave(wire);
            self.leftovers_made.push(TakeBack { record, id: wire, kind: Leftover::Cell { column } });
        }
    }

    /// Whether an access of the record on top split the world at its cells: its un-run is where worlds' parts
    /// meet, not built to run regardless (a debt, `HANDOFF.md`).
    pub(crate) fn cells_split(&self) -> bool {
        let journal = self.journal.as_ref().expect("undoing needs a journal");
        journal.cells_of(journal.records() - 1).iter().any(|entry| entry.split)
    }

    /// Where an access to memory qubits runs again whose un-run left wires over: each wire is taken back, giving
    /// the byte the access saw (the wire XOR the cell), which the access then reads in place of the cell's
    /// configurations (`read_view`). Returns the cells taken back.
    pub(crate) fn take_cells_back(&mut self, cells: &[(u64, usize)]) -> Vec<usize> {
        if self.taking_back.is_empty() {
            return Vec::new();
        }
        let record = self.next_record();
        let mut taken = Vec::new();
        for &(_, column) in cells {
            if let Some(take) = self.take_back_at(record, |kind| kind == Leftover::Cell { column }) {
                let byte = self.take_read_back(take.id, column) as u8;
                self.cell_reads.push((column, byte));
                taken.push(column);
            }
        }
        taken
    }

    /// Takes back the wire a read of `column` left over, where its call runs again: the wire XOR the register
    /// is the value read, the same in every configuration; the wire is XORed back to zero and un-allocated.
    /// Returns the value.
    pub(crate) fn take_read_back(&mut self, wire: usize, column: usize) -> u128 {
        self.quantum.xor_column(wire, column, 0);
        let value = self.quantum.first_value(wire);
        assert!(
            self.quantum.failing(|config| config[wire] != value).is_empty(),
            "the wire of a read taken back holds the value read in every configuration"
        );
        self.quantum.xor_register(wire, value);
        self.quantum.reclaim(wire);
        self.quantum.discard_wire(wire);
        u128::from(value)
    }

    /// Un-promotes, regardless, the byte at `address` whose qubits do not hold its old value `old`: the old
    /// value is XORed out, the register is left over and the byte is `old` in memory again.
    pub(crate) fn leave_promotion_over(&mut self, record: usize, address: u64, old: u8) {
        let column = self.quantum.cell(address).expect("an un-promoted byte is a cell");
        self.quantum.xor_register(column, u64::from(old));
        self.quantum.leave_allocation(column);
        self.quantum.bind_cell(address, None);
        self.memory.write(address as usize, &[old]);
        self.leftovers_made.push(TakeBack { record, id: column, kind: Leftover::Promotion { address, old } });
    }
}
