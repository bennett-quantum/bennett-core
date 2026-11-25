//! The state of an applied frame and the transition for one instruction.

use bennett_ir::{
    BinOp, Block, BlockId, Fault, FuncId, Inst, Operand, Slot, SwitchCase, Target, Terminator, Type, ValidModule,
};

use super::ApplyError;
use super::arithmetic;
use super::bits::{Bit, Circuit, Word, ZERO, constant};
use super::definite::Constants;
use super::division;
use super::switches::Edges;

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
        match failed {
            ZERO => Self::Transfer(next, writes),
            super::bits::ONE => Self::Fault(fault),
            Bit::Wire(..) => Self::CheckedTransfer(next, writes, failed.not(), vec![(failed, fault)]),
        }
    }
}

pub(super) struct Frame<'a> {
    pub(super) module: &'a ValidModule,
    pub(super) callee: FuncId,
    constants: Constants<'a>,
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
        match operand {
            Operand::Const(value) => constant(value.value(), value.width().bits() as usize),
            Operand::Slot(slot) => slots[slot.index()].clone(),
        }
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

    pub(super) fn known(&self, operand: &Operand) -> Option<u128> {
        self.constants.get(operand)
    }

    pub(super) fn known_bits(&self, operand: &Operand) -> Option<Word> {
        self.constants.word(operand)
    }

    pub(super) fn reaches(&self, block: BlockId) -> bool {
        self.constants.reaches(block)
    }

    pub(super) fn check(
        module: &'a ValidModule,
        callee: FuncId,
        input: impl Iterator<Item = Option<Word>>,
    ) -> Result<Self, ApplyError> {
        let mut frame = Self { module, callee, constants: Constants::default() };
        frame.check_instructions()?;
        frame.constants = Constants::analyze(&frame, input);
        Ok(frame)
    }

    fn check_instructions(&self) -> Result<(), ApplyError> {
        for (index, block) in self.module.function(self.callee).blocks.iter().enumerate() {
            let block_id = BlockId(index as u32);
            for (index, inst) in block.insts.iter().enumerate() {
                if let Some((operation, route)) = unsupported(inst) {
                    return Err(self.error(Point { block: block_id, index, copy: 0 }, operation, route));
                }
            }
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
        Ok(())
    }

    pub(super) fn step(
        &self,
        circuit: &mut Circuit,
        slots: &[Word],
        point: Point,
        widths: &[Option<usize>],
    ) -> Result<Step<'a>, ApplyError> {
        let block = self.module.function(self.callee).block(point.block);
        let read = |operand| Self::read(slots, operand);
        let result_width = |dst: Slot, declared| widths.get(dst.index()).copied().flatten().unwrap_or(declared);
        if let Some(inst) = block.insts.get(point.index) {
            let (dst, result) = match inst {
                Inst::Binary {
                    op: op @ (BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem),
                    dst,
                    lhs,
                    rhs,
                    ..
                } => {
                    let dividend = read(lhs);
                    let mut divisor = read(rhs);
                    if let Some(value) = self.constants.get(rhs) {
                        divisor = constant(value, divisor.len());
                    }
                    let zero = division::is_zero(circuit, &divisor);
                    if zero == super::bits::ONE {
                        return Ok(Step::Fault(Fault::DivideByZero));
                    }
                    let result = division::calculate(circuit, *op, &dividend, &divisor);
                    if zero != ZERO {
                        return Ok(Step::checked(point.next(), vec![(*dst, result)], zero, Fault::DivideByZero));
                    }
                    (*dst, result)
                }
                Inst::Binary { op, dst, lhs, rhs, .. } => {
                    let mut left = read(lhs);
                    let mut right = read(rhs);
                    if matches!(op, BinOp::Mul | BinOp::Shl | BinOp::Lshr | BinOp::Ashr) {
                        // A slot can contain other values when this point is inactive.
                        // Use the proven value for each input that executes the instruction.
                        if let Some(value) = self.constants.get(rhs) {
                            right = constant(value, right.len());
                        }
                        if *op == BinOp::Mul
                            && let Some(value) = self.constants.get(lhs)
                        {
                            left = constant(value, left.len());
                        }
                    }
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
                    (*dst, vec![arithmetic::compare(circuit, *pred, &read(lhs), &read(rhs))])
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
                Inst::Effect { .. } => return Ok(Step::Fault(Fault::EffectInRegion)),
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
                Inst::Qalloc { .. }
                | Inst::Qfree { .. }
                | Inst::Gate { .. }
                | Inst::GateAt { .. }
                | Inst::Measure { .. }
                | Inst::Apply { .. } => return Ok(Step::Fault(Fault::QuantumInRegion)),
            };
            return Ok(Step::Write(point.next(), dst, result));
        }
        Ok(match &block.term {
            Terminator::Jump(target) => Step::Jump(target),
            Terminator::Branch { cond, if_true, if_false } => {
                // A proved condition holds for every input that reaches this point,
                // even when an inactive input has a different value in its slot.
                let condition =
                    self.constants.get(cond).map_or_else(|| read(cond)[0], |value| Bit::Constant(value == 1));
                match condition {
                    Bit::Constant(true) => Step::Jump(if_true),
                    Bit::Constant(false) => Step::Jump(if_false),
                    bit @ Bit::Wire(..) => Step::Branch(bit, if_true, if_false),
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

// Call planning and constant analysis stop before the first unconditional
// instruction fault. The preflight check still validates the entire copy.
pub(super) fn before_instruction_fault(block: &Block) -> (&[Inst], Option<&Terminator>) {
    let fault = block.insts.iter().position(|inst| {
        matches!(
            inst,
            Inst::Effect { .. }
                | Inst::Qalloc { .. }
                | Inst::Qfree { .. }
                | Inst::Gate { .. }
                | Inst::GateAt { .. }
                | Inst::Measure { .. }
                | Inst::Apply { .. }
        )
    });
    match fault {
        Some(index) => (&block.insts[..index], None),
        None => (&block.insts, Some(&block.term)),
    }
}

fn switch_target<'a>(value: u128, cases: &'a [SwitchCase], default: &'a Target) -> &'a Target {
    cases.iter().find(|case| case.value == value).map_or(default, |case| &case.target)
}

pub(super) fn visit_possible_targets<'a>(term: &'a Terminator, mut visit: impl FnMut(&'a Target)) {
    if let Terminator::Branch { cond: Operand::Const(value), if_true, if_false } = term {
        visit(if value.value() == 1 { if_true } else { if_false });
    } else if let Terminator::Switch { value: Operand::Const(value), cases, default, .. } = term {
        visit(switch_target(value.value(), cases, default));
    } else {
        term.visit_targets(visit);
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
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => return None,
    })
}
