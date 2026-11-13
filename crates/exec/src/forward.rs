//! Forward execution of one step. Every step either faults without changing anything, or changes the
//! state and journals exactly what it overwrote.

use bennett_ir::{Fault, FuncId, Gate, Inst, Operand, Qubit, Slot, Target, Terminator, Width};

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
            Inst::Load { width, dst, address } => {
                let value = self.load(self.read_word(address), *width)?;
                self.produce(*dst, value);
            }
            Inst::FuncAddr { dst, func } => self.produce(*dst, u128::from(func.address())),
            Inst::Store { width, value, address } => self.store(*width, self.read(value), self.read_word(address))?,
            Inst::Alloca { dst, size } => self.alloca(*dst, self.read_word(size))?,
            Inst::StackSave { dst } => self.produce(*dst, u128::from(self.stack_pointer)),
            Inst::StackRestore { pointer } => self.stack_restore(self.read_word(pointer))?,
            Inst::Call { callee, args, .. } => self.call(*callee, args)?,
            Inst::CallIndirect { signature, target, args, .. } => {
                let callee = self.module.function_at(self.read_word(target));
                let callee = callee.filter(|&f| self.function(f).has_signature(signature)).ok_or(Fault::BadIndirect)?;
                self.call(callee, args)?;
            }
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
                let id = self.quantum.resolve(self.handle(*register))?;
                self.quantum.free(id)?;
                self.record(Undo::Qfree);
                self.advance();
            }
            Inst::Gate { gate, target, controls } => {
                self.gate(*gate, target, controls, self.max_configurations)?;
                self.record(Undo::Gate);
                self.advance();
            }
            Inst::Measure { dst, register } => {
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
            Terminator::Fault(fault) => return Err((*fault).into()),
        }
        Ok(())
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

    fn advance(&mut self) {
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
        bytes[..range.len()].copy_from_slice(&self.memory[range]);
        u128::from_le_bytes(bytes)
    }

    pub(crate) fn write_bytes(&mut self, range: std::ops::Range<usize>, value: u128) {
        let len = range.len();
        self.memory[range].copy_from_slice(&value.to_le_bytes()[..len]);
    }

    fn load(&self, address: u64, width: Width) -> Result<u128, Fault> {
        Ok(self.read_bytes(self.memory_range(address, width)?) & width.mask())
    }

    fn store(&mut self, width: Width, value: u128, address: u64) -> Result<(), Fault> {
        let range = self.memory_range(address, width)?;
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

    fn call(&mut self, callee: FuncId, args: &[Operand]) -> Result<(), Fault> {
        let mut values = std::mem::take(&mut self.scratch);
        values.clear();
        values.extend(args.iter().map(|arg| self.read(arg)));
        let pushed = self.push_frame(callee, &values);
        self.scratch = values;
        pushed
    }

    /// Enters `func` with `args`: reserves its frame on the stack and writes its parameters. The caller's
    /// position stays on the call until the callee returns.
    pub(crate) fn push_frame(&mut self, func: FuncId, args: &[u128]) -> Result<(), Fault> {
        let callee = self.function(func);
        let frame_bytes = (callee.slot_count() as u64).checked_mul(SLOT_BYTES);
        let lowered = frame_bytes.and_then(|bytes| self.stack_pointer.checked_sub(bytes));
        let new_sp = lowered.filter(|&sp| sp >= self.stack_floor).ok_or(Fault::StackOverflow)?;
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
