//! The history instructions: `mark` remembers a point of the journal, `rewind` XORs chosen values into
//! registers and then un-runs every step since that point (`docs/DESIGN.md`, "History: `mark` and
//! `rewind`").
//!
//! A rewind is one step. It is undone, by reversal or by an outer rewind, by running the rewound steps
//! again from the mark and XORing the kept values again: un-running it means running the region again, so
//! cleaning up inside cleaning-up regions doubles the work per level (measured in the tests).
//!
//! The keeps change registers the region may have read, so its un-run can meet a state its history does
//! not describe: a register read as an integer that no longer holds the value read, a register allocated
//! since the mark that is not zero when its allocation is un-run, or an inner rewind whose region faults
//! when it is run again (a fault, or a `measure` reached on a new path, in any world split off on the way).
//! The rewind then faults with `bad_rewind`, having changed nothing: it runs forward
//! again the steps it un-ran and XORs the keeps out, or, if worlds split off during its un-run, restores a
//! copy it took first.
//!
//! With worlds (`docs/DESIGN.md`, "Worlds"), a world un-runs its own history down to where it split from
//! others, then its *part* of the history they share. A part need not hold the value its shared history
//! read, nor be zero where a register was allocated: only the sum of the parts must. Below a world's split
//! point (`partial`) each of those checks is therefore deferred: the part notes the configurations that
//! fail it (`Executor::defer`), and once every world that rewound has done so, the noted parts of each
//! check are summed, which must be zero; if not, every such world is put back from a copy kept before its
//! rewind, and faults `bad_rewind` (`worlds.rs`).

use bennett_ir::{BlockId, Fault, Inst, Keep, KeepTarget, Operand, Slot, Type};

use crate::executor::{Executor, Halt};
use crate::journal::{Journal, MarkEntry, Undo};

impl<'m> Executor<'m> {
    /// `dst = mark`: the handle is the number of journal records up to and including the mark's own. A
    /// run that keeps no journal starts one here. Inside a coherent region the records are counted from
    /// the region's start, as on a journal of its own whose first record is the call into the region's
    /// function, so a handle never depends on the caller's history (`docs/DESIGN.md`, "History").
    pub(crate) fn mark(&mut self, dst: Slot) {
        let allocations = self.quantum.allocations();
        let journal = self.journal.get_or_insert_with(Journal::default);
        let position = journal.records() + 1;
        journal.push_mark(MarkEntry { position, allocations });
        self.write_slot(dst, (position - self.region_records) as u128);
        self.record(Undo::Mark);
        self.advance();
    }

    /// `rewind mark keep (...)`: XORs each kept value into its register, un-runs every step since the
    /// mark, and continues after the rewind. Faults without changing anything if it cannot. Worlds that
    /// split off during the un-run (it runs earlier rewinds again) finish the rewind too.
    pub(crate) fn rewind(&mut self, mark: Slot, keep: &'m [Keep]) -> Result<(), Halt> {
        if self.in_region && !keep.is_empty() {
            return Err(Fault::QuantumInRegion.into());
        }
        let entry = self.open_mark(self.slot(mark)).ok_or(Fault::BadRewind)?;
        let kept = self.kept_values(keep)?;
        // Un-running the allocation of a register (or the promotion of a memory byte to qubits) that holds
        // a kept value would discard the value.
        if kept.iter().any(|&(column, _)| self.quantum.allocation_number(column) > entry.allocations) {
            return Err(Fault::BadRewind.into());
        }
        let journal = self.journal.as_ref().expect("an open mark is in the journal");
        // What the keeps change, per column: keeps into the same register or memory byte XOR together, so
        // two equal keeps change nothing.
        let mut net: Vec<(usize, u64)> = Vec::with_capacity(kept.len());
        for &(column, value) in &kept {
            match net.iter_mut().find(|(seen, _)| *seen == column) {
                Some((_, sum)) => *sum ^= value,
                None => net.push((column, value)),
            }
        }
        let changes = net.iter().any(|&(_, value)| value != 0);
        // A net change to a register or memory qubit read since the mark: un-running the read would find it
        // changed.
        let reads = journal.reads_from(entry.position);
        let cells = journal.cells_from(entry.position);
        if net.iter().any(|&(column, value)| {
            value != 0
                && (reads.iter().any(|read| read.column == column) || cells.iter().any(|cell| cell.column == column))
        }) {
            return Err(Fault::BadRewind.into());
        }
        // Worlds can split off only where an earlier rewind's region runs again. If one does and the un-run
        // then fails, running forward again cannot join them, so keep a copy to restore.
        let rewinds_inside = (entry.position..journal.records()).any(|index| journal.kind_at(index) == Undo::Rewind);
        // A rewind taken as a step that may un-run past a read where its world split defers the checks below
        // that point to the sum of the worlds' parts (`worlds.rs`); if that sum fails them, this world is put
        // back from a copy. A rewind inside another's un-run or run-again belongs to that one's copy.
        let step = self.rewinding == 0 && self.rerunning == 0;
        let split_read = reads.iter().any(|read| read.split) || cells.iter().any(|cell| cell.split);
        let defers = step && (rewinds_inside || split_read);
        // Outside strict mode an un-run may leave a register over and then fail elsewhere; running forward
        // again could not take it back, so a copy is kept.
        let backup = (rewinds_inside && (changes || self.deferring) || defers || !self.strict && step)
            .then(|| self.clone_world(None));
        let counters = (self.rewound_steps, self.rerun_steps, self.mismatches, self.splits);
        for &(column, value) in &kept {
            self.quantum.xor_register(column, value);
        }
        let site = *self.top();
        let outer_partial = std::mem::replace(&mut self.partial, false);
        let saved = std::mem::replace(&mut self.allow_region_faults, true);
        let deferred = self.deferred.len();
        self.deferring |= defers;
        // Running again is charged to the run's budget, for a rewind taken as a step: nested ones are part
        // of its work.
        let charged = self.charged;
        if step {
            self.charging = true;
            self.budget_spent = false;
            self.left_behind = false;
        }
        self.rewinding += 1;
        let from = self.forked.len();
        let result = self.unwind_to_mark(entry.position, (site.block, site.inst), 0, 0);
        self.rewinding -= 1;
        if step {
            self.deferring = false;
            self.charging = false;
        }
        self.allow_region_faults = saved;
        let forked = self.forked.len() > from || self.splits != counters.3;
        let left_behind = step && std::mem::take(&mut self.left_behind);
        // Running regardless where worlds split off during the un-run is not built (a debt, `HANDOFF.md`):
        // that rewind refuses as in strict mode.
        if result.is_err() || self.mismatches != counters.2 || left_behind && forked {
            match backup {
                Some(mut backup) if forked || self.partial || left_behind => {
                    self.forked.truncate(from);
                    self.swap_world(&mut backup);
                    (self.rewound_steps, self.rerun_steps, self.mismatches, self.splits) = counters;
                }
                _ => {
                    assert!(!forked, "worlds split off only where a rewind's region runs again, with a copy kept");
                    let undone = result.err().map_or(0, |(undone, _)| undone);
                    self.run_forward(undone);
                    assert_eq!((self.top().block, self.top().inst), (site.block, site.inst), "back at the rewind");
                    for &(column, value) in &kept {
                        self.quantum.xor_register(column, value);
                    }
                    self.deferred.truncate(deferred);
                }
            }
            self.partial = outer_partial;
            self.charged = charged;
            // Running again stopped because the budget was spent, not because the history did not describe
            // the path: the rewind faults `budget`, having changed nothing.
            let spent = step && std::mem::take(&mut self.budget_spent);
            return Err(if spent { Fault::Budget } else { Fault::BadRewind }.into());
        }
        self.partial = outer_partial;
        self.for_forks(from, &|executor| executor.partial = outer_partial);
        // Ran regardless over a state its history does not describe: a barrier after the rewind's record.
        if left_behind {
            self.record(Undo::LeftBehind);
        }
        // Every world this rewind made waits for its deferred checks, holding the copy to restore.
        if let Some(backup) =
            backup.filter(|_| defers && (self.pending || self.forked[from..].iter().any(|world| world.pending)))
        {
            self.pending = true;
            self.forked[from..].iter_mut().for_each(|world| world.pending = true);
            self.backups.push(backup);
        }
        Ok(())
    }

    /// The columns a rewind's keeps XOR into, each with its value: a register's, or for memory one per
    /// byte, the byte's cell and the value's byte at its offset. A memory target whose bytes do not all
    /// hold qubits faults `bad_rewind` (`docs/DESIGN.md`, "Qubits in memory"); one outside memory faults
    /// `out_of_bounds`.
    pub(crate) fn kept_values(&self, keep: &[Keep]) -> Result<Vec<(usize, u64)>, Fault> {
        let mut kept = Vec::with_capacity(keep.len());
        for pair in keep {
            let value = self.read(&pair.value);
            match pair.target {
                KeepTarget::Register(register) => {
                    kept.push((self.quantum.resolve(self.handle(register))?, value as u64));
                }
                KeepTarget::Memory(address) => {
                    let width = match pair.value {
                        Operand::Const(value) => value.width(),
                        Operand::Slot(slot) => match self.slot_type(slot) {
                            Type::Int(width) => width,
                            Type::Qreg(_) => unreachable!("the validator keeps integers only"),
                        },
                    };
                    let range = self.memory_range(self.read_word(&address), width)?;
                    for (offset, address) in range.enumerate() {
                        let column = self.quantum.cell(address as u64).ok_or(Fault::BadRewind)?;
                        kept.push((column, (value >> (8 * offset) & 0xff) as u64));
                    }
                }
            }
        }
        Ok(kept)
    }

    /// Un-runs this world's history back to the mark at `position`, then finishes the rewind at `site`;
    /// worlds that split off on the way do the same. `visits` counts the earlier rewinds at `site` in this
    /// frame that the un-run met, `undone` the records popped so far. A mismatch stops the un-run, having
    /// changed nothing in the step it refused; it returns how many records this world had popped.
    fn unwind_to_mark(
        &mut self,
        position: usize,
        site: (BlockId, usize),
        mut visits: u64,
        mut undone: u64,
    ) -> Result<(), (u64, Mismatch)> {
        let depth = self.frames.len();
        while self.journal.as_ref().is_some_and(|journal| journal.records() > position) {
            let at_site = self.journal.as_ref().and_then(Journal::last) == Some(Undo::Rewind)
                && self.frames.len() == depth
                && (self.top().block, self.top().inst) == (site.0, site.1 + 1);
            let from = self.forked.len();
            self.unrun_last(true).map_err(|mismatch| (undone, mismatch))?;
            undone += 1;
            self.rewound_steps += 1;
            visits += u64::from(at_site);
            self.for_forks(from, &|executor| {
                if executor.unwind_to_mark(position, site, visits, undone).is_err() {
                    executor.mismatches += 1;
                }
            });
        }
        self.finish_rewind(site, visits + 1);
        Ok(())
    }

    /// The end of a rewind: the frame continues after the rewind instruction at `site`, and the record's
    /// word holds which visit of `site` in this frame since the mark this rewind is (the high 64 bits) and
    /// where the mark left the frame. The visit is the same in every world that took the same branches,
    /// unlike the number of steps un-run, so worlds that are equal again stay equal.
    fn finish_rewind(&mut self, site: (BlockId, usize), visit: u64) {
        let frame = self.top_mut();
        let mark_at = (frame.block, frame.inst);
        (frame.block, frame.inst) = site;
        self.record_word(u128::from(visit) << 64 | u128::from(mark_at.0.0) << 32 | mark_at.1 as u128);
        self.record(Undo::Rewind);
        self.advance();
    }

    /// The mark a handle names, if a rewind to it is possible: it is a mark of the running activation
    /// (the frame was never left below since), and no measurement or finish was journalled since.
    fn open_mark(&self, handle: u128) -> Option<MarkEntry> {
        let journal = self.journal.as_ref()?;
        let entry = journal.mark_at(usize::try_from(handle).ok()?.checked_add(self.region_records)?)?;
        let mut depth = 0usize;
        // A longjmp left the frames it discarded, as that many returns would have.
        let mut jumps = journal.jumps().iter().rev();
        for index in (entry.position..journal.records()).rev() {
            match journal.kind_at(index) {
                Undo::Return => depth += 1,
                Undo::Longjmp => depth += jumps.next().expect("a longjmp record has an entry"),
                Undo::Call => depth = depth.checked_sub(1)?,
                Undo::Measure | Undo::Finish | Undo::LeftBehind => return None,
                Undo::Slot
                | Undo::Store
                | Undo::Alloca
                | Undo::StackRestore
                | Undo::Effect
                | Undo::Jump
                | Undo::Gate
                | Undo::Qalloc
                | Undo::Qfree
                | Undo::Leftover
                | Undo::Apply
                | Undo::Mark
                | Undo::Rewind
                | Undo::Setjmp
                | Undo::GateAt
                | Undo::GateAtPromoted => {}
            }
        }
        (depth == 0).then_some(entry)
    }

    /// Undoes a `rewind` record, popped with its word, the top frame back on the rewind instruction. Runs
    /// the rewound steps again from where the mark left the frame until the frame reaches the rewind for
    /// the visit the word names, so a rewind reached again in a loop is restored at the visit being
    /// undone; then XORs the kept values again. Running them again may split the world as it did the first
    /// time; every part XORs its kept values. If running again faults in any world (`run_again`), the
    /// frame is put back on the rewind and, unless worlds split off (then the rewind that un-runs this
    /// restores its copy), nothing else has changed.
    pub(crate) fn undo_rewind(&mut self, rewind: &'m Inst, word: u128) -> Result<(), Mismatch> {
        let Inst::Rewind { keep, .. } = rewind else { unreachable!("a rewind record") };
        let stop = *self.top();
        let depth = self.frames.len();
        let frame = self.top_mut();
        (frame.block, frame.inst) = (BlockId((word >> 32) as u32), word as u32 as usize);
        let from = self.forked.len();
        if let Err(mismatch) = self.run_again(depth, (stop.block, stop.inst), (word >> 64) as u64) {
            let frame = self.top_mut();
            (frame.block, frame.inst) = (stop.block, stop.inst);
            return Err(mismatch);
        }
        let xor_kept = |executor: &mut Self| {
            assert!(
                executor.frames.len() == depth
                    && (executor.top().block, executor.top().inst) == (stop.block, stop.inst),
                "the steps of a rewound region run again to the rewind"
            );
            for (column, value) in executor.kept_values(keep).expect("a kept register or memory qubit is live again") {
                executor.quantum.xor_register(column, value);
            }
        };
        xor_kept(self);
        self.for_forks(from, &xor_kept);
        Ok(())
    }

    /// Runs steps again until the frame at `depth` is at `site` for the `visit`-th time. They ran before
    /// from the same classical state, so limits and checks that they passed then are not applied again,
    /// and the step count is left as it was; under a rewind taken as a step each is charged to the budget
    /// (`charged`), and one that would pass it fails the run again. Under a rewind's keeps a step can still fault (a `qfree` of a
    /// register that is not zero, an inner `bad_rewind`, a `measure`, which no rewound region holds, a
    /// return from the rewind's frame or the end of the run, which it did not reach before): then
    /// the steps run so far are undone, and the state is as it was. If worlds split off first, the fault,
    /// in this world or in one that split off, is counted as a mismatch instead and fails the whole run
    /// again: no world's kept values are XORed, and the rewind that ran this restores its copy.
    pub(crate) fn run_again(&mut self, depth: usize, site: (BlockId, usize), visit: u64) -> Result<(), Mismatch> {
        let max_configurations = self.max_configurations.take();
        let measurement = self.journal_measurement.take();
        let allow_region_faults = std::mem::replace(&mut self.allow_region_faults, true);
        let records = self.journal.as_ref().map_or(0, Journal::records);
        let from = self.forked.len();
        let mismatches = self.mismatches;
        self.rerunning += 1;
        let mut result = self.run_again_to(depth, site, visit);
        self.rerunning -= 1;
        // A world that split off and then faulted fails the whole run again, as this one faulting would.
        if result.is_err() || self.mismatches != mismatches {
            if self.forked.len() > from {
                self.mismatches += 1;
            } else {
                // The steps run so far left records above `records` only: a rewind among them pops back
                // to a mark at or above it. Undoing them restores; it is not charged, and runs again only
                // what just ran.
                let charging = std::mem::replace(&mut self.charging, false);
                while self.journal.as_ref().is_some_and(|journal| journal.records() > records) {
                    self.undo_last();
                    self.rewound_steps += 1;
                }
                self.charging = charging;
            }
            result = Err(Mismatch);
        }
        self.allow_region_faults = allow_region_faults;
        self.journal_measurement = measurement;
        self.max_configurations = max_configurations;
        result
    }

    fn run_again_to(&mut self, depth: usize, site: (BlockId, usize), mut visits: u64) -> Result<(), Mismatch> {
        loop {
            // A path that leaves the rewind's activation never reaches the rewind: the region ran to it
            // before, so the history does not describe this path.
            if self.frames.len() < depth {
                return Err(Mismatch);
            }
            if self.frames.len() == depth && (self.top().block, self.top().inst) == site {
                visits -= 1;
                if visits == 0 {
                    return Ok(());
                }
            }
            // Under a rewind taken as a step, each step run again is charged to the run's budget, after the
            // rewind's own step: one that would pass it stops running again (the rewind faults `budget`).
            if self.charging && self.budget.is_some_and(|budget| self.steps.saturating_add(self.charged) + 2 > budget) {
                self.budget_spent = true;
                return Err(Mismatch);
            }
            let from = self.forked.len();
            self.execute().map_err(|_| Mismatch)?;
            self.rerun_steps += 1;
            if self.charging {
                self.charged += 1;
            }
            // Nor does a path that finishes the run (a return from the outermost frame, an `exit`). The
            // outcome itself may be set already: unwinding a run that faulted runs rewound regions again.
            if self.journal.as_ref().and_then(Journal::last) == Some(Undo::Finish) {
                return Err(Mismatch);
            }
            self.for_forks(from, &|executor| {
                if executor.run_again_to(depth, site, visits).is_err() {
                    executor.mismatches += 1;
                }
            });
        }
    }

    /// Runs `steps` steps forward again, after a rewind's un-run of them failed: they ran from this state
    /// before, in one world, so they run again identically. This restores, so it is not charged to the
    /// budget, even inside a rewind that is.
    fn run_forward(&mut self, steps: u64) {
        let max_configurations = self.max_configurations.take();
        let allow_region_faults = std::mem::replace(&mut self.allow_region_faults, true);
        let charging = std::mem::replace(&mut self.charging, false);
        self.rerunning += 1;
        for _ in 0..steps {
            let from = self.forked.len();
            if let Err(halt) = self.execute() {
                panic!("steps just un-run run forward again, not to {halt:?}");
            }
            assert_eq!(self.forked.len(), from, "steps one world un-ran run forward again in one world");
            self.rerun_steps += 1;
        }
        self.rerunning -= 1;
        self.charging = charging;
        self.allow_region_faults = allow_region_faults;
        self.max_configurations = max_configurations;
    }
}

/// An un-run that met a state its history does not describe. It changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mismatch;
