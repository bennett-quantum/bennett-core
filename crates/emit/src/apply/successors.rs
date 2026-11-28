//! Successors allowed by operand facts. Dispatch supplies its current circuit values.

use std::collections::BTreeSet;

use bennett_ir::{Block, Fault, FuncId, Inst, Operand, Signature, SwitchCase, Target, Terminator, ValidModule};

use super::bits::{Bit, Word, definite};
use super::selectors::Selector;
use super::work::{self, Work};

#[derive(Clone, Default)]
pub(super) struct Edges<'a> {
    pub(super) cases: Vec<&'a SwitchCase>,
    pub(super) default: Option<&'a Target>,
}

impl<'a> Edges<'a> {
    pub(super) fn new(cases: &'a [SwitchCase], default: &'a Target) -> Self {
        Self { cases: cases.iter().collect(), default: Some(default) }
    }

    pub(super) fn retain_possible(&mut self, selector: &[Bit]) {
        let facts = Selector::new(selector);
        self.cases.retain(|case| facts.allows(case.value));
        if facts.covered_by(self.cases.len()) {
            self.default = None;
        }
    }
}

pub(super) fn branch<'a>(bit: Bit, yes: &'a Target, no: &'a Target) -> [Option<&'a Target>; 2] {
    work::add(Work::BranchTests, 1);
    [(!bit.is_zero()).then_some(yes), (!bit.is_one()).then_some(no)]
}

pub(super) fn terminator<'a>(
    term: &'a Terminator,
    mut read: impl FnMut(&Operand) -> Option<Word>,
    mut visit: impl FnMut(&'a Target),
) -> Option<Edges<'a>> {
    let mut reached = |target| {
        work::add(Work::Successors, 1);
        visit(target);
    };
    match term {
        Terminator::Branch { cond, if_true, if_false } => {
            if let Some(word) = read(cond) {
                branch(word[0], if_true, if_false).into_iter().flatten().for_each(reached);
            }
        }
        Terminator::Switch { value, cases, default, .. } => {
            let word = read(value)?;
            let mut edges = Edges::new(cases, default);
            edges.retain_possible(&word);
            for case in &edges.cases {
                reached(&case.target);
            }
            if let Some(target) = edges.default {
                reached(target);
            }
            return Some(edges);
        }
        Terminator::Jump(_)
        | Terminator::Return(_)
        | Terminator::Exit(_)
        | Terminator::Fault(_)
        | Terminator::Longjmp { .. } => term.visit_targets(reached),
    }
    None
}

pub(super) fn instruction_fault(inst: &Inst) -> Option<Fault> {
    match inst {
        Inst::Effect { .. } => Some(Fault::EffectInRegion),
        Inst::Qalloc { .. }
        | Inst::QallocBy { .. }
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::RotateBy { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => Some(Fault::QuantumInRegion),
        Inst::Binary { .. }
        | Inst::Compare { .. }
        | Inst::Select { .. }
        | Inst::Cast { .. }
        | Inst::Load { .. }
        | Inst::Store { .. }
        | Inst::StackSave { .. }
        | Inst::StackRestore { .. }
        | Inst::Alloca { .. }
        | Inst::Setjmp { .. }
        | Inst::ReturnAddress { .. }
        | Inst::Call { .. }
        | Inst::CallIndirect { .. }
        | Inst::FuncAddr { .. } => None,
    }
}

pub(super) fn block_end(block: &Block) -> Option<&Terminator> {
    (!block.insts.iter().any(|inst| {
        work::add(Work::FaultScans, 1);
        instruction_fault(inst).is_some()
    }))
    .then_some(&block.term)
}

pub(super) fn visit_possible_targets<'a>(term: &'a Terminator, mut visit: impl FnMut(&'a Target)) {
    let mut reached = |target| {
        work::add(Work::Successors, 1);
        visit(target);
    };
    match term {
        Terminator::Branch { cond: Operand::Const(value), if_true, if_false } => {
            branch(Bit::Constant(value.value() != 0), if_true, if_false).into_iter().flatten().for_each(reached);
        }
        Terminator::Switch { value: Operand::Const(value), cases, default, .. } => {
            let target = cases
                .iter()
                .find(|case| {
                    work::add(Work::TargetChecks, 1);
                    case.value == value.value()
                })
                .map_or(default, |case| &case.target);
            reached(target);
        }
        Terminator::Jump(_)
        | Terminator::Branch { .. }
        | Terminator::Switch { .. }
        | Terminator::Return(_)
        | Terminator::Exit(_)
        | Terminator::Fault(_)
        | Terminator::Longjmp { .. } => {
            term.visit_targets(reached);
        }
    }
}

pub(super) fn indirect_targets(
    module: &ValidModule,
    signature: &Signature,
    address: &[Bit],
) -> (BTreeSet<FuncId>, bool) {
    if let Some(value) = definite(address) {
        work::add(Work::LiteralTargetBits, address.len());
        work::add(Work::TargetChecks, 1);
        let callee =
            module.function_at(value as u64).filter(|&callee| module.function(callee).has_signature(signature));
        return (callee.into_iter().collect(), callee.is_none());
    }

    let (targets, invalid) = dispatch(
        address,
        true,
        module.functions.iter().enumerate().filter_map(|(index, function)| {
            work::add(Work::TargetChecks, 1);
            let callee = FuncId(index as u32);
            function.has_signature(signature).then_some((callee.address(), callee))
        }),
    );
    (targets.into_iter().map(|(_, callee)| callee).collect(), invalid)
}

pub(super) fn dispatch<T>(
    address: &[Bit],
    may_be_invalid: bool,
    candidates: impl IntoIterator<Item = (u64, T)>,
) -> (Vec<(u64, T)>, bool) {
    let selector = Selector::new(address);
    let targets: Vec<_> = candidates.into_iter().filter(|&(value, _)| selector.allows(u128::from(value))).collect();
    let invalid = may_be_invalid && !selector.covered_by(targets.len());
    (targets, invalid)
}
