//! Plan one copy per static call path before allocating any circuit wires.

use std::collections::BTreeMap;
use std::ops::Range;

use bennett_exec::frame_bytes;
use bennett_ir::{Block, BlockId, Fault, FuncId, Function, Inst, Operand, Slot, Target, Terminator, Type};

use super::bits::{Bit, Circuit, Word, constant};
use super::memory::{AccessError, Memory};
use super::stack::{self, StackSlots};
use super::symbolic::{Frame, Point, Step, before_instruction_fault};
use super::{ApplyError, ApplyInput, Argument};

// Count a unit for each copied slot and each instruction or terminator, including
// cells in cold code. This limits expansion from call paths independently of B.
const COPY_WORK_LIMIT: usize = 16 * 1024;

struct Copy<'a> {
    frame: Frame<'a>,
    slots: Range<usize>,
    available_stack: Option<u64>,
    stack: Option<StackSlots>,
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
    has_memory: bool,
    stack_floor: u64,
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
                stack: None,
                caller: None,
                sites: BTreeMap::new(),
            }],
            work: work(source),
            has_memory: false,
            stack_floor: input.module.static_end(),
        };

        let mut has_stack = false;
        let mut index = 0;
        while index < program.copies.len() {
            let func = program.copies[index].frame.callee;
            let source = input.module.function(func);
            program.has_memory |= source
                .blocks
                .iter()
                .flat_map(|block| &block.insts)
                .any(|inst| matches!(inst, Inst::Load { .. } | Inst::Store { .. }));
            has_stack |=
                source.blocks.iter().flat_map(|block| &block.insts).any(|inst| {
                    matches!(inst, Inst::Alloca { .. } | Inst::StackSave { .. } | Inst::StackRestore { .. })
                });
            for (block, body) in source.blocks.iter().enumerate() {
                let block = BlockId(block as u32);
                if !program.copies[index].frame.reaches(block) {
                    continue;
                }
                let (instructions, _) = before_instruction_fault(body);
                for (inst, instruction) in instructions.iter().enumerate() {
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
        if has_stack {
            program.has_memory = true;
            let entry = if input.entry_fault.is_some() {
                0
            } else {
                program.stack_floor.checked_add(input.stack_available).ok_or(ApplyError::Interface)?
            };
            for (index, copy) in program.copies.iter_mut().enumerate() {
                copy.stack = Some(StackSlots::append(&mut slots, if index == 0 { entry } else { 0 }));
            }
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
        self.copies.push(Copy {
            frame,
            slots: start..slots.len(),
            available_stack,
            stack: None,
            caller,
            sites: BTreeMap::new(),
        });
        Ok(index)
    }

    pub(super) fn root(&self) -> &Frame<'a> {
        &self.copies[0].frame
    }

    pub(super) fn has_calls(&self) -> bool {
        !self.copies[0].sites.is_empty()
    }

    pub(super) fn has_memory(&self) -> bool {
        self.has_memory
    }

    pub(super) fn frames(&self) -> impl Iterator<Item = &Frame<'a>> {
        self.copies.iter().map(|copy| &copy.frame)
    }

    pub(super) fn slot(&self, point: Point, slot: Slot) -> Slot {
        self.copies[point.copy].slot(slot)
    }

    pub(super) fn block(&self, point: Point) -> &'a Block {
        self.function(point).block(point.block)
    }

    pub(super) fn function(&self, point: Point) -> &'a Function {
        let frame = &self.copies[point.copy].frame;
        frame.module.function(frame.callee)
    }

    pub(super) fn direct_child(&self, point: Point) -> Option<Point> {
        if !matches!(self.block(point).insts.get(point.index), Some(Inst::Call { .. })) {
            return None;
        }
        let Site::Call(index, _) = self.copies[point.copy].sites.get(&(point.block, point.index))? else {
            return None;
        };
        self.copies[*index].available_stack?;
        Some(Point { copy: *index, ..Point::entry(BlockId(0)) })
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

    pub(super) fn step_in(
        &self,
        circuit: &mut Circuit,
        slots: &[Word],
        point: Point,
        memory: &mut Memory<'_>,
        guard: Bit,
    ) -> Result<Step<'a>, ApplyError> {
        let read = |operand| self.read(slots, point, operand);
        let next = point.next();
        let copy = &self.copies[point.copy];
        let (operation, result) = match self.block(point).insts.get(point.index) {
            Some(Inst::Load { width, dst, address }) => (
                "load",
                memory
                    .load(circuit, &read(address), guard, *width)
                    .map(|word| Step::Write(next, self.slot(point, *dst), word)),
            ),
            Some(Inst::Store { width, value, address }) => (
                "store",
                memory
                    .store(circuit, &read(address), guard, *width, read(value))
                    .map(|()| Step::Transfer(next, Vec::new())),
            ),
            Some(Inst::Alloca { size, dst }) => {
                let stack = copy.stack.expect("planned stack slots");
                return Ok(stack.allocate(circuit, slots, next, copy.slot(*dst), &read(size), self.stack_floor));
            }
            Some(Inst::StackSave { dst }) => {
                let stack = copy.stack.expect("planned stack slots");
                return Ok(Step::Write(next, copy.slot(*dst), slots[stack.current.index()].clone()));
            }
            Some(Inst::StackRestore { pointer }) => {
                let stack = copy.stack.expect("planned stack slots");
                return Ok(stack.restore(circuit, slots, next, read(pointer)));
            }
            Some(_) | None => return self.step(circuit, slots, point),
        };
        let unsupported = |route| self.copies[point.copy].frame.error(point, operation, route);
        match result {
            Ok(step) => Ok(step),
            Err(AccessError::Inactive) => Ok(Step::Inactive),
            Err(AccessError::Fault(fault)) => Ok(Step::Fault(fault)),
            Err(AccessError::MissingMemory) => Err(unsupported("the caller's memory")),
            Err(AccessError::Address) => Err(unsupported("a proved definite memory address")),
        }
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
            let mut writes: Vec<_> = child
                .frame
                .initial(args.iter().map(|arg| Frame::read(local, arg)))
                .into_iter()
                .enumerate()
                .map(|(index, word)| (child.slot(Slot(index as u32)), word))
                .collect();
            let next = Point { copy: *index, ..Point::entry(BlockId(0)) };
            if let (Some(parent_stack), Some(child_stack)) = (copy.stack, child.stack) {
                let size = frame_bytes(child.slots.len()).expect("checked frame size");
                let (top, failed) = stack::reserve(
                    circuit,
                    &slots[parent_stack.current.index()],
                    &stack::word(size),
                    self.stack_floor,
                    false,
                );
                writes.push((child_stack.current, top.clone()));
                writes.push((child_stack.top, top));
                return Ok(Step::checked(next, writes, failed, Fault::StackOverflow));
            }
            return Ok(Step::Transfer(next, writes));
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
            Step::CheckedTransfer(next, writes, failed, fault) => Step::CheckedTransfer(
                next,
                writes.into_iter().map(|(slot, word)| (copy.slot(slot), word)).collect(),
                failed,
                fault,
            ),
            Step::Jump(_)
            | Step::Branch(..)
            | Step::Switch(..)
            | Step::Return(_)
            | Step::Transfer(..)
            | Step::Fault(_)
            | Step::Inactive => step,
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
