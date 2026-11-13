//! The undo journal: for each executed step, exactly what the step destroyed.
//!
//! A record is one byte naming the kind of step, plus the words that kind needs, on a separate word
//! stack. Most steps need one word (the overwritten slot), so the journal costs about nine bytes per
//! step. Everything else needed to undo a step (operand values, addresses, which instruction ran) is
//! still in the state when the step is undone, because later steps have been undone first.

/// What kind of step a record undoes, and therefore which words it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Undo {
    /// An instruction that wrote its result slot. Words: the old slot value.
    Slot,
    /// A store. Words: the old bytes, little-endian.
    Store,
    /// An `alloca`. Words: the old slot value, the old stack pointer.
    Alloca,
    /// An `effect`, undone by dropping the last console byte. No words.
    Effect,
    /// A frame pushed by a call or by entering a region. Words: the old value of each parameter slot.
    Call,
    /// A branch. Words: the old value of each parameter slot of the target, then the source block.
    Jump,
    /// A return to a caller. Words: the callee's function and block, its stack pointer at the return, and
    /// the old value of the caller's result slot if the call names one.
    Return,
    /// The return from the outermost frame that finished the run. No words.
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
}

/// A stack of undo records.
#[derive(Clone, Debug, Default)]
pub(crate) struct Journal {
    kinds: Vec<Undo>,
    words: Vec<u64>,
}

impl Journal {
    pub(crate) fn push(&mut self, undo: Undo) {
        self.kinds.push(undo);
    }

    pub(crate) fn push_word(&mut self, word: u64) {
        self.words.push(word);
    }

    pub(crate) fn last(&self) -> Option<Undo> {
        self.kinds.last().copied()
    }

    pub(crate) fn pop(&mut self) -> Option<Undo> {
        self.kinds.pop()
    }

    pub(crate) fn pop_word(&mut self) -> u64 {
        self.words.pop().expect("every undo record's words were pushed with it")
    }

    /// The bytes the journal holds.
    pub(crate) fn bytes(&self) -> usize {
        self.kinds.len() * size_of::<Undo>() + self.words.len() * size_of::<u64>()
    }

    pub(crate) fn clear(&mut self) {
        self.kinds.clear();
        self.words.clear();
    }
}
