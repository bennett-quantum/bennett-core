//! Forward execution of one step. Every step either faults without changing anything, or changes the
//! state and journals exactly what it overwrote.

use bennett_ir::{
    BlockId, ENTRY_RETURN_ADDRESS, Fault, FuncId, Gate, Inst, JUMP_RECORD_BYTES, MeasureSource, Operand, Qubit, Slot,
    Target, Terminator, Type, Width,
};

use crate::executor::{Executor, Frame, Halt, Outcome, SLOT_BYTES};
use crate::gates;
use crate::journal::Undo;
use crate::quantum::QubitRef;

/// `alloca` results are aligned to this many bytes, the size of the widest value.
const STACK_ALIGN: u64 = 8;

impl<'m> Executor<'m> {
    /// Executes the instruction or terminator at the top frame's position.
    pub(crate) fn execute(&mut self) -> Result<(), Halt> {
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
            Inst::Call { callee, args, .. } => self.call(*callee, args)?,
            Inst::CallIndirect { signature, target, args, .. } => {
                let callee = self.module.function_at(self.read_word(target));
                let callee = callee.filter(|&f| self.function(f).has_signature(signature)).ok_or(Fault::BadIndirect)?;
                self.call(callee, args)?;
            }
            Inst::Setjmp { dst, buffer } => self.setjmp(*dst, self.read_word(buffer))?,
            Inst::Effect { byte } => {
                if self.in_region {
                    return Err(Fault::EffectInRegion.into());
                }
                self.tape.push(self.read(byte) as u8);
                self.record(Undo::Effect);
                self.advance();
            }
            _ if self.in_region => return Err(Fault::QuantumInRegion.into()),
            Inst::Qalloc { dst, width } => {
                let handle = self.quantum.allocate(*width);
                self.write_slot(*dst, u128::from(handle));
                self.record(Undo::Qalloc);
                self.advance();
            }
            Inst::Qfree { register } => {
                // The partial trace: the register is dropped unexamined, whatever it holds. How the simulator
                // stores the result is its own choice (`docs/DESIGN.md`, "Releasing a register").
                let id = self.quantum.resolve(self.handle(*register))?;
                if self.quantum.is_zero(id) {
                    self.quantum.free(id);
                    self.record(Undo::Qfree);
                } else {
                    self.nonzero_releases += 1;
                    self.nonzero_release_weight += self.quantum.nonzero_weight(id);
                    self.quantum.leave(id);
                    self.record(Undo::Leftover);
                }
                self.advance();
            }
            Inst::Gate { gate, target, controls } => {
                self.gate(*gate, target, controls, self.max_configurations)?;
                self.record(Undo::Gate);
                self.advance();
            }
            Inst::GateAt { gate, target, controls } => {
                self.gate_at(*gate, target, controls, self.max_configurations)?;
            }
            Inst::Measure { dst, source: MeasureSource::Memory { width, address } } => {
                let range = self.memory_range(self.read_word(address), *width)?;
                let value = self.measure_memory(range, *width);
                self.write_slot(*dst, value);
                self.record(Undo::Measure);
                self.advance();
            }
            Inst::Measure { dst, source: MeasureSource::Register(register) } => {
                let id = self.quantum.resolve(self.handle(*register))?;
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
                if self.in_region {
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
                self.longjmp(self.read_word(buffer), self.read(value))?;
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

    /// Writes a jump record naming the top frame and this `setjmp` at `buffer`, and gives 0. A buffer byte
    /// that holds qubits faults (`cells.rs`).
    fn setjmp(&mut self, dst: Slot, buffer: u64) -> Result<(), Halt> {
        let range = self.jump_record_range(buffer)?;
        self.check_no_cells(range.clone())?;
        let record = self.jump_record();
        self.record_word(self.read_bytes(range.start..range.start + 16));
        self.record_word(self.read_bytes(range.start + 16..range.end));
        self.memory.write(range.start, &record);
        self.write_slot(dst, 0);
        self.record(Undo::Setjmp);
        self.advance();
        Ok(())
    }

    /// Resumes the frame and `setjmp` the record at `buffer` names, with `value` as the `setjmp`'s
    /// result: discards the frames above it and sets the stack pointer back to where the `setjmp` left it.
    /// Every field is checked against the live frames, so a record that names no live frame of this run
    /// (a frame that returned, or one outside a coherent region) faults instead of jumping.
    fn longjmp(&mut self, buffer: u64, value: u128) -> Result<(), Halt> {
        let range = self.jump_record_range(buffer)?;
        self.check_no_cells(range.clone())?;
        let mut record = [0u8; JUMP_RECORD_BYTES as usize];
        self.memory.read_into(range, &mut record);
        let mut words = [0u64; 4];
        for (word, chunk) in words.iter_mut().zip(record.as_chunks::<8>().0) {
            *word = u64::from_le_bytes(*chunk);
        }
        let [frame_word, position, entry_stack_pointer, stack_pointer] = words;
        let depth = (frame_word & u64::from(u32::MAX)) as usize;
        let (func, block, inst) =
            (FuncId((frame_word >> 32) as u32), BlockId(position as u32), (position >> 32) as usize);
        // Inside a coherent region a jump may not leave the region.
        if depth < self.base_depth || depth >= self.frames.len() {
            return Err(Fault::BadLongjmp.into());
        }
        let target = self.frames[depth];
        if target.func != func || target.entry_stack_pointer != entry_stack_pointer {
            return Err(Fault::BadLongjmp.into());
        }
        let function = self.function(func);
        let dst = match function.blocks.get(block.0 as usize).and_then(|b| b.insts.get(inst)) {
            Some(Inst::Setjmp { dst, .. }) => *dst,
            _ => return Err(Fault::BadLongjmp.into()),
        };
        // Within the target frame's allocations, at or above where its allocations stand now.
        let allocations_top = target.entry_stack_pointer - function.slot_count() as u64 * SLOT_BYTES;
        let lowest = self.frames.get(depth + 1).map_or(self.stack_pointer, |frame| frame.entry_stack_pointer);
        if stack_pointer < lowest || stack_pointer > allocations_top {
            return Err(Fault::BadLongjmp.into());
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
        self.stack_pointer = stack_pointer;
        let top = self.top_mut();
        top.block = block;
        top.inst = inst;
        self.write_slot(dst, value);
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
    /// the bits of other bytes are the memory's, as a load reads them, with nothing to sample. A byte whose
    /// eight bits are all measured stops holding qubits: it is ordinary memory again, holding the result.
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
            for ((&(address, column), &(_, mask)), value) in cells.iter().zip(&picks).zip(values) {
                bytes[address as usize - range.start] = value as u8;
                // A byte whose eight bits were all measured is definite again: it becomes ordinary memory holding
                // the result, its column (now holding that value in every configuration) cleared and released.
                if mask == 0xff {
                    self.quantum.xor_register(column, value);
                    self.quantum.free(column);
                    self.quantum.bind_cell(address, None);
                    self.memory.write(address as usize, &[value as u8]);
                }
            }
        }
        u128::from_le_bytes(bytes) & width.mask()
    }

    /// `dst = load width address`. A byte that holds qubits faults (`cells.rs`).
    fn load(&mut self, dst: Slot, width: Width, address: &'m Operand) -> Result<(), Fault> {
        let range = self.memory_range(self.read_word(address), width)?;
        self.check_no_cells(range.clone())?;
        self.produce(dst, self.read_bytes(range) & width.mask());
        Ok(())
    }

    /// `store width value, address`. A byte that holds qubits faults (`cells.rs`).
    fn store(&mut self, width: Width, value: u128, address: u64) -> Result<(), Fault> {
        let range = self.memory_range(address, width)?;
        self.check_no_cells(range.clone())?;
        let old = self.read_bytes(range.clone());
        self.write_bytes(range, value);
        self.record_word(old);
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

    /// Calls `callee` with the values of `args`.
    fn call(&mut self, callee: FuncId, args: &'m [Operand]) -> Result<(), Fault> {
        let mut values = std::mem::take(&mut self.scratch);
        values.clear();
        values.extend(args.iter().map(|arg| self.read(arg)));
        let pushed = self.push_frame(callee, &values);
        self.scratch = values;
        pushed
    }

    /// The stack pointer below a new frame of `func`, if it fits on the stack.
    pub(crate) fn frame_fits(&self, func: FuncId) -> Result<u64, Fault> {
        let frame_bytes = (self.function(func).slot_count() as u64).checked_mul(SLOT_BYTES);
        let lowered = frame_bytes.and_then(|bytes| self.stack_pointer.checked_sub(bytes));
        lowered.filter(|&sp| sp >= self.stack_floor).ok_or(Fault::StackOverflow)
    }

    /// Enters `func` with zeroed slots and writes its parameters. The caller stays on the call until return.
    pub(crate) fn push_frame(&mut self, func: FuncId, args: &[u128]) -> Result<(), Fault> {
        let callee = self.function(func);
        let new_sp = self.frame_fits(func)?;
        let base = self.live_top();
        let end = base + callee.slot_count();
        if self.slots.len() < end {
            self.slots.resize(end, 0);
        }
        let slots = &mut self.slots[base..end];
        if let Some(journal) = &mut self.journal {
            // Undoing an earlier return or longjmp can bring these slots back to life.
            journal.push_words(slots);
        }
        slots.fill(0);
        for (param, &value) in callee.params.iter().zip(args) {
            slots[param.slot.index()] = value;
        }
        let entry_stack_pointer = std::mem::replace(&mut self.stack_pointer, new_sp);
        self.frames.push(Frame { func, block: bennett_ir::BlockId(0), inst: 0, base, entry_stack_pointer });
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
        self.record_word(u128::from(callee.func.0) << 32 | u128::from(callee.block.0));
        self.record_word(u128::from(self.stack_pointer));
        self.stack_pointer = callee.entry_stack_pointer;
        let caller = *self.top();
        let call = &self.function(caller.func).block(caller.block).insts[caller.inst];
        if let Some(dst) = call.result() {
            let value = value.expect("the validator lets a call name a result only if the callee returns one");
            self.write_slot(dst, value);
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
