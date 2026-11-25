//! Definite bit values for the inputs enabled on one control route.

use std::collections::BTreeMap;

use bennett_exec::Wire;

use super::super::bits::{Bit, Circuit, Word};

#[derive(Clone, Default)]
pub(super) struct Facts {
    bits: BTreeMap<Wire, bool>,
}

impl Facts {
    // Attach a fact to the wire, so moves and reassignments of slots preserve
    // exactly the facts about the bits they carry.
    pub(super) fn assume(&mut self, word: &[Bit], value: u128) {
        for (index, &bit) in word.iter().enumerate() {
            if let Bit::Wire(wire, inverted) = bit {
                self.bits.insert(wire, (value >> index & 1 != 0) ^ inverted);
            }
        }
    }

    pub(super) fn resolve(&self, bit: Bit) -> Bit {
        if let Bit::Wire(wire, inverted) = bit
            && let Some(&value) = self.bits.get(&wire)
        {
            return Bit::Constant(value ^ inverted);
        }
        bit
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
        if circuit.copy_source(left) == circuit.copy_source(right) {
            return right;
        }
        let known_left = self.resolve(left);
        let known_right = other.resolve(right);
        // A bit can represent the result when it agrees with both inputs
        // under their respective guards.
        if known_left == known_right {
            known_left
        } else if self.resolve(right) == known_left {
            right
        } else if other.resolve(left) == known_right {
            left
        } else {
            circuit.select(right_guard, known_right, known_left)
        }
    }

    pub(super) fn intersect(&mut self, other: &Self) {
        self.bits.retain(|wire, value| other.bits.get(wire).copied() == Some(*value));
    }
}
