//! `apply` computed as the device program's own forms (`docs/DESIGN.md`, "`apply` as the forms"):
//!
//! ```text
//! %h = mark budget B + 3
//! %y, %c = call @f(args) budget B
//! rewind %h keep (%y -> out, %c -> fault)
//! ```
//!
//! [`ApplyMode::Forms`](crate::ApplyMode::Forms) computes each `apply` this way, through the code that runs
//! those instructions: the mark's journal entry with its budget and deadline; the call with a budget (its
//! reads of registers, which split the world, the budget held in the callee's frame, the region's rules while
//! that frame is live, a fault caught as the code, a frame that does not fit caught as `stack_overflow`); and
//! the rewind with its keeps, its checks and its un-run. `%h`, `%y` and `%c` are the executor's own, not slots
//! of the caller, so the caller's frame, stack and instruction positions are those of the other modes and the
//! callee sees the same addresses and return addresses.
//!
//! The `apply` stays one step of the run, journalled as one record: once the rewind has run, its record and the
//! mark's are taken off the journal, which then holds what it held before the `apply`, and the caller journals
//! `Apply` as in the other modes (an `apply` is its own inverse). The worlds the call's reads and the callee's
//! loads split off run one after another in the world's place, as in [`ApplyMode::History`]: each rewinds, and
//! since nothing in a region changes a register or a cell, every world is then the world before the `apply`
//! again, and their amplitudes are gathered.
//!
//! [`ApplyMode::History`]: crate::ApplyMode::History

use std::collections::BTreeMap;

use bennett_ir::{Fault, FuncId, Operand, Slot};

use crate::executor::{Executor, Frame, Halt, Outcome};
use crate::journal::{Journal, MarkEntry, Undo};
use crate::quantum::Config;
use crate::region::Counters;
use crate::worlds::World;

/// The steps of the forms besides the callee's `B`: the call, a catch and the rewind (the mark's own step
/// comes before its budget). `docs/DESIGN.md`, "The call with a fault value".
const FORMS_STEPS: u64 = 3;

/// The run's state set aside while an `apply` is computed as the forms.
struct Outer {
    outcome: Option<Outcome>,
    steps: u64,
    world_steps: u64,
    charged: u64,
    charging: bool,
    budget_spent: bool,
    budget: Option<u64>,
    allow_region_faults: bool,
    any_guard: bool,
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

/// What the worlds of the forms left.
#[derive(Default)]
struct Gathered {
    amplitudes: Vec<(Config, num_complex::Complex64)>,
    faulting: Vec<(Config, f64, Fault)>,
    clock: u64,
    region_steps: u64,
    /// The first world that did not reach its rewind, or whose rewind faulted: the `apply` faults with it,
    /// changing nothing.
    stop: Option<Halt>,
}

/// The places of the forms: the `apply`'s position, its mark, the columns of `out` and `fault`, the depth
/// of the caller's frames.
#[derive(Clone, Copy)]
struct Places {
    site: Frame,
    entry: MarkEntry,
    out: usize,
    fault: usize,
    depth: usize,
}

impl<'m> Executor<'m> {
    /// `apply` as mark, the call with a budget and rewind-keep (module documentation).
    pub(crate) fn apply_by_forms(
        &mut self,
        callee: FuncId,
        args: &'m [Operand],
        out: Slot,
        fault: Slot,
        budget: &Operand,
    ) -> Result<(), Halt> {
        // The `apply`'s own operand checks: the keeps' targets resolved, no argument among them.
        let (out, fault, _) = self.region_operands(args, out, fault)?;
        let budget = self.read_word(budget);
        let configurations = self.quantum.amplitudes().len();
        if configurations == 0 {
            // A world's part that a deferred check emptied carries no amplitude: nothing to permute.
            return Ok(());
        }
        let site = *self.top();
        let start = self.clone_world(Some(BTreeMap::new()));
        let original = self.quantum.amplitudes().clone();
        let outer = self.begin_forms();

        // %h = mark budget B + 3: the `apply`'s own step.
        let mark_budget = budget.saturating_add(FORMS_STEPS);
        let allocations = self.quantum.allocations();
        let deadline = self.clock.checked_add(mark_budget);
        let depth = self.frames.len();
        let journal = self.journal.get_or_insert_with(Journal::default);
        let position = journal.records() + 1;
        journal.push_mark(MarkEntry { position, allocations, budget: Some(mark_budget), deadline }, depth);
        self.record(Undo::Mark);
        let entry = self.journal.as_ref().and_then(|journal| journal.mark_at(position)).expect("the mark just made");
        let places = Places { site, entry, out, fault, depth };

        // %y, %c = call @f(args) budget B: a step. Its reads split the world; each world runs to its rewind in
        // turn, in this world's place.
        let mut gathered = Gathered { clock: self.clock, ..Gathered::default() };
        let called = self.forms_step().and_then(|()| self.call_with_budget(callee, args, budget));
        let mut queue: Vec<World> = std::mem::take(&mut self.forked);
        match called {
            Ok(()) => self.finish_forms_world(places, &start, outer.had_journal, &mut queue, &mut gathered),
            Err(halt) => {
                gathered.stop.get_or_insert(halt);
            }
        }
        while let Some(mut world) = queue.pop() {
            self.swap_world(&mut world);
            self.finish_forms_world(places, &start, outer.had_journal, &mut queue, &mut gathered);
            self.swap_world(&mut world);
        }

        // Faults are counted in configuration order, as the branches were enumerated.
        gathered.faulting.sort_by(|a, b| a.0.cmp(&b.0));
        let faulting: Vec<(&Config, f64, Fault)> = gathered.faulting.iter().map(|(c, w, f)| (c, *w, *f)).collect();
        // The run's own setting decides whether a fault on some configurations stops it, as for the idiom.
        self.allow_region_faults = outer.allow_region_faults;
        let error =
            if gathered.stop.is_some() { None } else { self.count_region_faults(callee, &faulting, configurations) };
        let failed = gathered.stop.is_some() || error.is_some();
        if failed {
            // The state as before the `apply`: a world that did not finish is set aside.
            let mut world = start;
            world.quantum.replace_amplitudes(original);
            self.swap_world(&mut world);
        } else {
            gathered.amplitudes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            let amplitudes: BTreeMap<Config, _> = gathered.amplitudes.into_iter().collect();
            assert_eq!(amplitudes.len(), configurations, "the forms permuted the configurations");
            self.quantum.replace_amplitudes(amplitudes);
            self.clock = gathered.clock;
        }
        self.memory.fold();
        self.end_forms(outer, gathered.region_steps);
        match (gathered.stop, error) {
            (Some(halt), _) => Err(halt),
            (None, Some(error)) => Err(error.into()),
            (None, None) => Ok(()),
        }
    }

    /// The current world of the forms, its call made: runs the callee to its return or catch, then
    /// `rewind %h keep (%y -> out, %c -> fault)`, and gathers its amplitudes. Worlds that split off join
    /// `queue`.
    fn finish_forms_world(
        &mut self,
        places: Places,
        start: &World,
        had_journal: bool,
        queue: &mut Vec<World>,
        gathered: &mut Gathered,
    ) {
        let steps = self.steps;
        let weight = self.quantum.amplitudes().len() as u64;
        self.region_cost.runs += 1;
        // The callee runs until its frame is gone: a return or a catch, which give `%y`, `%c`.
        self.inspected = false;
        while self.frames.len() > places.depth {
            let outcome = self.step_world();
            queue.append(&mut self.forked);
            match outcome {
                None => {}
                Some(Outcome::Faulted(fault)) => {
                    let halt = if std::mem::take(&mut self.inspected) { Halt::Inspected(fault) } else { fault.into() };
                    gathered.stop.get_or_insert(halt);
                    return;
                }
                Some(Outcome::Stopped(error)) => {
                    gathered.stop.get_or_insert(error.into());
                    return;
                }
                Some(Outcome::Returned(_)) => unreachable!("the callee returns to the `apply`'s frame"),
            }
        }
        gathered.region_steps += (self.steps - steps) * weight;
        let (value, code) = self.forms_values.take().expect("a return or a catch gives the call's values");
        // rewind %h keep (%y -> out, %c -> fault): a step.
        let kept = vec![(places.out, value as u64), (places.fault, code as u64)];
        let rewound = self
            .forms_step()
            .and_then(|()| self.rewind_kept(places.entry, kept, (places.site.block, places.site.inst)));
        if let Err(halt) = rewound {
            gathered.stop.get_or_insert(halt);
            return;
        }
        // A region changes no register and no cell, so its parts hold what their history read: nothing is
        // deferred, and the copy kept to restore is dropped, as the checks on the sum would.
        assert!(self.deferred.is_empty(), "a part of a region's world holds what its history read");
        self.pending = false;
        self.backups.clear();
        // The forms are one step: the rewind's record and the mark's leave the journal, and the frame is back on
        // the `apply`, which the caller steps past.
        let journal = self.journal.as_mut().expect("the forms keep a journal");
        assert_eq!(journal.last(), Some(Undo::Rewind), "the rewind left nothing over");
        let index = journal.records() - 1;
        journal.pop();
        journal.pop_word();
        journal.take_rewind_cost(index);
        assert_eq!(journal.pop(), Some(Undo::Mark), "the forms' mark is under their rewind");
        journal.pop_mark();
        if !had_journal {
            let journal = self.journal.take().expect("the forms' journal");
            assert_eq!(journal.records(), 0, "the forms left no history");
        }
        let top = self.top_mut();
        (top.block, top.inst) = (places.site.block, places.site.inst);
        gathered.clock = gathered.clock.max(self.clock);
        let mut world = World::empty();
        self.swap_world(&mut world);
        assert!(self.same_world(start, &world), "a world of the forms is the world before the `apply` again");
        if code != 0 {
            let fault = Fault::from_code(code as u64).expect("a caught fault's code");
            // The configurations as they were before the `apply`, which order them.
            gathered.faulting.extend(world.quantum.amplitudes().iter().map(|(config, a)| {
                let mut config = config.clone();
                config[places.fault] ^= code as u64;
                (config, a.norm_sqr(), fault)
            }));
        }
        gathered.amplitudes.extend(world.quantum.take_amplitudes());
        self.swap_world(&mut world);
    }

    /// The execute preamble of one step of the forms outside the callee: a mark whose budget is spent faults
    /// `budget`; the step takes one unit of the world's time.
    fn forms_step(&mut self) -> Result<(), Halt> {
        let deadline = self.journal.as_ref().and_then(|journal| journal.mark_deadline(self.region_records));
        if deadline.is_some_and(|deadline| self.clock >= deadline) {
            return Err(Fault::Budget.into());
        }
        self.clock += 1;
        Ok(())
    }

    /// Sets the run's state aside for the forms: they run as one step of the run, faults caught per world.
    fn begin_forms(&mut self) -> Outer {
        Outer {
            outcome: self.outcome.take(),
            steps: self.steps,
            world_steps: self.world_steps,
            charged: std::mem::take(&mut self.charged),
            charging: std::mem::replace(&mut self.charging, false),
            budget_spent: std::mem::replace(&mut self.budget_spent, false),
            budget: self.budget.take(),
            allow_region_faults: std::mem::replace(&mut self.allow_region_faults, true),
            any_guard: self.any_guard,
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

    /// Puts the run's state back after the forms. Their splits, merges and history work are their own cost;
    /// the run counts the `apply` as one step, the region's steps per configuration and its faults.
    fn end_forms(&mut self, outer: Outer, region_steps: u64) {
        let (region_faults, weight, max_weight) =
            (self.region_faults, self.region_fault_weight, self.region_fault_max_weight);
        outer.counters.set(self);
        (self.region_faults, self.region_fault_weight, self.region_fault_max_weight) =
            (region_faults, weight, max_weight);
        self.region_steps += region_steps;
        self.outcome = outer.outcome;
        self.steps = outer.steps;
        self.world_steps = outer.world_steps;
        self.charged = outer.charged;
        self.charging = outer.charging;
        self.budget_spent = outer.budget_spent;
        self.budget = outer.budget;
        self.allow_region_faults = outer.allow_region_faults;
        self.any_guard = outer.any_guard;
        self.forked = outer.forked;
        self.partial = outer.partial;
        self.deferring = outer.deferring;
        self.rewinding = outer.rewinding;
        self.rerunning = outer.rerunning;
        self.splitting = outer.splitting;
        self.deferred = outer.deferred;
        self.backups = outer.backups;
        self.pending = outer.pending;
        self.forms_values = None;
        self.inspected = false;
    }
}
