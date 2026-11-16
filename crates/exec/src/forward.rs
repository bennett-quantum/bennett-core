//! Forward execution of one step. Every step either faults without changing anything, or changes the
//! state and journals exactly what it overwrote.

use bennett_ir::{
    BlockId, ENTRY_RETURN_ADDRESS, Fault, FuncId, Gate, Inst, JUMP_RECORD_BYTES, MeasureSource, Operand, Qubit, Slot,
    Target, Terminator, Type, Width,
};

use crate::executor::{Executor, Frame, Halt, Outcome, RunError, SLOT_BYTES};
use crate::gates;
use crate::journal::Undo;
use crate::quantum::QubitRef;

/// `alloca` results are aligned to this many bytes, the size of the widest value.
const STACK_ALIGN: u64 = 8;

impl<'m> Executor<'m> {
    /// Executes the instruction or terminator at the top frame's position. Going forward (not running a
    /// step again) the world's clock counts the step, and a mark's budget that is spent faults it first.
    pub(crate) fn execute(&mut self) -> Result<(), Halt> {
        if self.rerunning == 0 && self.rewinding == 0 {
            let deadline = self.journal.as_ref().and_then(|journal| journal.mark_deadline(self.region_records));
            if deadline.is_some_and(|deadline| self.clock >= deadline) {
                // A call with a budget spent at the same step ends first, the outermost as below: a catch
                // below it would count a step its frame has no budget for.
                if let Some(depth) = self.spent_guard() {
                    return self.catch(depth, Fault::Budget);
                }
                return self.fault_or_catch(Fault::Budget);
            }
            self.clock += 1;
        }
        if !self.any_guard || !self.counting {
            return self.execute_step();
        }
        // Inside calls with a budget: the outermost whose budget is spent ends with `budget`; a fault returns
        // to the innermost; every frame live before the step counts it (a callee's return included).
        if let Some(depth) = self.spent_guard() {
            return self.catch(depth, Fault::Budget);
        }
        let before = self.frames.len();
        self.count_guards(before, 1, false);
        match self.execute_step() {
            Ok(()) => Ok(()),
            Err(halt) => {
                // The step changed nothing; nor does its count.
                self.count_guards(before, 1, true);
                match halt {
                    Halt::Fault(fault) => self.fault_or_catch(fault),
                    // A fault the simulator found by inspecting the state is never a call's fault code.
                    Halt::Stop(_) | Halt::Discard | Halt::Inspected(_) => Err(halt),
                }
            }
        }
    }

    /// Closes the budgets of the marks of frames just left (`Journal::close_left`).
    fn frames_left(&mut self) {
        if let Some(journal) = &mut self.journal {
            journal.close_left(self.frames.len());
        }
    }

    /// The outermost live call with a budget whose budget is spent, if one is.
    fn spent_guard(&self) -> Option<usize> {
        if !self.any_guard || !self.counting {
            return None;
        }
        self.guards().find(|&depth| self.frames[depth].guard == Some(0))
    }

    /// The indices of the live frames of callees of calls with a budget, outermost first.
    pub(crate) fn guards(&self) -> impl Iterator<Item = usize> + '_ {
        (self.base_depth..self.frames.len()).filter(|&depth| self.frames[depth].guard.is_some())
    }

    /// Whether the current step runs inside a call with a budget: the region's rules apply.
    pub(crate) fn guarded(&self) -> bool {
        self.any_guard && self.guards().next().is_some()
    }

    /// Counts `steps` (or, undoing, gives them back) in the budgets of the frames below `live`.
    pub(crate) fn count_guards(&mut self, live: usize, steps: u64, back: bool) {
        let live = live.min(self.frames.len());
        for frame in &mut self.frames[self.base_depth.min(live)..live] {
            if let Some(left) = &mut frame.guard {
                *left = if back { *left + steps } else { *left - steps };
            }
        }
    }

    /// A fault: returned to the innermost call with a budget as its code, else the step's fault.
    fn fault_or_catch(&mut self, fault: Fault) -> Result<(), Halt> {
        match self.guards().last() {
            Some(depth) if self.any_guard => self.catch(depth, fault),
            Some(_) | None => Err(fault.into()),
        }
    }

    /// Ends the call with a budget whose callee's frame is at `depth` with `fault`: its frames are discarded,
    /// the caller's result is 0 and its fault code the fault's. A step of the frames below. In strict mode a
    /// fault while other worlds go on stops the run instead, as a world's fault does: the simulator's
    /// checks are never delivered to a program as a fault code.
    fn catch(&mut self, depth: usize, fault: Fault) -> Result<(), Halt> {
        self.deliver_fault(depth, fault)?;
        self.count_guards(depth, 1, false);
        Ok(())
    }

    /// The catch of a fault at `depth` (`catch`), but for counting its step: the frames from `depth` up are
    /// discarded (none, for a callee whose frame did not fit), and the call with a budget at the top frame's
    /// position gets the code.
    fn deliver_fault(&mut self, depth: usize, fault: Fault) -> Result<(), Halt> {
        if !self.in_region && !self.allow_region_faults && self.others_go_on() {
            return Err(Halt::Stop(RunError::WorldFault { fault, worlds: self.worlds.len() }));
        }
        let discarded = self.frames.len() - depth;
        for index in (depth..self.frames.len()).rev() {
            let frame = self.frames[index];
            self.record_word(pack_position(frame.func, frame.block, frame.inst));
            self.record_word(u128::from(frame.entry_stack_pointer));
            self.record_word(frame.guard.map_or(0, |left| u128::from(left) + 1));
        }
        self.record_word(u128::from(self.stack_pointer));
        self.record_word(discarded as u128);
        if discarded > 0 {
            self.stack_pointer = self.frames[depth].entry_stack_pointer;
            self.frames.truncate(depth);
            self.frames_left();
        }
        let caller = *self.top();
        let Inst::CallBudget { dst, fault: code, .. } =
            &self.function(caller.func).block(caller.block).insts[caller.inst]
        else {
            unreachable!("a frame with a budget was entered by a call with a budget")
        };
        if let Some(dst) = dst {
            self.write_slot(*dst, 0);
        }
        self.write_slot(*code, u128::from(fault.code()));
        if let Some(journal) = &mut self.journal {
            journal.push_jump(discarded);
        }
        self.record(Undo::Catch);
        self.advance();
        Ok(())
    }

    /// Executes the step at the top frame's position.
    fn execute_step(&mut self) -> Result<(), Halt> {
        let frame = *self.top();
        let block = self.function(frame.func).block(frame.block);
        match block.insts.get(frame.inst) {
            Some(inst) => self.execute_inst(inst),
            None => self.execute_terminator(&block.term),
        }
    }

    fn execute_inst(&mut self, inst: &'m Inst) -> Result<(), Halt> {
        match inst {
            Inst::Binary { op, width, dst, lhs, rhs } => {
                let value = crate::arith::binary(*op, *width, self.read(lhs), self.read(rhs))?;
                self.produce(*dst, value);
            }
            Inst::Compare { pred, width, dst, lhs, rhs } => {
                let holds = crate::arith::compare(*pred, *width, self.read(lhs), self.read(rhs));
                self.produce(*dst, u128::from(holds));
            }
            Inst::Select { dst, cond, if_true, if_false, .. } => {
                let value = if self.read(cond) == 1 { self.read(if_true) } else { self.read(if_false) };
                self.produce(*dst, value);
            }
            Inst::Cast { op, from, to, dst, value } => {
                self.produce(*dst, crate::arith::cast(*op, *from, *to, self.read(value)));
            }
            Inst::Load { width, dst, address } => self.load(*dst, *width, address)?,
            Inst::FuncAddr { dst, func } => self.produce(*dst, u128::from(func.address())),
            Inst::Store { width, value, address } => self.store(*width, self.read(value), self.read_word(address))?,
            Inst::Alloca { dst, size } => self.alloca(*dst, self.read_word(size))?,
            Inst::StackSave { dst } => self.produce(*dst, u128::from(self.stack_pointer)),
            Inst::ReturnAddress { dst, level } => self.produce(*dst, u128::from(self.return_address(*level))),
            Inst::StackRestore { pointer } => self.stack_restore(self.read_word(pointer))?,
            Inst::Call { callee, args, .. } => self.call(*callee, args, None)?,
            Inst::CallBudget { callee, args, budget, .. } => {
                // A callee whose frame does not fit faults inside the call, as an applied function's does: the
                // code is the call's, in every world alike (the frame is tried before any read splits).
                match self.call(*callee, args, Some(self.read_word(budget))) {
                    Err(Fault::StackOverflow) => return self.deliver_fault(self.frames.len(), Fault::StackOverflow),
                    entered => entered?,
                }
            }
            Inst::CallIndirect { signature, target, args, .. } => {
                let callee = self.module.function_at(self.read_word(target));
                let callee = callee.filter(|&f| self.function(f).has_signature(signature)).ok_or(Fault::BadIndirect)?;
                self.call(callee, args, None)?;
            }
            Inst::Setjmp { dst, buffer } => self.setjmp(*dst, self.read_word(buffer))?,
            Inst::Effect { byte } => {
                if self.in_region || self.guarded() {
                    return Err(Fault::EffectInRegion.into());
                }
                self.tape.push(self.read(byte) as u8);
                self.record(Undo::Effect);
                self.advance();
            }
            Inst::Mark { dst, budget } => self.mark(*dst, budget.as_ref()),
            Inst::Rewind { mark, keep } => self.rewind(*mark, keep)?,
            _ if self.in_region || self.guarded() => return Err(Fault::QuantumInRegion.into()),
            Inst::Qalloc { dst, width } => {
                let handle = self.quantum.allocate(*width);
                self.write_slot(*dst, u128::from(handle));
                self.record(Undo::Qalloc);
                self.advance();
            }
            Inst::Qfree { register } => {
                let id = self.quantum.resolve(self.handle(*register))?;
                if self.partial {
                    let nonzero = self.quantum.failing(|config| config[id] != 0);
                    self.defer(Undo::Qfree, nonzero);
                }
                // `dirty_free` is a check of the simulator's strict mode: on the machine the qubits are left
                // over, going forward and when the step runs again alike. On one world's part of a shared
                // state the check is deferred to the sum of the parts as before (`history.rs`).
                if !self.strict && !self.partial && !self.quantum.failing(|config| config[id] != 0).is_empty() {
                    self.quantum.leave(id);
                    self.record(Undo::Leftover);
                } else {
                    self.quantum.free(id, !self.partial)?;
                    self.record(Undo::Qfree);
                }
                self.advance();
            }
            Inst::Gate { gate, target, controls } => {
                let limit = self.max_configurations.map(|limit| limit.saturating_sub(self.others_configurations));
                self.gate(*gate, target, controls, limit)?;
                self.record(Undo::Gate);
                self.advance();
            }
            Inst::GateAt { gate, target, controls } => {
                let limit = self.max_configurations.map(|limit| limit.saturating_sub(self.others_configurations));
                self.gate_at(*gate, target, controls, limit)?;
            }
            Inst::Measure { dst, source: MeasureSource::Memory { width, address } } => {
                let range = self.memory_range(self.read_word(address), *width)?;
                // As for a register: a measurement reached while running again is refused (below).
                if self.rerunning > 0 {
                    return Err(Fault::BadRewind.into());
                }
                if self.has_worlds() {
                    self.observe_world()?;
                }
                let value = self.measure_memory(range, *width);
                self.write_slot(*dst, value);
                self.record(Undo::Measure);
                self.advance();
            }
            Inst::Measure { dst, source: MeasureSource::Register(register) } => {
                let id = self.quantum.resolve(self.handle(*register))?;
                // Steps run again to undo a rewind ran before without a measurement (a rewind refuses
                // across one), so one reached now is on a path the history does not describe: refuse it
                // before anything is sampled or observed.
                if self.rerunning > 0 {
                    return Err(Fault::BadRewind.into());
                }
                if self.has_worlds() {
                    self.observe_world()?;
                }
                let value = self.quantum.measure(id, &mut self.rng);
                self.write_slot(*dst, u128::from(value));
                self.record(Undo::Measure);
                self.advance();
            }
            Inst::Apply { callee, args, out, fault, budget } => {
                self.apply(*callee, args, *out, *fault, budget)?;
                self.record(Undo::Apply);
                self.advance();
            }
        }
        Ok(())
    }

    fn execute_terminator(&mut self, term: &'m Terminator) -> Result<(), Halt> {
        match term {
            Terminator::Jump(target) => self.jump(target),
            Terminator::Branch { cond, if_true, if_false } => {
                self.jump(if self.read(cond) == 1 { if_true } else { if_false });
            }
            Terminator::Switch { value, cases, default, .. } => {
                let value = self.read(value);
                let case = cases.iter().find(|case| case.value == value);
                self.jump(case.map_or(default, |case| &case.target));
            }
            Terminator::Return(value) => self.ret(value.map(|value| self.read(&value))),
            Terminator::Exit(value) => {
                if self.in_region || self.guarded() {
                    return Err(Fault::ExitInRegion.into());
                }
                let ty = match value {
                    Operand::Const(value) => Type::Int(value.width()),
                    Operand::Slot(slot) => self.slot_type(*slot),
                };
                // The caller selects the entry at run time; the module validator cannot know its type.
                if self.function(self.frames[0].func).result != Some(ty) {
                    return Err(Fault::ExitTypeMismatch.into());
                }
                self.outcome = Some(Outcome::Returned(Some(self.read(value))));
                // Like an outer return, finishing changes only the outcome. Keep all frames for reversal.
                self.record(Undo::Finish);
            }
            Terminator::Longjmp { buffer, value } => {
                let buffer = self.read_word(buffer);
                if self.jump_record_disagrees(buffer) {
                    return Err(Halt::Inspected(Fault::BadLongjmp));
                }
                self.longjmp(buffer, self.read(value))?;
            }
            Terminator::Fault(fault) => return Err((*fault).into()),
        }
        Ok(())
    }

    /// The memory range of a jump record at `address`.
    pub(crate) fn jump_record_range(&self, address: u64) -> Result<std::ops::Range<usize>, Fault> {
        let end = address.checked_add(JUMP_RECORD_BYTES).filter(|&end| end <= self.memory.len() as u64);
        let end = end.ok_or(Fault::OutOfBounds)?;
        Ok(address as usize..end as usize)
    }

    /// The jump record a `setjmp` at the top frame's position writes.
    pub(crate) fn jump_record(&self) -> [u8; JUMP_RECORD_BYTES as usize] {
        let frame = *self.top();
        // Depth and instruction index fit 32 bits: more frames or a longer block would need more memory than
        // a host has.
        let depth = (self.frames.len() - 1) as u64;
        let words = [
            depth | u64::from(frame.func.0) << 32,
            u64::from(frame.block.0) | (frame.inst as u64) << 32,
            frame.entry_stack_pointer,
            self.stack_pointer,
        ];
        let mut record = [0u8; JUMP_RECORD_BYTES as usize];
        for (chunk, word) in record.as_chunks_mut::<8>().0.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
        record
    }

    /// Writes a jump record naming the top frame and this `setjmp` at `buffer`, and gives 0. Bytes of the
    /// buffer that hold qubits are read first (`cells.rs`).
    fn setjmp(&mut self, dst: Slot, buffer: u64) -> Result<(), Fault> {
        let range = self.jump_record_range(buffer)?;
        let (cells, split) = self.read_cells(range.clone(), &|executor| executor.setjmp(dst, buffer));
        let record = self.jump_record();
        let mut old = [0u8; JUMP_RECORD_BYTES as usize];
        self.read_view(range.clone(), &cells, &mut old);
        let (low, high) = old.split_at(16);
        self.record_word(u128::from_le_bytes(low.try_into().expect("16 bytes")));
        self.record_word(u128::from_le_bytes(high.try_into().expect("16 bytes")));
        self.write_view(range.start, &record, &old, &cells);
        self.note_cells(&cells, range.start, &record, split);
        self.write_slot(dst, 0);
        self.record(Undo::Setjmp);
        self.advance();
        Ok(())
    }

    /// Resumes the frame and `setjmp` the record at `buffer` names, with `value` as the `setjmp`'s
    /// result: discards the frames above it and sets the stack pointer back to where the `setjmp` left it.
    /// Every field is checked against the live frames, so a record that names no live frame of this run
    /// (a frame that returned, or one outside a coherent region) faults instead of jumping.
    /// Whether the jump record at `buffer` is held in bytes holding qubits whose configurations disagree. A
    /// record in such bytes is read without splitting: worlds that disagreed on it would jump to different
    /// targets, and some could fault while others go on (a debt, `docs/DESIGN.md`, "Qubits in memory").
    /// Where the configurations disagree it names no single target, and the `longjmp` faults
    /// `bad_longjmp`: a fault found by looking at every configuration at once ([`Halt::Inspected`]).
    fn jump_record_disagrees(&self, buffer: u64) -> bool {
        let Ok(range) = self.jump_record_range(buffer) else { return false };
        !self.materialised
            && self
                .quantum
                .cells_in(range.start as u64..range.end as u64)
                .iter()
                .any(|&(_, column)| self.quantum.partition(&[column]).len() > 1)
    }

    fn longjmp(&mut self, buffer: u64, value: u128) -> Result<(), Fault> {
        let range = self.jump_record_range(buffer)?;
        let mut record = [0u8; JUMP_RECORD_BYTES as usize];
        let cells =
            if self.materialised { Vec::new() } else { self.quantum.cells_in(range.start as u64..range.end as u64) };
        self.read_view(range.clone(), &cells, &mut record);
        let mut words = [0u64; 4];
        for (word, chunk) in words.iter_mut().zip(record.as_chunks::<8>().0) {
            *word = u64::from_le_bytes(*chunk);
        }
        let [frame_word, position, entry_stack_pointer, stack_pointer] = words;
        let depth = (frame_word & u64::from(u32::MAX)) as usize;
        let (func, block, inst) =
            (FuncId((frame_word >> 32) as u32), BlockId(position as u32), (position >> 32) as usize);
        // Inside a call with a budget a jump may not leave its callee's frame.
        let floor = self.guards().last().unwrap_or(self.base_depth);
        if depth < floor || depth >= self.frames.len() {
            return Err(Fault::BadLongjmp);
        }
        let target = self.frames[depth];
        if target.func != func || target.entry_stack_pointer != entry_stack_pointer {
            return Err(Fault::BadLongjmp);
        }
        let function = self.function(func);
        let dst = match function.blocks.get(block.0 as usize).and_then(|b| b.insts.get(inst)) {
            Some(Inst::Setjmp { dst, .. }) => *dst,
            _ => return Err(Fault::BadLongjmp),
        };
        // Within the target frame's allocations, at or above where its allocations stand now.
        let allocations_top = target.entry_stack_pointer - function.slot_count() as u64 * SLOT_BYTES;
        let lowest = self.frames.get(depth + 1).map_or(self.stack_pointer, |frame| frame.entry_stack_pointer);
        if stack_pointer < lowest || stack_pointer > allocations_top {
            return Err(Fault::BadLongjmp);
        }
        let discarded = self.frames.len() - depth - 1;
        for index in (depth + 1..self.frames.len()).rev() {
            let frame = self.frames[index];
            self.record_word(pack_position(frame.func, frame.block, frame.inst));
            self.record_word(u128::from(frame.entry_stack_pointer));
        }
        self.record_word(pack_position(target.func, target.block, target.inst));
        self.record_word(u128::from(self.stack_pointer));
        self.record_word(discarded as u128);
        self.frames.truncate(depth + 1);
        self.frames_left();
        self.stack_pointer = stack_pointer;
        let top = self.top_mut();
        top.block = block;
        top.inst = inst;
        self.write_slot(dst, value);
        self.note_cells(&cells, range.start, &record, false);
        if let Some(journal) = &mut self.journal {
            journal.push_jump(discarded);
        }
        self.record(Undo::Longjmp);
        self.advance();
        Ok(())
    }

    /// The code address the frame `level` frames below the top returns to: the position after the call in
    /// the frame below it, [`ENTRY_RETURN_ADDRESS`] for the outermost frame, 0 beyond it. Frames below a
    /// coherent region are read like the region's own: they are the same on every branch.
    pub(crate) fn return_address(&self, level: u32) -> u64 {
        let Some(callee) = (self.frames.len() - 1).checked_sub(level as usize) else {
            return 0;
        };
        match callee.checked_sub(1) {
            None => ENTRY_RETURN_ADDRESS,
            Some(caller) => {
                let frame = self.frames[caller];
                self.function(frame.func).code_address_after(frame.func, frame.block, frame.inst)
            }
        }
    }

    /// Writes an instruction's result and moves on: the common end of a value instruction.
    fn produce(&mut self, dst: Slot, value: u128) {
        self.write_slot(dst, value);
        self.record(Undo::Slot);
        self.advance();
    }

    /// Overwrites a slot of the top frame, journaling the old value.
    pub(crate) fn write_slot(&mut self, slot: Slot, value: u128) {
        let index = self.top().base + slot.index();
        let old = std::mem::replace(&mut self.slots[index], value);
        self.record_word(old);
    }

    pub(crate) fn advance(&mut self) {
        self.top_mut().inst += 1;
    }

    /// The memory range a `width` access at `address` touches.
    pub(crate) fn memory_range(&self, address: u64, width: Width) -> Result<std::ops::Range<usize>, Fault> {
        let end = address.checked_add(width.bytes()).filter(|&end| end <= self.memory.len() as u64);
        let end = end.ok_or(Fault::OutOfBounds)?;
        Ok(address as usize..end as usize)
    }

    /// The bytes at `range`, little-endian, unmasked.
    pub(crate) fn read_bytes(&self, range: std::ops::Range<usize>) -> u128 {
        let mut bytes = [0; 16];
        self.memory.read_into(range.clone(), &mut bytes[..range.len()]);
        u128::from_le_bytes(bytes)
    }

    pub(crate) fn write_bytes(&mut self, range: std::ops::Range<usize>, value: u128) {
        self.memory.write(range.start, &value.to_le_bytes()[..range.len()]);
    }

    /// Measures the `width` bits from bit 0 of the integer at `range` (its bytes, little-endian): the bits
    /// of bytes that hold qubits are sampled jointly and the state collapsed onto them, only those bits;
    /// the bits of other bytes are the memory's, as a load reads them, with nothing to sample. Called with
    /// one world (`observe_world` first).
    fn measure_memory(&mut self, range: std::ops::Range<usize>, width: Width) -> u128 {
        let mut bytes = [0u8; 16];
        self.memory.read_into(range.clone(), &mut bytes[..range.len()]);
        let cells = self.quantum.cells_in(range.start as u64..range.end as u64);
        if !cells.is_empty() {
            let bits = u64::from(width.bits());
            let picks: Vec<(usize, u64)> = cells
                .iter()
                .map(|&(address, column)| {
                    let low = (address - range.start as u64) * 8;
                    let mask = if bits >= low + 8 { 0xff } else { (1u64 << (bits - low)) - 1 };
                    (column, mask)
                })
                .collect();
            let values = self.quantum.measure_bits(&picks, &mut self.rng);
            for (&(address, _), value) in cells.iter().zip(values) {
                bytes[address as usize - range.start] = value as u8;
            }
        }
        u128::from_le_bytes(bytes) & width.mask()
    }

    /// `dst = load width address`. Bytes that hold qubits are read as a register is read (`cells.rs`).
    fn load(&mut self, dst: Slot, width: Width, address: &'m Operand) -> Result<(), Fault> {
        let range = self.memory_range(self.read_word(address), width)?;
        let (cells, split) = self.read_cells(range.clone(), &|executor| executor.load(dst, width, address));
        let mut bytes = [0; 16];
        self.read_view(range.clone(), &cells, &mut bytes[..range.len()]);
        self.note_cells(&cells, range.start, &bytes, split);
        self.produce(dst, u128::from_le_bytes(bytes) & width.mask());
        Ok(())
    }

    /// `store width value, address`. Bytes that hold qubits are read first, then each gets the new value
    /// XORed over the old one (`cells.rs`).
    fn store(&mut self, width: Width, value: u128, address: u64) -> Result<(), Fault> {
        let range = self.memory_range(address, width)?;
        let (cells, split) = self.read_cells(range.clone(), &|executor| executor.store(width, value, address));
        if cells.is_empty() {
            let old = self.read_bytes(range.clone());
            self.write_bytes(range, value);
            self.record_word(old);
        } else {
            let mut old = [0; 16];
            self.read_view(range.clone(), &cells, &mut old[..range.len()]);
            let new = value.to_le_bytes();
            self.write_view(range.start, &new[..range.len()], &old[..range.len()], &cells);
            self.record_word(u128::from_le_bytes(old));
            self.note_cells(&cells, range.start, &new, split);
        }
        self.record(Undo::Store);
        self.advance();
        Ok(())
    }

    fn alloca(&mut self, dst: Slot, size: u64) -> Result<(), Fault> {
        let lowered = self.stack_pointer.checked_sub(size).map(|sp| sp & !(STACK_ALIGN - 1));
        let new_sp = lowered.filter(|&sp| sp >= self.stack_floor).ok_or(Fault::StackOverflow)?;
        let old_sp = std::mem::replace(&mut self.stack_pointer, new_sp);
        self.write_slot(dst, u128::from(new_sp));
        self.record_word(u128::from(old_sp));
        self.record(Undo::Alloca);
        self.advance();
        Ok(())
    }

    /// Sets the stack pointer back to `pointer`, freeing the allocations made since: within the current
    /// frame's allocations, from the stack pointer up to where the frame's slots end.
    fn stack_restore(&mut self, pointer: u64) -> Result<(), Fault> {
        let frame = *self.top();
        let slots = self.function(frame.func).slot_count() as u64 * SLOT_BYTES;
        let allocations_top = frame.entry_stack_pointer - slots;
        if pointer < self.stack_pointer || pointer > allocations_top {
            return Err(Fault::BadStackRestore);
        }
        let old = std::mem::replace(&mut self.stack_pointer, pointer);
        self.record_word(u128::from(old));
        self.record(Undo::StackRestore);
        self.advance();
        Ok(())
    }

    /// Calls `callee`. An argument that is a register passed to an integer parameter reads the register
    /// as an integer: if the world's configurations disagree on its value, the world splits into one world
    /// per value first (`docs/DESIGN.md`, "Worlds"), and the journal notes the read, which un-running the
    /// call checks.
    /// With a `guard`, the callee's frame gets that budget (a call with a budget): in every world, those
    /// that split off at its read included.
    fn call(&mut self, callee: FuncId, args: &'m [Operand], guard: Option<u64>) -> Result<(), Fault> {
        let function = self.function(callee);
        let mut reads = Vec::new();
        for (index, (arg, param)) in args.iter().zip(&function.params).enumerate() {
            if let (Operand::Slot(slot), Type::Int(_)) = (arg, param.ty)
                && matches!(self.slot_type(*slot), Type::Qreg(_))
            {
                reads.push((index, self.quantum.resolve(self.handle(*slot))?, param.slot));
            }
        }
        let mut split = false;
        if !reads.is_empty() {
            // A call that cannot enter faults in every world alike, before any splits.
            self.frame_fits(callee)?;
            let columns: Vec<usize> = reads.iter().map(|&(_, column, _)| column).collect();
            split = self.split(&columns, &|executor| executor.call(callee, args, guard)) || self.splitting;
        }
        let mut values = std::mem::take(&mut self.scratch);
        values.clear();
        values.extend(args.iter().map(|arg| self.read(arg)));
        if !reads.is_empty() {
            // A world's part that a deferred check emptied (`Executor::defer`) reads zero: it carries no
            // amplitude, and its sum with the other parts decides.
            let config = self.quantum.amplitudes().keys().next();
            for &(index, column, slot) in &reads {
                values[index] = config.map_or(0, |config| u128::from(config[column]));
                if let Some(journal) = &mut self.journal {
                    journal.push_read(column, slot, split);
                }
            }
        }
        let pushed = self.push_frame(callee, &values);
        self.scratch = values;
        if pushed.is_ok() && guard.is_some() {
            self.any_guard = true;
            self.top_mut().guard = guard;
        }
        pushed
    }

    /// The stack pointer below a new frame of `func`, if it fits on the stack.
    pub(crate) fn frame_fits(&self, func: FuncId) -> Result<u64, Fault> {
        let frame_bytes = (self.function(func).slot_count() as u64).checked_mul(SLOT_BYTES);
        let lowered = frame_bytes.and_then(|bytes| self.stack_pointer.checked_sub(bytes));
        lowered.filter(|&sp| sp >= self.stack_floor).ok_or(Fault::StackOverflow)
    }

    /// Enters `func` with `args`: reserves its frame on the stack and writes its parameters. The caller's
    /// position stays on the call until the callee returns.
    pub(crate) fn push_frame(&mut self, func: FuncId, args: &[u128]) -> Result<(), Fault> {
        let callee = self.function(func);
        let new_sp = self.frame_fits(func)?;
        let base = self.live_top();
        let end = base + callee.slot_count();
        if self.slots.len() < end {
            self.slots.resize(end, 0);
        }
        for (param, &value) in callee.params.iter().zip(args) {
            let old = std::mem::replace(&mut self.slots[base + param.slot.index()], value);
            self.record_word(old);
        }
        let entry_stack_pointer = std::mem::replace(&mut self.stack_pointer, new_sp);
        self.frames.push(Frame {
            func,
            block: bennett_ir::BlockId(0),
            inst: 0,
            base,
            entry_stack_pointer,
            guard: None,
        });
        self.record(Undo::Call);
        Ok(())
    }

    /// Moves to `target`, assigning all its parameters at once from the arguments' current values.
    fn jump(&mut self, target: &'m Target) {
        let mut values = std::mem::take(&mut self.scratch);
        values.clear();
        values.extend(target.args.iter().map(|arg| self.read(arg)));
        let frame = *self.top();
        let params = &self.function(frame.func).block(target.block).params;
        for (param, &value) in params.iter().zip(&values) {
            self.write_slot(param.slot, value);
        }
        self.scratch = values;
        self.record_word(u128::from(frame.block.0));
        self.record(Undo::Jump);
        let top = self.top_mut();
        top.block = target.block;
        top.inst = 0;
    }

    /// Returns `value` to the caller, or finishes the run when the frame is the run's outermost.
    fn ret(&mut self, value: Option<u128>) {
        if self.frames.len() == self.base_depth + 1 {
            self.outcome = Some(Outcome::Returned(value));
            self.record(Undo::Finish);
            return;
        }
        let callee = self.frames.pop().expect("a returning frame exists");
        self.frames_left();
        self.record_word(u128::from(callee.func.0) << 32 | u128::from(callee.block.0));
        self.record_word(u128::from(self.stack_pointer));
        if let Some(left) = callee.guard {
            self.record_word(u128::from(left));
        }
        self.stack_pointer = callee.entry_stack_pointer;
        let caller = *self.top();
        let call = &self.function(caller.func).block(caller.block).insts[caller.inst];
        if let Some(dst) = call.result() {
            let value = value.expect("the validator lets a call name a result only if the callee returns one");
            self.write_slot(dst, value);
        }
        // A call with a budget gives the fault code 0 on a return.
        if let Some(code) = call.fault_result() {
            self.write_slot(code, 0);
        }
        self.record(Undo::Return);
        self.advance();
    }

    /// Resolves the qubits of a gate and applies it, within `limit` basis configurations.
    pub(crate) fn gate(
        &mut self,
        gate: Gate,
        target: &Qubit,
        controls: &[Qubit],
        limit: Option<usize>,
    ) -> Result<(), Halt> {
        let target = self.qubit(target)?;
        let controls = controls.iter().map(|control| self.qubit(control)).collect::<Result<Vec<_>, _>>()?;
        if controls.iter().enumerate().any(|(k, c)| *c == target || controls[..k].contains(c)) {
            return Err(Fault::QubitAlias.into());
        }
        Ok(self.quantum.apply_gate(&gates::matrix(gate), target, &controls, limit)?)
    }

    fn qubit(&self, qubit: &Qubit) -> Result<QubitRef, Fault> {
        Ok(QubitRef { column: self.quantum.resolve(self.handle(qubit.register))?, bit: qubit.bit })
    }
}

/// A frame's position in one journal word: function, block and instruction index.
pub(crate) fn pack_position(func: FuncId, block: BlockId, inst: usize) -> u128 {
    u128::from(func.0) | u128::from(block.0) << 32 | (inst as u128) << 64
}

/// The function, block and instruction index of a packed position.
pub(crate) fn unpack_position(word: u128) -> (FuncId, BlockId, usize) {
    (FuncId(word as u32), BlockId((word >> 32) as u32), (word >> 64) as usize)
}
