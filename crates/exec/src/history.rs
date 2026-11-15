//! The history instructions: `mark` remembers a point of the journal, `rewind` XORs chosen values into
//! registers and then un-runs every step since that point (`docs/DESIGN.md`, "History: `mark` and
//! `rewind`").
//!
//! A rewind is one step. It is undone, by reversal or by an outer rewind, by running the rewound steps
//! again from the mark and XORing the kept values again: un-running it means running the region again, so
//! cleaning up inside cleaning-up regions doubles the work per level (measured in the tests).

use bennett_ir::{BlockId, Fault, Inst, Keep, Slot};

use crate::executor::{Executor, Halt};
use crate::journal::{Journal, MarkEntry, Undo};

impl<'m> Executor<'m> {
    /// `dst = mark`: the handle is the number of journal records up to and including the mark's own. A
    /// run that keeps no journal starts one here.
    pub(crate) fn mark(&mut self, dst: Slot) {
        let allocations = self.quantum.allocations();
        let journal = self.journal.get_or_insert_with(Journal::default);
        let position = journal.records() + 1;
        journal.push_mark(MarkEntry { position, allocations });
        self.write_slot(dst, position as u128);
        self.record(Undo::Mark);
        self.advance();
    }

    /// `rewind mark keep (...)`: XORs each kept value into its register, un-runs every step since the
    /// mark, and continues after the rewind. Faults without changing anything if it cannot.
    pub(crate) fn rewind(&mut self, mark: Slot, keep: &'m [Keep]) -> Result<(), Halt> {
        if self.in_region && !keep.is_empty() {
            return Err(Fault::QuantumInRegion.into());
        }
        let entry = self.open_mark(self.slot(mark)).ok_or(Fault::BadRewind)?;
        let mut kept = Vec::with_capacity(keep.len());
        for pair in keep {
            let column = self.quantum.resolve(self.handle(pair.register))?;
            // Un-running the allocation of a register that holds a kept value would discard the value.
            if self.quantum.allocation_number(column) > entry.allocations {
                return Err(Fault::BadRewind.into());
            }
            kept.push((column, self.read(&pair.value) as u64));
        }
        for (column, value) in kept {
            self.quantum.xor_register(column, value);
        }
        let rewind_at = *self.top();
        let saved = std::mem::replace(&mut self.allow_region_faults, true);
        while self.journal.as_ref().is_some_and(|journal| journal.records() > entry.position) {
            self.undo_last();
            self.rewound_steps += 1;
        }
        self.allow_region_faults = saved;
        let frame = self.top_mut();
        let mark_at = (frame.block, frame.inst);
        (frame.block, frame.inst) = (rewind_at.block, rewind_at.inst);
        self.record_word(u128::from(mark_at.0.0) << 32 | mark_at.1 as u128);
        self.record(Undo::Rewind);
        self.advance();
        Ok(())
    }

    /// The mark a handle names, if a rewind to it is possible: it is a mark of the running activation
    /// (the frame was never left below since), and no measurement or finish was journalled since.
    fn open_mark(&self, handle: u128) -> Option<MarkEntry> {
        let journal = self.journal.as_ref()?;
        let entry = journal.mark_at(usize::try_from(handle).ok()?)?;
        let mut depth = 0usize;
        // A longjmp left the frames it discarded, as that many returns would have.
        let mut jumps = journal.jumps().iter().rev();
        for index in (entry.position..journal.records()).rev() {
            match journal.kind_at(index) {
                Undo::Return => depth += 1,
                Undo::Longjmp => depth += jumps.next().expect("a longjmp record has an entry"),
                Undo::Call => depth = depth.checked_sub(1)?,
                Undo::Measure | Undo::Finish => return None,
                _ => {}
            }
        }
        (depth == 0).then_some(entry)
    }

    /// Undoes a `rewind` record: the top frame is back after the rewind instruction. Runs the rewound
    /// steps again from where the mark left the frame, up to the rewind, then XORs the kept values again.
    pub(crate) fn undo_rewind(&mut self, rewind: &'m Inst, mark_at: u128) {
        let Inst::Rewind { keep, .. } = rewind else { unreachable!("a rewind record") };
        let stop = *self.top();
        let depth = self.frames.len();
        let frame = self.top_mut();
        (frame.block, frame.inst) = (BlockId((mark_at >> 32) as u32), mark_at as u32 as usize);
        self.run_again_to(depth, (stop.block, stop.inst));
        for pair in keep {
            let column = self.quantum.resolve(self.handle(pair.register)).expect("a kept register is live again");
            self.quantum.xor_register(column, self.read(&pair.value) as u64);
        }
    }

    /// Steps until the frame at `depth` is the top one and at `stop`. The steps ran before from the same
    /// state, so they reach it; limits and checks that they passed then are not applied again, and the
    /// step count is left as it was.
    fn run_again_to(&mut self, depth: usize, stop: (BlockId, usize)) {
        let budget = self.budget.take();
        let max_configurations = self.max_configurations.take();
        let measurement = self.journal_measurement.take();
        let allow_region_faults = std::mem::replace(&mut self.allow_region_faults, true);
        let steps = self.steps;
        while self.frames.len() != depth || (self.top().block, self.top().inst) != stop {
            let outcome = self.step();
            assert!(outcome.is_none(), "steps that ran to a rewind run to it again, not to {outcome:?}");
            self.rerun_steps += 1;
        }
        self.steps = steps;
        self.allow_region_faults = allow_region_faults;
        self.journal_measurement = measurement;
        self.max_configurations = max_configurations;
        self.budget = budget;
    }
}
