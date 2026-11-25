//! Prove constants for the operations that need them, independently of the step budget.

use std::collections::{BTreeMap, BTreeSet};

use bennett_exec::Wire;
use bennett_ir::{BinOp, BlockId, FuncId, Inst, Operand, Signature, Slot, Terminator, ValidModule, Width};

use super::bits::{Bit, Circuit, Word, ZERO, constant, definite};
use super::selectors::Selector;
use super::switches::Edges;
use super::symbolic::{Frame, before_instruction_fault};
use super::{arithmetic, division, ranges};

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
                } else if *op == BinOp::Urem && definite(&right) != Some(0) {
                    let width = division::result_bits(&left, &right);
                    word[width..].fill(ZERO);
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
            condition @ Bit::Wire(..) => Some(
                read(if_true)?
                    .into_iter()
                    .zip(read(if_false)?)
                    .map(|(yes, no)| circuit.select(condition, yes, no))
                    .collect(),
            ),
        },
        Inst::FuncAddr { func, .. } => Some(constant(u128::from(func.address()), u64::BITS as usize)),
        Inst::Call { .. }
        | Inst::CallIndirect { .. }
        | Inst::Load { .. }
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
        dst: call.dst,
        candidates: BTreeSet::new(),
        invalid_possible: false,
    });
    // Retain the possibilities from earlier visits as input facts change at joins.
    site.candidates.extend(call.candidates);
    site.invalid_possible |= call.invalid_possible;
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

#[derive(Default)]
pub(super) struct Constants<'a> {
    slots: Vec<Option<Word>>,
    switches: Vec<Edges<'a>>,
    pub(super) calls: Calls<'a>,
    may_return: bool,
}

impl<'a> Constants<'a> {
    pub(super) fn get(&self, operand: &Operand) -> Option<u128> {
        match operand {
            Operand::Const(value) => Some(value.value()),
            Operand::Slot(slot) => definite(self.slots.get(slot.index())?.as_ref()?),
        }
    }

    pub(super) fn word(&self, operand: &Operand) -> Option<Word> {
        match operand {
            Operand::Const(value) => Some(constant(value.value(), value.width().bits() as usize)),
            Operand::Slot(slot) => self.slots.get(slot.index())?.clone(),
        }
    }

    pub(super) fn switch_edges(&self, block: BlockId) -> &Edges<'a> {
        &self.switches[block.index()]
    }

    pub(super) fn for_emission(
        frame: &Frame<'a>,
        inputs: impl Iterator<Item = Option<Word>>,
        can_return: impl FnMut(FuncId) -> bool,
        stopping_proof: Option<&[Option<Word>]>,
    ) -> Self {
        let function = frame.module.function(frame.callee);
        if !function.blocks.iter().any(|block| {
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
        }) {
            // A caller may already have relied on these exact facts to omit
            // its continuation. Use them even in a frame that needs no local pass.
            return Self { slots: stopping_proof.unwrap_or_default().to_vec(), ..Self::default() };
        }
        let mut plan = Self::analyze(frame, inputs, can_return);
        if let Some(proof) = stopping_proof {
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
        mut can_return: impl FnMut(FuncId) -> bool,
    ) -> Self {
        let function = frame.module.function(frame.callee);
        let mut slots = vec![None; function.slot_count()];
        let mut arguments = BTreeMap::new();
        let mut next_argument_wire = 0;
        for (parameter, input) in function.params.iter().zip(inputs) {
            let mut fresh = || {
                let wire = next_argument_wire;
                next_argument_wire += 1;
                wire
            };
            let word = match input {
                Some(word) => word
                    .into_iter()
                    .map(|bit| match bit {
                        Bit::Constant(_) => bit,
                        Bit::Wire(wire, inverted) => {
                            Bit::Wire(*arguments.entry(wire).or_insert_with(&mut fresh), inverted)
                        }
                    })
                    .collect(),
                None => (0..frame.slot_width(parameter.slot)).map(|_| Bit::Wire(fresh(), false)).collect(),
            };
            slots[parameter.slot.index()] = Some(word);
        }

        // Immutable argument bits keep their identities. Other unknown bits get distinct
        // slot identities at block boundaries, where different loop visits can meet.
        let argument_wires = next_argument_wire;
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
                        Some(CallSite { target: Some(target), args, dst: *dst, candidates, invalid_possible })
                    } else {
                        None
                    };
                    if let Some(call) = call {
                        let site = record_call(&mut calls, (BlockId(index as u32), at), call);
                        if !site.candidates.iter().copied().any(&mut can_return) {
                            term = None;
                            break;
                        }
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
                            Bit::Wire(..) => {
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
                    term.visit_targets(transfer);
                }
                for (slot, mut word) in local.into_iter().chain(incoming) {
                    let old = &mut slots[slot.index()];
                    for (index, bit) in word.iter_mut().enumerate() {
                        let unknown = unknown_bit(slot, index);
                        if matches!(*bit, Bit::Wire(wire, _) if wire >= argument_wires)
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
                return Self { slots, switches, calls, may_return };
            }
        }
    }
}
