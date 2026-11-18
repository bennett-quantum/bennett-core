//! Prove constants for the operations that need them, independently of the step budget.

use std::collections::BTreeMap;

use bennett_exec::Wire;
use bennett_ir::{BinOp, BlockId, Inst, Operand, Slot, Terminator, Width};

use super::arithmetic;
use super::bits::{Bit, Circuit, Word, constant, definite};
use super::symbolic::{Frame, switch_target, visit_possible_targets};

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
            Some(arithmetic::fold_binary(circuit, *op, &read(lhs)?, &read(rhs)?).unwrap_or_else(unknown))
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
        Inst::Call { .. } | Inst::CallIndirect { .. } => Some(unknown()),
        Inst::Load { .. }
        | Inst::Store { .. }
        | Inst::Alloca { .. }
        | Inst::StackSave { .. }
        | Inst::StackRestore { .. }
        | Inst::Setjmp { .. }
        | Inst::ReturnAddress { .. }
        | Inst::Effect { .. }
        | Inst::Qalloc { .. }
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => unreachable!("checked the supported instructions"),
    }
}

/// The required constant inputs for a shift or multiply. A literal needs no data-flow proof.
pub(super) fn requirement(inst: &Inst) -> Option<(BinOp, &'static str)> {
    let Inst::Binary { op, rhs, .. } = inst else { return None };
    match op {
        BinOp::Shl | BinOp::Lshr | BinOp::Ashr if !literal(rhs) => Some((*op, "a proved constant shift amount")),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::And
        | BinOp::Xor
        | BinOp::Shl
        | BinOp::Lshr
        | BinOp::Ashr
        | BinOp::Udiv
        | BinOp::Sdiv
        | BinOp::Urem
        | BinOp::Srem
        | BinOp::Or => None,
    }
}

fn literal(operand: &Operand) -> bool {
    matches!(operand, Operand::Const(_))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    Reachable,
    LiteralEdges,
    EveryBlock,
}

#[derive(Default)]
pub(super) struct Constants {
    slots: Vec<Option<Word>>,
    reachable: Vec<bool>,
}

impl Constants {
    pub(super) fn get(&self, operand: &Operand) -> Option<u128> {
        match operand {
            Operand::Const(value) => Some(value.value()),
            Operand::Slot(slot) => definite(self.slots.get(slot.index())?.as_ref()?),
        }
    }

    pub(super) fn reaches(&self, block: BlockId) -> bool {
        self.reachable.get(block.index()).copied().unwrap_or(false)
    }

    pub(super) fn analyze(frame: &Frame<'_>, inputs: &[Word], scope: Scope) -> Self {
        let function = frame.module.function(frame.callee);
        if !function.blocks.iter().any(|block| {
            matches!(&block.term, Terminator::Switch { value: Operand::Slot(_), .. })
                || block.insts.iter().any(|inst| {
                    requirement(inst).is_some()
                        || matches!(
                            inst,
                            Inst::Call { .. }
                                | Inst::CallIndirect { .. }
                                | Inst::Binary { op: BinOp::Mul, lhs: Operand::Slot(_), rhs: Operand::Slot(_), .. }
                        )
                })
        }) {
            return Self::default();
        }
        let mut slots = vec![None; function.slot_count()];
        let mut arguments = BTreeMap::new();
        for (parameter, input) in function.params.iter().zip(inputs) {
            let word = input
                .iter()
                .map(|&bit| match bit {
                    Bit::Constant(_) => bit,
                    Bit::Wire(wire, inverted) => {
                        let next = arguments.len() as Wire;
                        Bit::Wire(*arguments.entry(wire).or_insert(next), inverted)
                    }
                })
                .collect();
            slots[parameter.slot.index()] = Some(word);
        }

        // Immutable argument bits keep their identities. Other unknown bits get distinct
        // slot identities at block boundaries, where different loop visits can meet.
        let argument_wires = arguments.len() as Wire;
        let slot_width = Wire::from(Width::MAX_BITS);
        let scratch = argument_wires + slot_width * slots.len() as Wire;
        let unknown_bit =
            |slot: Slot, index| Bit::Wire(argument_wires + slot_width * Wire::from(slot.0) + index as Wire, false);
        let unknown_word = |slot| (0..frame.slot_width(slot)).map(|index| unknown_bit(slot, index)).collect();
        // Reachability grows with the value facts. For example, a loop can
        // first deliver a constant condition and later make both outcomes possible.
        let every_block = scope != Scope::Reachable;
        let mut reachable = vec![every_block; function.blocks.len()];
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
                for inst in &block.insts {
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
                if scope == Scope::EveryBlock {
                    block.term.visit_targets(transfer);
                } else if scope == Scope::LiteralEdges {
                    visit_possible_targets(&block.term, transfer);
                } else if let Terminator::Branch { cond, if_true, if_false } = &block.term {
                    if let Some(word) = read(&local, cond) {
                        match word[0] {
                            Bit::Constant(choice) => transfer(if choice { if_true } else { if_false }),
                            Bit::Wire(..) => {
                                transfer(if_true);
                                transfer(if_false);
                            }
                        }
                    }
                } else if let Terminator::Switch { value, cases, default, .. } = &block.term {
                    if let Some(word) = read(&local, value) {
                        if let Some(value) = definite(&word) {
                            transfer(switch_target(value, cases, default));
                        } else {
                            block.term.visit_targets(transfer);
                        }
                    }
                } else {
                    block.term.visit_targets(transfer);
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
            if !changed && every_block {
                // A cold cycle may have no entry value to start propagation. Give
                // its remaining slots unknown values, then propagate those inputs
                // before accepting any proof from this pass.
                for (index, word) in slots.iter_mut().enumerate() {
                    if word.is_none() {
                        *word = Some(unknown_word(Slot(index as u32)));
                        changed = true;
                    }
                }
            }
            if !changed {
                return Self { slots, reachable };
            }
        }
    }
}
