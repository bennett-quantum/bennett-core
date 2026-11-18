//! Little-endian words whose bits are constants or qubit values.

use std::ops::Range;

use bennett_exec::{BlockOp, GateBlock, GateOp, Wire};
use bennett_ir::{FixedGate, Gate};

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
    next: Wire,
    gates: Vec<GateOp>,
}

impl Circuit {
    pub(super) fn new(start: Wire) -> Self {
        Self { start, next: start, gates: Vec::new() }
    }

    pub(super) fn fresh(&mut self) -> Wire {
        let wire = self.next;
        self.next += 1;
        wire
    }

    fn flip(&mut self, target: Wire, controls: Vec<Wire>) {
        self.gates.push(GateOp { kind: X, target, controls });
    }

    pub(super) fn xor_into(&mut self, target: Wire, value: Bit, enabled: Bit) {
        let controls = match (value, enabled) {
            (ZERO, _) | (_, ZERO) => return,
            (ONE, ONE) => Vec::new(),
            (Bit::Wire(w, neg), ONE) | (ONE, Bit::Wire(w, neg)) => vec![(w, neg)],
            (Bit::Wire(a, neg_a), Bit::Wire(b, neg_b)) if a == b => {
                if neg_a != neg_b {
                    return;
                }
                vec![(a, neg_a)]
            }
            (Bit::Wire(a, neg_a), Bit::Wire(b, neg_b)) => vec![(a, neg_a), (b, neg_b)],
        };
        for &(wire, inverted) in &controls {
            if inverted {
                self.flip(wire, Vec::new());
            }
        }
        self.flip(target, controls.iter().map(|&(wire, _)| wire).collect());
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
                self.flip(wire, vec![a]);
                self.flip(wire, vec![b]);
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
        let difference = self.xor(yes, no);
        let change = self.and(condition, difference);
        self.xor(no, change)
    }

    pub(super) fn copy(&mut self, target: Range<Wire>, word: &[Bit], enabled: Bit) {
        for (wire, &bit) in target.zip(word) {
            self.xor_into(wire, bit, enabled);
        }
    }

    pub(super) fn forward_len(&self) -> usize {
        self.gates.len()
    }

    pub(super) fn finish(self, forward_len: usize, name: &str) -> GateBlock {
        let scratch = self.start..self.next;
        let inverse = self.gates[..forward_len].iter().rev().cloned().map(BlockOp::Gate).collect::<Vec<_>>();
        let ops = [BlockOp::Allocate(scratch.clone())]
            .into_iter()
            .chain(self.gates.into_iter().map(BlockOp::Gate))
            .chain(inverse)
            .chain([BlockOp::ReleaseZero(scratch)]);
        GateBlock::from_ops(name, ops)
    }
}
