//! Undoing one journaled step. Each undo reads back what the forward step destroyed; everything else it
//! needs is still in the state, because every later step has already been undone.

use bennett_ir::{BlockId, FuncId, Inst, Slot, Type};

use crate::executor::{Executor, Frame};
use crate::gates;
use crate::journal::Undo;

impl<'m> Executor<'m> {
    /// Pops the most recent journal record and undoes its step.
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
                self.gate(inverse, target, controls, None).expect("a gate that ran forward runs inverted");
            }
            Undo::Qfree => {
                let Inst::Qfree { register } = self.retreat() else { unreachable!("a qfree record") };
                let Type::Qreg(width) = self.slot_type(*register) else { unreachable!("a register slot") };
                self.quantum.revive(self.handle(*register), width);
            }
            Undo::Apply => {
                let Inst::Apply { callee, args, out, fault, budget } = self.retreat() else {
                    unreachable!("an apply record")
                };
                // A coherent region is its own inverse.
                self.apply(*callee, args, *out, *fault, budget).expect("an apply that ran forward runs again");
            }
            Undo::Measure => unreachable!("reversal stops before a measurement"),
        }
    }

    /// Undoes every record in the journal.
    pub(crate) fn unwind(&mut self) {
        while self.journal.as_ref().is_some_and(|journal| journal.last().is_some()) {
            self.undo_last();
        }
    }

    fn undo_definition(&mut self, undo: Undo) {
        let inst = self.retreat();
        if undo == Undo::Qalloc {
            let dst = inst.result().expect("qalloc defines its handle");
            self.quantum.unallocate(self.handle(dst));
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
        for param in self.function(frame.func).params.iter().rev() {
            self.slots[frame.base + param.slot.index()] = self.pop_word();
        }
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

    /// Steps the top frame back onto the instruction being undone, and returns it.
    fn retreat(&mut self) -> &'m Inst {
        let top = self.top_mut();
        top.inst -= 1;
        let frame = *top;
        &self.function(frame.func).block(frame.block).insts[frame.inst]
    }

    /// Restores the result slot of the instruction at the top frame's position.
    fn restore_result(&mut self) {
        let frame = *self.top();
        let inst = &self.function(frame.func).block(frame.block).insts[frame.inst];
        let dst: Slot = inst.result().expect("the instruction defines a result");
        self.slots[frame.base + dst.index()] = self.pop_word();
    }

    fn pop_word(&mut self) -> u128 {
        self.journal.as_mut().expect("undoing needs a journal").pop_word()
    }
}
