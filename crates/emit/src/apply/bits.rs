//! Little-endian words whose bits are constants or qubit values.

use std::ops::Range;

use bennett_exec::{BlockOp, GateBlock, GateOp, Wire};
use bennett_ir::{FixedGate, Gate};

use super::ApplyError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Bit {
    Constant(bool),
    Wire(Wire, bool),
}

pub(super) const ZERO: Bit = Bit::Constant(false);
pub(super) const ONE: Bit = Bit::Constant(true);
const X: Gate = Gate::Fixed(FixedGate::X);

impl Bit {
    pub(super) fn not(self) -> Self {
        match self {
            Self::Constant(value) => Self::Constant(!value),
            Self::Wire(wire, inverted) => Self::Wire(wire, !inverted),
        }
    }
}

pub(super) type Word = Vec<Bit>;

pub(super) fn constant(value: u128, width: usize) -> Word {
    (0..width).map(|bit| Bit::Constant(value >> bit & 1 != 0)).collect()
}

pub(super) fn definite(word: &[Bit]) -> Option<u128> {
    word.iter().enumerate().try_fold(0, |value, (index, bit)| match bit {
        Bit::Constant(set) => Some(value | u128::from(*set) << index),
        Bit::Wire(..) => None,
    })
}

pub(super) struct Circuit {
    start: Wire,
    next: Option<Wire>,
    gates: Vec<GateOp>,
    enabled: Bit,
    control_scratch: Option<Wire>,
}

pub(super) struct Checkpoint {
    next: Option<Wire>,
    gates: usize,
    control_scratch: Option<Wire>,
}

impl Circuit {
    pub(super) fn new(start: Wire) -> Self {
        Self { start, next: Some(start), gates: Vec::new(), enabled: ONE, control_scratch: None }
    }

    pub(super) fn fresh(&mut self) -> Wire {
        // A range ending at MAX is valid. Allocating MAX itself would need an end past it.
        let wire = self.next.unwrap_or(Wire::MAX);
        self.next = wire.checked_add(1);
        wire
    }

    fn flip(&mut self, target: Wire, controls: Vec<Wire>) {
        self.gates.push(GateOp { kind: X, target, controls });
    }

    pub(super) fn checkpoint(&self) -> Checkpoint {
        Checkpoint { next: self.next, gates: self.gates.len(), control_scratch: self.control_scratch }
    }

    pub(super) fn rewind(&mut self, checkpoint: Checkpoint) {
        self.next = checkpoint.next;
        self.gates.truncate(checkpoint.gates);
        self.control_scratch = checkpoint.control_scratch;
    }

    pub(super) fn under<T>(&mut self, enabled: Bit, emit: impl FnOnce(&mut Self) -> T) -> T {
        debug_assert_eq!(self.enabled, ONE);
        self.enabled = enabled;
        let result = emit(self);
        self.enabled = ONE;
        result
    }

    pub(super) fn xor_into(&mut self, target: Wire, value: Bit, enabled: Bit) {
        let mut controls = Vec::new();
        for bit in [value, enabled, self.enabled] {
            match bit {
                ZERO => return,
                ONE => continue,
                Bit::Wire(wire, inverted) => {
                    if controls.contains(&(wire, !inverted)) {
                        return;
                    }
                    if !controls.contains(&(wire, inverted)) {
                        controls.push((wire, inverted));
                    }
                }
            }
        }
        for &(wire, inverted) in &controls {
            if inverted {
                self.flip(wire, Vec::new());
            }
        }
        if let [(a, _), (b, _), (c, _)] = controls.as_slice() {
            // Compute and clear the extra control using a single reusable qubit.
            let scratch = match self.control_scratch {
                Some(wire) => wire,
                None => {
                    let wire = self.fresh();
                    self.control_scratch = Some(wire);
                    wire
                }
            };
            self.flip(scratch, vec![*a, *b]);
            self.flip(target, vec![scratch, *c]);
            self.flip(scratch, vec![*a, *b]);
        } else {
            self.flip(target, controls.iter().map(|&(wire, _)| wire).collect());
        }
        for &(wire, inverted) in controls.iter().rev() {
            if inverted {
                self.flip(wire, Vec::new());
            }
        }
    }

    pub(super) fn and(&mut self, left: Bit, right: Bit) -> Bit {
        if left == right.not() || left == ZERO || right == ZERO {
            return ZERO;
        }
        if left == ONE || left == right {
            return right;
        }
        if right == ONE {
            return left;
        }
        let wire = self.fresh();
        self.xor_into(wire, left, right);
        Bit::Wire(wire, false)
    }

    pub(super) fn xor(&mut self, left: Bit, right: Bit) -> Bit {
        match (left, right) {
            (ZERO, bit) | (bit, ZERO) => bit,
            (ONE, bit) | (bit, ONE) => bit.not(),
            (Bit::Wire(a, neg_a), Bit::Wire(b, neg_b)) if a == b => Bit::Constant(neg_a != neg_b),
            (Bit::Wire(a, neg_a), Bit::Wire(b, neg_b)) => {
                let wire = self.fresh();
                self.xor_into(wire, Bit::Wire(a, false), ONE);
                self.xor_into(wire, Bit::Wire(b, false), ONE);
                Bit::Wire(wire, neg_a != neg_b)
            }
        }
    }

    pub(super) fn select(&mut self, condition: Bit, yes: Bit, no: Bit) -> Bit {
        if condition == ONE || yes == no {
            return yes;
        }
        if condition == ZERO {
            return no;
        }
        match (yes, no) {
            (ZERO, bit) => self.and(condition.not(), bit),
            (ONE, bit) => self.and(condition.not(), bit.not()).not(),
            (bit, ZERO) => self.and(condition, bit),
            (bit, ONE) => self.and(condition, bit.not()).not(),
            (Bit::Wire(..), Bit::Wire(..)) if yes == no.not() => self.xor(no, condition),
            (Bit::Wire(..), Bit::Wire(..)) => {
                let wire = self.fresh();
                self.xor_into(wire, no, ONE);
                self.xor_into(wire, yes, condition);
                self.xor_into(wire, no, condition);
                Bit::Wire(wire, false)
            }
        }
    }

    pub(super) fn copy(&mut self, target: Range<Wire>, word: &[Bit], enabled: Bit) {
        for (wire, &bit) in target.zip(word) {
            self.xor_into(wire, bit, enabled);
        }
    }

    pub(super) fn forward_len(&self) -> usize {
        self.gates.len()
    }

    pub(super) fn finish(mut self, forward_len: usize, name: &str) -> Result<GateBlock, ApplyError> {
        let scratch = self.start..self.next.ok_or(ApplyError::WireSpace)?;
        self.gates.reserve(forward_len);
        for index in (0..forward_len).rev() {
            self.gates.push(self.gates[index].clone());
        }
        let ops = [BlockOp::Allocate(scratch.clone())]
            .into_iter()
            .chain(self.gates.into_iter().map(BlockOp::Gate))
            .chain([BlockOp::ReleaseZero(scratch)]);
        Ok(GateBlock::from_ops(name, ops))
    }
}
