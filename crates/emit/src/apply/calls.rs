//! Plan one copy per static call path before allocating any circuit wires.

use std::collections::BTreeMap;
use std::ops::Range;

use bennett_exec::frame_bytes;
use bennett_ir::{Block, BlockId, Fault, FuncId, Function, Inst, Operand, Slot, Target, Terminator, Type};

use super::bits::{Bit, Circuit, Word, ZERO, constant};
use super::definite::CallProofs;
use super::memory::{AccessError, Memory};
use super::selectors::Selector;
use super::stack::{self, StackSlots};
use super::symbolic::{Frame, IndirectCall, Point, Step};
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
    Indirect { target: &'a Operand, args: &'a [Operand], targets: Vec<(u64, usize)>, invalid_possible: bool },
}

pub(super) struct CallEntry {
    pub(super) next: Point,
    pub(super) writes: Vec<(Slot, Word)>,
    pub(super) failed: Bit,
}

pub(super) struct Program<'a> {
    copies: Vec<Copy<'a>>,
    work: usize,
    has_memory: bool,
    stack_floor: u64,
    #[cfg(test)]
    pub(super) return_analysis_work: usize,
}

impl<'a> Program<'a> {
    pub(super) fn check(input: &ApplyInput<'a>) -> Result<(Self, Vec<Word>), ApplyError> {
        let source = input.module.function(input.callee);
        let args = arguments(input);
        let mut proofs = CallProofs::new(input.module, COPY_WORK_LIMIT);
        let root = Frame::check(input.module, input.callee, args.iter().cloned().map(Some), &mut proofs)?;
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
            #[cfg(test)]
            return_analysis_work: 0,
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
            for ((block, instruction), call) in program.copies[index].frame.take_calls() {
                let point = Point { copy: index, block, index: instruction };
                let operation = if call.target.is_some() { "call_indirect" } else { "call" };
                let mut targets = Vec::new();
                for callee in call.candidates {
                    let child =
                        program.child(&mut proofs, point, callee, call.args, (call.dst, operation), &mut slots)?;
                    targets.push((callee.address(), child));
                }
                let site = match (targets.as_slice(), call.invalid_possible) {
                    ([], _) => Site::BadAddress,
                    ([(_, child)], false) => Site::Call(*child, call.args),
                    ([_, _, ..], _) | ([_], true) => Site::Indirect {
                        target: call.target.expect("an indirect call can have several addresses"),
                        args: call.args,
                        targets,
                        invalid_possible: call.invalid_possible,
                    },
                };
                program.copies[index].sites.insert((block, instruction), site);
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
        #[cfg(test)]
        {
            program.return_analysis_work = proofs.returns.work;
        }
        Ok((program, slots))
    }

    fn child(
        &mut self,
        proofs: &mut CallProofs<'a>,
        at: Point,
        callee: FuncId,
        args: &[Operand],
        (dst, call): (Option<Slot>, &'static str),
        slots: &mut Vec<Word>,
    ) -> Result<usize, ApplyError> {
        let parent = &self.copies[at.copy];
        let module = parent.frame.module;
        let function = module.function(callee);
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
        let arguments = args.iter().map(|arg| parent.frame.known_bits(arg));
        let frame = Frame::check(module, callee, arguments, proofs)?;
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

    pub(super) fn has_dynamic_stack(&self) -> bool {
        self.copies[0].stack.is_some()
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
        let next = point.next();
        let copy = &self.copies[point.copy];
        let local = &slots[copy.slots.clone()];
        let read = |operand| copy.frame.operand(local, operand);
        let (operation, result) = match self.block(point).insts.get(point.index) {
            Some(Inst::Load { width, dst, address }) => (
                "load",
                memory.load(circuit, &read(address), guard, *width).map(|access| {
                    let dst = self.slot(point, *dst);
                    if access.faults.is_empty() {
                        Step::Write(next, dst, access.value)
                    } else {
                        Step::CheckedTransfer(next, vec![(dst, access.value)], access.succeeded, access.faults)
                    }
                }),
            ),
            Some(Inst::Store { width, value, address }) => (
                "store",
                memory.store(circuit, &read(address), guard, *width, read(value)).map(|access| {
                    if access.faults.is_empty() {
                        Step::Transfer(next, Vec::new())
                    } else {
                        Step::CheckedTransfer(next, Vec::new(), access.succeeded, access.faults)
                    }
                }),
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
            Err(AccessError::Address) => Err(unsupported("a proved address bound within a declared memory region")),
        }
    }

    pub(super) fn step_narrow(
        &self,
        circuit: &mut Circuit,
        slots: &[Word],
        point: Point,
        widths: &[Option<usize>],
    ) -> Result<Step<'a>, ApplyError> {
        self.step_with(circuit, slots, point, widths, |_, word| word)
    }

    pub(super) fn step_with(
        &self,
        circuit: &mut Circuit,
        slots: &[Word],
        point: Point,
        widths: &[Option<usize>],
        resolve: impl Fn(&Circuit, Word) -> Word,
    ) -> Result<Step<'a>, ApplyError> {
        let copy = &self.copies[point.copy];
        let local = &slots[copy.slots.clone()];
        let read = |circuit: &Circuit, operand: &Operand| resolve(circuit, copy.frame.operand(local, operand));
        if let Some(site) = copy.sites.get(&(point.block, point.index)) {
            return Ok(match site {
                Site::Call(index, args) => call_step(self.enter(circuit, slots, point, *index, args)),
                Site::BadAddress => Step::Fault(Fault::BadIndirect),
                Site::Indirect { target, args, targets, invalid_possible } => {
                    let address = read(circuit, target);
                    let facts = Selector::new(&address);
                    let targets: Vec<_> =
                        targets.iter().filter(|&&(value, _)| facts.allows(u128::from(value))).copied().collect();
                    let invalid_possible = *invalid_possible && !facts.covered_by(targets.len());
                    if targets.is_empty() {
                        Step::Fault(Fault::BadIndirect)
                    } else if targets.len() == 1 && !invalid_possible {
                        call_step(self.enter(circuit, slots, point, targets[0].1, args))
                    } else {
                        Step::Indirect(IndirectCall { address, args, targets, invalid_possible })
                    }
                }
            });
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
        let step = copy.frame.step(circuit, read, point, widths)?;
        Ok(match step {
            Step::Write(next, dst, word) => Step::Write(next, copy.slot(dst), word),
            Step::CheckedTransfer(next, writes, succeeded, faults) => Step::CheckedTransfer(
                next,
                writes.into_iter().map(|(slot, word)| (copy.slot(slot), word)).collect(),
                succeeded,
                faults,
            ),
            Step::Jump(_)
            | Step::Branch(..)
            | Step::Switch(..)
            | Step::Indirect(..)
            | Step::Return(_)
            | Step::Transfer(..)
            | Step::Fault(_)
            | Step::Inactive => step,
        })
    }

    pub(super) fn enter(
        &self,
        circuit: &mut Circuit,
        slots: &[Word],
        point: Point,
        index: usize,
        args: &[Operand],
    ) -> Result<CallEntry, Fault> {
        let child = &self.copies[index];
        child.available_stack.ok_or(Fault::StackOverflow)?;
        let parent = &self.copies[point.copy];
        let local = &slots[parent.slots.clone()];
        let mut writes: Vec<_> = child
            .frame
            .initial(args.iter().map(|arg| Frame::read(local, arg)))
            .into_iter()
            .enumerate()
            .map(|(slot, word)| (child.slot(Slot(slot as u32)), word))
            .collect();
        let next = Point { copy: index, ..Point::entry(BlockId(0)) };
        let mut failed = ZERO;
        if let (Some(parent_stack), Some(child_stack)) = (parent.stack, child.stack) {
            let size = frame_bytes(child.slots.len()).expect("checked frame size");
            let (top, overflow) = stack::reserve(
                circuit,
                &slots[parent_stack.current.index()],
                &stack::word(size),
                self.stack_floor,
                false,
            );
            writes.push((child_stack.current, top.clone()));
            writes.push((child_stack.top, top));
            failed = overflow;
        }
        Ok(CallEntry { next, writes, failed })
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

fn call_step(entry: Result<CallEntry, Fault>) -> Step<'static> {
    entry.map_or_else(Step::Fault, |entry| Step::checked(entry.next, entry.writes, entry.failed, Fault::StackOverflow))
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

pub(super) fn work(function: &Function) -> usize {
    function
        .blocks
        .iter()
        .fold(function.slot_count(), |total, block| total.saturating_add(block.insts.len()).saturating_add(1))
}
