//! Evaluate reached operands on demand in the shared call solver.

use std::collections::BTreeMap;

use bennett_exec::Wire;
use bennett_ir::{BlockId, Inst, Operand, Slot};

use super::{Solver, Visit, fold_inst};
use crate::apply::bits::{Bit, Word, constant};
use crate::apply::work::{self, Work};

pub(super) enum Definition<'a> {
    Pending(&'a Inst),
    Ready(Option<Word>),
}

impl<'a> Solver<'a> {
    pub(super) fn define(&mut self, visit: &mut Visit<'a>, result: Slot, inst: &'a Inst) {
        work::add(Work::OperandDefinitions, 3);
        self.producers.insert(result, visit.block);
        visit.local.insert(result, Definition::Pending(inst));
        if self.readers.contains_key(&result) {
            self.evaluate(visit, result);
        }
    }

    pub(super) fn read(&mut self, visit: &mut Visit<'a>, operand: &Operand) -> Option<Word> {
        if let Operand::Slot(slot) = operand {
            self.evaluate(visit, *slot);
        }
        self.lookup(visit.block, &visit.local, operand)
    }

    // Evaluate only the producers requested by a reached consumer. Pure local
    // expressions share this visit's circuit so they can retain bit identities.
    fn evaluate(&mut self, visit: &mut Visit<'a>, slot: Slot) -> bool {
        work::add(Work::OperandDefinitions, 1);
        let inst = match visit.local.get(&slot) {
            Some(Definition::Pending(inst)) => *inst,
            Some(Definition::Ready(_)) => return true,
            None => return false,
        };
        let mut pending = vec![(slot, inst, false)];
        while let Some((slot, inst, ready)) = pending.pop() {
            if ready {
                let width = self.frame.slot_width(slot);
                let first = self.unknown_bit(slot, 0).wire().expect("abstract slot bit").0;
                let word = fold_inst(
                    inst,
                    |operand| self.lookup(visit.block, &visit.local, operand),
                    &mut visit.circuit,
                    || (0..width).map(|index| Bit::input(first + index as Wire)).collect(),
                );
                work::add(Work::OperandDefinitions, 1);
                visit.local.insert(slot, Definition::Ready(word));
                continue;
            }
            work::add(Work::OperandDefinitions, 1);
            if !matches!(visit.local.get(&slot), Some(Definition::Pending(_))) {
                continue;
            }
            pending.push((slot, inst, true));
            inst.visit_uses(|source| {
                work::add(Work::OperandDefinitions, 1);
                if let Some(Definition::Pending(producer)) = visit.local.get(&source) {
                    pending.push((source, *producer, false));
                }
            });
        }
        true
    }

    fn lookup(&mut self, block: BlockId, local: &BTreeMap<Slot, Definition<'a>>, operand: &Operand) -> Option<Word> {
        let word = match operand {
            Operand::Const(value) => Some(constant(value.value(), value.width().bits() as usize)),
            Operand::Slot(slot) => {
                work::add(Work::TableEntries, 1);
                match local.get(slot) {
                    Some(Definition::Ready(word)) => word.clone(),
                    Some(Definition::Pending(_)) => unreachable!("evaluated the local dependencies"),
                    None => self.external_fact(block, *slot).cloned(),
                }
            }
        };
        work::add(Work::OperandBits, word.as_ref().map_or(0, Vec::len));
        word
    }

    fn external_fact(&mut self, block: BlockId, slot: Slot) -> Option<&Word> {
        let readers = self.readers.entry(slot).or_default();
        let first_use = readers.is_empty();
        readers.insert(block);
        if first_use && self.plan.slots[slot.index()].is_none() {
            work::add(Work::OperandDefinitions, 1);
            if let Some(&producer) = self.producers.get(&slot) {
                self.enqueue(producer);
            }
        }
        self.plan.slots[slot.index()].as_ref()
    }

    pub(super) fn demand(&mut self, visit: &mut Visit<'a>, operand: &Operand) {
        if let Operand::Slot(slot) = operand
            && !self.evaluate(visit, *slot)
        {
            self.external_fact(visit.block, *slot);
        }
    }
}
