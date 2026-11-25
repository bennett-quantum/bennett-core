//! Prove constants for the operations that need them, independently of the step budget.

use std::collections::BTreeMap;

use bennett_exec::Wire;
use bennett_ir::{BinOp, BlockId, Inst, Operand, Slot, Terminator, Width};

use super::bits::{Bit, Circuit, Word, ZERO, constant, definite};
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
        | Inst::StackSave { .. } => Some(unknown()),
        Inst::Store { .. } | Inst::StackRestore { .. } => None,
        Inst::Setjmp { .. }
        | Inst::ReturnAddress { .. }
        | Inst::Effect { .. }
        | Inst::Qalloc { .. }
        | Inst::QallocBy { .. }
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::RotateBy { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => unreachable!("checked the supported instructions"),
    }
}

#[derive(Default)]
pub(super) struct Constants<'a> {
    slots: Vec<Option<Word>>,
    reachable: Vec<bool>,
    switches: Vec<Edges<'a>>,
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

    pub(super) fn reaches(&self, block: BlockId) -> bool {
        self.reachable.get(block.index()).copied().unwrap_or(false)
    }

    pub(super) fn switch_edges(&self, block: BlockId) -> &Edges<'a> {
        &self.switches[block.index()]
    }

    pub(super) fn analyze(frame: &Frame<'a>, inputs: impl Iterator<Item = Option<Word>>) -> Self {
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
            return Self::default();
        }
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
                let (instructions, term) = before_instruction_fault(block);
                for inst in instructions {
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
                return Self { slots, reachable, switches };
            }
        }
    }
}
