//! The undo journal: for each executed step, exactly what the step destroyed.
//!
//! A record is one byte naming the kind of step, plus the words that kind needs, on a separate word
//! stack. A word holds any value (up to 128 bits). Most steps need one word (the overwritten slot), so
//! the journal costs about seventeen bytes per step. Everything else needed to undo a step (operand values, addresses, which instruction ran) is
//! still in the state when the step is undone, because later steps have been undone first.

/// What kind of step a record undoes, and therefore which words it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Undo {
    /// An instruction that wrote its result slot. Words: the old slot value.
    Slot,
    /// A store. Words: the old bytes, little-endian.
    Store,
    /// An `alloca`. Words: the old slot value, the old stack pointer.
    Alloca,
    /// A `stackrestore`. Words: the old stack pointer.
    StackRestore,
    /// An `effect`, undone by dropping the last console byte. No words.
    Effect,
    /// A frame pushed by a call or by entering a region. Words: the old value of each parameter slot.
    Call,
    /// A branch. Words: the old value of each parameter slot of the target, then the source block.
    Jump,
    /// A return to a caller. Words: the callee's function and block, its stack pointer at the return, and
    /// the old value of the caller's result slot if the call names one.
    Return,
    /// The outermost return or whole-run exit that finished the run. No words.
    Finish,
    /// A gate, undone by its inverse. No words.
    Gate,
    /// A `qalloc`. Words: the old slot value.
    Qalloc,
    /// A `qfree`, undone by reviving the register. No words.
    Qfree,
    /// A measurement: a barrier that reversal refuses to cross. Words: the old slot value.
    Measure,
    /// An `apply`, undone by applying it again. No words.
    Apply,
    /// A `mark`. Words: the old slot value. The mark is also listed in the journal's marks.
    Mark,
    /// A `rewind`, undone by running the rewound steps again from the mark and XORing the kept values
    /// again. Words: the number of steps it un-ran (the high 64 bits), and the block and position the
    /// mark left the frame at, packed in one word.
    Rewind,
    /// A `setjmp`. Words: the record's old bytes (two words, low half first), then the old result slot.
    Setjmp,
    /// A gate with a qubit in memory (`GateAt`) whose memory bytes all held qubits already, undone by its
    /// inverse. No words.
    GateAt,
    /// A `GateAt` that promoted memory bytes to qubits, undone by its inverse and then by un-promoting
    /// them. Words: for each byte it promoted, in order, the address (shifted left by 8) and the byte's
    /// old value (the low 8 bits); then the number of bytes promoted.
    GateAtPromoted,
    /// A `longjmp`. Words: for each discarded frame from the top down, its packed position and its entry
    /// stack pointer; then the target frame's packed position and the stack pointer before the jump, the
    /// number of discarded frames, and the old value of the `setjmp`'s result slot.
    Longjmp,
}

/// A stack of undo records.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Journal {
    kinds: Vec<Undo>,
    words: Vec<u128>,
    /// One entry per `Mark` record, in order: what a `rewind` to it needs.
    marks: Vec<MarkEntry>,
    /// One entry per `Longjmp` record, in order: the number of frames it discarded, which a `rewind`
    /// counts as returns when it checks that its mark belongs to the running activation.
    jumps: Vec<usize>,
    /// One entry per register a `Call` record's call read as an integer, in order.
    reads: Vec<ReadEntry>,
    /// One entry per memory byte holding qubits that a step's ordinary access touched, in order.
    cells: Vec<CellEntry>,
}

/// A memory byte holding qubits (a cell) that ordinary code read or wrote: what un-running the step must
/// find there (`docs/DESIGN.md`, "Qubits in memory").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CellEntry {
    /// The index of the step's record.
    pub(crate) record: usize,
    /// The cell's column.
    pub(crate) column: usize,
    /// The byte the cell holds after the step: the value read, or the value written.
    pub(crate) value: u8,
    /// Whether the world split at this access: its configurations disagreed on the cells.
    pub(crate) split: bool,
}

/// A register a call read as an integer: what un-running the call must find still there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ReadEntry {
    /// The index of the call's `Call` record.
    pub(crate) record: usize,
    /// The register's column.
    pub(crate) column: usize,
    /// The callee's parameter slot that received the value.
    pub(crate) slot: bennett_ir::Slot,
    /// Whether the world split at this read: its configurations disagreed on the value.
    pub(crate) split: bool,
}

/// What the journal keeps about a `mark` besides its record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct MarkEntry {
    /// The number of records up to and including the mark's own: what a `rewind` pops back to.
    pub(crate) position: usize,
    /// How many registers had been allocated when the mark ran (`QuantumState::allocations`).
    pub(crate) allocations: u64,
}

impl Journal {
    /// Appends a `Mark` record's entry; the record itself is pushed as usual.
    pub(crate) fn push_mark(&mut self, entry: MarkEntry) {
        self.marks.push(entry);
    }

    /// Removes the last mark entry, when its record is undone.
    pub(crate) fn pop_mark(&mut self) {
        self.marks.pop().expect("a mark record has an entry");
    }

    /// Appends a `Longjmp` record's count of discarded frames; the record itself is pushed as usual.
    pub(crate) fn push_jump(&mut self, discarded: usize) {
        self.jumps.push(discarded);
    }

    /// Removes the last `Longjmp` entry, when its record is undone.
    pub(crate) fn pop_jump(&mut self) {
        self.jumps.pop().expect("a longjmp record has an entry");
    }

    /// Notes that the call whose `Call` record is pushed next read a register.
    pub(crate) fn push_read(&mut self, column: usize, slot: bennett_ir::Slot, split: bool) {
        self.reads.push(ReadEntry { record: self.kinds.len(), column, slot, split });
    }

    /// Notes that the step whose record is pushed next touched a cell.
    pub(crate) fn push_cell(&mut self, column: usize, value: u8, split: bool) {
        self.cells.push(CellEntry { record: self.kinds.len(), column, value, split });
    }

    /// Removes and returns the last cell entry if it belongs to the record at `record`.
    pub(crate) fn pop_cell_of(&mut self, record: usize) -> Option<CellEntry> {
        self.cells.pop_if(|entry| entry.record == record)
    }

    /// The cell entries of the record at `record`.
    pub(crate) fn cells_of(&self, record: usize) -> &[CellEntry] {
        let start = self.cells.partition_point(|entry| entry.record < record);
        let end = self.cells.partition_point(|entry| entry.record <= record);
        &self.cells[start..end]
    }

    /// The cell entries of records at `record` or later.
    pub(crate) fn cells_from(&self, record: usize) -> &[CellEntry] {
        let start = self.cells.partition_point(|entry| entry.record < record);
        &self.cells[start..]
    }

    /// Every cell entry, in order.
    #[cfg(test)]
    pub(crate) fn cell_entries(&self) -> &[CellEntry] {
        &self.cells
    }

    /// The word `depth` places below the top of the word stack (0 is the top).
    pub(crate) fn word_from_top(&self, depth: usize) -> u128 {
        self.words[self.words.len() - 1 - depth]
    }

    /// Removes and returns the last read if it belongs to the `Call` record at `record`.
    pub(crate) fn pop_read_of(&mut self, record: usize) -> Option<ReadEntry> {
        self.reads.pop_if(|entry| entry.record == record)
    }

    /// The reads made by calls whose records are at `record` or later.
    pub(crate) fn reads_from(&self, record: usize) -> &[ReadEntry] {
        let start = self.reads.partition_point(|entry| entry.record < record);
        &self.reads[start..]
    }

    /// The discarded-frame counts of the `Longjmp` records, in order.
    pub(crate) fn jumps(&self) -> &[usize] {
        &self.jumps
    }

    /// Every mark entry, in order.
    #[cfg(test)]
    pub(crate) fn marks(&self) -> &[MarkEntry] {
        &self.marks
    }

    /// The mark whose record is the last of the first `position` records, if there is one.
    pub(crate) fn mark_at(&self, position: usize) -> Option<MarkEntry> {
        let index = self.marks.binary_search_by_key(&position, |entry| entry.position).ok()?;
        Some(self.marks[index])
    }

    pub(crate) fn push(&mut self, undo: Undo) {
        self.kinds.push(undo);
    }

    pub(crate) fn push_word(&mut self, word: u128) {
        self.words.push(word);
    }

    pub(crate) fn last(&self) -> Option<Undo> {
        self.kinds.last().copied()
    }

    pub(crate) fn pop(&mut self) -> Option<Undo> {
        self.kinds.pop()
    }

    pub(crate) fn pop_word(&mut self) -> u128 {
        self.words.pop().expect("every undo record's words were pushed with it")
    }

    /// The bytes the journal holds.
    pub(crate) fn bytes(&self) -> usize {
        self.kinds.len() * size_of::<Undo>()
            + self.words.len() * size_of::<u128>()
            + self.marks.len() * size_of::<MarkEntry>()
            + self.jumps.len() * size_of::<usize>()
            + self.reads.len() * size_of::<ReadEntry>()
            + self.cells.len() * size_of::<CellEntry>()
    }

    pub(crate) fn clear(&mut self) {
        self.kinds.clear();
        self.words.clear();
        self.marks.clear();
        self.jumps.clear();
        self.reads.clear();
        self.cells.clear();
    }

    /// The number of records.
    pub(crate) fn records(&self) -> usize {
        self.kinds.len()
    }

    /// The kind of record `index`.
    pub(crate) fn kind_at(&self, index: usize) -> Undo {
        self.kinds[index]
    }

    /// All words, in push order, for the journal-content measurement.
    pub(crate) fn words(&self) -> &[u128] {
        &self.words
    }
}
