//! Plan one copy per static call path before allocating any circuit wires.

use std::collections::BTreeMap;
use std::ops::Range;

use bennett_exec::frame_bytes;
use bennett_ir::{Block, BlockId, Fault, FuncId, Function, Inst, Operand, Slot, Target, Terminator, Type};

use super::bits::{Bit, Circuit, Word, constant};
use super::symbolic::{Frame, Point, Step};
use super::{ApplyError, ApplyInput, Argument};

// Count a unit for each copied slot and each instruction or terminator, including
// cells in cold code. This limits expansion from call paths independently of B.
const COPY_WORK_LIMIT: usize = 16 * 1024;

struct Copy<'a> {
    frame: Frame<'a>,
    slots: Range<usize>,
    available_stack: Option<u64>,
    caller: Option<(Point, Option<Slot>)>,
    sites: BTreeMap<(BlockId, usize), Site<'a>>,
}

enum Site<'a> {
    Call(usize, &'a [Operand]),
    BadAddress,
}

pub(super) struct Program<'a> {
    copies: Vec<Copy<'a>>,
    work: usize,
}

impl<'a> Program<'a> {
    pub(super) fn check(input: &ApplyInput<'a>) -> Result<(Self, Vec<Word>), ApplyError> {
        let source = input.module.function(input.callee);
        let args = arguments(input);
        let root = Frame::check(input.module, input.callee, args.iter().cloned().map(Some))?;
        let mut slots = root.initial(args);
        let mut program = Self {
            copies: vec![Copy {
                frame: root,
                slots: 0..slots.len(),
                available_stack: input.entry_fault.is_none().then_some(input.stack_available),
                caller: None,
                sites: BTreeMap::new(),
            }],
            work: work(source),
        };

        let mut index = 0;
        while index < program.copies.len() {
            let func = program.copies[index].frame.callee;
            let source = input.module.function(func);
            for (block, body) in source.blocks.iter().enumerate() {
                let block = BlockId(block as u32);
                if !program.copies[index].frame.reaches(block) {
                    continue;
                }
                for (inst, instruction) in body.insts.iter().enumerate() {
                    let point = Point { copy: index, block, index: inst };
                    let site = match instruction {
                        Inst::Call { callee, args, dst } => {
                            let child = program.child(input, point, *callee, args, (*dst, "call"), &mut slots)?;
                            Site::Call(child, args)
                        }
                        Inst::CallIndirect { signature, target, args, dst } => {
                            let frame = &program.copies[index].frame;
                            let Some(address) = frame.known(target) else {
                                return Err(frame.error(point, "call_indirect", "a proved definite function address"));
                            };
                            let callee = input
                                .module
                                .function_at(address as u64)
                                .filter(|&callee| input.module.function(callee).has_signature(signature));
                            match callee {
                                Some(callee) => {
                                    let child = program.child(
                                        input,
                                        point,
                                        callee,
                                        args,
                                        (*dst, "call_indirect"),
                                        &mut slots,
                                    )?;
                                    Site::Call(child, args)
                                }
                                None => Site::BadAddress,
                            }
                        }
                        Inst::Binary { .. }
                        | Inst::Compare { .. }
                        | Inst::Select { .. }
                        | Inst::Cast { .. }
                        | Inst::FuncAddr { .. }
                        | Inst::Load { .. }
                        | Inst::Store { .. }
                        | Inst::Alloca { .. }
                        | Inst::StackSave { .. }
                        | Inst::StackRestore { .. }
                        | Inst::ReturnAddress { .. }
                        | Inst::Setjmp { .. }
                        | Inst::Effect { .. }
                        | Inst::Qalloc { .. }
                        | Inst::Qfree { .. }
                        | Inst::Gate { .. }
                        | Inst::GateAt { .. }
                        | Inst::Measure { .. }
                        | Inst::Apply { .. } => continue,
                    };
                    program.copies[index].sites.insert((point.block, point.index), site);
                }
            }
            index += 1;
        }
        Ok((program, slots))
    }

    fn child(
        &mut self,
        input: &ApplyInput<'a>,
        at: Point,
        callee: FuncId,
        args: &[Operand],
        (dst, call): (Option<Slot>, &'static str),
        slots: &mut Vec<Word>,
    ) -> Result<usize, ApplyError> {
        let parent = &self.copies[at.copy];
        let function = input.module.function(callee);
        let error = |route| parent.frame.error(at, call, route);
        let mut ancestor = Some(at);
        while let Some(point) = ancestor {
            let copy = &self.copies[point.copy];
            if copy.frame.callee == callee {
                return Err(error("recursive call frames"));
            }
            ancestor = copy.caller.map(|(resume, _)| resume);
        }
        self.work = self.work.saturating_add(work(function));
        if self.work > COPY_WORK_LIMIT {
            return Err(error("a smaller copy expansion (limit: 16384 slots and program points)"));
        }

        if function.params.iter().any(|param| matches!(param.ty, Type::Qreg(_)))
            || matches!(function.result, Some(Type::Qreg(_)))
        {
            return Err(error("integer call signatures"));
        }
        // These facts hold across every visit in the caller copy. The frame
        // remaps their wire identities together, preserving shared input bits.
        let proofs = args.iter().map(|arg| parent.frame.known_bits(arg));
        let frame = Frame::check(input.module, callee, proofs)?;
        let available_stack =
            parent.available_stack.and_then(|bytes| bytes.checked_sub(frame_bytes(function.slot_count())?));
        let caller = Some((at.next(), dst.map(|slot| parent.slot(slot))));
        let start = slots.len();
        slots.extend(frame.initial([]));
        let index = self.copies.len();
        self.copies.push(Copy { frame, slots: start..slots.len(), available_stack, caller, sites: BTreeMap::new() });
        Ok(index)
    }

    pub(super) fn root(&self) -> &Frame<'a> {
        &self.copies[0].frame
    }

    pub(super) fn has_calls(&self) -> bool {
        !self.copies[0].sites.is_empty()
    }

    pub(super) fn frames(&self) -> impl Iterator<Item = &Frame<'a>> {
        self.copies.iter().map(|copy| &copy.frame)
    }

    pub(super) fn slot(&self, point: Point, slot: Slot) -> Slot {
        self.copies[point.copy].slot(slot)
    }

    pub(super) fn block(&self, point: Point) -> &'a Block {
        let frame = &self.copies[point.copy].frame;
        frame.module.function(frame.callee).block(point.block)
    }

    pub(super) fn resume(&self, point: Point) -> Option<Point> {
        self.copies[point.copy].caller.map(|(resume, _)| resume)
    }

    pub(super) fn read(&self, slots: &[Word], point: Point, operand: &Operand) -> Word {
        Frame::read(&slots[self.copies[point.copy].slots.clone()], operand)
    }

    pub(super) fn step(&self, circuit: &mut Circuit, slots: &[Word], point: Point) -> Result<Step<'a>, ApplyError> {
        self.step_narrow(circuit, slots, point, &[])
    }

    pub(super) fn step_narrow(
        &self,
        circuit: &mut Circuit,
        slots: &[Word],
        point: Point,
        widths: &[Option<usize>],
    ) -> Result<Step<'a>, ApplyError> {
        let copy = &self.copies[point.copy];
        let local = &slots[copy.slots.clone()];
        if let Some(site) = copy.sites.get(&(point.block, point.index)) {
            let Site::Call(index, args) = site else {
                return Ok(Step::Fault(Fault::BadIndirect));
            };
            let child = &self.copies[*index];
            if child.available_stack.is_none() {
                return Ok(Step::Fault(Fault::StackOverflow));
            }
            let writes = child
                .frame
                .initial(args.iter().map(|arg| Frame::read(local, arg)))
                .into_iter()
                .enumerate()
                .map(|(index, word)| (child.slot(Slot(index as u32)), word))
                .collect();
            return Ok(Step::Transfer(Point { copy: *index, ..Point::entry(BlockId(0)) }, writes));
        }

        let block = self.block(point);
        if point.index == block.insts.len()
            && let Terminator::Return(value) = &block.term
            && let Some((resume, dst)) = copy.caller
        {
            let writes =
                dst.zip(value.as_ref()).into_iter().map(|(slot, arg)| (slot, Frame::read(local, arg))).collect();
            return Ok(Step::Transfer(resume, writes));
        }

        let widths = if widths.is_empty() { widths } else { &widths[copy.slots.clone()] };
        let step = copy.frame.step(circuit, local, point, widths)?;
        Ok(match step {
            Step::Write(next, dst, word) => Step::Write(next, copy.slot(dst), word),
            Step::CheckedWrite(next, dst, word, enabled, fault) => {
                Step::CheckedWrite(next, copy.slot(dst), word, enabled, fault)
            }
            Step::Jump(_) | Step::Branch(..) | Step::Return(_) | Step::Transfer(..) | Step::Fault(_) => step,
        })
    }

    pub(super) fn incoming(&self, slots: &[Word], point: Point, target: &Target) -> Vec<(Slot, Word)> {
        let copy = &self.copies[point.copy];
        copy.frame
            .incoming(&slots[copy.slots.clone()], target)
            .into_iter()
            .map(|(slot, word)| (copy.slot(slot), word))
            .collect()
    }

    pub(super) fn jump(&self, slots: &mut [Word], point: Point, target: &Target) -> Point {
        for (slot, word) in self.incoming(slots, point, target) {
            slots[slot.index()] = word;
        }
        Point { copy: point.copy, ..Point::entry(target.block) }
    }
}

pub(super) fn arguments(input: &ApplyInput<'_>) -> Vec<Word> {
    input
        .module
        .function(input.callee)
        .params
        .iter()
        .zip(input.args)
        .map(|(param, arg)| match arg {
            Argument::Value(value) => {
                let Type::Int(width) = param.ty else { unreachable!("checked the argument type") };
                constant(*value, width.bits() as usize)
            }
            Argument::Register(wires) => wires.clone().map(|wire| Bit::Wire(wire, false)).collect(),
        })
        .collect()
}

impl Copy<'_> {
    fn slot(&self, local: Slot) -> Slot {
        Slot((self.slots.start + local.index()) as u32)
    }
}

fn work(function: &Function) -> usize {
    function
        .blocks
        .iter()
        .fold(function.slot_count(), |total, block| total.saturating_add(block.insts.len()).saturating_add(1))
}
