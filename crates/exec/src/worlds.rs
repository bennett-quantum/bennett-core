//! Worlds: the quantum state as a superposition of whole machine states (`docs/DESIGN.md`, "Worlds").
//!
//! A world is a classical machine state (memory, frames, slots, stack pointer, console tape, register table,
//! journal, outcome) with the amplitudes of the register configurations that go with it. Ordinary code
//! reads a register as an integer when a call passes it to an integer parameter; a world whose
//! configurations disagree on the value splits into one world per value. Every step of every world is
//! journalled, so two different worlds stay different until a `rewind` un-runs the step that told them
//! apart; worlds that are the same machine state again are one basis state, and their amplitudes add.
//!
//! The executor's own state fields hold the current world; the others wait in a list. Worlds run in
//! lockstep, one step each per tick, so the budget counts ticks. A world that has just rewound parks: it
//! waits while another world is running, because that world may still rewind the same mark to the same
//! state. When no world runs, equal worlds merge and the parked worlds with the longest history go on.
//! A world that reaches a measurement waits until no world runs or is parked, so that every world that
//! will merge has merged and the worlds' weights are the probabilities of their machine states.
//!
//! A rewind un-runs a world's own part of history the worlds share (from the mark to where they split)
//! in that world alone. Every step undone there is linear, so the parts sum to the right state once they
//! merge, but the checks of the un-run (a read still holds its value, a register un-allocated is zero) hold
//! only for the sum: each part notes what fails them (`Defect`), and where no world runs, the worlds that
//! rewound last (`pending`) add their notes per check, which must cancel (`make_deferred_checks`). If they
//! do not, those worlds are put back from copies kept before their rewinds and fault `bad_rewind`. The norm
//! is still checked where every world has stopped.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use bennett_ir::{Fault, Inst};
use rand::Rng;

use crate::executor::{Executor, Frame, Halt, Outcome, ReverseError, RunError};
use crate::journal::{Journal, Undo};
use crate::memory::Memory;
use crate::quantum::{Amplitudes, QuantumState};

/// How far the worlds' summed squared amplitudes may be from 1 where every world has stopped. Rounding
/// over the few hundred gates of a test program is of order 1e-14, far below this.
const NORM_TOLERANCE: f64 = 1e-9;

/// Where a world is in the current tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Steps every tick.
    Running,
    /// Has just rewound, and waits until no other world is running.
    Parked,
    /// Is at a measurement, and waits until no other world is running or parked.
    Waiting,
    /// Is at a measurement and measures in the next tick.
    Measuring,
    /// Has returned, exited or faulted.
    Ended,
    /// Was observed not to be there (a measurement in another world), and is about to be dropped.
    Discarded,
}

/// A view of one world, for snapshots and reports.
pub(crate) struct WorldRef<'w> {
    pub(crate) memory: &'w Memory,
    pub(crate) frames: &'w [Frame],
    pub(crate) slots: &'w [u128],
    pub(crate) stack_pointer: u64,
    pub(crate) tape: &'w [u8],
    pub(crate) outcome: Option<Outcome>,
    pub(crate) quantum: &'w QuantumState,
}

/// A world that is not the current one: the executor's per-world fields, set aside.
#[derive(Clone, Debug)]
pub(crate) struct World {
    pub(crate) memory: Memory,
    pub(crate) slots: Vec<u128>,
    pub(crate) frames: Vec<Frame>,
    pub(crate) stack_pointer: u64,
    pub(crate) tape: Vec<u8>,
    pub(crate) quantum: QuantumState,
    pub(crate) journal: Option<Journal>,
    pub(crate) outcome: Option<Outcome>,
    pub(crate) status: Status,
    pub(crate) partial: bool,
    pub(crate) deferred: Vec<Defect>,
    pub(crate) backups: Vec<World>,
    pub(crate) pending: bool,
}

/// A check of a rewind's un-run below a world's split point, deferred to the sum of the worlds' parts: the
/// configurations of this world's part that fail it, keyed by the classical machine state where it was
/// made. Parts of the same state add, as their worlds would if they were there together.
#[derive(Clone, Debug)]
pub(crate) struct Defect {
    key: u64,
    amplitudes: Amplitudes,
}

impl World {
    fn empty() -> Self {
        World {
            memory: Memory::default(),
            slots: Vec::new(),
            frames: Vec::new(),
            stack_pointer: 0,
            tape: Vec::new(),
            quantum: QuantumState::default(),
            journal: None,
            outcome: None,
            status: Status::Running,
            partial: false,
            deferred: Vec::new(),
            backups: Vec::new(),
            pending: false,
        }
    }

    fn records(&self) -> usize {
        self.journal.as_ref().map_or(0, Journal::records)
    }

    fn live(&self) -> bool {
        self.status != Status::Discarded
    }
}

impl<'m> Executor<'m> {
    /// Exchanges the current world with `world`.
    pub(crate) fn swap_world(&mut self, world: &mut World) {
        std::mem::swap(&mut self.memory, &mut world.memory);
        std::mem::swap(&mut self.slots, &mut world.slots);
        std::mem::swap(&mut self.frames, &mut world.frames);
        std::mem::swap(&mut self.stack_pointer, &mut world.stack_pointer);
        std::mem::swap(&mut self.tape, &mut world.tape);
        std::mem::swap(&mut self.quantum, &mut world.quantum);
        std::mem::swap(&mut self.journal, &mut world.journal);
        std::mem::swap(&mut self.outcome, &mut world.outcome);
        std::mem::swap(&mut self.status, &mut world.status);
        std::mem::swap(&mut self.partial, &mut world.partial);
        std::mem::swap(&mut self.deferred, &mut world.deferred);
        std::mem::swap(&mut self.backups, &mut world.backups);
        std::mem::swap(&mut self.pending, &mut world.pending);
    }

    /// A copy of the current world, with `amplitudes` in place of its own if given.
    pub(crate) fn clone_world(&self, amplitudes: Option<crate::quantum::Amplitudes>) -> World {
        World {
            memory: self.memory.clone(),
            slots: self.slots.clone(),
            frames: self.frames.clone(),
            stack_pointer: self.stack_pointer,
            tape: self.tape.clone(),
            quantum: match amplitudes {
                Some(amplitudes) => self.quantum.with_amplitudes(amplitudes),
                None => self.quantum.clone(),
            },
            journal: self.journal.clone(),
            outcome: self.outcome,
            status: self.status,
            partial: self.partial,
            deferred: self.deferred.clone(),
            backups: self.backups.clone(),
            pending: self.pending,
        }
    }

    /// Splits the current world by the values of registers `columns` if its configurations disagree on
    /// them: it keeps the configurations of the smallest values, and each other part becomes a world of
    /// its own, which runs `step` itself and is left in `forked`. Every world keeps a journal from here on,
    /// so that the worlds stay told apart until a rewind un-runs what told them apart.
    /// Returns whether it split.
    pub(crate) fn split(&mut self, columns: &[usize], step: &dyn Fn(&mut Self) -> Result<(), Fault>) -> bool {
        let parts = self.quantum.partition(columns);
        // A part with no configuration left (`Executor::defer`) has nothing to split.
        if parts.len() <= 1 {
            return false;
        }
        self.journal.get_or_insert_with(Journal::default);
        let mut parts = parts.into_iter();
        let (_, first) = parts.next().expect("a state has a configuration");
        for (_, amplitudes) in parts {
            let mut world = self.clone_world(Some(amplitudes));
            // What this world's part failed before it split stays noted once, here.
            world.deferred.clear();
            self.splits += 1;
            self.split_memory_bytes += self.memory.overlay_len() as u64;
            self.split_journal_bytes += self.journal.as_ref().map_or(0, Journal::bytes) as u64;
            self.swap_world(&mut world);
            self.splitting = true;
            step(self).expect("a part of a world runs the step that its world runs");
            self.splitting = false;
            self.swap_world(&mut world);
            self.forked.push(world);
        }
        self.quantum.replace_amplitudes(first);
        true
    }

    /// Runs `rest` on every world that forked off since `forked` had `from` entries, each in turn as the
    /// current world, and leaves them (with any worlds that fork off during `rest`) in `forked`. An
    /// operation that may fork calls this after each part that may fork, with its own remainder as
    /// `rest`, so that every world finishes the whole operation.
    pub(crate) fn for_forks(&mut self, from: usize, rest: &dyn Fn(&mut Self)) {
        let pending = self.forked.split_off(from);
        for mut world in pending {
            self.swap_world(&mut world);
            rest(self);
            self.swap_world(&mut world);
            self.forked.push(world);
        }
    }

    /// Whether more than one world exists.
    pub(crate) fn has_worlds(&self) -> bool {
        !self.worlds.is_empty()
    }

    /// Makes world `index` the current one, for a check that reads each world's state.
    #[cfg(test)]
    pub(crate) fn select_world(&mut self, index: usize) {
        self.select(index);
    }

    /// Makes world `index` the current one.
    fn select(&mut self, index: usize) {
        if index == self.current {
            return;
        }
        let mut world = self.worlds[index].take().expect("a stored world");
        self.swap_world(&mut world);
        self.worlds[self.current] = Some(world);
        self.current = index;
    }

    /// Takes every world out, the current one included, in order.
    fn take_worlds(&mut self) -> Vec<World> {
        if self.worlds.is_empty() {
            let mut world = World::empty();
            self.swap_world(&mut world);
            return vec![world];
        }
        let mut current = World::empty();
        self.swap_world(&mut current);
        self.worlds[self.current] = Some(current);
        std::mem::take(&mut self.worlds).into_iter().map(|world| world.expect("every world is stored")).collect()
    }

    /// Puts `worlds` back, the first as the current one. A single world is the ordinary machine again.
    fn put_worlds(&mut self, worlds: Vec<World>) {
        self.peak_worlds = self.peak_worlds.max(worlds.len());
        let overlays: usize = worlds.iter().map(|world| world.memory.overlay_len()).sum();
        self.peak_overlay_bytes = self.peak_overlay_bytes.max(overlays);
        let total: usize = worlds.iter().map(|world| world.quantum.amplitudes().len()).sum();
        self.peak_world_configurations = self.peak_world_configurations.max(total);
        let mut worlds: Vec<Option<World>> = worlds.into_iter().map(Some).collect();
        let mut first = worlds[0].take().expect("at least one world");
        self.swap_world(&mut first);
        self.current = 0;
        if worlds.len() == 1 {
            // The other worlds are gone, so this one owns the base alone and its writes become the base.
            drop(first);
            self.memory.fold();
            self.status = Status::Running;
            self.run_outcome = None;
            self.departed.clear();
        } else {
            self.worlds = worlds;
        }
    }

    /// Moves the worlds that forked off during the current world's step into the list, with `status`.
    pub(crate) fn adopt_forks(&mut self, status: Status) {
        if self.forked.is_empty() {
            return;
        }
        if self.worlds.is_empty() {
            self.worlds.push(None);
            self.current = 0;
        }
        for mut world in self.forked.drain(..) {
            world.status = status;
            self.worlds.push(Some(world));
        }
        self.peak_worlds = self.peak_worlds.max(self.worlds.len());
    }

    fn world_status(&self, index: usize) -> Status {
        if index == self.current { self.status } else { self.worlds[index].as_ref().expect("stored").status }
    }

    /// Whether the top frame's next instruction is a `rewind`.
    fn next_is_rewind(&self) -> bool {
        matches!(self.next_inst(), Some(Inst::Rewind { .. }))
    }

    fn next_inst(&self) -> Option<&'m Inst> {
        let frame = *self.top();
        self.function(frame.func).block(frame.block).insts.get(frame.inst)
    }

    /// One step of a single world, outside any coherent region; worlds may fork off during it. Returns
    /// the outcome once the run has ended.
    pub(crate) fn step_alone(&mut self) -> Option<Outcome> {
        let was_rewind = self.next_is_rewind();
        let outcome = self.step_world();
        if !self.forked.is_empty() || self.pending {
            self.world_steps += self.forked.len() as u64;
            self.adopt_forks(if was_rewind { Status::Parked } else { Status::Running });
            if was_rewind {
                self.status = Status::Parked;
            }
            return self.end_of_tick();
        }
        outcome
    }

    /// One tick of several worlds: every running world takes one step.
    pub(crate) fn tick(&mut self) -> Option<Outcome> {
        if self.run_outcome.is_some() {
            return self.run_outcome;
        }
        if self.budget.is_some_and(|budget| self.steps >= budget) {
            self.run_outcome = Some(Outcome::Faulted(Fault::Budget));
            return self.run_outcome;
        }
        let count = self.worlds.len();
        for index in 0..count {
            let status = self.world_status(index);
            if !matches!(status, Status::Running | Status::Measuring) {
                continue;
            }
            self.select(index);
            if status == Status::Running && matches!(self.next_inst(), Some(Inst::Measure { .. })) {
                self.status = Status::Waiting;
                continue;
            }
            self.status = Status::Running;
            self.others_configurations = self.configurations_besides_current();
            let was_rewind = self.next_is_rewind();
            let result = self.execute();
            self.world_steps += 1 + self.forked.len() as u64;
            match result {
                Ok(()) => {
                    if self.outcome.is_some() {
                        self.status = Status::Ended;
                    } else if was_rewind {
                        self.status = Status::Parked;
                    }
                    let status = self.status;
                    self.adopt_forks(status);
                }
                Err(Halt::Fault(fault)) => {
                    if !self.allow_region_faults && self.others_go_on() {
                        let worlds = self.worlds.len();
                        self.run_outcome = Some(Outcome::Stopped(RunError::WorldFault { fault, worlds }));
                        return self.run_outcome;
                    }
                    self.outcome = Some(Outcome::Faulted(fault));
                    self.status = Status::Ended;
                }
                Err(Halt::Stop(error)) => {
                    self.run_outcome = Some(Outcome::Stopped(error));
                    return self.run_outcome;
                }
                Err(Halt::Discard) => self.status = Status::Discarded,
            }
        }
        self.others_configurations = 0;
        self.steps += 1;
        self.end_of_tick()
    }

    /// Whether a world other than the current one has not ended.
    fn others_go_on(&self) -> bool {
        self.worlds.iter().flatten().any(|world| !matches!(world.status, Status::Ended | Status::Discarded))
    }

    fn configurations_besides_current(&self) -> usize {
        self.worlds.iter().flatten().map(|world| world.quantum.amplitudes().len()).sum()
    }

    /// After a tick: drops discarded worlds, merges equal parked worlds, and when no world runs, lets the
    /// deepest parked worlds go on or, once every world has ended, finishes the run.
    fn end_of_tick(&mut self) -> Option<Outcome> {
        let mut worlds = self.take_worlds();
        worlds.retain(World::live);
        self.merge_equal(&mut worlds, |status| status == Status::Parked);
        if !worlds.iter().any(|world| matches!(world.status, Status::Running | Status::Measuring))
            && let Some(error) = self.make_deferred_checks(&mut worlds)
        {
            return self.stop_worlds(worlds, error);
        }
        if worlds.len() == 1
            && let Err(error) = self.check_norm(&worlds)
        {
            return self.stop_worlds(worlds, error);
        }
        let hashes: Vec<Option<u64>> = worlds
            .iter()
            .map(|world| (world.status == Status::Parked && !self.departed.is_empty()).then(|| Self::hash_world(world)))
            .collect();
        if hashes.iter().flatten().any(|hash| self.departed.contains(hash)) {
            return self.stop_worlds(worlds, RunError::Unaligned);
        }
        if !worlds.iter().any(|world| matches!(world.status, Status::Running | Status::Measuring)) {
            let deepest = worlds.iter().filter(|w| w.status == Status::Parked).map(World::records).max();
            match deepest {
                Some(depth) => {
                    let alone = worlds.iter().filter(|world| world.status != Status::Ended).count() == 1;
                    for world in &mut worlds {
                        if world.status == Status::Parked && world.records() == depth {
                            world.status = Status::Running;
                        }
                    }
                    if !alone {
                        let departing: Vec<u64> = worlds
                            .iter()
                            .filter(|world| world.status == Status::Running)
                            .map(Self::hash_world)
                            .collect();
                        self.departed.extend(departing);
                    }
                }
                None if worlds.iter().any(|world| world.status == Status::Waiting) => {
                    // Every world has stopped: merge what is equal, check the norm, and let the first
                    // waiting world measure.
                    self.merge_equal(&mut worlds, |_| true);
                    if let Err(error) = self.check_norm(&worlds) {
                        return self.stop_worlds(worlds, error);
                    }
                    let first = worlds.iter_mut().find(|world| world.status == Status::Waiting);
                    first.expect("a waiting world").status = Status::Measuring;
                }
                None => return self.finish_worlds(worlds),
            }
        }
        self.put_worlds(worlds);
        if self.has_worlds() { None } else { self.outcome }
    }

    /// Ends a run of several worlds at a check of the simulator, keeping every world.
    fn stop_worlds(&mut self, worlds: Vec<World>, error: RunError) -> Option<Outcome> {
        let single = worlds.len() == 1;
        self.put_worlds(worlds);
        let outcome = Some(Outcome::Stopped(error));
        if single {
            self.outcome = outcome;
        } else {
            self.run_outcome = outcome;
        }
        outcome
    }

    /// Every world has ended: equal worlds merge, and if several remain, the host reading the result
    /// observes the machine: one world is sampled by its weight and the others are dropped.
    fn finish_worlds(&mut self, mut worlds: Vec<World>) -> Option<Outcome> {
        self.merge_equal(&mut worlds, |_| true);
        if let Err(error) = self.check_norm(&worlds) {
            return self.stop_worlds(worlds, error);
        }
        if worlds.len() > 1 {
            let weights: Vec<f64> = worlds.iter().map(|world| world.quantum.weight()).collect();
            let total: f64 = weights.iter().sum();
            let mut draw = self.rng.random::<f64>() * total;
            let mut chosen = worlds.len() - 1;
            for (index, &weight) in weights.iter().enumerate() {
                if draw < weight {
                    chosen = index;
                    break;
                }
                draw -= weight;
            }
            let mut world = worlds.swap_remove(chosen);
            world.quantum.scale(weights[chosen].sqrt().recip());
            worlds = vec![world];
            self.observations += 1;
            self.observed = true;
        }
        self.put_worlds(worlds);
        self.outcome
    }

    /// Checks that the worlds' summed squared amplitudes are 1, where every world has stopped.
    fn check_norm(&self, worlds: &[World]) -> Result<(), RunError> {
        let total: f64 = worlds.iter().map(|world| world.quantum.weight()).sum();
        if (total - 1.0).abs() > NORM_TOLERANCE {
            return Err(RunError::NormLost(((total - 1.0).abs() * 1e9) as u64));
        }
        Ok(())
    }

    /// Merges every pair of worlds whose status `eligible` accepts and that are the same machine state,
    /// into the earlier one: one basis state, so the amplitudes add.
    fn merge_equal(&mut self, worlds: &mut Vec<World>, eligible: impl Fn(Status) -> bool) {
        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        for (index, world) in worlds.iter().enumerate() {
            if eligible(world.status) {
                buckets.entry(Self::hash_world(world)).or_default().push(index);
            }
        }
        let mut groups: Vec<Vec<usize>> = buckets.into_values().filter(|group| group.len() > 1).collect();
        if groups.is_empty() {
            return;
        }
        groups.sort();
        let mut gone = vec![false; worlds.len()];
        for group in groups {
            for (k, &into) in group.iter().enumerate() {
                if gone[into] {
                    continue;
                }
                for &from in &group[k + 1..] {
                    if gone[from] || !self.same_world(&worlds[into], &worlds[from]) {
                        continue;
                    }
                    let quantum = std::mem::take(&mut worlds[from].quantum);
                    let deferred = std::mem::take(&mut worlds[from].deferred);
                    let backups = std::mem::take(&mut worlds[from].backups);
                    let pending = worlds[from].pending;
                    let world = &mut worlds[into];
                    world.quantum.absorb(quantum);
                    world.deferred.extend(deferred);
                    world.backups.extend(backups);
                    world.pending |= pending;
                    gone[from] = true;
                    self.merges += 1;
                }
            }
        }
        let mut index = 0;
        worlds.retain(|_| {
            index += 1;
            !gone[index - 1]
        });
    }

    /// The live slots of a world's frames.
    fn live_slots<'w>(&self, world: &'w World) -> &'w [u128] {
        let live = world.frames.last().map_or(0, |frame| frame.base + self.function(frame.func).slot_count());
        &world.slots[..live]
    }

    /// Whether two worlds are the same machine state: everything but the amplitudes. The slots above the
    /// live frames count too: a returned callee's slots are kept there, unjournalled, for undoing the
    /// return, so they hold what the callee computed (a register value it read, for one).
    fn same_world(&self, a: &World, b: &World) -> bool {
        a.stack_pointer == b.stack_pointer
            && a.frames == b.frames
            && a.outcome == b.outcome
            && written_slots(&a.slots) == written_slots(&b.slots)
            && a.tape == b.tape
            && a.journal == b.journal
            && a.quantum.same_registers(&b.quantum)
            && a.memory.same(&b.memory)
    }

    fn hash_world(world: &World) -> u64 {
        let mut state = DefaultHasher::new();
        world.stack_pointer.hash(&mut state);
        world.frames.hash(&mut state);
        world.outcome.is_some().hash(&mut state);
        written_slots(&world.slots).hash(&mut state);
        world.tape.hash(&mut state);
        world.journal.hash(&mut state);
        world.quantum.hash_registers(&mut state);
        world.memory.hash_overlay(&mut state);
        state.finish()
    }

    /// Notes a check deferred below the current world's split point (`history.rs`): `failing`, the
    /// configurations of this world's part that fail it, keyed by the check (`kind`, the record it undoes
    /// or the step it runs again) and the classical machine state here, which is what worlds share where
    /// their parts add. Nothing is noted outside a rewind taken as a step, or if nothing fails.
    pub(crate) fn defer(&mut self, kind: Undo, failing: Amplitudes) {
        if !self.deferring || failing.is_empty() {
            return;
        }
        let mut state = DefaultHasher::new();
        kind.hash(&mut state);
        self.stack_pointer.hash(&mut state);
        self.frames.hash(&mut state);
        self.outcome.is_some().hash(&mut state);
        written_slots(&self.slots).hash(&mut state);
        self.tape.hash(&mut state);
        self.journal.hash(&mut state);
        self.quantum.hash_registers(&mut state);
        self.memory.hash_overlay(&mut state);
        self.deferred.push(Defect { key: state.finish(), amplitudes: failing });
    }

    /// Makes the checks the worlds that rewound last deferred, where no world runs: those worlds (`pending`,
    /// parked with the longest history) are every part that is summed at those checks, as a rewind joins
    /// every world at it. The parts' failing configurations at each check add; if any sum is not zero (its
    /// weight above the norm tolerance), every one of those worlds is put back as it was before its rewind,
    /// which faults `bad_rewind`, changing nothing. Returns the stop if such a fault stops the run (some
    /// other world goes on and region faults are not allowed).
    fn make_deferred_checks(&mut self, worlds: &mut Vec<World>) -> Option<RunError> {
        loop {
            let deepest = worlds.iter().filter(|world| world.status == Status::Parked).map(World::records).max()?;
            let joined = |world: &World| world.status == Status::Parked && world.records() == deepest && world.pending;
            if !worlds.iter().any(joined) {
                return None;
            }
            let mut sums = HashMap::<u64, Amplitudes>::new();
            for world in worlds.iter_mut().filter(|world| joined(world)) {
                for defect in std::mem::take(&mut world.deferred) {
                    let sum = sums.entry(defect.key).or_default();
                    for (config, amplitude) in defect.amplitudes {
                        *sum.entry(config).or_default() += amplitude;
                    }
                }
            }
            let weight = |sum: &Amplitudes| sum.values().map(|amplitude| amplitude.norm_sqr()).sum::<f64>();
            if sums.values().all(|sum| weight(sum) <= NORM_TOLERANCE) {
                // A part that a deferred check emptied held only what other parts cancelled.
                worlds.retain_mut(|world| {
                    if !joined(world) {
                        return true;
                    }
                    world.pending = false;
                    world.backups.clear();
                    !world.quantum.amplitudes().is_empty()
                });
                return None;
            }
            let mut restored = Vec::new();
            worlds.retain_mut(|world| {
                let keep = !joined(world);
                if !keep {
                    restored.append(&mut world.backups);
                }
                keep
            });
            for mut world in restored {
                world.status = Status::Ended;
                world.outcome = Some(Outcome::Faulted(Fault::BadRewind));
                worlds.push(world);
            }
            if !self.allow_region_faults
                && worlds.iter().any(|world| !matches!(world.status, Status::Ended | Status::Discarded))
            {
                return Some(RunError::WorldFault { fault: Fault::BadRewind, worlds: worlds.len() });
            }
        }
    }

    /// A measurement in the current world while other worlds exist. The machine is observed to be in this
    /// world at this measurement, or not: with probability equal to this world's weight it is, and every
    /// other world is dropped; otherwise this world is dropped (`Halt::Discard`) and the others keep their
    /// coherence, scaled up to the norm they now carry. No other world can still become this one, so the
    /// observation takes nothing that a later merge would have needed.
    pub(crate) fn observe_world(&mut self) -> Result<(), Halt> {
        let mine = self.quantum.weight();
        let others: f64 =
            self.worlds.iter().flatten().filter(|world| world.live()).map(|world| world.quantum.weight()).sum();
        let draw = self.rng.random::<f64>() * (mine + others);
        self.observations += 1;
        self.observed = true;
        if draw < mine {
            for world in self.worlds.iter_mut().flatten() {
                world.status = Status::Discarded;
            }
            return Ok(());
        }
        let factor = ((mine + others) / others).sqrt();
        for world in self.worlds.iter_mut().flatten().filter(|world| world.live()) {
            world.quantum.scale(factor);
        }
        Err(Halt::Discard)
    }

    /// Undoes the latest step of every world with the longest history, then merges worlds that are equal
    /// again. Worlds that split apart have the same history up to the split, so the longer ones come back
    /// to it first and the worlds merge there before any of them un-runs what they share.
    pub(crate) fn step_back_worlds(&mut self) -> Result<bool, ReverseError> {
        // A rewind whose deferred checks were never made (the run stopped first) is taken back from its
        // copies: its parts need not sum to a state its history describes.
        if self.pending || self.worlds.iter().flatten().any(|world| world.pending) {
            let mut worlds = self.take_worlds();
            let mut restored = Vec::new();
            worlds.retain_mut(|world| {
                restored.append(&mut world.backups);
                !world.pending
            });
            worlds.append(&mut restored);
            self.put_worlds(worlds);
        }
        let records: Vec<usize> = self.journals().iter().map(|journal| journal.map_or(0, Journal::records)).collect();
        let longest = records.iter().copied().max().unwrap_or(0);
        let count = self.worlds.len();
        for index in (0..count).filter(|&index| records[index] == longest) {
            self.select(index);
            if self.journal.as_ref().and_then(Journal::last) == Some(Undo::Measure) {
                return Err(ReverseError::PastMeasurement);
            }
        }
        self.run_outcome = None;
        for (index, &length) in records.iter().enumerate() {
            self.select(index);
            self.status = Status::Running;
            if matches!(self.outcome, Some(Outcome::Faulted(_) | Outcome::Stopped(_))) && !self.frames.is_empty() {
                self.outcome = None;
            }
            if longest > 0 && length == longest {
                self.undo_last();
                self.adopt_forks(Status::Running);
            }
        }
        let mut worlds = self.take_worlds();
        self.merge_equal(&mut worlds, |_| true);
        self.put_worlds(worlds);
        if longest > 0 {
            self.steps = self.steps.saturating_sub(1);
        }
        Ok(longest > 0)
    }

    /// Every world, in order.
    pub(crate) fn world_states(&self) -> Vec<WorldRef<'_>> {
        let current = WorldRef {
            memory: &self.memory,
            frames: &self.frames,
            slots: &self.slots[..self.live_top()],
            stack_pointer: self.stack_pointer,
            tape: &self.tape,
            outcome: self.outcome,
            quantum: &self.quantum,
        };
        if self.worlds.is_empty() {
            return vec![current];
        }
        let mut current = Some(current);
        self.worlds
            .iter()
            .map(|world| match world {
                None => current.take().expect("one current world"),
                Some(world) => WorldRef {
                    memory: &world.memory,
                    frames: &world.frames,
                    slots: self.live_slots(world),
                    stack_pointer: world.stack_pointer,
                    tape: &world.tape,
                    outcome: world.outcome,
                    quantum: &world.quantum,
                },
            })
            .collect()
    }

    /// The journals of every world.
    pub(crate) fn journals(&self) -> Vec<Option<&Journal>> {
        if self.worlds.is_empty() {
            return vec![self.journal.as_ref()];
        }
        self.worlds
            .iter()
            .map(|world| match world {
                None => self.journal.as_ref(),
                Some(world) => world.journal.as_ref(),
            })
            .collect()
    }

    /// The bytes every world's overlay holds now, summed.
    pub(crate) fn overlay_bytes(&self) -> usize {
        self.world_states().iter().map(|world| world.memory.overlay_len()).sum()
    }

    /// The quantum states of every world.
    pub(crate) fn quantum_states(&self) -> Vec<&QuantumState> {
        if self.worlds.is_empty() {
            return vec![&self.quantum];
        }
        self.worlds
            .iter()
            .map(|world| match world {
                None => &self.quantum,
                Some(world) => &world.quantum,
            })
            .collect()
    }
}

/// The slots up to the last nonzero one: the slot stack only grows, with zeros, so two worlds that went
/// to different depths compare equal where they hold the same values.
fn written_slots(slots: &[u128]) -> &[u128] {
    let end = slots.iter().rposition(|&slot| slot != 0).map_or(0, |last| last + 1);
    &slots[..end]
}

#[cfg(test)]
mod tests {
    use rand::seq::SliceRandom;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    use crate::executor::{Executor, Options};

    /// The merge rule on memory: two worlds that are the same machine state but for their memory merge
    /// exactly when their overlays describe the same bytes, however their writes were ordered. Each case
    /// writes the same final bytes into both worlds' overlays in different random orders, with other
    /// values and the base's own bytes written in between; in half the cases one byte then differs.
    #[test]
    fn worlds_merge_exactly_when_their_overlays_describe_equal_memory() {
        let module = bennett_ir::load("memory 64\nfunc @main() {\nentry:\n    ret\n}\n").expect("valid");
        let entry = module.function_named("main").expect("@main");
        let mut executor = Executor::new(&module, entry, &[], Options::default()).expect("starts");
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let (mut merged, mut apart) = (0, 0);
        for case in 0..400 {
            let base: Vec<u8> = executor.memory.to_vec();
            let target: Vec<u8> =
                (0..base.len()).map(|_| if rng.random_bool(0.2) { rng.random_range(0..3) } else { 0 }).collect();
            let mut worlds = vec![executor.clone_world(None), executor.clone_world(None)];
            for world in &mut worlds {
                let mut writes: Vec<(usize, u8, bool)> = Vec::new();
                for (address, &byte) in target.iter().enumerate() {
                    for _ in 0..rng.random_range(0..3u32) {
                        writes.push((address, rng.random_range(0..3), false));
                    }
                    writes.push((address, byte, true));
                }
                writes.shuffle(&mut rng);
                writes.sort_by_key(|&(_, _, last)| last);
                for (address, byte, _) in writes {
                    world.memory.write(address, &[byte]);
                }
            }
            let differ = case % 2 == 1;
            if differ {
                let address = rng.random_range(0..target.len());
                worlds[1].memory.write(address, &[target[address] ^ 4]);
            }
            let merges = executor.merges;
            executor.merge_equal(&mut worlds, |_| true);
            if differ {
                assert_eq!((worlds.len(), executor.merges), (2, merges), "case {case}: unequal memory merged");
                apart += 1;
            } else {
                assert_eq!((worlds.len(), executor.merges), (1, merges + 1), "case {case}: equal memory apart");
                assert_eq!(worlds[0].memory.to_vec(), target);
                merged += 1;
            }
        }
        assert_eq!((merged, apart), (200, 200));
    }

    /// Equal complete worlds after overlapping stores have been un-run: only their memory can
    /// differ. All 256 pairs of four-byte targets are enumerated, including the unchanged base.
    #[test]
    fn independent_memory_only_worlds_merge_after_overlapping_writes_are_un_run() {
        let p = bennett_ir::load("memory 128\ndata 0 = i32 151521030\nfunc @main() {\nentry:\nstore i32 305419896, 0\nstore i16 48879, 1\nret\n}\n").expect("valid");
        let mut e = Executor::new(
            &p,
            p.function_named("main").expect("main"),
            &[],
            Options { reversible: true, ..Options::default() },
        )
        .expect("start");
        let base = e.memory.to_vec();
        let mut counts = [0, 0];
        for a in 0_u8..16 {
            for b in 0_u8..16 {
                let mut worlds = vec![e.clone_world(None), e.clone_world(None)];
                for (i, (w, target)) in worlds.iter_mut().zip([a, b]).enumerate() {
                    let order: Vec<_> = if i == 0 { (0..4).collect() } else { (0..4).rev().collect() };
                    w.memory.write(0, &[255; 4]);
                    for address in order {
                        w.memory.write(address, &[base[address] ^ if target & (1 << address) != 0 { 128 } else { 0 }]);
                    }
                    let bytes = w.memory.to_vec();
                    let overlay = w.memory.overlay();
                    let journal = w.journal.clone();
                    let frames = w.frames.clone();
                    e.swap_world(w);
                    assert_eq!(e.step_world(), None);
                    assert_eq!(e.step_world(), None);
                    e.undo_last();
                    e.undo_last();
                    assert_eq!(e.memory.to_vec(), bytes);
                    assert_eq!(e.memory.overlay(), overlay);
                    assert_eq!(e.journal, journal);
                    assert_eq!(e.frames, frames);
                    e.swap_world(w);
                }
                e.merge_equal(&mut worlds, |_| true);
                assert_eq!(worlds.len(), if a == b { 1 } else { 2 }, "targets {a}/{b}");
                counts[usize::from(a != b)] += 1;
                assert_eq!(e.memory.to_vec(), base, "base held by the template stays unchanged");
            }
        }
        assert_eq!(counts, [16, 240]);
        println!("256 complete world pairs after store undo: 16 equal merge, 240 unequal stay apart");
    }
}
