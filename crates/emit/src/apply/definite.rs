//! Prove constants for the operations that need them, independently of the step budget.

use std::collections::BTreeMap;

use bennett_exec::Wire;
use bennett_ir::{BinOp, Inst, Operand, Slot, Terminator, Width};

use super::arithmetic;
use super::bits::{Bit, Circuit, Word, constant, definite};
use super::symbolic::{Frame, switch_target};
use super::{ApplyInput, Argument};

fn fold_inst(inst: &Inst, read: impl Fn(&Operand) -> Option<Word>, circuit: &mut Circuit) -> Option<Word> {
    match inst {
        Inst::Binary { op, lhs, rhs, .. } => arithmetic::binary(circuit, *op, &read(lhs)?, &read(rhs)?),
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
        Inst::Load { .. }
        | Inst::Store { .. }
        | Inst::Alloca { .. }
        | Inst::StackSave { .. }
        | Inst::StackRestore { .. }
        | Inst::Setjmp { .. }
        | Inst::ReturnAddress { .. }
        | Inst::Call { .. }
        | Inst::CallIndirect { .. }
        | Inst::FuncAddr { .. }
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
    let Inst::Binary { op, lhs, rhs, .. } = inst else { return None };
    match op {
        BinOp::Mul if !literal(lhs) && !literal(rhs) => Some((*op, "a proved constant multiplication factor")),
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
pub(super) enum Edges {
    Selected,
    All,
}

#[derive(Default)]
pub(super) struct Constants(Vec<Option<Word>>);

impl Constants {
    pub(super) fn get(&self, operand: &Operand) -> Option<u128> {
        match operand {
            Operand::Const(value) => Some(value.value()),
            Operand::Slot(slot) => definite(self.0.get(slot.index())?.as_ref()?),
        }
    }

    pub(super) fn add_fallback(&mut self, fallback: Self) {
        // A fact proved over every edge holds on the selected edges as well.
        for (current, candidate) in self.0.iter_mut().zip(fallback.0) {
            if current.as_deref().and_then(definite).is_none() && candidate.as_deref().and_then(definite).is_some() {
                *current = candidate;
            }
        }
    }

    pub(super) fn analyze(frame: &Frame<'_>, input: &ApplyInput<'_>, edges: Edges) -> Self {
        let function = frame.module.function(frame.callee);
        if !function.blocks.iter().any(|block| {
            matches!(&block.term, Terminator::Switch { value: Operand::Slot(_), .. })
                || block.insts.iter().any(|inst| requirement(inst).is_some())
        }) {
            return Self::default();
        }
        let mut slots = vec![None; function.slot_count()];
        let mut arguments = BTreeMap::new();
        for (parameter, argument) in function.params.iter().zip(input.args) {
            let bennett_ir::Type::Int(width) = parameter.ty else { unreachable!("checked the circuit interface") };
            let word = match argument {
                Argument::Value(value) => constant(*value, width.bits() as usize),
                Argument::Register(wires) => wires
                    .clone()
                    .map(|wire| {
                        let next = arguments.len() as Wire;
                        Bit::Wire(*arguments.entry(wire).or_insert(next), false)
                    })
                    .collect(),
            };
            slots[parameter.slot.index()] = Some(word);
        }

        // Immutable argument bits keep their identities. Other unknown bits get distinct
        // slot identities at block boundaries, where different loop visits can meet.
        let argument_wires = arguments.len() as Wire;
        let slot_width = Wire::from(Width::MAX_BITS);
        let scratch = argument_wires + slot_width * slots.len() as Wire;
        loop {
            let mut changed = false;
            for block in &function.blocks {
                let mut circuit = Circuit::new(scratch);
                let mut local = BTreeMap::new();
                let read = |local: &BTreeMap<Slot, Word>, operand: &Operand| match operand {
                    Operand::Const(value) => Some(constant(value.value(), value.width().bits() as usize)),
                    Operand::Slot(slot) => local.get(slot).or(slots[slot.index()].as_ref()).cloned(),
                };
                for inst in &block.insts {
                    if let Some(word) = fold_inst(inst, |op| read(&local, op), &mut circuit) {
                        local.insert(inst.result().expect("a supported instruction produces a value"), word);
                    }
                }
                // Read every incoming word before joining any update into the slot facts.
                let mut incoming = Vec::new();
                let mut transfer = |target: &bennett_ir::Target| {
                    for (param, operand) in function.block(target.block).params.iter().zip(&target.args) {
                        if let Some(word) = read(&local, operand) {
                            incoming.push((param.slot, word));
                        }
                    }
                };
                if edges == Edges::All {
                    block.term.visit_targets(transfer);
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
                        let unknown =
                            Bit::Wire(argument_wires + slot_width * Wire::from(slot.0) + index as Wire, false);
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
                return Self(slots);
            }
        }
    }
}
