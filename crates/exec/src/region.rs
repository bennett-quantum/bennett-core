//! `apply`: a coherent region, as mark, call, rewind-keep (`docs/DESIGN.md`, "`apply` as mark, call,
//! rewind-keep").
//!
//! `apply @f(args) -> out, fault budget B` is one step of the run that called it, and its meaning is the
//! history idiom on worlds: mark the journal; call `f`, where a register passed as an argument is read as
//! an integer, so the world splits into one world per value; run each world to its end under the region's
//! rules (an effect, an `exit` or a quantum instruction faults; at most `B` steps of its own); then each
//! world keeps its result into `out`, or its fault code into `fault`, by XOR, and un-runs every step since
//! the mark, after which the worlds are the same machine state again and merge. A region cannot touch a
//! register, so nothing it read has changed when it is un-run.
//!
//! The earlier definition, branch enumeration, is kept as the comparison ([`ApplyMode::Branches`]): on each
//! basis configuration the callee runs on the argument values, on private memory, with its own budget, and
//! is undone at once. [`ApplyMode::Compare`] computes both at every `apply` and stops at any difference.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bennett_ir::{Fault, FuncId, Operand, Slot, Type};

use crate::executor::{Executor, Halt, Outcome, RunError};
use crate::journal::Journal;
use crate::quantum::Config;
use crate::worlds::World;

/// How the executor computes an `apply`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApplyMode {
    /// Mark, call, rewind-keep on worlds: the meaning.
    #[default]
    History,
    /// Branch enumeration on private memory, the earlier definition, kept as the comparison. The
    /// journal-content observer always uses it (it classifies the journals of branches).
    Branches,
    /// Both at every `apply`, from the same state; any difference in the result, the world's whole state,
    /// its amplitudes or the counts the documents promise panics. The history result is kept.
    Compare,
}

impl ApplyMode {
    /// The mode the environment variable `BENNETT_APPLY` names (`history`, `branches` or `compare`), if it
    /// is set: a switch for running a whole test suite in one mode. Any other value panics.
    pub fn from_env() -> Option<ApplyMode> {
        static MODE: OnceLock<Option<ApplyMode>> = OnceLock::new();
        *MODE.get_or_init(|| {
            let value = std::env::var("BENNETT_APPLY").ok()?;
            Some(match value.as_str() {
                "history" => ApplyMode::History,
                "branches" => ApplyMode::Branches,
                "compare" => ApplyMode::Compare,
                other => panic!("BENNETT_APPLY={other}: expected history, branches or compare"),
            })
        })
    }
}

/// What computing coherent regions cost the executor, summed over every `apply` executed, forward or
/// backward. A run is one basis configuration enumerated ([`ApplyMode::Branches`]) or one world of the
/// idiom ([`ApplyMode::History`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegionCost {
    /// Runs of region callees.
    pub runs: u64,
    /// Steps executed for regions, forward and back (un-run by the keep, or undone by the enumeration),
    /// steps rewound and run again by `rewind`s inside the region included.
    pub work: u64,
    /// The most journal bytes each run held above the journal at the region's start, summed over runs.
    pub journal_bytes: u64,
    /// The most overlay bytes each run held above the world's overlay at the region's start, summed over runs.
    pub overlay_bytes: u64,
    /// Bytes copied where a region's world split: the journal, overlay and slots of the world it split from.
    pub copied_bytes: u64,
}

/// Where an argument's value comes from on each configuration.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Input {
    /// A register, read as an integer: a column of the configuration.
    Register(usize),
    /// A classical value, the same on every configuration.
    Value(u128),
}

impl Executor<'_> {
    /// XORs `callee(args)` into `out`, or its fault code into `fault`, on every configuration.
    ///
    /// Every execution that faults on some configuration is counted, with the weight of the faulting
    /// configurations. Unless the run allows region faults, such an execution stops the run instead, with
    /// the state as it was before the `apply`.
    pub(crate) fn apply(
        &mut self,
        callee: FuncId,
        args: &[Operand],
        out: Slot,
        fault: Slot,
        budget: &Operand,
    ) -> Result<(), Halt> {
        let mode = if self.journal_measurement.is_some() { ApplyMode::Branches } else { self.apply_mode };
        match mode {
            ApplyMode::History => self.apply_by_history(callee, args, out, fault, budget),
            ApplyMode::Branches => self.apply_by_branches(callee, args, out, fault, budget),
            ApplyMode::Compare => self.apply_compared(callee, args, out, fault, budget),
        }
    }

    /// The columns of `out` and `fault` and where each argument comes from; `qubit_alias` if a target is
    /// an argument or both are one register.
    fn region_operands(&self, args: &[Operand], out: Slot, fault: Slot) -> Result<(usize, usize, Vec<Input>), Halt> {
        let out = self.quantum.resolve(self.handle(out))?;
        let fault = self.quantum.resolve(self.handle(fault))?;
        let inputs = args.iter().map(|arg| self.input(arg)).collect::<Result<Vec<_>, _>>()?;
        let targets = [Input::Register(out), Input::Register(fault)];
        if out == fault || inputs.iter().any(|input| targets.contains(input)) {
            return Err(Fault::QubitAlias.into());
        }
        Ok((out, fault, inputs))
    }

    fn input(&self, arg: &Operand) -> Result<Input, Fault> {
        match arg {
            Operand::Slot(slot) if matches!(self.slot_type(*slot), Type::Qreg(_)) => {
                Ok(Input::Register(self.quantum.resolve(self.handle(*slot))?))
            }
            _ => Ok(Input::Value(self.read(arg))),
        }
    }

    /// The fault counts of an `apply` whose faulting configurations are `faulting` (configuration, its
    /// squared amplitude, the fault), in configuration order: counted, and unless the run allows region
    /// faults, the stop to return.
    fn count_region_faults(
        &mut self,
        callee: FuncId,
        faulting: &[(&Config, f64, Fault)],
        configurations: usize,
    ) -> Option<RunError> {
        let &(_, _, code) = faulting.first()?;
        let weight: f64 = faulting.iter().map(|&(_, weight, _)| weight).sum();
        self.region_faults += 1;
        self.region_fault_weight += weight;
        self.region_fault_max_weight = self.region_fault_max_weight.max(weight);
        (!self.allow_region_faults).then_some(RunError::RegionFault {
            callee,
            fault: code,
            branches: faulting.len(),
            configurations,
        })
    }

    // ---------------------------------------------------------------------------------------------------
    // Mark, call, rewind-keep.

    /// `apply` as the history idiom on worlds (module documentation).
    fn apply_by_history(
        &mut self,
        callee: FuncId,
        args: &[Operand],
        out: Slot,
        fault: Slot,
        budget: &Operand,
    ) -> Result<(), Halt> {
        let (out, fault, inputs) = self.region_operands(args, out, fault)?;
        let budget = self.read_word(budget);
        let configurations = self.quantum.amplitudes().len();
        if configurations == 0 {
            // A world's part that a deferred check emptied carries no amplitude: nothing to permute.
            return Ok(());
        }
        let outer = self.begin_history_region();
        // The mark: the journal's position. The region leaves nothing above it, so it needs no record.
        let position = self.journal.get_or_insert_with(Journal::default).records();
        // Marks inside the region count from here, as on a journal of the region's own (the enumeration's).
        self.region_records = position;
        let bases = (self.journal.as_ref().map_or(0, Journal::bytes), self.memory.overlay_len());
        let slot_bytes = (self.slots.len() * size_of::<u128>()) as u64;
        // What every world of the region is again after its un-run, but for its amplitudes.
        let start = self.clone_world(Some(BTreeMap::new()));

        // The call reads the argument registers: where the configurations disagree, the world splits into
        // one world per value. The worlds are independent until they merge, so each runs and un-runs in
        // turn in this world's place, which holds only its configurations meanwhile.
        let columns: Vec<usize> =
            inputs.iter().filter_map(|input| if let Input::Register(c) = *input { Some(c) } else { None }).collect();
        let parts = self.quantum.partition(&columns);
        let original = self.quantum.take_amplitudes();
        let split = parts.len() > 1;
        self.splits += parts.len() as u64 - 1;
        let mut done = Done { parts: parts.len() as u64, ..Done::default() };
        for (_, part) in parts {
            self.quantum.replace_amplitudes(part);
            let mut queue: Vec<Pending> = Vec::new();
            // A frame that does not fit faults alike in every world.
            let run = match self.frame_fits(callee) {
                Ok(_) => {
                    self.enter_region(callee, &inputs, split).expect("the frame fits");
                    self.run_region_world(budget, Pending::current(), &mut queue, bases)
                }
                Err(code) => Ran { outcome: Outcome::Faulted(code), steps: 0, rewound: 0, rerun: 0 },
            };
            self.close_region_world(&run, (out, fault), position, &start, &mut done);
            // Worlds split off inside the region (a load of qubits in memory), each in turn.
            while let Some(pending) = queue.pop() {
                let mut world = pending.world;
                self.swap_world(&mut world);
                let pending = Pending { world: World::empty(), ..pending };
                let run = self.run_region_world(budget, pending, &mut queue, bases);
                self.close_region_world(&run, (out, fault), position, &start, &mut done);
                self.swap_world(&mut world);
            }
        }
        drop(start);

        // Faults are counted in configuration order, as the branches were enumerated.
        done.faulting.sort_by(|a, b| a.0.cmp(&b.0));
        let faulting: Vec<(&Config, f64, Fault)> = done.faulting.iter().map(|(c, w, f)| (c, *w, *f)).collect();
        let stop = self.count_region_faults(callee, &faulting, configurations);
        if stop.is_some() {
            // Strict: the run stops with the state as before the `apply`, as if every world kept nothing.
            self.quantum.replace_amplitudes(original);
        } else {
            // The worlds' configurations are disjoint, so merging them gathers their amplitudes, once.
            done.amplitudes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            let amplitudes: BTreeMap<Config, _> = done.amplitudes.into_iter().collect();
            assert_eq!(amplitudes.len(), configurations, "the region permuted the configurations");
            self.quantum.replace_amplitudes(amplitudes);
        }
        self.memory.fold();
        // A world split off inside the region is a copy; the worlds of the call's read run in this one's place.
        self.region_cost.copied_bytes += slot_bytes * (self.splits - outer.counters.splits - (done.parts - 1));
        self.rewound_steps = outer.counters.rewound_steps + done.rewound;
        self.rerun_steps = outer.counters.rerun_steps + done.rerun;
        self.end_history_region(outer);
        match stop {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    /// The end of one world of a region: notes its fault, keeps its result into `out` (or its fault code
    /// into `fault`), un-runs it to the mark and checks that it is the world at the mark again, `start`,
    /// then gathers its amplitudes into `done`. The history work of `rewind`s inside the region is counted
    /// as the enumeration counts it, once per configuration; what the idiom did is its cost.
    fn close_region_world(
        &mut self,
        run: &Ran,
        (out, fault): (usize, usize),
        position: usize,
        start: &World,
        done: &mut Done,
    ) {
        let weight = self.quantum.amplitudes().len() as u64;
        self.region_steps += run.steps * weight;
        let (column, value) = match run.outcome {
            // The validator made the result as wide as the register, at most 64 bits.
            Outcome::Returned(result) => (out, result.expect("a region callee returns an integer") as u64),
            Outcome::Faulted(code) => (fault, code.code()),
            Outcome::Stopped(_) => unreachable!("a region reaches no limit of the simulator"),
        };
        self.quantum.xor_register(column, value);
        self.outcome = None;
        let (rewound, rerun) = (self.rewound_steps, self.rerun_steps);
        let undone = self.unrun_region(position);
        let (rewound, rerun) = (self.rewound_steps - rewound, self.rerun_steps - rerun);
        self.region_cost.work += undone + rewound + rerun;
        done.rewound += (run.rewound + rewound) * weight;
        done.rerun += (run.rerun + rerun) * weight;
        let mut world = self.take_current();
        assert!(self.same_world(start, &world), "a region's world is the world at the mark again after its un-run");
        // The faulting configurations as they were before the `apply`, which orders them: the region's writes
        // to qubits in memory changed them until its un-run, and the keep changed `fault` since.
        if let Outcome::Faulted(code) = run.outcome {
            done.faulting.extend(world.quantum.amplitudes().iter().map(|(config, a)| {
                let mut config = config.clone();
                config[fault] ^= code.code();
                (config, a.norm_sqr(), code)
            }));
        }
        done.amplitudes.extend(world.quantum.take_amplitudes());
        self.swap_world(&mut world);
    }

    /// Enters `callee` on the values of `inputs` that this world's configurations agree on, noting each
    /// register read (whether the world `split` there) for the un-run's check.
    fn enter_region(&mut self, callee: FuncId, inputs: &[Input], split: bool) -> Result<(), Fault> {
        let split = split || self.splitting;
        let config = self.quantum.amplitudes().keys().next();
        let values: Vec<u128> = inputs
            .iter()
            .map(|input| match *input {
                Input::Register(column) => config.map_or(0, |config| u128::from(config[column])),
                Input::Value(value) => value,
            })
            .collect();
        let params = &self.function(callee).params;
        if let Some(journal) = &mut self.journal {
            for (input, param) in inputs.iter().zip(params) {
                if let Input::Register(column) = *input {
                    journal.push_read(column, param.slot, split);
                }
            }
        }
        self.push_frame(callee, &values)
    }

    /// Runs the current world of a region to its end, within `budget` steps of its own (counted from
    /// `pending.steps`, the steps it shares with the world it split from). Worlds that split off join
    /// `queue`, with the steps and the history work they share.
    fn run_region_world(
        &mut self,
        budget: u64,
        pending: Pending,
        queue: &mut Vec<Pending>,
        (journal_base, overlay_base): (usize, usize),
    ) -> Ran {
        self.steps = pending.steps;
        self.budget = Some(budget);
        self.finish_region_rewind();
        self.run_journal_peak = self.journal.as_ref().map_or(0, Journal::bytes);
        self.run_overlay_peak = self.memory.overlay_len();
        let (rewound, rerun) = (self.rewound_steps, self.rerun_steps);
        let steps = self.steps;
        let outcome = loop {
            if let Some(outcome) = self.step_world() {
                break outcome;
            }
            self.finish_region_rewind();
            let shared = (pending.rewound + self.rewound_steps - rewound, pending.rerun + self.rerun_steps - rerun);
            queue.extend(self.forked.drain(..).map(|world| Pending {
                world,
                steps: self.steps,
                rewound: shared.0,
                rerun: shared.1,
            }));
        };
        let (own_rewound, own_rerun) = (self.rewound_steps - rewound, self.rerun_steps - rerun);
        self.region_cost.runs += 1;
        self.region_cost.work += self.steps - steps + own_rewound + own_rerun;
        self.region_cost.journal_bytes += self.run_journal_peak.saturating_sub(journal_base) as u64;
        self.region_cost.overlay_bytes += self.run_overlay_peak.saturating_sub(overlay_base) as u64;
        Ran { outcome, steps: self.steps, rewound: pending.rewound + own_rewound, rerun: pending.rerun + own_rerun }
    }

    /// A `rewind` inside a region that un-ran past a read where its world split defers its checks to the
    /// sum of the worlds' parts (`worlds.rs`). A region cannot change a register or a qubit in memory, and a
    /// rewind in it cannot keep, so every part holds what its history read: no part fails a check and the
    /// copy kept to restore is dropped, as the checks on the sum would.
    fn finish_region_rewind(&mut self) {
        if self.pending {
            assert!(self.deferred.is_empty(), "a part of a region's world holds what its history read");
            self.pending = false;
            self.backups.clear();
        }
    }

    /// Un-runs every step of the current world since the journal had `position` records, checked as a
    /// rewind checks; returns the records popped.
    fn unrun_region(&mut self, position: usize) -> u64 {
        let mut undone = 0;
        while self.journal.as_ref().is_some_and(|journal| journal.records() > position) {
            let forks = self.forked.len();
            self.unrun_last(true).expect("nothing in a region changes what it read, so its history un-runs");
            assert_eq!(self.forked.len(), forks, "a world's part un-runs its region in one world");
            undone += 1;
        }
        undone
    }

    /// The current world, taken out; the executor holds an empty one until another is swapped in.
    fn take_current(&mut self) -> World {
        let mut world = World::empty();
        self.swap_world(&mut world);
        world
    }

    /// Sets the run's state aside for a region computed on worlds.
    fn begin_history_region(&mut self) -> HistoryRegion {
        let base_depth = self.frames.len();
        HistoryRegion {
            outcome: self.outcome.take(),
            steps: std::mem::take(&mut self.steps),
            budget: self.budget.take(),
            base_depth: std::mem::replace(&mut self.base_depth, base_depth),
            in_region: std::mem::replace(&mut self.in_region, true),
            region_records: self.region_records,
            allow_region_faults: self.allow_region_faults,
            forked: std::mem::take(&mut self.forked),
            partial: std::mem::replace(&mut self.partial, false),
            deferring: std::mem::replace(&mut self.deferring, false),
            rewinding: std::mem::replace(&mut self.rewinding, 0),
            rerunning: std::mem::replace(&mut self.rerunning, 0),
            splitting: std::mem::replace(&mut self.splitting, false),
            deferred: std::mem::take(&mut self.deferred),
            backups: std::mem::take(&mut self.backups),
            pending: std::mem::replace(&mut self.pending, false),
            had_journal: self.journal.is_some(),
            counters: Counters::of(self),
        }
    }

    /// Puts the run's state back after a region computed on worlds. The region's splits are its own cost,
    /// not the run's.
    fn end_history_region(&mut self, outer: HistoryRegion) {
        let c = outer.counters;
        self.region_cost.copied_bytes +=
            self.split_memory_bytes - c.split_memory_bytes + self.split_journal_bytes - c.split_journal_bytes;
        (self.splits, self.split_memory_bytes, self.split_journal_bytes) =
            (c.splits, c.split_memory_bytes, c.split_journal_bytes);
        assert_eq!(self.mismatches, c.mismatches, "a region's un-run meets the state its history describes");
        if !outer.had_journal {
            let journal = self.journal.take().expect("the region's journal");
            assert_eq!(journal.records(), 0, "the region left no history");
        }
        self.outcome = outer.outcome;
        self.steps = outer.steps;
        self.budget = outer.budget;
        self.base_depth = outer.base_depth;
        self.in_region = outer.in_region;
        self.region_records = outer.region_records;
        self.allow_region_faults = outer.allow_region_faults;
        self.forked = outer.forked;
        self.partial = outer.partial;
        self.deferring = outer.deferring;
        self.rewinding = outer.rewinding;
        self.rerunning = outer.rerunning;
        self.splitting = outer.splitting;
        self.deferred = outer.deferred;
        self.backups = outer.backups;
        self.pending = outer.pending;
    }

    // ---------------------------------------------------------------------------------------------------
    // Branch enumeration, the comparison.

    /// `apply` by branch enumeration (module documentation).
    fn apply_by_branches(
        &mut self,
        callee: FuncId,
        args: &[Operand],
        out: Slot,
        fault: Slot,
        budget: &Operand,
    ) -> Result<(), Halt> {
        let (out, fault, inputs) = self.region_operands(args, out, fault)?;
        let budget = self.read_word(budget);
        let mut values = Vec::with_capacity(inputs.len());
        let original = self.quantum.take_amplitudes();
        self.measure_begin_apply(callee);
        let mut outcomes = Vec::with_capacity(original.len());
        // The private memory of each branch holds the values its configuration gives the cells.
        let cells = self.region_cells();
        for config in original.keys() {
            values.clear();
            values.extend(inputs.iter().map(|input| match *input {
                Input::Register(column) => u128::from(config[column]),
                Input::Value(value) => value,
            }));
            self.materialise_cells(&cells, Some(config));
            outcomes.push(self.evaluate(callee, &values, budget));
            self.materialise_cells(&cells, None);
        }
        let faulting: Vec<(&Config, f64, Fault)> = original
            .iter()
            .zip(&outcomes)
            .filter_map(|((config, amplitude), outcome)| match *outcome {
                Outcome::Faulted(code) => Some((config, amplitude.norm_sqr(), code)),
                _ => None,
            })
            .collect();
        if let Some(error) = self.count_region_faults(callee, &faulting, original.len()) {
            self.quantum.replace_amplitudes(original);
            return Err(error.into());
        }
        let mut permuted = BTreeMap::new();
        for ((mut config, amplitude), outcome) in original.into_iter().zip(outcomes) {
            match outcome {
                // The validator made the result as wide as the register, at most 64 bits.
                Outcome::Returned(result) => config[out] ^= result.expect("a region callee returns an integer") as u64,
                Outcome::Faulted(code) => config[fault] ^= code.code(),
                Outcome::Stopped(_) => unreachable!("a region reaches no gate, so no limit of the quantum state"),
            }
            permuted.insert(config, amplitude);
        }
        self.quantum.replace_amplitudes(permuted);
        Ok(())
    }

    /// Runs `callee(args)` within `budget` steps on top of the current frames, then undoes the run.
    fn evaluate(&mut self, callee: FuncId, args: &[u128], budget: u64) -> Outcome {
        let region_journal = std::mem::take(&mut self.spare_journal);
        let outer = Saved {
            journal: self.journal.replace(region_journal),
            outcome: self.outcome.take(),
            steps: std::mem::take(&mut self.steps),
            budget: self.budget.replace(budget),
            base_depth: std::mem::replace(&mut self.base_depth, self.frames.len()),
            in_region: std::mem::replace(&mut self.in_region, true),
            region_records: std::mem::take(&mut self.region_records),
            materialised: std::mem::replace(&mut self.materialised, true),
        };
        self.run_journal_peak = 0;
        self.run_overlay_peak = self.memory.overlay_len();
        let overlay_base = self.run_overlay_peak;
        let (rewound, rerun) = (self.rewound_steps, self.rerun_steps);
        self.measure_begin_branch(args);
        let outcome = match self.push_frame(callee, args) {
            Ok(()) => {
                self.measure_entry_call(callee);
                self.run()
            }
            Err(fault) => Outcome::Faulted(fault),
        };
        self.region_steps += self.steps;
        let journal = self.journal.as_ref().expect("a region runs with a journal");
        let (records, words) = (journal.records(), journal.words().len());
        if self.journal_measurement.is_some() {
            self.measure_end_branch(outcome, self.steps, records, words);
        }
        let undone = self.unwind();
        self.region_cost.runs += 1;
        self.region_cost.work += self.steps + undone + self.rewound_steps - rewound + self.rerun_steps - rerun;
        self.region_cost.journal_bytes += self.run_journal_peak as u64;
        self.region_cost.overlay_bytes += self.run_overlay_peak.saturating_sub(overlay_base) as u64;
        self.restore(outer);
        outcome
    }

    fn restore(&mut self, outer: Saved) {
        let mut region_journal = std::mem::replace(&mut self.journal, outer.journal).unwrap_or_default();
        region_journal.clear();
        self.spare_journal = region_journal;
        self.outcome = outer.outcome;
        self.steps = outer.steps;
        self.budget = outer.budget;
        self.base_depth = outer.base_depth;
        self.in_region = outer.in_region;
        self.region_records = outer.region_records;
        self.materialised = outer.materialised;
    }

    // ---------------------------------------------------------------------------------------------------
    // Both, compared.

    /// `apply` by both definitions from the same state: panics at any difference in the result, in the
    /// world's state after it (classical state and journal exactly, amplitudes exactly, its history state),
    /// in the counts the documents promise or in anything else of the run outside the world (`Run`). Both
    /// start from the whole executor as it was, but for the cost report and reused buffers. Keeps the
    /// history result.
    fn apply_compared(
        &mut self,
        callee: FuncId,
        args: &[Operand],
        out: Slot,
        fault: Slot,
        budget: &Operand,
    ) -> Result<(), Halt> {
        let start = self.clone_world(None);
        let before = Run::of(self);
        let cost_before = self.region_cost;
        let clock = Instant::now();
        let history = self.apply_by_history(callee, args, out, fault, budget);
        let history_time = clock.elapsed();
        #[cfg(test)]
        if let Some(corrupt) = self.corrupt_history {
            corrupt(self);
        }
        let mut by_history = self.take_current();
        let history_run = Run::take(self);
        let history_cost = self.region_cost;
        let mut world = start;
        self.swap_world(&mut world);
        before.set(self);
        self.region_cost = cost_before;
        let clock = Instant::now();
        let branches = self.apply_by_branches(callee, args, out, fault, budget);
        let branches_time = clock.elapsed();
        let by_branches = self.take_current();
        let mut branches_run = Run::take(self);
        let name = &self.function(callee).name;
        assert_eq!(history, branches, "apply @{name}: the result by history and by branches");
        assert!(self.same_world(&by_history, &by_branches), "apply @{name}: the machine state after it");
        assert_eq!(
            by_history.quantum.amplitudes(),
            by_branches.quantum.amplitudes(),
            "apply @{name}: the amplitudes after it"
        );
        assert!(self.same_history_state(&by_history, &by_branches), "apply @{name}: the world's history state");
        assert_eq!(history_run.counters, branches_run.counters, "apply @{name}: the counts");
        assert_eq!(history_run.state, branches_run.state, "apply @{name}: the run's state");
        assert!(
            self.identical_worlds(&history_run.forked, &branches_run.forked)
                && history_run.worlds.len() == branches_run.worlds.len()
                && history_run.worlds.iter().zip(&branches_run.worlds).all(|(a, b)| match (a, b) {
                    (Some(a), Some(b)) => self.identical_world(a, b),
                    (a, b) => a.is_none() && b.is_none(),
                }),
            "apply @{name}: the other worlds"
        );
        let tally = Tally {
            callee: name,
            configurations: by_branches.quantum.amplitudes().len(),
            outcome: match history {
                Ok(()) => "ok",
                Err(Halt::Stop(_)) => "region_fault",
                Err(_) => "fault",
            },
            history: (history_cost, history_time),
            branches: (self.region_cost, branches_time),
            before: cost_before,
        };
        tally.write();
        // The other worlds, the same both ways, go back.
        self.worlds = std::mem::take(&mut branches_run.worlds);
        self.forked = std::mem::take(&mut branches_run.forked);
        // Both cost reports are kept, the enumeration's in the run's: the idiom's can be told apart by runs.
        self.region_cost = add(history_cost, diff(self.region_cost, cost_before));
        self.swap_world(&mut by_history);
        drop(by_history);
        self.memory.fold();
        history
    }

    /// Whether two worlds are the same in every field: machine state, amplitudes and history state, the
    /// copies held for deferred checks included.
    fn identical_world(&self, a: &World, b: &World) -> bool {
        self.same_world(a, b) && a.quantum.amplitudes() == b.quantum.amplitudes() && self.same_history_state(a, b)
    }

    /// Whether two worlds are in the same history state: status, deferred checks and the copies they hold.
    fn same_history_state(&self, a: &World, b: &World) -> bool {
        (a.status, a.partial, a.pending) == (b.status, b.partial, b.pending)
            && a.deferred == b.deferred
            && self.identical_worlds(&a.backups, &b.backups)
    }

    fn identical_worlds(&self, a: &[World], b: &[World]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| self.identical_world(a, b))
    }
}

fn diff(a: RegionCost, b: RegionCost) -> RegionCost {
    RegionCost {
        runs: a.runs - b.runs,
        work: a.work - b.work,
        journal_bytes: a.journal_bytes - b.journal_bytes,
        overlay_bytes: a.overlay_bytes - b.overlay_bytes,
        copied_bytes: a.copied_bytes - b.copied_bytes,
    }
}

fn add(a: RegionCost, b: RegionCost) -> RegionCost {
    RegionCost {
        runs: a.runs + b.runs,
        work: a.work + b.work,
        journal_bytes: a.journal_bytes + b.journal_bytes,
        overlay_bytes: a.overlay_bytes + b.overlay_bytes,
        copied_bytes: a.copied_bytes + b.copied_bytes,
    }
}

/// The run's counts that both definitions of `apply` must give alike.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Counters {
    region_steps: u64,
    region_faults: u64,
    region_fault_weight: f64,
    region_fault_max_weight: f64,
    rewound_steps: u64,
    rerun_steps: u64,
    mismatches: u64,
    splits: u64,
    split_memory_bytes: u64,
    split_journal_bytes: u64,
}

impl Counters {
    fn of(executor: &Executor<'_>) -> Self {
        Counters {
            region_steps: executor.region_steps,
            region_faults: executor.region_faults,
            region_fault_weight: executor.region_fault_weight,
            region_fault_max_weight: executor.region_fault_max_weight,
            rewound_steps: executor.rewound_steps,
            rerun_steps: executor.rerun_steps,
            mismatches: executor.mismatches,
            splits: executor.splits,
            split_memory_bytes: executor.split_memory_bytes,
            split_journal_bytes: executor.split_journal_bytes,
        }
    }

    fn set(self, executor: &mut Executor<'_>) {
        executor.region_steps = self.region_steps;
        executor.region_faults = self.region_faults;
        executor.region_fault_weight = self.region_fault_weight;
        executor.region_fault_max_weight = self.region_fault_max_weight;
        executor.rewound_steps = self.rewound_steps;
        executor.rerun_steps = self.rerun_steps;
        executor.mismatches = self.mismatches;
        executor.splits = self.splits;
        executor.split_memory_bytes = self.split_memory_bytes;
        executor.split_journal_bytes = self.split_journal_bytes;
    }
}

/// Everything of the executor outside the current world, but the cost report ([`RegionCost`], which says
/// how `apply` was computed), the cost of one region run in progress and buffers kept for reuse: what
/// compare mode restores before the enumeration and requires both ways to leave alike.
struct Run {
    counters: Counters,
    state: RunState,
    /// The other worlds of a run of several, and those split off in the step in progress.
    worlds: Vec<Option<World>>,
    forked: Vec<World>,
}

/// The run's fields outside the world besides [`Counters`] and the other worlds.
#[derive(Clone, Debug, PartialEq)]
struct RunState {
    steps: u64,
    budget: Option<u64>,
    max_configurations: Option<usize>,
    base_depth: usize,
    in_region: bool,
    region_records: usize,
    materialised: bool,
    apply_mode: ApplyMode,
    allow_region_faults: bool,
    reversible: bool,
    rng: rand_chacha::ChaCha8Rng,
    current: usize,
    run_outcome: Option<Outcome>,
    departed: Vec<u64>,
    observed: bool,
    merges: u64,
    observations: u64,
    world_steps: u64,
    peak_worlds: usize,
    peak_overlay_bytes: usize,
    peak_world_configurations: usize,
    others_configurations: usize,
    rerunning: u32,
    rewinding: u32,
    splitting: bool,
    deferring: bool,
}

impl Run {
    /// The run as it is, the other worlds copied.
    fn of(e: &Executor<'_>) -> Self {
        let (worlds, forked) = (e.worlds.clone(), e.forked.clone());
        Run { worlds, forked, ..Run::state(e) }
    }

    /// The run as it is, the other worlds taken out.
    fn take(e: &mut Executor<'_>) -> Self {
        let (worlds, forked) = (std::mem::take(&mut e.worlds), std::mem::take(&mut e.forked));
        Run { worlds, forked, ..Run::state(e) }
    }

    fn state(e: &Executor<'_>) -> Self {
        Run {
            counters: Counters::of(e),
            state: RunState {
                steps: e.steps,
                budget: e.budget,
                max_configurations: e.max_configurations,
                base_depth: e.base_depth,
                in_region: e.in_region,
                region_records: e.region_records,
                materialised: e.materialised,
                apply_mode: e.apply_mode,
                allow_region_faults: e.allow_region_faults,
                reversible: e.reversible,
                rng: e.rng.clone(),
                current: e.current,
                run_outcome: e.run_outcome,
                departed: e.departed.clone(),
                observed: e.observed,
                merges: e.merges,
                observations: e.observations,
                world_steps: e.world_steps,
                peak_worlds: e.peak_worlds,
                peak_overlay_bytes: e.peak_overlay_bytes,
                peak_world_configurations: e.peak_world_configurations,
                others_configurations: e.others_configurations,
                rerunning: e.rerunning,
                rewinding: e.rewinding,
                splitting: e.splitting,
                deferring: e.deferring,
            },
            worlds: Vec::new(),
            forked: Vec::new(),
        }
    }

    fn set(self, e: &mut Executor<'_>) {
        self.counters.set(e);
        let s = self.state;
        e.steps = s.steps;
        e.budget = s.budget;
        e.max_configurations = s.max_configurations;
        e.base_depth = s.base_depth;
        e.in_region = s.in_region;
        e.region_records = s.region_records;
        e.materialised = s.materialised;
        e.apply_mode = s.apply_mode;
        e.allow_region_faults = s.allow_region_faults;
        e.reversible = s.reversible;
        e.rng = s.rng;
        e.current = s.current;
        e.run_outcome = s.run_outcome;
        e.departed = s.departed;
        e.observed = s.observed;
        e.merges = s.merges;
        e.observations = s.observations;
        e.world_steps = s.world_steps;
        e.peak_worlds = s.peak_worlds;
        e.peak_overlay_bytes = s.peak_overlay_bytes;
        e.peak_world_configurations = s.peak_world_configurations;
        e.others_configurations = s.others_configurations;
        e.rerunning = s.rerunning;
        e.rewinding = s.rewinding;
        e.splitting = s.splitting;
        e.deferring = s.deferring;
        e.worlds = self.worlds;
        e.forked = self.forked;
    }
}

/// A world of a region waiting to run: split off at `steps` of the region's budget, sharing with the world
/// it split from the steps `rewind`s inside the region had rewound and run again by then.
struct Pending {
    world: World,
    steps: u64,
    rewound: u64,
    rerun: u64,
}

impl Pending {
    /// The current world, which runs from the region's start.
    fn current() -> Self {
        Pending { world: World::empty(), steps: 0, rewound: 0, rerun: 0 }
    }
}

/// How a world of a region ended.
struct Ran {
    outcome: Outcome,
    /// Its steps, those it shares with the world it split from included: what one branch of the
    /// enumeration on each of its configurations takes.
    steps: u64,
    /// Steps `rewind`s inside the region rewound and ran again on its way, likewise.
    rewound: u64,
    rerun: u64,
}

/// What the worlds of a region left: their amplitudes after the keep, the faulting configurations with
/// their weights, and the history work counted per configuration.
#[derive(Default)]
struct Done {
    parts: u64,
    amplitudes: Vec<(Config, num_complex::Complex64)>,
    faulting: Vec<(Config, f64, Fault)>,
    rewound: u64,
    rerun: u64,
}

/// The run's state, set aside while a region is computed on worlds.
struct HistoryRegion {
    outcome: Option<Outcome>,
    steps: u64,
    budget: Option<u64>,
    base_depth: usize,
    in_region: bool,
    region_records: usize,
    allow_region_faults: bool,
    forked: Vec<World>,
    partial: bool,
    deferring: bool,
    rewinding: u32,
    rerunning: u32,
    splitting: bool,
    deferred: Vec<crate::worlds::Defect>,
    backups: Vec<World>,
    pending: bool,
    had_journal: bool,
    counters: Counters,
}

/// The run's state set aside while one branch of a region is enumerated.
struct Saved {
    journal: Option<Journal>,
    outcome: Option<Outcome>,
    steps: u64,
    budget: Option<u64>,
    base_depth: usize,
    in_region: bool,
    region_records: usize,
    materialised: bool,
}

/// One line of the cost comparison, appended to the file `BENNETT_APPLY_TALLY` names in compare mode.
struct Tally<'a> {
    callee: &'a str,
    configurations: usize,
    outcome: &'static str,
    history: (RegionCost, Duration),
    branches: (RegionCost, Duration),
    before: RegionCost,
}

impl Tally<'_> {
    fn write(&self) {
        static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
        let Some(path) = PATH.get_or_init(|| std::env::var_os("BENNETT_APPLY_TALLY").map(PathBuf::from)) else {
            return;
        };
        let (h, b) = (diff(self.history.0, self.before), diff(self.branches.0, self.before));
        let thread = std::thread::current();
        let line = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            thread.name().unwrap_or("main"),
            self.callee,
            self.configurations,
            self.outcome,
            h.runs,
            h.work,
            h.journal_bytes,
            h.overlay_bytes,
            h.copied_bytes,
            self.history.1.as_nanos(),
            b.runs,
            b.work,
            b.journal_bytes,
            b.overlay_bytes,
            b.copied_bytes,
            self.branches.1.as_nanos(),
        );
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path);
        file.and_then(|mut file| file.write_all(line.as_bytes()))
            .unwrap_or_else(|error| panic!("BENNETT_APPLY_TALLY {}: {error}", path.display()));
    }
}
