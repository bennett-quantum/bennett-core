//! Identities of immutable bit values, independent of their gate operands.

use std::sync::atomic::{AtomicU64, Ordering};

use bennett_exec::Wire;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Source {
    wire: Wire,
    generation: Generation,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Generation(u64);

impl Generation {
    // Separate allocations at the same address after a reversal or in distinct
    // speculative circuits. Generation zero is reserved for stable names.
    pub(super) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Source {
    pub(super) fn argument_index(self) -> Option<usize> {
        (self.generation == Generation(0)).then_some(self.wire as usize)
    }

    pub(super) fn precedes(self, scratch: Wire) -> bool {
        self.wire < scratch
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Identity {
    Literal(bool),
    Source(Source, bool),
}

impl Identity {
    pub(super) fn input(wire: Wire, inverted: bool) -> Self {
        Self::Source(Source { wire, generation: Generation(0) }, inverted)
    }

    pub(super) fn argument_bit(self) -> Bit {
        match self {
            Self::Literal(value) => Bit::Constant(value),
            Self::Source(source, inverted) => {
                debug_assert!(source.argument_index().is_some());
                Bit::input(source.wire).with_flip(inverted)
            }
        }
    }

    pub(super) fn not(self) -> Self {
        match self {
            Self::Literal(value) => Self::Literal(!value),
            Self::Source(source, inverted) => Self::Source(source, !inverted),
        }
    }
}

// Callers choose between value equality, stored-version equality, and
// physical gate addresses. A bit has no default equality.
#[derive(Clone, Copy, Debug)]
pub(super) enum Bit {
    Constant(bool),
    Wire(Qubit),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Qubit {
    allocation: Source,
    // The identity describes the physical qubit's positive polarity.
    positive: Identity,
    inverted: bool,
    version: u32,
}

impl Bit {
    // Entry wires and the stable names of the static analyses never get reused.
    // The modular multiplier also uses this when naming a physical gate operand.
    pub(super) fn input(wire: Wire) -> Self {
        Self::produced(wire, Generation(0))
    }

    pub(super) fn produced(wire: Wire, generation: Generation) -> Self {
        let allocation = Source { wire, generation };
        Self::Wire(Qubit { allocation, positive: Identity::Source(allocation, false), inverted: false, version: 0 })
    }

    pub(super) fn copied(self, wire: Wire, generation: Generation) -> Self {
        let allocation = Source { wire, generation };
        Self::Wire(Qubit { allocation, positive: self.identity(), inverted: false, version: 0 })
    }

    pub(super) fn not(self) -> Self {
        match self {
            Self::Constant(value) => Self::Constant(!value),
            Self::Wire(qubit) => Self::Wire(Qubit { inverted: !qubit.inverted, ..qubit }),
        }
    }

    pub(super) fn with_flip(self, inverted: bool) -> Self {
        if inverted { self.not() } else { self }
    }

    pub(super) fn wire(self) -> Option<(Wire, bool)> {
        match self {
            Self::Constant(_) => None,
            Self::Wire(qubit) => Some((qubit.allocation.wire, qubit.inverted)),
        }
    }

    pub(super) fn identity(self) -> Identity {
        match self {
            Self::Constant(value) => Identity::Literal(value),
            Self::Wire(Qubit { positive, inverted: false, .. }) => positive,
            Self::Wire(Qubit { positive, inverted: true, .. }) => positive.not(),
        }
    }

    pub(super) fn literal(self) -> Option<bool> {
        match self.identity() {
            Identity::Literal(value) => Some(value),
            Identity::Source(..) => None,
        }
    }

    pub(super) fn is_zero(self) -> bool {
        self.literal() == Some(false)
    }

    pub(super) fn is_one(self) -> bool {
        self.literal() == Some(true)
    }

    pub(super) fn same_value(self, other: Self) -> bool {
        self.identity() == other.identity()
    }

    pub(super) fn stored(self, version: u32) -> Self {
        match self {
            Self::Constant(_) => self,
            Self::Wire(qubit) => Self::Wire(Qubit { version, ..qubit }),
        }
    }

    pub(super) fn same_version(self, other: Self) -> bool {
        match (self, other) {
            (Self::Constant(left), Self::Constant(right)) => left == right,
            (Self::Wire(left), Self::Wire(right)) => {
                left.allocation == right.allocation && left.inverted == right.inverted && left.version == right.version
            }
            (Self::Constant(_), Self::Wire(_)) | (Self::Wire(_), Self::Constant(_)) => false,
        }
    }
}

pub(super) fn same_word(left: &[Bit], right: &[Bit]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(&a, &b)| a.same_value(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::bits::{Circuit, ONE, definite};

    #[test]
    fn two_complemented_copies_keep_the_original_polarity() {
        let mut circuit = Circuit::new(1);
        let source = Bit::input(0);
        let first = circuit.retain_copy(source.not());
        let second = circuit.retain_copy(first.not());

        assert_ne!(first.wire(), second.wire());
        assert!(source.not().same_value(first));
        assert!(source.same_value(second));
        let gates = circuit.forward_len();
        assert!(circuit.xor(source, second).is_zero());
        assert_eq!(circuit.forward_len(), gates);
    }

    #[test]
    fn reusing_reversed_scratch_gives_the_wire_a_new_identity() {
        let mut circuit = Circuit::new(2);
        let first = Bit::input(0);
        let second = Bit::input(1);
        let checkpoint = circuit.checkpoint();
        let source = circuit.and(first, second);
        let copy = circuit.retain_copy(source);
        circuit.rewind(checkpoint);
        let new = circuit.xor(first, second);

        assert_eq!(source.wire(), new.wire());
        assert!(copy.same_value(source));
        assert!(!copy.same_value(new));
    }

    #[test]
    fn a_result_copied_out_of_local_reversal_keeps_a_live_source() {
        let mut circuit = Circuit::new(2);
        let first = Bit::input(0);
        let second = Bit::input(1);
        let result = circuit.clean_word(2, |circuit| {
            let local = circuit.and(first, second);
            let retained = circuit.retain_copy(first.not());
            vec![local, retained]
        });
        let new = circuit.xor(first, second);

        assert_eq!(result[1].wire(), Some((3, false)));
        assert_eq!(new.wire(), Some((4, false)));
        assert!(first.not().same_value(result[1]));
        assert!(!new.same_value(result[0]));
    }

    #[test]
    fn a_wire_that_copies_a_literal_has_the_literal_identity() {
        let mut circuit = Circuit::new(0);
        let copied = circuit.retain_copy(ONE);

        assert_eq!(copied.wire(), Some((0, false)));
        assert!(copied.same_value(ONE));
        assert_eq!(definite(&[copied, copied.not()]), Some(1));
    }
}
