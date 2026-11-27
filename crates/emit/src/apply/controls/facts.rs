//! Definite bit values for the inputs enabled on one control route.

use std::collections::BTreeMap;

use super::super::bits::{Bit, Circuit, Word};
use super::super::identity::{Identity, Source};

#[derive(Clone, Default)]
pub(super) struct Facts {
    bits: BTreeMap<Source, bool>,
}

impl Facts {
    // Key facts on the underlying wire shared by a bit and its retained copies.
    pub(super) fn assume(&mut self, word: &[Bit], value: u128) {
        for (index, &bit) in word.iter().enumerate() {
            if let Identity::Source(source, inverted) = bit.identity() {
                self.bits.insert(source, (value >> index & 1 != 0) ^ inverted);
            }
        }
    }

    fn known(&self, bit: Bit) -> Option<bool> {
        match bit.identity() {
            Identity::Literal(value) => Some(value),
            Identity::Source(source, inverted) => self.bits.get(&source).map(|&value| value ^ inverted),
        }
    }

    pub(super) fn resolve(&self, bit: Bit) -> Bit {
        self.known(bit).map_or(bit, Bit::Constant)
    }

    pub(super) fn resolve_word(&self, mut word: Word) -> Word {
        if !self.bits.is_empty() {
            for bit in &mut word {
                *bit = self.resolve(*bit);
            }
        }
        word
    }

    pub(super) fn join(&self, left: Bit, other: &Self, right: Bit, right_guard: Bit, circuit: &mut Circuit) -> Bit {
        if left.same_value(right) {
            return right;
        }
        let known_left = self.resolve(left);
        let known_right = other.resolve(right);
        // A bit can represent the result when it agrees with both inputs
        // under their respective guards.
        if known_left.same_value(known_right) {
            known_left
        } else if self.known(right).is_some_and(|value| known_left.literal() == Some(value)) {
            right
        } else if other.known(left).is_some_and(|value| known_right.literal() == Some(value)) {
            left
        } else {
            circuit.select(right_guard, known_right, known_left)
        }
    }

    pub(super) fn intersect(&mut self, other: &Self) {
        self.bits.retain(|source, value| other.bits.get(source).copied() == Some(*value));
    }
}
