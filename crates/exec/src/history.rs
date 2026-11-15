//! The history instructions: `mark` remembers a point of the journal, `rewind` XORs chosen values into
//! registers and then un-runs every step since that point (`docs/DESIGN.md`, "History: `mark` and
//! `rewind`").
//!
//! A rewind is one step. It is undone, by reversal or by an outer rewind, by running the rewound steps
//! again from the mark and XORing the kept values again: un-running it means running the region again, so
//! cleaning up inside cleaning-up regions doubles the work per level (measured in the tests).
//!
//! The keeps change registers the region may have read, so its un-run can meet a state its history does
//! not describe: a register allocated since the mark that is not zero when its allocation is un-run, or an
//! inner rewind whose region faults when it is run again. The rewind then runs forward again the steps it
//! un-ran, XORs the keeps out, and faults with `bad_rewind`, having changed nothing.

use bennett_ir::{BlockId, Fault, Inst, Keep, Slot};

use crate::executor::{Executor, Halt, Outcome};
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
        for &(column, value) in &kept {
            self.quantum.xor_register(column, value);
        }
        let rewind_at = *self.top();
        let saved = std::mem::replace(&mut self.allow_region_faults, true);
        // The steps un-run so far: as many steps as running the region again takes (`undo_rewind`).
        let mut undone = 0u64;
        let mut matched = Ok(());
        while matched.is_ok() && self.journal.as_ref().is_some_and(|journal| journal.records() > entry.position) {
            matched = self.unrun_last(true);
            if matched.is_ok() {
                undone += 1;
                self.rewound_steps += 1;
            }
        }
        self.allow_region_faults = saved;
        if matched.is_err() {
            self.run_again(undone).expect("steps just un-run run forward again");
            assert_eq!((self.top().block, self.top().inst), (rewind_at.block, rewind_at.inst), "back at the rewind");
            for (column, value) in kept {
                self.quantum.xor_register(column, value);
            }
            return Err(Fault::BadRewind.into());
        }
        let frame = self.top_mut();
        let mark_at = (frame.block, frame.inst);
        (frame.block, frame.inst) = (rewind_at.block, rewind_at.inst);
        self.record_word(u128::from(undone) << 64 | u128::from(mark_at.0.0) << 32 | mark_at.1 as u128);
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

    /// Undoes a `rewind` record, popped with its word, the top frame back on the rewind instruction. Runs
    /// the rewound steps again from where the mark left the frame, as many as the rewind un-ran, so a
    /// rewind reached again in a loop is restored at the visit being undone; then XORs the kept values
    /// again. If running again faults (`run_again`), the frame is put back on the rewind and nothing else
    /// has changed.
    pub(crate) fn undo_rewind(&mut self, rewind: &'m Inst, word: u128) -> Result<(), Mismatch> {
        let Inst::Rewind { keep, .. } = rewind else { unreachable!("a rewind record") };
        let stop = *self.top();
        let depth = self.frames.len();
        let frame = self.top_mut();
        (frame.block, frame.inst) = (BlockId((word >> 32) as u32), word as u32 as usize);
        if let Err(mismatch) = self.run_again((word >> 64) as u64) {
            let frame = self.top_mut();
            (frame.block, frame.inst) = (stop.block, stop.inst);
            return Err(mismatch);
        }
        assert!(
            self.frames.len() == depth && (self.top().block, self.top().inst) == (stop.block, stop.inst),
            "the steps of a rewound region run again to the rewind"
        );
        for pair in keep {
            let column = self.quantum.resolve(self.handle(pair.register)).expect("a kept register is live again");
            self.quantum.xor_register(column, self.read(&pair.value) as u64);
        }
        Ok(())
    }

    /// Runs `steps` steps again. They ran before from the same classical state, so limits and checks that
    /// they passed then are not applied again, and the step count is left as it was. Under a rewind's
    /// keeps a step can still fault (a `qfree` of a register that is not zero, an inner `bad_rewind`):
    /// then the steps run so far are undone, and the state is as it was.
    pub(crate) fn run_again(&mut self, steps: u64) -> Result<(), Mismatch> {
        assert!(self.outcome.is_none(), "steps run again from a running state");
        let budget = self.budget.take();
        let max_configurations = self.max_configurations.take();
        let measurement = self.journal_measurement.take();
        let allow_region_faults = std::mem::replace(&mut self.allow_region_faults, true);
        let saved_steps = self.steps;
        let records = self.journal.as_ref().map_or(0, Journal::records);
        let mut result = Ok(());
        for _ in 0..steps {
            if let Some(outcome) = self.step() {
                assert!(matches!(outcome, Outcome::Faulted(_)), "steps that ran before run again, not to {outcome:?}");
                self.outcome = None;
                // The steps run so far left records above `records` only: a rewind among them pops back
                // to a mark at or above it.
                while self.journal.as_ref().is_some_and(|journal| journal.records() > records) {
                    self.undo_last();
                    self.rewound_steps += 1;
                }
                result = Err(Mismatch);
                break;
            }
            self.rerun_steps += 1;
        }
        self.steps = saved_steps;
        self.allow_region_faults = allow_region_faults;
        self.journal_measurement = measurement;
        self.max_configurations = max_configurations;
        self.budget = budget;
        result
    }
}

/// An un-run that met a state its history does not describe. It changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mismatch;
