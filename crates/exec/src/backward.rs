//! Undoing one journaled step. Each undo reads back what the forward step destroyed; everything else it
//! needs is still in the state, because every later step has already been undone.

use bennett_ir::{BlockId, FuncId, Inst, Slot, Type};

use crate::executor::{Executor, Frame};
use crate::forward::unpack_position;
use crate::gates;
use crate::history::Mismatch;
use crate::journal::{Journal, Undo};
use crate::quantum::Amplitudes;

impl<'m> Executor<'m> {
    /// Pops the most recent journal record and undoes its step, which the run took from this state.
    pub(crate) fn undo_last(&mut self) {
        self.unrun_last(false).expect("a step the run took is undone");
    }

    /// Pops the most recent journal record and undoes its step, unless that would not invert a step from
    /// this state; then it changes nothing. `checked` is set by a rewind's un-run, whose keeps changed
    /// registers the history may have read: it also refuses to un-run a `qalloc` whose register is not
    /// zero in every configuration, which would discard those configurations. Undoing a `rewind` record
    /// refuses whenever running its region again faults.
    ///
    /// Un-running a call that read registers as integers also refuses, checked, if a register no longer
    /// holds the value the call read. Below a world's split point these two checks are made on the sum of
    /// the worlds' parts instead: this world's part only notes what fails them (`Executor::defer`, see
    /// `history.rs`).
    pub(crate) fn unrun_last(&mut self, checked: bool) -> Result<(), Mismatch> {
        let deferred = checked && self.partial;
        let checked = checked && !self.partial;
        let last = self.journal.as_ref().and_then(Journal::last).expect("a record to undo");
        if (checked || deferred) && matches!(last, Undo::Qalloc | Undo::Call) {
            let failing =
                if last == Undo::Qalloc { self.allocation_undone_nonzero() } else { self.reads_no_longer_held() };
            if checked && !failing.is_empty() {
                return Err(Mismatch);
            }
            if deferred {
                self.defer(last, failing);
            }
        }
        if matches!(last, Undo::GateAt | Undo::GateAtPromoted) {
            return self.unrun_gate_at(checked, deferred, last == Undo::GateAtPromoted);
        }
        // A step that read or wrote memory qubits: they must still hold what it read or wrote.
        if (checked || deferred) && self.journal.as_ref().is_some_and(Journal::top_has_cells) {
            let failing = self.cells_no_longer_held();
            if checked && !failing.is_empty() {
                return Err(Mismatch);
            }
            if deferred {
                self.defer(last, failing);
            }
        }
        let undo = self.journal.as_mut().and_then(|journal| journal.pop()).expect("a record to undo");
        self.pop_cell_entries();
        match undo {
            Undo::Slot | Undo::Qalloc => self.undo_definition(undo),
            Undo::Mark => {
                self.undo_definition(undo);
                self.journal.as_mut().expect("a mark record is in the journal").pop_mark();
            }
            Undo::Rewind => {
                let rewind = self.retreat();
                let word = self.pop_word();
                if let Err(mismatch) = self.undo_rewind(rewind, word) {
                    self.advance();
                    self.record_word(word);
                    self.record(Undo::Rewind);
                    return Err(mismatch);
                }
            }
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
            Undo::Setjmp => {
                let Inst::Setjmp { buffer, .. } = self.retreat() else { unreachable!("a setjmp record") };
                let range = self.jump_record_range(self.read_word(buffer)).expect("a setjmp that ran is in bounds");
                self.restore_result();
                let high = self.pop_word();
                let low = self.pop_word();
                let cells = self.ordinary_cells(range.clone());
                if cells.is_empty() {
                    self.write_bytes(range.start..range.start + 16, low);
                    self.write_bytes(range.start + 16..range.end, high);
                } else {
                    let old = [low.to_le_bytes(), high.to_le_bytes()].concat();
                    let record = self.jump_record();
                    self.write_view(range.start, &old, &record, &cells);
                }
            }
            Undo::Longjmp => self.undo_longjmp(),
            Undo::Measure => unreachable!("reversal stops before a measurement"),
            Undo::GateAt | Undo::GateAtPromoted => unreachable!("a GateAt record is un-run before it is popped"),
        }
        Ok(())
    }

    /// The configurations where the register that the `qalloc` of the last record allocated is not zero.
    /// The top frame is just after that `qalloc`.
    fn allocation_undone_nonzero(&self) -> Amplitudes {
        let frame = *self.top();
        let inst = &self.function(frame.func).block(frame.block).insts[frame.inst - 1];
        let dst = inst.result().expect("qalloc defines its handle");
        let id = self.quantum.resolve(self.handle(dst)).expect("an allocation being undone is live");
        self.quantum.failing(|config| config[id] != 0)
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
        let Inst::Store { width, address, value } = self.retreat() else { unreachable!("a store record") };
        let range = self.memory_range(self.read_word(address), *width).expect("a store that ran forward is in bounds");
        let old = self.pop_word();
        let cells = self.ordinary_cells(range.clone());
        if cells.is_empty() {
            self.write_bytes(range, old);
        } else {
            let (old, new) = (old.to_le_bytes(), self.read(value).to_le_bytes());
            self.write_view(range.start, &old[..range.len()], &new[..range.len()], &cells);
        }
    }

    /// The cells among the bytes of `range` that ordinary code touches: none inside a coherent region,
    /// whose private memory holds the cells' values.
    fn ordinary_cells(&self, range: std::ops::Range<usize>) -> Vec<(u64, usize)> {
        if self.in_region { Vec::new() } else { self.quantum.cells_in(range.start as u64..range.end as u64) }
    }

    /// The configurations where a register the call of the last record read as an integer no longer holds
    /// the value it read. The top frame is the callee's.
    fn reads_no_longer_held(&self) -> Amplitudes {
        let journal = self.journal.as_ref().expect("undoing needs a journal");
        let record = journal.records() - 1;
        let base = self.top().base;
        let reads = journal.reads_from(record);
        if reads.is_empty() {
            return Amplitudes::new();
        }
        let values: Vec<(usize, u64)> =
            reads.iter().map(|read| (read.column, self.slots[base + read.slot.index()] as u64)).collect();
        self.quantum.failing(|config| values.iter().any(|&(column, value)| config[column] != value))
    }

    fn undo_call(&mut self) {
        // Below a read that split the world, a rewind's un-run is of this world's part of a shared state.
        let journal = self.journal.as_mut().expect("undoing needs a journal");
        let record = journal.records();
        while let Some(read) = journal.pop_read_of(record) {
            if read.split && self.rewinding > 0 {
                self.partial = true;
                self.pending |= self.deferring;
            }
        }
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

    /// Puts back the frames a `longjmp` discarded, and the target frame's position and stack pointer.
    fn undo_longjmp(&mut self) {
        self.journal.as_mut().expect("undoing needs a journal").pop_jump();
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
