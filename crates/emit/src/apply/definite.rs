//! Prove constants for the operations that need them, independently of the step budget.

use std::collections::{BTreeMap, BTreeSet};

use bennett_exec::Wire;
use bennett_ir::{BinOp, BlockId, FuncId, Inst, Operand, Signature, Slot, Terminator, ValidModule, Width};

use super::bits::{Bit, Circuit, Word, ZERO, constant, definite};
use super::selectors::Selector;
use super::switches::Edges;
use super::symbolic::{Frame, before_instruction_fault};
use super::{arithmetic, division, ranges};

mod results;
use results::ReturnWord;
pub(super) use results::{CallProofs, CallResult};

fn fold_inst(
    inst: &Inst,
    read: impl Fn(&Operand) -> Option<Word>,
    circuit: &mut Circuit,
    unknown: impl FnOnce() -> Word,
) -> Option<Word> {
    match inst {
        // Missing inputs can arrive on a later visit. An operation with known
        // operands that cannot be folded contributes an unknown value now.
        Inst::Binary { op, lhs, rhs, .. } => {
            let left = read(lhs)?;
            let right = read(rhs)?;
            Some(arithmetic::fold_binary(circuit, *op, &left, &right).unwrap_or_else(|| {
                let mut word = unknown();
                if *op == BinOp::Mul {
                    let width =
                        ranges::product_bits(ranges::unsigned_max(&left), ranges::unsigned_max(&right), word.len());
                    word[width..].fill(ZERO);
                } else if matches!(op, BinOp::Urem | BinOp::Srem) {
                    division::apply_remainder_facts(*op, &left, &right, &mut word);
                }
                word
            }))
        }
        Inst::Compare { pred, lhs, rhs, .. } => {
            Some(vec![arithmetic::compare(circuit, *pred, &read(lhs)?, &read(rhs)?)])
        }
        Inst::Cast { op, to, value, .. } => Some(arithmetic::cast(*op, read(value)?, to.bits() as usize)),
        Inst::Select { cond, if_true, if_false, .. } => match read(cond)?[0] {
            Bit::Constant(choice) => read(if choice { if_true } else { if_false }),
            condition @ (Bit::Wire(..) | Bit::Shared { .. }) => Some(
                read(if_true)?
                    .into_iter()
                    .zip(read(if_false)?)
                    .map(|(yes, no)| circuit.select(condition, yes, no))
                    .collect(),
            ),
        },
        Inst::FuncAddr { func, .. } => Some(constant(u128::from(func.address()), u64::BITS as usize)),
        Inst::Call { .. } | Inst::CallIndirect { .. } => unreachable!("calls use the return proof"),
        Inst::Load { .. }
        | Inst::Alloca { .. }
        | Inst::StackSave { .. }
        | Inst::Setjmp { .. }
        | Inst::ReturnAddress { .. } => Some(unknown()),
        Inst::Store { .. } | Inst::StackRestore { .. } => None,
        Inst::Effect { .. }
        | Inst::Qalloc { .. }
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => unreachable!("checked the supported instructions"),
    }
}

pub(super) struct CallSite<'a> {
    pub(super) target: Option<&'a Operand>,
    pub(super) args: &'a [Operand],
    pub(super) argument_facts: Vec<Option<Word>>,
    pub(super) dst: Option<Slot>,
    pub(super) candidates: BTreeSet<FuncId>,
    pub(super) invalid_possible: bool,
}

type Calls<'a> = BTreeMap<(BlockId, usize), CallSite<'a>>;

fn record_call<'a, 'sites>(
    calls: &'sites mut Calls<'a>,
    at: (BlockId, usize),
    call: CallSite<'a>,
) -> &'sites CallSite<'a> {
    let site = calls.entry(at).or_insert_with(|| CallSite {
        target: call.target,
        args: call.args,
        argument_facts: Vec::new(),
        dst: call.dst,
        candidates: BTreeSet::new(),
        invalid_possible: false,
    });
    // Retain the possibilities from earlier visits as input facts change at joins.
    site.candidates.extend(call.candidates);
    site.invalid_possible |= call.invalid_possible;
    site.argument_facts = call.argument_facts;
    site
}

fn indirect_targets(module: &ValidModule, signature: &Signature, address: &[Bit]) -> (BTreeSet<FuncId>, bool) {
    if let Some(address) = definite(address) {
        let callee =
            module.function_at(address as u64).filter(|&callee| module.function(callee).has_signature(signature));
        return (callee.into_iter().collect(), callee.is_none());
    }

    let selector = Selector::new(address);
    let candidates: BTreeSet<_> = module
        .functions
        .iter()
        .enumerate()
        .filter_map(|(index, function)| {
            let callee = FuncId(index as u32);
            (function.has_signature(signature) && selector.allows(u128::from(callee.address()))).then_some(callee)
        })
        .collect();
    let invalid_possible = !selector.covered_by(candidates.len());
    (candidates, invalid_possible)
}

fn intersect_return(joined: &mut Option<ReturnWord>, word: ReturnWord) {
    if let Some(joined) = joined {
        for (old, bit) in joined.iter_mut().zip(word) {
            if *old != bit {
                *old = None;
            }
        }
    } else {
        *joined = Some(word);
    }
}

fn fold_call(
    slot: Option<Slot>,
    candidates: &BTreeSet<FuncId>,
    args: &[Option<Word>],
    mut call_result: impl FnMut(FuncId, &[Option<Word>]) -> CallResult,
    unknown_word: impl FnOnce(Slot) -> Word,
) -> Option<Option<Word>> {
    let mut returning = false;
    let mut joined = None;
    let unknown = slot.map(unknown_word);
    for &callee in candidates {
        let CallResult::Returns(mut word) = call_result(callee, args) else {
            continue;
        };
        returning = true;
        let Some(unknown) = &unknown else { continue };
        word.resize(unknown.len(), None);
        intersect_return(&mut joined, word);
    }
    returning.then(|| {
        joined.zip(unknown).map(|(proved, unknown)| {
            proved.into_iter().zip(unknown).map(|(bit, unknown)| bit.unwrap_or(unknown)).collect()
        })
    })
}

// Number the argument identities by their first occurrence. The reverse map
// translates the callee's argument identities back into the caller's word.
fn normalize_arguments(
    frame: &Frame<'_>,
    inputs: impl Iterator<Item = Option<Word>>,
) -> (Vec<Word>, Vec<Option<Wire>>) {
    let mut identities = BTreeMap::new();
    let mut sources = Vec::new();
    let words = frame
        .module
        .function(frame.callee)
        .params
        .iter()
        .zip(inputs)
        .map(|(parameter, input)| match input {
            Some(word) => word
                .into_iter()
                .map(|bit| match bit {
                    Bit::Constant(_) => bit,
                    Bit::Wire(wire, inverted) | Bit::Shared { wire, inverted, .. } => {
                        let number = *identities.entry(wire).or_insert_with(|| {
                            let number = sources.len() as Wire;
                            sources.push(Some(wire));
                            number
                        });
                        Bit::Wire(number, inverted)
                    }
                })
                .collect(),
            None => (0..frame.slot_width(parameter.slot))
                .map(|_| {
                    let number = sources.len() as Wire;
                    sources.push(None);
                    Bit::Wire(number, false)
                })
                .collect(),
        })
        .collect();
    (words, sources)
}

#[derive(Clone, Copy)]
struct ArgumentBit {
    slot: Slot,
    index: usize,
    inverted: bool,
}

#[derive(Default)]
pub(super) struct Constants<'a> {
    slots: Vec<Option<Word>>,
    argument_bits: Vec<ArgumentBit>,
    returned: Option<ReturnWord>,
    switches: Vec<Edges<'a>>,
    pub(super) calls: Calls<'a>,
    may_return: bool,
}

impl<'a> Constants<'a> {
    pub(super) fn is_definite(&self, operand: &Operand) -> bool {
        match operand {
            Operand::Const(_) => true,
            Operand::Slot(slot) => self.slots.get(slot.index()).and_then(Option::as_deref).and_then(definite).is_some(),
        }
    }

    pub(super) fn project(&self, slots: &[Word], operand: &Operand, mut word: Word) -> Word {
        let Operand::Slot(slot) = operand else { return word };
        if let Some(proved) = self.slots.get(slot.index()).and_then(Option::as_ref) {
            for (bit, fact) in word.iter_mut().zip(proved) {
                match *fact {
                    Bit::Constant(_) => *bit = *fact,
                    Bit::Wire(wire, inverted) | Bit::Shared { wire, inverted, .. } => {
                        // A current frame's immutable argument slots carry
                        // the physical bits for its abstract identities.
                        if let Some(source) = self.argument_bits.get(wire as usize) {
                            let original = slots[source.slot.index()][source.index];
                            *bit = if inverted ^ source.inverted { original.not() } else { original };
                        }
                    }
                }
            }
        }
        word
    }

    pub(super) fn switch_edges(&self, block: BlockId) -> &Edges<'a> {
        &self.switches[block.index()]
    }

    pub(super) fn for_emission(
        frame: &Frame<'a>,
        inputs: impl Iterator<Item = Option<Word>>,
        proofs: &mut CallProofs<'a>,
    ) -> Self {
        let function = frame.module.function(frame.callee);
        let (inputs, _) = normalize_arguments(frame, inputs);
        let stopping_proof = proofs.returns.stopping_proof(frame.callee);
        let needs_analysis = function.blocks.iter().any(|block| {
            matches!(&block.term, Terminator::Switch { .. })
                || block.insts.iter().any(|inst| {
                    matches!(
                        inst,
                        Inst::Binary { op: BinOp::Shl | BinOp::Lshr | BinOp::Ashr, rhs: Operand::Slot(_), .. }
                            | Inst::Call { .. }
                            | Inst::CallIndirect { .. }
                            | Inst::Binary { op: BinOp::Mul, lhs: Operand::Slot(_), rhs: Operand::Slot(_), .. }
                            | Inst::Binary {
                                op: BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem,
                                rhs: Operand::Slot(_),
                                ..
                            }
                    )
                })
        });
        // Emit a proved stopping copy with the same argument facts that
        // let its caller omit the continuation.
        let mut plan = if needs_analysis || proofs.stops_with_arguments(frame.callee, &inputs) {
            Self::solve(frame, &inputs, |target, args| proofs.call(frame.module, target, args))
        } else {
            Self::with_arguments(frame, &inputs)
        };
        if let Some(proof) = stopping_proof.as_deref() {
            // Keep the control constants behind the caller's non-return proof
            // alongside any additional facts proved for this copy's arguments.
            for (slot, fact) in plan.slots.iter_mut().zip(proof) {
                if fact.is_some() {
                    *slot = fact.clone();
                }
            }
        }
        plan
    }

    pub(super) fn into_stopping_proof(mut self) -> Option<Vec<Option<Word>>> {
        if self.may_return {
            return None;
        }
        for slot in &mut self.slots {
            if slot.as_deref().and_then(definite).is_none() {
                *slot = None;
            }
        }
        Some(self.slots)
    }

    pub(super) fn analyze(
        frame: &Frame<'a>,
        inputs: impl Iterator<Item = Option<Word>>,
        call_result: impl FnMut(FuncId, &[Option<Word>]) -> CallResult,
    ) -> Self {
        let (inputs, _) = normalize_arguments(frame, inputs);
        Self::solve(frame, &inputs, call_result)
    }

    fn with_arguments(frame: &Frame<'a>, inputs: &[Word]) -> Self {
        let function = frame.module.function(frame.callee);
        let mut slots = vec![None; function.slot_count()];
        let mut argument_bits = Vec::new();
        for (parameter, input) in function.params.iter().zip(inputs) {
            for (index, bit) in input.iter().enumerate() {
                if let Bit::Wire(wire, inverted) = *bit
                    && wire as usize == argument_bits.len()
                {
                    argument_bits.push(ArgumentBit { slot: parameter.slot, index, inverted });
                }
            }
            slots[parameter.slot.index()] = Some(input.clone());
        }

        Self { slots, argument_bits, ..Self::default() }
    }

    fn solve(
        frame: &Frame<'a>,
        inputs: &[Word],
        mut call_result: impl FnMut(FuncId, &[Option<Word>]) -> CallResult,
    ) -> Self {
        let function = frame.module.function(frame.callee);
        let Self { mut slots, argument_bits, .. } = Self::with_arguments(frame, inputs);
        // Immutable argument bits keep their identities. Other unknown bits get distinct
        // slot identities at block boundaries, where different loop visits can meet.
        let argument_wires = argument_bits.len() as Wire;
        let slot_width = Wire::from(Width::MAX_BITS);
        let scratch = argument_wires + slot_width * slots.len() as Wire;
        let unknown_bit =
            |slot: Slot, index| Bit::Wire(argument_wires + slot_width * Wire::from(slot.0) + index as Wire, false);
        let unknown_word = |slot| (0..frame.slot_width(slot)).map(|index| unknown_bit(slot, index)).collect();
        // Entry reachability grows with the value facts. For example, a loop can
        // first deliver a constant condition and later make both outcomes possible.
        let mut reachable = vec![false; function.blocks.len()];
        let mut switches = vec![Edges::default(); function.blocks.len()];
        let mut calls = Calls::new();
        let mut may_return = false;
        let mut returned = None;
        reachable[0] = true;
        loop {
            let mut changed = false;
            for (index, block) in function.blocks.iter().enumerate() {
                if !reachable[index] {
                    continue;
                }
                let mut circuit = Circuit::new(scratch);
                let mut local = BTreeMap::new();
                let read = |local: &BTreeMap<Slot, Word>, operand: &Operand| match operand {
                    Operand::Const(value) => Some(constant(value.value(), value.width().bits() as usize)),
                    Operand::Slot(slot) => local.get(slot).or(slots[slot.index()].as_ref()).cloned(),
                };
                let (instructions, mut term) = before_instruction_fault(block);
                for (at, inst) in instructions.iter().enumerate() {
                    let call = if let Inst::Call { callee, args, dst } = inst {
                        Some(CallSite {
                            target: None,
                            args,
                            argument_facts: Vec::new(),
                            dst: *dst,
                            candidates: BTreeSet::from([*callee]),
                            invalid_possible: false,
                        })
                    } else if let Inst::CallIndirect { signature, target, args, dst } = inst {
                        let Some(address) = read(&local, target) else {
                            // The address can acquire facts from a later predecessor.
                            term = None;
                            break;
                        };
                        let (candidates, invalid_possible) = indirect_targets(frame.module, signature, &address);
                        Some(CallSite {
                            target: Some(target),
                            args,
                            argument_facts: Vec::new(),
                            dst: *dst,
                            candidates,
                            invalid_possible,
                        })
                    } else {
                        None
                    };
                    if let Some(mut call) = call {
                        call.argument_facts = call.args.iter().map(|arg| read(&local, arg)).collect();
                        let site = record_call(&mut calls, (BlockId(index as u32), at), call);
                        let Some(word) =
                            fold_call(site.dst, &site.candidates, &site.argument_facts, &mut call_result, unknown_word)
                        else {
                            term = None;
                            break;
                        };
                        if let Some((slot, word)) = site.dst.zip(word) {
                            local.insert(slot, word);
                        }
                        continue;
                    }
                    let Some(result) = inst.result() else { continue };
                    if let Some(word) = fold_inst(inst, |op| read(&local, op), &mut circuit, || unknown_word(result)) {
                        local.insert(result, word);
                    }
                }
                // Read every incoming word before joining any update into the slot facts.
                let mut incoming = Vec::new();
                let mut transfer = |target: &bennett_ir::Target| {
                    let reached = &mut reachable[target.block.index()];
                    changed |= !*reached;
                    *reached = true;
                    for (param, operand) in function.block(target.block).params.iter().zip(&target.args) {
                        if let Some(word) = read(&local, operand) {
                            incoming.push((param.slot, word));
                        }
                    }
                };
                if let Some(Terminator::Branch { cond, if_true, if_false }) = term {
                    if let Some(word) = read(&local, cond) {
                        match word[0] {
                            Bit::Constant(choice) => transfer(if choice { if_true } else { if_false }),
                            Bit::Wire(..) | Bit::Shared { .. } => {
                                transfer(if_true);
                                transfer(if_false);
                            }
                        }
                    }
                } else if let Some(Terminator::Switch { value, cases, default, .. }) = term {
                    if let Some(word) = read(&local, value) {
                        let mut edges = Edges::new(cases, default);
                        edges.retain_possible(&word);
                        for case in &edges.cases {
                            transfer(&case.target);
                        }
                        if let Some(default) = edges.default {
                            transfer(default);
                        }
                        // Save the same edges for emission before the slot join
                        // forgets identities of locally computed selector bits.
                        switches[index] = edges;
                    }
                } else if let Some(term) = term {
                    // Non-local control is unsupported by emission. Treat it
                    // conservatively when summarizing potential callees.
                    may_return |= matches!(term, Terminator::Return(_) | Terminator::Longjmp { .. });
                    if let Terminator::Return(Some(value)) = term
                        && let Some(word) = read(&local, value)
                    {
                        let proof = word.into_iter().map(|bit| match bit {
                            Bit::Constant(_) => Some(bit),
                            Bit::Wire(wire, inverted) | Bit::Shared { wire, inverted, .. } => {
                                (wire < argument_wires).then_some(Bit::Wire(wire, inverted))
                            }
                        });
                        intersect_return(&mut returned, proof.collect());
                    }
                    term.visit_targets(transfer);
                }
                for (slot, mut word) in local.into_iter().chain(incoming) {
                    let old = &mut slots[slot.index()];
                    for (index, bit) in word.iter_mut().enumerate() {
                        let unknown = unknown_bit(slot, index);
                        if bit.wire().is_some_and(|(wire, _)| wire >= argument_wires)
                            || old.as_ref().is_some_and(|old| old[index] != *bit)
                        {
                            *bit = unknown;
                        }
                    }
                    changed |= old.as_ref() != Some(&word);
                    *old = Some(word);
                }
            }
            if !changed {
                return Self { slots, argument_bits, returned, switches, calls, may_return };
            }
        }
    }
}
