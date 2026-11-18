//! Prove one loop with fixed continuation and exit charges, then save its decisions.

use std::collections::BTreeSet;

use bennett_ir::{BinOp, BlockId, Fault, Inst, Operand, Target, Terminator};

use super::bits::{Circuit, ONE, Word, ZERO};
use super::symbolic::{Frame, Point, Step};
use super::{ApplyError, Copies};

enum End {
    Header,
    Return,
}

struct Path<'a> {
    start: &'a Target,
    charge: u64,
    end: End,
}

pub(super) struct Loop<'a> {
    header: Point,
    cond: &'a Operand,
    exit_when_true: bool,
    continuing: Path<'a>,
    exiting: Path<'a>,
}

impl<'a> Loop<'a> {
    pub(super) fn prove(frame: &Frame<'a>, header: Point) -> Option<Self> {
        let function = frame.module.function(frame.callee);
        let block = function.block(header.block);
        let Terminator::Branch { cond, if_true, if_false } = &block.term else {
            return None;
        };
        // The definite prefix reaches this test. With only this conditional in the function,
        // the prefix cannot reach another cycle and still reach the test.
        if function.blocks.iter().filter(|block| matches!(block.term, Terminator::Branch { .. })).count() != 1 {
            return None;
        }
        if !block.insts.iter().all(total) {
            return None;
        }
        let yes = Path::prove(frame, header, if_true)?;
        let no = Path::prove(frame, header, if_false)?;
        let (exit_when_true, continuing, exiting) = match (&yes.end, &no.end) {
            (End::Return, End::Header) => (true, no, yes),
            (End::Header, End::Return) => (false, yes, no),
            (End::Header, End::Header) | (End::Return, End::Return) => return None,
        };
        Some(Self { header, cond, exit_when_true, continuing, exiting })
    }

    pub(super) fn run(
        &self,
        circuit: &mut Circuit,
        frame: &Frame<'a>,
        slots: &mut [Word],
        mut remaining: u64,
    ) -> Result<(Copies, u64), ApplyError> {
        let mut active = ONE;
        let mut copies = Copies::default();
        let mut decisions = 0;
        // The first return must use one of these fixed-cost exit paths. If none fits for
        // an active input, its suffix is total and the first fault is the budget boundary.
        while remaining >= self.exiting.charge && active != ZERO {
            let condition = Frame::read(slots, self.cond)[0];
            let exit = if self.exit_when_true { condition } else { condition.not() };
            let returning = circuit.and(active, exit);
            if returning != ZERO {
                let mut exit_slots = slots.to_vec();
                let result =
                    self.follow(circuit, frame, &mut exit_slots, &self.exiting)?.expect("the proved exit path returns");
                copies.returned.push((returning, result));
            }
            active = circuit.and(active, exit.not());
            decisions += 1;
            let Some(after_continue) = remaining.checked_sub(self.continuing.charge) else {
                break;
            };
            if after_continue < self.exiting.charge || active == ZERO {
                break;
            }
            // This total computation can run on every definite value. The saved decision
            // ensures that returned inputs cannot contribute to a later result or fault.
            self.follow(circuit, frame, slots, &self.continuing)?;
            remaining = after_continue;
        }
        copies.faulted.push((active, Fault::Budget));
        Ok((copies, decisions))
    }

    fn follow(
        &self,
        circuit: &mut Circuit,
        frame: &Frame<'a>,
        slots: &mut [Word],
        path: &Path<'a>,
    ) -> Result<Option<Word>, ApplyError> {
        let mut point = frame.jump(slots, path.start);
        while point != self.header {
            point = match frame.step(circuit, slots, point)? {
                Step::Write(next, dst, word) => {
                    slots[dst.index()] = word;
                    next
                }
                Step::Jump(target) => frame.jump(slots, target),
                Step::Return(word) => return Ok(Some(word)),
                Step::Branch(..) | Step::Transfer(..) | Step::Fault(_) => unreachable!("proved a straight path"),
            };
        }
        Ok(None)
    }
}

impl<'a> Path<'a> {
    fn prove(frame: &Frame<'a>, header: Point, start: &'a Target) -> Option<Self> {
        let function = frame.module.function(frame.callee);
        let mut charge = 1u64; // the decision at the header
        let mut seen = BTreeSet::<BlockId>::new();
        let mut block = start.block;
        let end = loop {
            if block == header.block {
                charge = charge.checked_add(header.index as u64)?;
                break End::Header;
            }
            if !seen.insert(block) {
                return None;
            }
            let body = function.block(block);
            if !body.insts.iter().all(total) {
                return None;
            }
            charge = charge.checked_add(body.insts.len() as u64 + 1)?;
            block = match &body.term {
                Terminator::Jump(target) => target.block,
                Terminator::Return(Some(_)) => break End::Return,
                Terminator::Branch { .. }
                | Terminator::Switch { .. }
                | Terminator::Return(None)
                | Terminator::Exit(_)
                | Terminator::Longjmp { .. }
                | Terminator::Fault(_) => return None,
            };
        };
        Some(Self { start, charge, end })
    }
}

fn total(inst: &Inst) -> bool {
    match inst {
        Inst::Binary { op, lhs, rhs, .. } => match op {
            BinOp::Add | BinOp::Sub | BinOp::And | BinOp::Xor => true,
            BinOp::Mul => matches!(lhs, Operand::Const(_)) || matches!(rhs, Operand::Const(_)),
            BinOp::Shl | BinOp::Lshr | BinOp::Ashr => matches!(rhs, Operand::Const(_)),
            BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem | BinOp::Or => false,
        },
        Inst::Compare { .. } | Inst::Select { .. } | Inst::Cast { .. } => true,
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
        | Inst::Apply { .. } => false,
    }
}
