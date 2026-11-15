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
    /// again. Words: the block and position the mark left the frame at, packed.
    Rewind,
}

/// A stack of undo records.
#[derive(Clone, Debug, Default)]
pub(crate) struct Journal {
    kinds: Vec<Undo>,
    words: Vec<u128>,
    /// One entry per `Mark` record, in order: what a `rewind` to it needs.
    marks: Vec<MarkEntry>,
}

/// What the journal keeps about a `mark` besides its record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    }

    pub(crate) fn clear(&mut self) {
        self.kinds.clear();
        self.words.clear();
        self.marks.clear();
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
