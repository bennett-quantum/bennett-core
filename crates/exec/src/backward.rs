//! Undoing one journaled step. Each undo reads back what the forward step destroyed; everything else it
//! needs is still in the state, because every later step has already been undone.

use bennett_ir::{BlockId, FuncId, Inst, Slot, Type};

use crate::device_io::DeviceContext;
use crate::executor::{Executor, Frame};
use crate::forward::unpack_position;
use crate::gates;
use crate::journal::Undo;

impl<'m> Executor<'m> {
    /// Pops the most recent journal record and undoes its step, which the run took from this state.
    pub(crate) fn undo_last(&mut self) {
        let undo = self.journal.as_mut().and_then(|journal| journal.pop()).expect("a record to undo");
        match undo {
            Undo::Slot | Undo::Qalloc => self.undo_definition(undo),
            Undo::Store => self.undo_store(),
            Undo::Alloca => {
                self.retreat();
                self.stack_pointer = self.pop_word() as u64;
                self.restore_result();
            }
            Undo::StackRestore => {
                self.retreat();
                self.stack_pointer = self.pop_word() as u64;
            }
            Undo::Effect => {
                self.retreat();
                self.tape.pop();
            }
            Undo::Call => self.undo_call(),
            Undo::Jump => self.undo_jump(),
            Undo::Return => self.undo_return(),
            Undo::Finish => self.outcome = None,
            Undo::Gate => {
                let Inst::Gate { gate, target, controls } = self.retreat() else { unreachable!("a gate record") };
                // Undoing returns to a state the run already held, so the configuration limit is not
                // applied: rounding residue must not make reversal fail.
                let inverse = gates::inverse(*gate);
                self.gate(inverse, target, controls, &mut DeviceContext::default())
                    .expect("a gate that ran forward runs inverted");
            }
            Undo::Qfree => {
                let Inst::Qfree { register } = self.retreat() else { unreachable!("a qfree record") };
                let Type::Qreg(qubits) = self.slot_type(*register) else { unreachable!("a register slot") };
                self.quantum.revive(self.handle(*register), qubits);
            }
            Undo::Leftover => {
                let Inst::Qfree { register } = self.retreat() else { unreachable!("a qfree record") };
                let Type::Qreg(qubits) = self.slot_type(*register) else { unreachable!("a register slot") };
                self.quantum.take_back(self.handle(*register), qubits);
            }
            Undo::Apply => {
                let Inst::Apply { callee, args, out, fault, budget } = self.retreat() else {
                    unreachable!("an apply record")
                };
                // The operation is its own inverse, including fault codes written by a gate circuit.
                let allow = std::mem::replace(&mut self.allow_region_faults, true);
                let result = self.apply(*callee, args, *out, *fault, budget);
                self.allow_region_faults = allow;
                result.expect("an apply that ran forward runs again");
            }
            Undo::Setjmp => {
                let Inst::Setjmp { buffer, .. } = self.retreat() else { unreachable!("a setjmp record") };
                let range = self.jump_record_range(self.read_word(buffer)).expect("a setjmp that ran is in bounds");
                self.restore_result();
                let high = self.pop_word();
                let low = self.pop_word();
                self.write_bytes(range.start..range.start + 16, low);
                self.write_bytes(range.start + 16..range.end, high);
            }
            Undo::Longjmp => self.undo_longjmp(),
            Undo::GateAt => self.undo_gate_at(false),
            Undo::GateAtPromoted => self.undo_gate_at(true),
            Undo::Measure => unreachable!("reversal stops before a measurement"),
        }
    }

    /// Undoes every record in the journal; returns the records popped.
    pub(crate) fn unwind(&mut self) -> u64 {
        let mut undone = 0;
        while self.journal.as_ref().is_some_and(|journal| journal.last().is_some()) {
            self.undo_last();
            undone += 1;
        }
        undone
    }

    fn undo_definition(&mut self, undo: Undo) {
        let inst = self.retreat();
        if undo == Undo::Qalloc {
            let dst = inst.result().expect("qalloc defines its handle");
            let Type::Qreg(qubits) = self.slot_type(dst) else { unreachable!("a register slot") };
            self.quantum.unallocate(self.handle(dst), qubits);
        }
        self.restore_result();
    }

    fn undo_store(&mut self) {
        let Inst::Store { width, address, .. } = self.retreat() else { unreachable!("a store record") };
        let range = self.memory_range(self.read_word(address), *width).expect("a store that ran forward is in bounds");
        let old = self.pop_word();
        self.write_bytes(range, old);
    }

    fn undo_call(&mut self) {
        let frame = self.frames.pop().expect("a pushed frame");
        let end = frame.base + self.function(frame.func).slot_count();
        self.journal.as_mut().expect("undoing needs a journal").restore_words(&mut self.slots[frame.base..end]);
        self.stack_pointer = frame.entry_stack_pointer;
    }

    fn undo_jump(&mut self) {
        let source = BlockId(self.pop_word() as u32);
        let frame = *self.top();
        let function = self.function(frame.func);
        for param in function.block(frame.block).params.iter().rev() {
            self.slots[frame.base + param.slot.index()] = self.pop_word();
        }
        let top = self.top_mut();
        top.block = source;
        top.inst = function.block(source).insts.len();
    }

    fn undo_return(&mut self) {
        let call = self.retreat();
        if call.result().is_some() {
            self.restore_result();
        }
        let callee_stack_pointer = self.pop_word() as u64;
        let packed = self.pop_word() as u64;
        let (func, block) = (FuncId((packed >> 32) as u32), BlockId(packed as u32));
        let frame = Frame {
            func,
            block,
            inst: self.function(func).block(block).insts.len(),
            base: self.live_top(),
            entry_stack_pointer: self.stack_pointer,
        };
        self.frames.push(frame);
        self.stack_pointer = callee_stack_pointer;
    }

    /// Puts back the frames a `longjmp` discarded, and the target frame's position and stack pointer.
    fn undo_longjmp(&mut self) {
        self.retreat();
        self.restore_result();
        let discarded = self.pop_word() as usize;
        self.stack_pointer = self.pop_word() as u64;
        let (_, block, inst) = unpack_position(self.pop_word());
        let top = self.top_mut();
        top.block = block;
        top.inst = inst;
        for _ in 0..discarded {
            let entry_stack_pointer = self.pop_word() as u64;
            let (func, block, inst) = unpack_position(self.pop_word());
            let base = self.live_top();
            self.frames.push(Frame { func, block, inst, base, entry_stack_pointer });
        }
    }

    /// Steps the top frame back onto the instruction being undone, and returns it.
    pub(crate) fn retreat(&mut self) -> &'m Inst {
        let top = self.top_mut();
        top.inst -= 1;
        let frame = *top;
        &self.function(frame.func).block(frame.block).insts[frame.inst]
    }

    /// Restores the result slot of the instruction at the top frame's position.
    pub(crate) fn restore_result(&mut self) {
        let frame = *self.top();
        let inst = &self.function(frame.func).block(frame.block).insts[frame.inst];
        let dst: Slot = inst.result().expect("the instruction defines a result");
        self.slots[frame.base + dst.index()] = self.pop_word();
    }

    fn pop_word(&mut self) -> u128 {
        self.journal.as_mut().expect("undoing needs a journal").pop_word()
    }
}
