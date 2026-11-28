//! Little-endian words whose bits are constants or qubit values.

use std::ops::Range;

use bennett_exec::{BlockOp, GateBlock, GateOp, Wire};
use bennett_ir::{FixedGate, Gate};

use super::ApplyError;

pub(super) use super::identity::Bit;
use super::identity::{Generation, Identity};

pub(super) const ZERO: Bit = Bit::Constant(false);
pub(super) const ONE: Bit = Bit::Constant(true);
const X: Gate = Gate::Fixed(FixedGate::X);

pub(super) type Word = Vec<Bit>;

pub(super) fn constant(value: u128, width: usize) -> Word {
    (0..width).map(|bit| Bit::Constant(value >> bit & 1 != 0)).collect()
}

pub(super) fn definite(word: &[Bit]) -> Option<u128> {
    word.iter().enumerate().try_fold(0, |value, (index, bit)| bit.literal().map(|set| value | u128::from(set) << index))
}

pub(super) struct Circuit {
    start: Wire,
    next: Option<Wire>,
    peak: Option<Wire>,
    gates: Vec<GateOp>,
    enabled: Bit,
    control_scratch: Option<Wire>,
    generation: Generation,
}

pub(super) struct Checkpoint {
    next: Option<Wire>,
    peak: Option<Wire>,
    gates: usize,
    control_scratch: Option<Wire>,
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
            generation: Generation::next(),
        }
    }

    pub(super) fn fresh(&mut self) -> Wire {
        // A range ending at MAX is valid. Allocating MAX itself would need an end past it.
        let wire = self.next.unwrap_or(Wire::MAX);
        self.next = wire.checked_add(1);
        self.peak = self.peak.zip(self.next).map(|(peak, next)| peak.max(next));
        wire
    }

    // Publish a newly computed immutable wire in the current allocation lifetime.
    pub(super) fn value(&self, wire: Wire) -> Bit {
        Bit::produced(wire, self.generation)
    }

    fn flip(&mut self, target: Wire, controls: Vec<Wire>) {
        self.gates.push(GateOp { kind: X, target, controls });
    }

    pub(super) fn checkpoint(&self) -> Checkpoint {
        Checkpoint { next: self.next, peak: self.peak, gates: self.gates.len(), control_scratch: self.control_scratch }
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
            generation: Generation::next(),
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
        self.generation = Generation::next();
    }

    pub(super) fn under<T>(&mut self, enabled: Bit, emit: impl FnOnce(&mut Self) -> T) -> T {
        debug_assert!(self.enabled.is_one());
        self.enabled = enabled;
        let result = emit(self);
        self.enabled = ONE;
        result
    }

    pub(super) fn xor_into(&mut self, target: Wire, value: Bit, enabled: Bit) {
        let mut controls = Vec::new();
        for bit in [value, enabled, self.enabled] {
            match bit.literal() {
                Some(false) => return,
                Some(true) => continue,
                None => {}
            }
            // Deduplicate operands of this physical gate, including the
            // mutable work wires of an in-place update.
            let (wire, inverted) = bit.wire().expect("a nonliteral control has a wire");
            if controls.contains(&(wire, !inverted)) {
                return;
            }
            if !controls.contains(&(wire, inverted)) {
                controls.push((wire, inverted));
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
        if left.same_value(right.not()) || left.is_zero() || right.is_zero() {
            return ZERO;
        }
        if left.is_one() || left.same_value(right) {
            return right;
        }
        if right.is_one() {
            return left;
        }
        let wire = self.fresh();
        self.xor_into(wire, left, right);
        self.value(wire)
    }

    pub(super) fn xor(&mut self, left: Bit, right: Bit) -> Bit {
        match (left.literal(), right.literal()) {
            (Some(value), _) => right.with_flip(value),
            (_, Some(value)) => left.with_flip(value),
            (None, None) => {
                if left.same_value(right) {
                    return ZERO;
                }
                if left.same_value(right.not()) {
                    return ONE;
                }
                let (_, neg_a) = left.wire().expect("symbolic XOR operand");
                let (_, neg_b) = right.wire().expect("symbolic XOR operand");
                let wire = self.fresh();
                self.xor_into(wire, left.with_flip(neg_a), ONE);
                self.xor_into(wire, right.with_flip(neg_b), ONE);
                self.value(wire).with_flip(neg_a ^ neg_b)
            }
        }
    }

    pub(super) fn select(&mut self, condition: Bit, yes: Bit, no: Bit) -> Bit {
        if condition.is_one() || yes.same_value(no) {
            return yes;
        }
        if condition.is_zero() {
            return no;
        }
        match (yes.literal(), no.literal()) {
            (Some(false), _) => self.and(condition.not(), no),
            (Some(true), _) => self.and(condition.not(), no.not()).not(),
            (_, Some(false)) => self.and(condition, yes),
            (_, Some(true)) => self.and(condition, yes.not()).not(),
            (None, None) if yes.same_value(no.not()) => self.xor(no, condition),
            (None, None) => {
                let wire = self.fresh();
                self.xor_into(wire, no, ONE);
                self.xor_into(wire, yes, condition);
                self.xor_into(wire, no, condition);
                self.value(wire)
            }
        }
    }

    pub(super) fn copy(&mut self, target: Range<Wire>, word: &[Bit], enabled: Bit) {
        for (wire, &bit) in target.zip(word) {
            self.xor_into(wire, bit, enabled);
        }
    }

    pub(super) fn retain_copy(&mut self, bit: Bit) -> Bit {
        debug_assert!(self.enabled.is_one());
        let wire = self.fresh();
        self.xor_into(wire, bit, ONE);
        bit.copied(wire, self.generation)
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
        if let Some(value) = definite(&computed) {
            self.rewind(checkpoint);
            return constant(value, width);
        }
        let Some((first, temporary)) = checkpoint.next.zip(temporary) else {
            self.peak = None;
            return computed;
        };
        self.peak = self.peak.map(|peak| peak.max(temporary));
        let end = self.forward_len();
        let result_generation = Generation::next();
        let result = (first..temporary)
            .zip(computed)
            .map(|(wire, value)| match value.identity() {
                Identity::Literal(literal) => Bit::Constant(literal),
                Identity::Source(source, _) => {
                    self.xor_into(wire, value, ONE);
                    // Only an unconditional copy of a source that predates
                    // this computation survives its local reversal.
                    if self.enabled.is_one() && source.precedes(first) {
                        value.copied(wire, result_generation)
                    } else {
                        Bit::produced(wire, result_generation)
                    }
                }
            })
            .collect();
        for index in (start..end).rev() {
            self.gates.push(self.gates[index].clone());
        }
        self.next = Some(temporary);
        self.control_scratch = control_scratch;
        self.generation = result_generation;
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

    pub(super) fn before(&self, checkpoint: &Checkpoint) -> &[GateOp] {
        &self.gates[..checkpoint.gates]
    }

    pub(super) fn forward_len(&self) -> usize {
        self.gates.len()
    }

    pub(super) fn fits_at(&self, start: Wire) -> bool {
        self.peak.is_some_and(|end| start.checked_add(end - self.start).is_some())
    }

    pub(super) fn peak_scratch(&self) -> Option<Wire> {
        self.peak.map(|end| end - self.start)
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
