//! The state of an applied frame and the transition for one instruction.

use std::rc::Rc;

use bennett_ir::{BinOp, BlockId, Fault, FuncId, Inst, Operand, Slot, Target, Terminator, Type, ValidModule};

use super::ApplyError;
use super::arithmetic;
use super::bits::{Bit, Circuit, Word, ZERO, constant};
use super::definite::{Accesses, CallProofs, Constants};
use super::division;
use super::successors::{self, Edges};
use super::work::{self, Work};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Point {
    pub(super) copy: usize,
    pub(super) block: BlockId,
    pub(super) index: usize,
}

impl Point {
    pub(super) fn entry(block: BlockId) -> Self {
        Self { copy: 0, block, index: 0 }
    }

    pub(super) fn next(self) -> Self {
        Self { index: self.index + 1, ..self }
    }
}

pub(super) enum Status {
    Running(Point),
    Returned(Word),
    Faulted(Fault),
}

pub(super) struct IndirectCall<'a> {
    pub(super) address: Word,
    pub(super) args: &'a [Operand],
    pub(super) targets: Vec<(u64, usize)>,
    pub(super) invalid_possible: bool,
}

pub(super) enum Step<'a> {
    Write(Point, Slot, Word),
    Jump(&'a Target),
    Branch(Bit, &'a Target, &'a Target),
    Switch(Word, Edges<'a>),
    Indirect(IndirectCall<'a>),
    Return(Word),
    Transfer(Point, Vec<(Slot, Word)>),
    CheckedTransfer(Point, Vec<(Slot, Word)>, Bit, Vec<(Bit, Fault)>),
    Inactive,
    Fault(Fault),
}

impl<'a> Step<'a> {
    pub(super) fn checked(next: Point, writes: Vec<(Slot, Word)>, failed: Bit, fault: Fault) -> Self {
        match failed.literal() {
            Some(false) => Self::Transfer(next, writes),
            Some(true) => Self::Fault(fault),
            None => Self::CheckedTransfer(next, writes, failed.not(), vec![(failed, fault)]),
        }
    }
}

pub(super) struct Frame<'a> {
    pub(super) module: &'a ValidModule,
    pub(super) callee: FuncId,
    constants: Rc<Constants<'a>>,
    structural_accesses: Accesses,
}

impl<'a> Frame<'a> {
    pub(super) fn slot_width(&self, slot: Slot) -> usize {
        match self.module.slot_type(self.callee, slot) {
            Type::Int(width) => width.bits() as usize,
            Type::Qreg(_) => u64::BITS as usize,
        }
    }

    pub(super) fn initial(&self, args: impl IntoIterator<Item = Word>) -> Vec<Word> {
        let function = self.module.function(self.callee);
        let mut slots =
            (0..function.slot_count()).map(|index| vec![ZERO; self.slot_width(Slot(index as u32))]).collect::<Vec<_>>();
        for (param, value) in function.params.iter().zip(args) {
            slots[param.slot.index()] = value;
        }
        slots
    }

    pub(super) fn read(slots: &[Word], operand: &Operand) -> Word {
        work::add(
            Work::OperandBits,
            match operand {
                Operand::Const(value) => value.width().bits() as usize,
                Operand::Slot(slot) => slots[slot.index()].len(),
            },
        );
        match operand {
            Operand::Const(value) => constant(value.value(), value.width().bits() as usize),
            Operand::Slot(slot) => slots[slot.index()].clone(),
        }
    }

    pub(super) fn accesses(&self) -> Accesses {
        self.constants.accesses().unwrap_or(self.structural_accesses)
    }

    pub(super) fn visit_flow_targets(&self, block: BlockId, mut visit: impl FnMut(&'a Target)) {
        if let Some(targets) = self.constants.outgoing(block) {
            work::add(Work::FlowEdges, targets.len());
            targets.iter().copied().for_each(visit);
        } else if let Some(term) = successors::block_end(self.module.function(self.callee).block(block)) {
            successors::visit_possible_targets(term, &mut visit);
        }
    }

    pub(super) fn definite_stores(&self) -> bool {
        self.constants.definite_stores()
    }

    pub(super) fn operand(&self, slots: &[Word], operand: &Operand) -> Word {
        self.constants.project(slots, operand, Self::read(slots, operand))
    }

    pub(super) fn incoming(&self, slots: &[Word], target: &Target) -> Vec<(Slot, Word)> {
        self.module
            .function(self.callee)
            .block(target.block)
            .params
            .iter()
            .zip(&target.args)
            .map(|(param, argument)| (param.slot, Self::read(slots, argument)))
            .collect()
    }

    pub(super) fn error(&self, point: Point, operation: &'static str, route: &'static str) -> ApplyError {
        ApplyError::Unsupported { callee: self.callee, block: point.block, index: point.index, operation, route }
    }

    pub(super) fn plan(&self) -> Rc<Constants<'a>> {
        Rc::clone(&self.constants)
    }

    pub(super) fn for_analysis(module: &'a ValidModule, callee: FuncId) -> Self {
        Self { module, callee, constants: Rc::default(), structural_accesses: Accesses::default() }
    }

    pub(super) fn check(
        module: &'a ValidModule,
        callee: FuncId,
        input: impl Iterator<Item = Option<Word>>,
        proofs: &mut CallProofs<'a>,
    ) -> Result<Self, ApplyError> {
        let mut frame = Self::for_analysis(module, callee);
        let (needs_analysis, accesses) = frame.check_instructions()?;
        frame.structural_accesses = accesses;
        frame.constants = Constants::for_emission(&frame, input, proofs, needs_analysis);
        Ok(frame)
    }

    fn check_instructions(&self) -> Result<(bool, Accesses), ApplyError> {
        let mut needs_analysis = false;
        let mut accesses = Accesses::default();
        for (index, block) in self.module.function(self.callee).blocks.iter().enumerate() {
            let block_id = BlockId(index as u32);
            needs_analysis |= matches!(block.term, Terminator::Switch { .. });
            for (index, inst) in block.insts.iter().enumerate() {
                work::add(Work::StructuralCells, 1);
                accesses.observe(inst);
                needs_analysis |= matches!(
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
                        | Inst::Store { address: Operand::Slot(_), .. }
                );
                if let Some((operation, route)) = unsupported(inst) {
                    return Err(self.error(Point { block: block_id, index, copy: 0 }, operation, route));
                }
            }
            work::add(Work::StructuralCells, 1);
            let (operation, route) = match &block.term {
                Terminator::Jump(_)
                | Terminator::Branch { .. }
                | Terminator::Switch { .. }
                | Terminator::Return(_)
                | Terminator::Exit(_)
                | Terminator::Fault(_) => continue,
                Terminator::Longjmp { .. } => ("longjmp", "non-local control and memory"),
            };
            return Err(self.error(Point { block: block_id, index: block.insts.len(), copy: 0 }, operation, route));
        }
        Ok((needs_analysis, accesses))
    }

    pub(super) fn step(
        &self,
        circuit: &mut Circuit,
        read: impl Fn(&Operand) -> Word,
        point: Point,
        widths: &[Option<usize>],
    ) -> Result<Step<'a>, ApplyError> {
        work::add(Work::Operations, 1);
        let block = self.module.function(self.callee).block(point.block);
        let result_width = |dst: Slot, declared| widths.get(dst.index()).copied().flatten().unwrap_or(declared);
        if let Some(inst) = block.insts.get(point.index) {
            if let Some(fault) = successors::instruction_fault(inst) {
                return Ok(Step::Fault(fault));
            }
            let (dst, result) = match inst {
                Inst::Binary {
                    op: op @ (BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem),
                    dst,
                    lhs,
                    rhs,
                    ..
                } => {
                    let dividend = read(lhs);
                    let divisor = read(rhs);
                    let zero = division::is_zero(circuit, &divisor);
                    if zero.is_one() {
                        return Ok(Step::Fault(Fault::DivideByZero));
                    }
                    let result = division::calculate(circuit, *op, &dividend, &divisor);
                    if !zero.is_zero() {
                        return Ok(Step::checked(point.next(), vec![(*dst, result)], zero, Fault::DivideByZero));
                    }
                    (*dst, result)
                }
                Inst::Binary { op, dst, lhs, rhs, .. } => {
                    let mut left = read(lhs);
                    let mut right = read(rhs);
                    let declared = left.len();
                    // Low bits of these operations never depend on higher input bits.
                    if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::And | BinOp::Or | BinOp::Xor) {
                        let width = result_width(*dst, declared);
                        left.truncate(width);
                        right.truncate(width);
                    }
                    let mut result =
                        arithmetic::binary(circuit, *op, &left, &right).expect("checked operands before lowering");
                    result.resize(declared, ZERO);
                    (*dst, result)
                }
                Inst::Compare { pred, dst, lhs, rhs, .. } => {
                    let left = read(lhs);
                    let right = read(rhs);
                    (*dst, vec![arithmetic::compare(circuit, *pred, &left, &right)])
                }
                Inst::Select { dst, cond, if_true, if_false, .. } => {
                    let condition = read(cond)[0];
                    let left = read(if_true);
                    let declared = left.len();
                    let mut result: Word = left
                        .into_iter()
                        .zip(read(if_false))
                        .take(result_width(*dst, declared))
                        .map(|(a, b)| circuit.select(condition, a, b))
                        .collect();
                    result.resize(declared, ZERO);
                    (*dst, result)
                }
                Inst::Cast { op, dst, value, to, .. } => (*dst, arithmetic::cast(*op, read(value), to.bits() as usize)),
                Inst::FuncAddr { dst, func } => (*dst, constant(u128::from(func.address()), u64::BITS as usize)),
                Inst::Call { .. } => return Err(self.error(point, "call", "a planned call copy")),
                Inst::CallIndirect { .. } => return Err(self.error(point, "call_indirect", "a planned call copy")),
                Inst::Load { .. } => return Err(self.error(point, "load", super::memory::DISPATCH_REQUIRED)),
                Inst::Store { .. } => return Err(self.error(point, "store", super::memory::DISPATCH_REQUIRED)),
                Inst::Alloca { .. } => return Err(self.error(point, "alloca", super::memory::DISPATCH_REQUIRED)),
                Inst::StackSave { .. } => return Err(self.error(point, "stacksave", super::memory::DISPATCH_REQUIRED)),
                Inst::StackRestore { .. } => {
                    return Err(self.error(point, "stackrestore", super::memory::DISPATCH_REQUIRED));
                }
                Inst::Setjmp { .. } | Inst::ReturnAddress { .. } => {
                    unreachable!("checked instructions before lowering")
                }
                Inst::Effect { .. }
                | Inst::Qalloc { .. }
                | Inst::QallocBy { .. }
                | Inst::Qfree { .. }
                | Inst::Gate { .. }
                | Inst::GateAt { .. }
                | Inst::RotateBy { .. }
                | Inst::Measure { .. }
                | Inst::Apply { .. } => unreachable!("unconditional fault handled before lowering"),
            };
            return Ok(Step::Write(point.next(), dst, result));
        }
        Ok(match &block.term {
            Terminator::Jump(target) => Step::Jump(target),
            Terminator::Branch { cond, if_true, if_false } => {
                let bit = read(cond)[0];
                match successors::branch(bit, if_true, if_false) {
                    [Some(target), None] | [None, Some(target)] => Step::Jump(target),
                    [Some(yes), Some(no)] => Step::Branch(bit, yes, no),
                    [None, None] => unreachable!("a Boolean value permits at least one successor"),
                }
            }
            Terminator::Switch { value, .. } => {
                let selector = read(value);
                let mut edges = self.constants.switch_edges(point.block).clone();
                edges.retain_possible(&selector);
                if let ([], Some(default)) = (edges.cases.as_slice(), edges.default) {
                    Step::Jump(default)
                } else if let ([case], None) = (edges.cases.as_slice(), edges.default) {
                    Step::Jump(&case.target)
                } else {
                    Step::Switch(selector, edges)
                }
            }
            Terminator::Return(Some(value)) => Step::Return(read(value)),
            Terminator::Exit(_) => Step::Fault(Fault::ExitInRegion),
            Terminator::Fault(code) => Step::Fault(*code),
            Terminator::Return(None) | Terminator::Longjmp { .. } => {
                unreachable!("checked the terminators before lowering")
            }
        })
    }
}

fn unsupported(inst: &Inst) -> Option<(&'static str, &'static str)> {
    Some(match inst {
        Inst::Binary { .. } | Inst::Compare { .. } | Inst::Select { .. } | Inst::Cast { .. } => return None,
        Inst::Load { .. } | Inst::Store { .. } => return None,
        Inst::Alloca { .. } | Inst::StackSave { .. } | Inst::StackRestore { .. } => return None,
        Inst::Setjmp { .. } => ("setjmp", "non-local control and memory"),
        Inst::ReturnAddress { .. } => ("retaddr", "applied call frames"),
        Inst::Call { .. } | Inst::CallIndirect { .. } | Inst::FuncAddr { .. } => return None,
        Inst::Effect { .. }
        | Inst::Qalloc { .. }
        | Inst::QallocBy { .. }
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::RotateBy { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => return None,
    })
}
