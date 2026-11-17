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
    /// A `qfree` of a register that was not zero: the simulator keeps its column (left over, its handle
    /// stale), undone by taking it back. No words.
    Leftover,
    /// A measurement: a barrier that reversal refuses to cross. Words: the old slot value.
    Measure,
    /// An `apply`, undone by applying it again. No words.
    Apply,
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
}

impl Journal {
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
        self.kinds.len() * size_of::<Undo>() + self.words.len() * size_of::<u128>()
    }

    pub(crate) fn clear(&mut self) {
        self.kinds.clear();
        self.words.clear();
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
