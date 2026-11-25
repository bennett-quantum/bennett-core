//! Definite bit values for the inputs enabled on one control route.

use std::collections::BTreeMap;

use bennett_exec::Wire;

use super::super::bits::{Bit, Circuit, Word};

#[derive(Clone, Default)]
pub(super) struct Facts {
    bits: BTreeMap<Wire, bool>,
}

impl Facts {
    // Key facts on the underlying wire shared by a bit and its retained copies.
    pub(super) fn assume(&mut self, circuit: &Circuit, word: &[Bit], value: u128) {
        for (index, &bit) in word.iter().enumerate() {
            if let Bit::Wire(wire, inverted) = circuit.copy_source(bit) {
                self.bits.insert(wire, (value >> index & 1 != 0) ^ inverted);
            }
        }
    }

    fn known(&self, source: Bit) -> Option<bool> {
        match source {
            Bit::Constant(value) => Some(value),
            Bit::Wire(wire, inverted) | Bit::Shared { wire, inverted, .. } => {
                self.bits.get(&wire).map(|&value| value ^ inverted)
            }
        }
    }

    pub(super) fn resolve(&self, circuit: &Circuit, bit: Bit) -> Bit {
        self.known(circuit.copy_source(bit)).map_or(bit, Bit::Constant)
    }

    pub(super) fn resolve_word(&self, circuit: &Circuit, mut word: Word) -> Word {
        if !self.bits.is_empty() {
            for bit in &mut word {
                *bit = self.resolve(circuit, *bit);
            }
        }
        word
    }

    pub(super) fn join(&self, left: Bit, other: &Self, right: Bit, right_guard: Bit, circuit: &mut Circuit) -> Bit {
        let left_source = circuit.copy_source(left);
        let right_source = circuit.copy_source(right);
        if left_source == right_source {
            return right;
        }
        let known_left = self.known(left_source).map_or(left, Bit::Constant);
        let known_right = other.known(right_source).map_or(right, Bit::Constant);
        // A bit can represent the result when it agrees with both inputs
        // under their respective guards.
        if known_left == known_right {
            known_left
        } else if self.known(right_source).map(Bit::Constant) == Some(known_left) {
            right
        } else if other.known(left_source).map(Bit::Constant) == Some(known_right) {
            left
        } else {
            circuit.select(right_guard, known_right, known_left)
        }
    }

    pub(super) fn intersect(&mut self, other: &Self) {
        self.bits.retain(|wire, value| other.bits.get(wire).copied() == Some(*value));
    }
}
