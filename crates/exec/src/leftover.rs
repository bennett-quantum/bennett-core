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
//! - **An access** to memory qubits that no longer hold the byte noted: likewise, a byte wire.
//! - **An un-promotion** of a memory byte whose qubits do not hold the byte's old value: the old value is XORed
//!   out as always and the register that held the byte is left over; the byte goes back to memory.
//!
//! **Where worlds join** (`Executor::joint`: below a split point, where other worlds exist, or where worlds split
//! off during the un-run) every check site leaves its register or wire over, faithful or not, so that every
//! world's register table is the same and the worlds that become one machine state at the rewind merge: their
//! sum is the circuit's. Once they have merged (or, alone, at once) `resolve_leftovers` discards the wires that
//! are zero in every configuration, which no program can tell (its comment says why). Deciding in each world
//! alone, or each part of a shared state, by its own configurations kept a world whose wire was zero apart from
//! one whose wire was not, and the deferred check on the sum of the parts faulted every world (until
//! 2026-11-17).
//!
//! No check of the whole state decides what happens next: the leftovers are journalled with the rewind (its
//! `LeftBehind` record), and undoing the rewind (reversal, or an outer rewind) runs its region again taking
//! each one back where its step runs again: the `qalloc` gets the left-over register, the read gets its value
//! from the wire (the wire XOR the register) and the wire is un-allocated, the promotion gets its register
//! back. So undoing the rewind restores the state before it exactly, whatever the configurations hold. What is
//! left over, and what is to be taken back, is each world's own: a world that splits off during an un-run, or
//! while steps run again, carries them and takes back its own copy of each wire.

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

    /// Whether a check of this un-run may be summed with other worlds' where they join: this world is one part
    /// of a state other worlds share (below a split point), other worlds exist, or worlds split off during the
    /// un-run. There every check site leaves its wire over, faithful or not, so that the worlds' register tables
    /// agree whatever their configurations hold; which wires are zero is decided once they have joined
    /// (`resolve_leftovers`). Deciding in each world alone kept a world whose wire was zero apart from one whose
    /// wire was not, so that they never merged (until 2026-11-17).
    pub(crate) fn joint(&self) -> bool {
        self.partial || self.has_worlds() || !self.forked.is_empty()
    }

    /// Un-runs, regardless, the reads of the `Call` record on top of the journal: each read whose register no
    /// longer holds the value read, and in a joint un-run (`joint`) or where the read split the world every
    /// read, leaves its wire over, holding the value read XOR the register, per configuration. The top frame is
    /// the callee's.
    pub(crate) fn leave_reads_over(&mut self) {
        let joint = self.joint();
        let journal = self.journal.as_ref().expect("undoing needs a journal");
        let record = journal.records() - 1;
        let base = self.top().base;
        let reads: Vec<(usize, u64, bool)> = journal
            .reads_from(record)
            .iter()
            .map(|read| (read.column, self.slots[base + read.slot.index()] as u64, read.split))
            .collect();
        for (column, value, split) in reads {
            if !joint && !split && self.quantum.failing(|config| config[column] != value).is_empty() {
                continue;
            }
            let wire = self.quantum.allocate_wire(self.quantum.width_of(column));
            self.quantum.xor_column(wire, column, value);
            self.quantum.leave(wire);
            self.leftovers_made.push(TakeBack { record, id: wire, kind: Leftover::Read { column } });
        }
    }

    /// Un-runs, regardless, the access of the record on top of the journal to memory qubits: each cell that no
    /// longer holds the byte noted, and in a joint un-run or where the access split the world every cell,
    /// leaves the access's wire over, holding the byte noted XOR the cell, per configuration (an access copied
    /// the cell's byte into the classical step; un-running XORs it back).
    pub(crate) fn leave_cells_over(&mut self) {
        let joint = self.joint();
        let journal = self.journal.as_ref().expect("undoing needs a journal");
        let record = journal.records() - 1;
        let entries: Vec<(usize, u64, bool)> =
            journal.cells_of(record).iter().map(|entry| (entry.column, u64::from(entry.value), entry.split)).collect();
        for (column, value, split) in entries {
            if !joint && !split && self.quantum.failing(|config| config[column] != value).is_empty() {
                continue;
            }
            let wire = self.quantum.allocate_wire(self.quantum.width_of(column));
            self.quantum.xor_column(wire, column, value);
            self.quantum.leave(wire);
            self.leftovers_made.push(TakeBack { record, id: wire, kind: Leftover::Cell { column } });
        }
    }

    /// The cells among `cells` whose wires an access running again takes back (`take_cells_back`).
    pub(crate) fn wired_cells(&self, cells: &[(u64, usize)]) -> Vec<usize> {
        if self.taking_back.is_empty() {
            return Vec::new();
        }
        let record = self.next_record();
        let wired = |column| {
            self.taking_back.iter().any(|take| take.record == record && take.kind == Leftover::Cell { column })
        };
        cells.iter().map(|&(_, column)| column).filter(|&column| wired(column)).collect()
    }

    /// Where an access to memory qubits runs again whose un-run left wires over: the wire of each cell of
    /// `columns` is taken back, giving the byte the access saw (the wire XOR the cell), which the access then
    /// reads in place of the cell's configurations (`read_view`). Called after the world split by its other
    /// cells, so that every world split off takes back its own copy of the wire.
    pub(crate) fn take_cells_back(&mut self, columns: &[usize]) {
        let record = self.next_record();
        for &column in columns {
            let take = self.take_back_at(record, |kind| kind == Leftover::Cell { column });
            let byte = self.take_read_back(take.expect("a wire was found for this cell").id, column) as u8;
            self.cell_reads.push((column, byte));
        }
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

    /// Un-promotes, regardless, the byte at `address` whose qubits need not hold its old value `old`: the old
    /// value is XORed out, the register is left over and the byte is `old` in memory again.
    pub(crate) fn leave_promotion_over(&mut self, record: usize, address: u64, old: u8) {
        let column = self.quantum.cell(address).expect("an un-promoted byte is a cell");
        self.quantum.xor_register(column, u64::from(old));
        self.quantum.leave_allocation(column);
        self.quantum.bind_cell(address, None);
        self.memory.write(address as usize, &[old]);
        self.leftovers_made.push(TakeBack { record, id: column, kind: Leftover::Promotion { address, old } });
    }

    /// Un-runs, regardless, the `qalloc` of the last record: the register is left over, its handle slot back
    /// to its old value.
    pub(crate) fn leave_allocation_over(&mut self) {
        self.journal.as_mut().and_then(Journal::pop).expect("a qalloc record");
        self.pop_cell_entries();
        let record = self.next_record();
        let inst = self.retreat();
        let dst = inst.result().expect("qalloc defines its handle");
        let id = self.quantum.resolve(self.handle(dst)).expect("an allocation being undone is live");
        self.quantum.leave_allocation(id);
        self.restore_result();
        self.leftovers_made.push(TakeBack { record, id, kind: Leftover::Allocation });
    }

    /// Where a `qalloc` (or a promotion) runs again whose un-run left its register over at `wire`: the
    /// register is taken back. In place, where the allocation would take that id (as it did when it first
    /// ran); else (the leftover was moved, `make_room`) a register is allocated as the step allocates and the
    /// wire's values move into it. Returns the handle.
    pub(crate) fn take_allocation_back(&mut self, wire: usize, width: bennett_ir::Width) -> u64 {
        if self.quantum.first_free() > wire {
            return self.quantum.reclaim_allocation(wire);
        }
        let handle = self.quantum.allocate(width);
        let column = self.quantum.resolve(handle).expect("a fresh register is live");
        self.quantum.xor_column(column, wire, 0);
        self.quantum.xor_column(wire, column, 0);
        self.quantum.reclaim(wire);
        self.quantum.discard_wire(wire);
        handle
    }

    /// Before a step of a rewind's un-run revives the freed register at `id`: a register this un-run left over
    /// may hold that id (a wire, or an allocation, takes the first free id, and the id was freed since the
    /// mark); it moves to the first free id, and its record with it. Until 2026-11-17 the revival panicked
    /// ("an allocation being undone is live").
    pub(crate) fn make_room(&mut self, id: usize) {
        if !self.quantum.occupied(id) {
            return;
        }
        let to = self.quantum.move_leftover(id);
        let take = self.leftovers_made.iter_mut().rev().find(|take| take.id == id);
        take.expect("a register in a freed register's place was left over by this un-run").id = to;
    }

    /// Settles the leftovers of the rewind on top of the journal, once no other world can join this one there:
    /// a wire zero in every configuration is discarded, and each other one moves down into the lowest id that a
    /// discarded or moved wire of this rewind freed below it, as if the zero wires had never been made; the
    /// `LeftBehind` record is rewritten, or dropped if nothing is left over.
    ///
    /// Discarding a wire that is zero in every configuration changes no amplitude (the state is the rest times
    /// |0>); no world that could still become equal to this one holds it (every world the rewind joins has
    /// joined: they merged first); and undoing the rewind gives the same state with or without it (where the
    /// step runs again, the wire XOR the register is the register). So it is no decision a program can see; it
    /// keeps a faithful un-run outside strict mode exactly as in strict mode.
    pub(crate) fn resolve_leftovers(&mut self) {
        self.unresolved = false;
        if self.journal.as_ref().and_then(Journal::last) != Some(Undo::LeftBehind) {
            return;
        }
        let made = self.pop_leftovers();
        let mut kept = Vec::with_capacity(made.len());
        let mut freed = Vec::new();
        for take in made.into_iter().rev() {
            if self.quantum.is_zero(take.id) {
                self.quantum.reclaim(take.id);
                self.quantum.discard_wire(take.id);
                freed.push(take.id);
            } else {
                kept.push(take);
            }
        }
        kept.reverse();
        let mut order: Vec<usize> = (0..kept.len()).collect();
        order.sort_by_key(|&index| kept[index].id);
        for index in order {
            let id = kept[index].id;
            if let Some(low) = freed.iter().copied().filter(|&low| low < id).min() {
                self.quantum.move_leftover_to(id, low);
                kept[index].id = low;
                freed.retain(|&free| free != low);
                freed.push(id);
            }
        }
        if !kept.is_empty() {
            self.record_leftovers(&kept);
        }
    }
}
