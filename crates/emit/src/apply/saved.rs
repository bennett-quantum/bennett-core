//! Prove a single fixed-charge loop, possibly preceded by an early exit.

use std::collections::BTreeSet;

use bennett_ir::{BinOp, Fault, Inst, Operand, Target, Terminator};

use super::bits::{Bit, Circuit, ONE, Word};
use super::calls::Program;
use super::symbolic::{Point, Step};
use super::work::{self, Work};
use super::{ApplyError, Copies};

struct Path<'a> {
    start: &'a Target,
    charge: u64,
    // The next conditional jump, or None for an applied return.
    next_test: Option<Point>,
}

struct Test<'a> {
    point: Point,
    cond: &'a Operand,
    exit_when_true: bool,
    continuing: Path<'a>,
    exiting: Path<'a>,
}

pub(super) struct Loop<'a> {
    entry: Option<Test<'a>>,
    body: Test<'a>,
}

impl<'a> Loop<'a> {
    pub(super) fn prove(program: &Program<'a>, start: Point) -> Option<Self> {
        let entry = Test::prove(program, start)?;
        let next = entry.continuing.next_test?;
        if next == start {
            return Some(Self { entry: None, body: entry });
        }
        // Both exits go straight to the applied return. The entry test
        // cannot be revisited by this loop or by a caller continuation.
        let body = Test::prove(program, next)?;
        (body.continuing.next_test == Some(next)).then_some(Self { entry: Some(entry), body })
    }

    pub(super) fn run(
        &self,
        circuit: &mut Circuit,
        program: &Program<'a>,
        slots: &mut [Word],
        mut remaining: u64,
        widths: &[Option<usize>],
    ) -> Result<(Copies, u64), ApplyError> {
        let mut active = ONE;
        let mut copies = Copies::default();
        let mut decisions = 0;

        if let Some(entry) = &self.entry {
            let exit = entry.exit(program, slots);
            if remaining >= entry.exiting.charge {
                entry.record(circuit, program, slots, exit, widths, &mut copies)?;
            } else {
                copies.faulted.push((exit, Fault::Budget));
            }
            active = exit.not();
            let Some(after_entry) = remaining.checked_sub(entry.continuing.charge) else {
                copies.faulted.push((active, Fault::Budget));
                return Ok((copies, decisions));
            };
            if after_entry < self.body.exiting.charge || active.is_zero() {
                copies.faulted.push((active, Fault::Budget));
                return Ok((copies, decisions));
            }
            entry.continuing.follow(circuit, program, slots, entry.point, widths)?;
            remaining = after_entry;
        }

        // If no more returns fit, the remaining instructions are total and
        // active inputs can only exhaust the budget.
        while remaining >= self.body.exiting.charge && !active.is_zero() {
            let exit = self.body.exit(program, slots);
            let returning = circuit.and(active, exit);
            self.body.record(circuit, program, slots, returning, widths, &mut copies)?;
            active = circuit.and(active, exit.not());
            decisions += 1;
            let Some(after_continue) = remaining.checked_sub(self.body.continuing.charge) else {
                break;
            };
            if after_continue < self.body.exiting.charge || active.is_zero() {
                break;
            }
            // Unselected inputs can run this total computation speculatively.
            // Only enabled inputs can contribute to a return or fault.
            self.body.continuing.follow(circuit, program, slots, self.body.point, widths)?;
            remaining = after_continue;
        }
        copies.faulted.push((active, Fault::Budget));
        Ok((copies, decisions))
    }
}

impl<'a> Test<'a> {
    fn prove(program: &Program<'a>, point: Point) -> Option<Self> {
        let block = program.block(point);
        let Terminator::Branch { cond, if_true, if_false } = &block.term else {
            return None;
        };
        if point.index != block.insts.len() {
            return None;
        }
        let yes = Path::prove(program, point, if_true)?;
        let no = Path::prove(program, point, if_false)?;
        let (exit_when_true, continuing, exiting) = match (yes.next_test, no.next_test) {
            (None, Some(_)) => (true, no, yes),
            (Some(_), None) => (false, yes, no),
            (Some(_), Some(_)) | (None, None) => return None,
        };
        Some(Self { point, cond, exit_when_true, continuing, exiting })
    }

    fn exit(&self, program: &Program<'a>, slots: &[Word]) -> Bit {
        let condition = program.read(slots, self.point, self.cond)[0];
        if self.exit_when_true { condition } else { condition.not() }
    }

    fn record(
        &self,
        circuit: &mut Circuit,
        program: &Program<'a>,
        slots: &[Word],
        returning: Bit,
        widths: &[Option<usize>],
        copies: &mut Copies,
    ) -> Result<(), ApplyError> {
        if !returning.is_zero() {
            let word = self
                .exiting
                .follow(circuit, program, &mut slots.to_vec(), self.point, widths)?
                .expect("proved an exit to the applied return");
            copies.returned.push((returning, word));
        }
        Ok(())
    }
}

impl<'a> Path<'a> {
    fn prove(program: &Program<'a>, test: Point, start: &'a Target) -> Option<Self> {
        let mut charge = 1; // the conditional jump
        let mut seen = BTreeSet::new();
        let mut point = Point { block: start.block, index: 0, ..test };
        let next_test = loop {
            work::add(Work::ShapeVisits, 1);
            if !seen.insert(point) {
                return None;
            }
            let block = program.block(point);
            if let Some(inst) = block.insts.get(point.index) {
                if !total(inst) {
                    return None;
                }
                charge += 1;
                point = point.next();
                continue;
            }
            match &block.term {
                Terminator::Branch { .. } => break Some(point),
                Terminator::Jump(target) => point = Point { block: target.block, index: 0, ..point },
                Terminator::Return(_) => {
                    let Some(resume) = program.resume(point) else {
                        charge += 1;
                        break None;
                    };
                    point = resume;
                }
                Terminator::Switch { .. } | Terminator::Exit(_) | Terminator::Longjmp { .. } | Terminator::Fault(_) => {
                    return None;
                }
            }
            charge += 1;
        };
        Some(Self { start, charge, next_test })
    }

    fn follow(
        &self,
        circuit: &mut Circuit,
        program: &Program<'a>,
        slots: &mut [Word],
        test: Point,
        widths: &[Option<usize>],
    ) -> Result<Option<Word>, ApplyError> {
        let mut point = program.jump(slots, test, self.start);
        while Some(point) != self.next_test {
            point = match program.step_narrow(circuit, slots, point, widths)? {
                Step::Write(next, dst, word) => {
                    slots[dst.index()] = word;
                    next
                }
                Step::Jump(target) => program.jump(slots, point, target),
                Step::Transfer(next, writes) => {
                    for (slot, word) in writes {
                        slots[slot.index()] = word;
                    }
                    next
                }
                Step::Return(word) => return Ok(Some(word)),
                Step::Branch(..)
                | Step::Switch(..)
                | Step::Indirect(..)
                | Step::Fault(_)
                | Step::CheckedTransfer(..)
                | Step::Inactive => {
                    unreachable!("proved a total straight path")
                }
            };
        }
        Ok(None)
    }
}

fn total(inst: &Inst) -> bool {
    match inst {
        Inst::Binary { op, lhs, rhs, .. } => match op {
            BinOp::Add | BinOp::Sub | BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Shl | BinOp::Lshr | BinOp::Ashr => {
                true
            }
            BinOp::Mul => matches!(lhs, Operand::Const(_)) || matches!(rhs, Operand::Const(_)),
            BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem => false,
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
        | Inst::QallocBy { .. }
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::RotateBy { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => false,
    }
}
