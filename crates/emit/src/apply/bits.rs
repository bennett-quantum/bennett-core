//! Little-endian words whose bits are constants or qubit values.

use std::ops::Range;

use bennett_exec::{BlockOp, GateBlock, GateOp, Wire};
use bennett_ir::{FixedGate, Gate};

use super::ApplyError;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
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
    peak: Option<Wire>,
    gates: Vec<GateOp>,
    enabled: Bit,
    control_scratch: Option<Wire>,
    retained_copies: Vec<(Wire, Bit)>,
}

pub(super) struct Checkpoint {
    next: Option<Wire>,
    peak: Option<Wire>,
    gates: usize,
    control_scratch: Option<Wire>,
    retained_copies: usize,
}

impl Circuit {
    pub(super) fn new(start: Wire) -> Self {
        Self {
            start,
            next: Some(start),
            peak: Some(start),
            gates: Vec::new(),
            enabled: ONE,
            control_scratch: None,
            retained_copies: Vec::new(),
        }
    }

    pub(super) fn fresh(&mut self) -> Wire {
        // A range ending at MAX is valid. Allocating MAX itself would need an end past it.
        let wire = self.next.unwrap_or(Wire::MAX);
        self.next = wire.checked_add(1);
        self.peak = self.peak.zip(self.next).map(|(peak, next)| peak.max(next));
        wire
    }

    fn flip(&mut self, target: Wire, controls: Vec<Wire>) {
        self.gates.push(GateOp { kind: X, target, controls });
    }

    pub(super) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            next: self.next,
            peak: self.peak,
            gates: self.gates.len(),
            control_scratch: self.control_scratch,
            retained_copies: self.retained_copies.len(),
        }
    }

    // The in-place update uses the saved allocator state without copying the
    // already emitted prefix. Its gates start a separate suffix.
    pub(super) fn suffix(&self, checkpoint: &Checkpoint) -> Self {
        Self {
            start: self.start,
            next: checkpoint.next,
            peak: checkpoint.peak,
            gates: Vec::new(),
            enabled: self.enabled,
            control_scratch: checkpoint.control_scratch,
            retained_copies: self.retained_copies[..checkpoint.retained_copies].to_vec(),
        }
    }

    pub(super) fn replace_after(&mut self, checkpoint: Checkpoint, mut suffix: Self) {
        self.gates.truncate(checkpoint.gates);
        self.gates.append(&mut suffix.gates);
        std::mem::swap(&mut self.gates, &mut suffix.gates);
        *self = suffix;
    }

    pub(super) fn rewind(&mut self, checkpoint: Checkpoint) {
        self.next = checkpoint.next;
        self.peak = checkpoint.peak;
        self.gates.truncate(checkpoint.gates);
        self.control_scratch = checkpoint.control_scratch;
        self.retained_copies.truncate(checkpoint.retained_copies);
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
        let (known_left, known_right) = (self.copy_source(left), self.copy_source(right));
        if known_left == known_right.not() || left == ZERO || right == ZERO {
            return ZERO;
        }
        if left == ONE || known_left == known_right {
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
            (Bit::Wire(a, neg_a), Bit::Wire(b, neg_b)) => {
                let (known_left, known_right) = (self.copy_source(left), self.copy_source(right));
                if known_left == known_right {
                    return ZERO;
                }
                if known_left == known_right.not() {
                    return ONE;
                }
                let wire = self.fresh();
                self.xor_into(wire, Bit::Wire(a, false), ONE);
                self.xor_into(wire, Bit::Wire(b, false), ONE);
                Bit::Wire(wire, neg_a != neg_b)
            }
        }
    }

    pub(super) fn select(&mut self, condition: Bit, yes: Bit, no: Bit) -> Bit {
        let (known_yes, known_no) = (self.copy_source(yes), self.copy_source(no));
        if condition == ONE || known_yes == known_no {
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
            (Bit::Wire(..), Bit::Wire(..)) if known_yes == known_no.not() => self.xor(no, condition),
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

    // Follow a retained copy back to its immutable source.
    pub(super) fn copy_source(&self, bit: Bit) -> Bit {
        if let Bit::Wire(wire, inverted) = bit
            && let Ok(index) = self.retained_copies.binary_search_by_key(&wire, |&(copy, _)| copy)
        {
            let source = self.retained_copies[index].1;
            return if inverted { source.not() } else { source };
        }
        bit
    }

    // Reserve the result before computing, then clear and reuse all later wires.
    pub(super) fn clean_word(&mut self, width: usize, compute: impl FnOnce(&mut Self) -> Word) -> Word {
        let checkpoint = self.checkpoint();
        // The reservation counts toward the peak only if the result needs wires.
        self.next = self.next.and_then(|next| next.checked_add(width as Wire));
        let temporary = self.next;
        let control_scratch = self.control_scratch;
        let start = self.forward_len();
        let mut computed = compute(self);
        computed.resize(width, ZERO);
        // Failed allocations use a placeholder wire. Equality involving it
        // cannot prove that the speculative computation is safe to erase.
        if self.peak.is_none() {
            return computed;
        }
        if definite(&computed).is_some() {
            self.rewind(checkpoint);
            return computed;
        }
        let Some((first, temporary)) = checkpoint.next.zip(temporary) else {
            self.peak = None;
            return computed;
        };
        self.peak = self.peak.map(|peak| peak.max(temporary));
        let end = self.forward_len();
        let mut copies = Vec::new();
        let result = (first..temporary)
            .zip(computed)
            .map(|(wire, value)| match value {
                Bit::Constant(_) => value,
                Bit::Wire(..) => {
                    // Only an unconditional copy of a source that predates
                    // this computation survives its local reversal.
                    if self.enabled == ONE
                        && let source @ Bit::Wire(source_wire, _) = self.copy_source(value)
                        && source_wire < first
                    {
                        copies.push((wire, source));
                    }
                    self.xor_into(wire, value, ONE);
                    Bit::Wire(wire, false)
                }
            })
            .collect();
        for index in (start..end).rev() {
            self.gates.push(self.gates[index].clone());
        }
        self.next = Some(temporary);
        self.control_scratch = control_scratch;
        self.retained_copies.truncate(checkpoint.retained_copies);
        self.retained_copies.extend(copies);
        result
    }

    pub(super) fn fits(&self) -> bool {
        self.peak.is_some()
    }

    pub(super) fn scratch_end(&self) -> Option<Wire> {
        self.next
    }

    pub(super) fn forward_gates(&self) -> &[GateOp] {
        &self.gates
    }

    pub(super) fn forward_len(&self) -> usize {
        self.gates.len()
    }

    pub(super) fn fits_at(&self, start: Wire) -> bool {
        self.peak.is_some_and(|end| start.checked_add(end - self.start).is_some())
    }

    pub(super) fn finish(self, forward_len: usize, name: &str) -> Result<GateBlock, ApplyError> {
        self.finish_placed(forward_len, name, std::convert::identity)
    }

    pub(super) fn finish_placed(
        mut self,
        forward_len: usize,
        name: &str,
        place: impl Fn(Wire) -> Wire,
    ) -> Result<GateBlock, ApplyError> {
        let scratch = place(self.start)..place(self.peak.ok_or(ApplyError::WireSpace)?);
        for gate in &mut self.gates {
            gate.target = place(gate.target);
            for control in &mut gate.controls {
                *control = place(*control);
            }
        }
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
