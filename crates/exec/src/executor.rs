//! The executor's state, its construction, and the public stepping interface.

use bennett_ir::{BlockId, Fault, FuncId, Function, Module, Operand, Slot, Type, ValidModule};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

use crate::journal::{Journal, Undo};
use crate::measure::Collector;
use crate::memory::Memory;
use crate::quantum::QuantumState;
use crate::region::{ApplyMode, RegionCost};
use crate::snapshot::{ClassicalSnapshot, QuantumSnapshot, Snapshot};
use crate::worlds::{Defect, Status, World};

/// How to run.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// The most steps the run may take; the next step faults with [`Fault::Budget`]. `None` is unbounded.
    /// Steps a `rewind` runs again are charged to it too ([`Stats::replay_charged`]).
    pub budget: Option<u64>,
    /// The most basis configurations the quantum state may hold; a gate that would exceed it stops the run
    /// with [`RunError::ConfigurationLimit`]. `None` is unbounded. Each configuration costs memory and time
    /// in every later quantum step, and its count can double with every gate. This is a limit of the
    /// simulator, not part of the program's meaning, so it is not a fault.
    pub max_configurations: Option<usize>,
    /// The seed of the generator that `measure` samples with.
    pub seed: u64,
    /// Whether to keep the undo journal, so that the run can be reversed.
    pub reversible: bool,
    /// Whether an `apply` may fault on some of its branches and the run continue. By default (`false`) the
    /// first such `apply` stops the run with [`RunError::RegionFault`]: a program cannot detect a fault on
    /// only some branches with certainty, and the other branches continue with the interference gone, so
    /// a run that went on would look like a success. Set it for programs that inspect their fault
    /// registers on purpose. Either way the faults are counted in [`Stats`].
    pub allow_region_faults: bool,
    /// Whether to run the machine's meaning instead of the simulator's strict mode (the default,
    /// `docs/DESIGN.md`, "Strict mode"). Strict mode adds diagnostics no device can make, as a sanitizer
    /// does: an `apply` or a world faulting on only some branches stops the run, and `qfree` of a register
    /// that is not zero faults `dirty_free`. On the machine a fault on some branches is a fault code the
    /// program may measure (as with `allow_region_faults`), and the qubits of a dirty `qfree` are left
    /// over: kept, never released, counted in [`Stats::leftover_qubits`], the toolchain's warning.
    pub machine_meaning: bool,
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The entry returned, or `exit` finished the run, with the result value if it has one.
    Returned(Option<u128>),
    /// Execution stopped with a named fault. The state is as it was before the faulting step.
    Faulted(Fault),
    /// The simulator stopped the run at one of its own limits or checks; the program has no result. The
    /// state is as it was before the step that would have exceeded the limit or failed the check.
    Stopped(RunError),
}

/// A limit or check of the simulator that a run reached. Unlike a [`Fault`], it is not part of the
/// program's meaning: another simulator, or the same one with a larger limit or the check switched off,
/// would continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RunError {
    /// A gate would raise the number of basis configurations above the run's limit.
    #[error("a gate would raise the number of basis configurations above the limit of {limit}")]
    ConfigurationLimit {
        /// The limit the run was given ([`Options::max_configurations`]).
        limit: usize,
    },
    /// An `apply` faulted on at least one branch, and the run does not allow region faults
    /// ([`Options::allow_region_faults`]). The simulator sees every branch; a program could not.
    #[error("region fault: an apply faulted ({fault}) on {branches} of {configurations} branches")]
    RegionFault {
        /// The applied function.
        callee: FuncId,
        /// The fault of the first faulting branch, in configuration order.
        fault: Fault,
        /// The number of basis configurations on which the function faulted.
        branches: usize,
        /// The number of basis configurations it ran on.
        configurations: usize,
    },
    /// A world faulted while other worlds go on, and the run does not allow region faults
    /// ([`Options::allow_region_faults`]): as for an `apply`, a program cannot detect a fault on only some
    /// worlds with certainty. The faulting world is as it was before its step; worlds before it in the
    /// executor's order have taken this tick's step.
    #[error("world fault: one of {worlds} worlds faulted ({fault}) while others go on")]
    WorldFault {
        /// The fault.
        fault: Fault,
        /// The number of worlds.
        worlds: usize,
    },
    /// A world reached, by a rewind, a machine state that another world had reached by a rewind and
    /// already left. Whether the two interfere depends on a choice not yet made (`docs/DESIGN.md`,
    /// "Worlds"), so the simulator stops instead of guessing. The state is the one after the rewind.
    #[error("a world rewound to a state another world had already left; the schedule cannot align them")]
    Unaligned,
    /// When every world had stopped (to measure, or at the end), their summed squared amplitudes were not
    /// 1: a step that un-ran shared history in one world's part was not unitary on the whole state (it
    /// freed or un-allocated a register that was not zero there). A rewind's deferred checks refuse such an
    /// un-run first (`bad_rewind`); this is the backstop. The state is kept as it is.
    #[error("the worlds' summed probability is {0} parts per billion away from 1: an un-run step was not unitary")]
    NormLost(u64),
}

/// Why a step did not complete: a fault of the program or a limit of the simulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Halt {
    Fault(Fault),
    Stop(RunError),
    /// The world was observed not to be there and is dropped (a measurement in another world).
    Discard,
}

impl From<Fault> for Halt {
    fn from(fault: Fault) -> Self {
        Halt::Fault(fault)
    }
}

impl From<RunError> for Halt {
    fn from(error: RunError) -> Self {
        Halt::Stop(error)
    }
}

/// Why a run could not start.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StartError {
    /// The requested entry function is not in the module.
    #[error("there is no entry function {0}")]
    InvalidEntry(u32),
    /// The module's memory cannot be allocated on this host.
    #[error("cannot allocate {0} bytes of memory")]
    MemoryTooLarge(u64),
    /// The wrong number of entry arguments.
    #[error("@{function} takes {expected} arguments, {found} given")]
    ArgumentCount {
        /// The entry function.
        function: String,
        /// Its number of parameters.
        expected: usize,
        /// The number of arguments given.
        found: usize,
    },
    /// An entry parameter that is a register handle, which cannot be passed from outside.
    #[error("parameter {index} of @{function} is a register; an entry function takes integers")]
    RegisterParameter {
        /// The entry function.
        function: String,
        /// The parameter's position, from 0.
        index: usize,
    },
    /// An entry argument that does not fit its parameter.
    #[error("argument {index} ({value}) does not fit in {ty}")]
    ArgumentRange {
        /// The argument's position, from 0.
        index: usize,
        /// The value given.
        value: u64,
        /// The parameter type.
        ty: Type,
    },
}

/// Why a reverse step was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ReverseError {
    /// The run was not started in reversible mode.
    #[error("the run keeps no journal; start it with `reversible` set")]
    NotReversible,
    /// The next step to undo is a measurement, which destroyed information.
    #[error("cannot reverse past a measurement")]
    PastMeasurement,
}

/// Counts reported after a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    /// Steps taken by the run, not counting steps inside coherent regions.
    pub steps: u64,
    /// Steps run again by `rewind`s taken as steps of the run, charged to its budget with `steps`: a step
    /// faults with [`Fault::Budget`] once `steps + replay_charged` reaches the budget, and a rewind whose
    /// running again would pass it faults with it, changing nothing (`docs/DESIGN.md`, "History", Cost).
    /// Not counting coherent regions, whose worlds charge their own budgets. It never goes back but on a
    /// reversal to the start.
    pub replay_charged: u64,
    /// The run's step budget, if it has one.
    pub budget: Option<u64>,
    /// The run's limit on basis configurations, if it has one.
    pub max_configurations: Option<usize>,
    /// Steps taken inside coherent regions, summed over every basis configuration they ran on.
    pub region_steps: u64,
    /// What computing the coherent regions cost the executor, by whichever way it computed them
    /// ([`ApplyMode`]): a report, not part of the meaning.
    pub region_cost: RegionCost,
    /// Executions of `apply` (forward or, when reversing, backward) that faulted on at least one basis
    /// configuration. The configurations are those of nonzero amplitude: negligible ones are dropped.
    pub region_faults: u64,
    /// The weight (summed squared amplitude) of the faulting configurations, summed over those executions.
    pub region_fault_weight: f64,
    /// The largest weight of the faulting configurations in any one of those executions.
    pub region_fault_max_weight: f64,
    /// The size of the undo journal, in reversible mode or once a `mark` has run.
    pub journal_bytes: Option<usize>,
    /// Steps undone by `rewind`, including those undone again inside steps run again.
    pub rewound_steps: u64,
    /// Steps run again to undo an earlier `rewind`: by a later `rewind` that reaches back past it, or by
    /// reversal. With `steps` and `rewound_steps` this is the work the history instructions cost.
    pub rerun_steps: u64,
    /// The number of worlds now: whole machine states in superposition (`docs/DESIGN.md`, "Worlds").
    pub worlds: usize,
    /// The most worlds at once.
    pub peak_worlds: usize,
    /// Steps taken, summed over the worlds that took them: `steps` counts ticks, in which every running
    /// world takes one step.
    pub world_steps: u64,
    /// Worlds that split off because ordinary code read a register whose configurations disagree.
    pub splits: u64,
    /// Worlds that merged into another because both became the same machine state.
    pub merges: u64,
    /// Bytes of memory copied by splits: each new world copies the overlay of the world it splits from,
    /// the bytes that world wrote since it shares the base (`docs/DESIGN.md`, "Memory as an overlay");
    /// never the memory itself.
    pub split_memory_bytes: u64,
    /// Bytes of journal copied by splits: each new world copies the journal of the world it splits from.
    pub split_journal_bytes: u64,
    /// Bytes the worlds' overlays hold now, summed: where each world's memory differs from the shared
    /// base. Zero with one world, whose writes are the base.
    pub overlay_bytes: usize,
    /// The most bytes the overlays held at once, sampled after every tick of several worlds.
    pub peak_overlay_bytes: usize,
    /// Times the host or a measurement observed which of several worlds the machine is in: the history
    /// those worlds carried was still entangled (the warning of `docs/DESIGN.md`, "What is promised").
    pub observations: u64,
    /// Memory bytes that hold qubits now, the most in any one world (`docs/DESIGN.md`, "Qubits in memory").
    /// Each is an 8-qubit register, counted in `live_qubits` too.
    pub memory_qubit_bytes: usize,
    /// Qubits allocated now, those left over included.
    pub live_qubits: u32,
    /// Qubits left over now, the most in any one world: kept by a `qfree` of a register that was not
    /// zero, outside strict mode ([`Options::machine_meaning`]). Each is a warning: history left behind.
    pub leftover_qubits: u32,
    /// The most qubits allocated at once.
    pub peak_live_qubits: u32,
    /// Basis configurations with nonzero amplitude now.
    pub configurations: usize,
    /// The most basis configurations at once.
    pub peak_configurations: usize,
}

/// One activation of a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Frame {
    /// The function.
    pub func: FuncId,
    /// The block executing.
    pub block: BlockId,
    /// The next instruction in the block; the terminator's position is the block's instruction count.
    pub inst: usize,
    /// Where the frame's slots start in the executor's slot stack.
    pub base: usize,
    /// The stack pointer when the frame was entered, restored when it returns.
    pub entry_stack_pointer: u64,
}

/// Bytes of stack a frame reserves per slot. A slot holds a value of up to 128 bits, but nearly all are
/// 64 bits or less, so a slot is accounted as one 64-bit word.
pub(crate) const SLOT_BYTES: u64 = 8;

/// Executes a valid module, forward and, in reversible mode, backward.
pub struct Executor<'m> {
    pub(crate) module: &'m ValidModule,
    /// This world's memory: the classical base every world shares and its overlay (`memory.rs`).
    pub(crate) memory: Memory,
    /// The slots of every frame, each frame a contiguous window. Values above the live frames are dead.
    pub(crate) slots: Vec<u128>,
    pub(crate) frames: Vec<Frame>,
    pub(crate) stack_pointer: u64,
    pub(crate) stack_floor: u64,
    pub(crate) tape: Vec<u8>,
    pub(crate) quantum: QuantumState,
    pub(crate) journal: Option<Journal>,
    pub(crate) outcome: Option<Outcome>,
    pub(crate) steps: u64,
    /// Steps run again by rewinds taken as steps, charged to the budget (`Stats::replay_charged`).
    pub(crate) charged: u64,
    /// Whether a rewind taken as a step is un-running, so that the steps run again are charged.
    pub(crate) charging: bool,
    /// Whether running again under `charging` would have passed the budget: the rewind faults `budget`.
    pub(crate) budget_spent: bool,
    pub(crate) budget: Option<u64>,
    pub(crate) max_configurations: Option<usize>,
    /// The depth of the frames belonging to whoever started the current run: 0 for the program, the
    /// caller's depth inside a coherent region.
    pub(crate) base_depth: usize,
    pub(crate) in_region: bool,
    /// The journal records below the coherent region running, which its marks do not count: the caller's
    /// history (`history.rs`). Zero outside a region and on a region's own journal (`ApplyMode::Branches`).
    pub(crate) region_records: usize,
    /// Whether the current run is one branch of a coherent region enumerated on private memory, which holds
    /// the values its configuration gives the qubits in memory (`ApplyMode::Branches`).
    pub(crate) materialised: bool,
    /// How `apply` is computed.
    pub(crate) apply_mode: ApplyMode,
    /// What computing regions cost, and the most journal and overlay bytes the current region run held.
    pub(crate) region_cost: RegionCost,
    pub(crate) run_journal_peak: usize,
    pub(crate) run_overlay_peak: usize,
    /// A journal kept for reuse by coherent regions, which always run reversibly.
    pub(crate) spare_journal: Journal,
    /// A buffer for argument values, kept to avoid allocating per call.
    pub(crate) scratch: Vec<u128>,
    pub(crate) rng: ChaCha8Rng,
    pub(crate) region_steps: u64,
    pub(crate) allow_region_faults: bool,
    /// The simulator's strict mode (`Options::machine_meaning` off).
    pub(crate) strict: bool,
    pub(crate) region_faults: u64,
    pub(crate) region_fault_weight: f64,
    pub(crate) region_fault_max_weight: f64,
    /// The journal-content observer, when turned on. It only copies what steps already journalled.
    pub(crate) journal_measurement: Option<Collector>,
    /// Whether the run was started reversible, so that `step_back` may undo it. A run that is not keeps a
    /// journal anyway from its first `mark`, for `rewind`.
    pub(crate) reversible: bool,
    pub(crate) rewound_steps: u64,
    pub(crate) rerun_steps: u64,
    /// The current world's status, when there are several.
    pub(crate) status: Status,
    /// Every world when there are several, `None` at the current one's index; empty for one world.
    pub(crate) worlds: Vec<Option<World>>,
    pub(crate) current: usize,
    /// Worlds that split off during the step in progress and have finished it.
    pub(crate) forked: Vec<World>,
    /// How a run of several worlds ended; with one world, `outcome` says it.
    pub(crate) run_outcome: Option<Outcome>,
    /// Hashes of states parked worlds left while other worlds went on (`RunError::Unaligned`).
    pub(crate) departed: Vec<u64>,
    /// Whether a run of several worlds was observed, which reversal cannot cross.
    pub(crate) observed: bool,
    /// Un-runs of a register read that found the register no longer holding the value read.
    pub(crate) mismatches: u64,
    pub(crate) splits: u64,
    pub(crate) merges: u64,
    pub(crate) observations: u64,
    pub(crate) world_steps: u64,
    pub(crate) peak_worlds: usize,
    /// Overlay bytes copied by splits, and journal bytes.
    pub(crate) split_memory_bytes: u64,
    pub(crate) split_journal_bytes: u64,
    /// The most overlay bytes all worlds held at once, sampled where worlds are put back after a tick.
    pub(crate) peak_overlay_bytes: usize,
    pub(crate) peak_world_configurations: usize,
    /// The configurations of the worlds other than the current one, while it steps among several.
    pub(crate) others_configurations: usize,
    /// How deep in running steps again to undo a rewind the executor is.
    pub(crate) rerunning: u32,
    /// How deep in rewinds' un-runs the executor is.
    pub(crate) rewinding: u32,
    /// Whether the current world's rewind has un-run past a read that split it, so that it now un-runs its
    /// part of a state shared with other worlds (`history.rs`).
    pub(crate) partial: bool,
    /// Whether the world being made by a split is running its first step.
    pub(crate) splitting: bool,
    /// Whether a rewind taken as a step (not run again or un-run by another) is un-running: below a split
    /// point its checks are then deferred to the sum of the worlds' parts (`worlds.rs`).
    pub(crate) deferring: bool,
    /// The current world's checks deferred below its split point, not yet made on the sum.
    pub(crate) deferred: Vec<Defect>,
    /// Copies of worlds as they were before rewinds whose deferred checks are not yet made.
    pub(crate) backups: Vec<World>,
    /// Whether the current world is the result of a rewind whose deferred checks are not yet made.
    pub(crate) pending: bool,
    /// A change made to the run after computing an `apply` by history, before compare mode computes it by
    /// enumeration: the comparison's own test corrupts the history path with it (`compare_tests.rs`).
    #[cfg(test)]
    pub(crate) corrupt_history: Option<fn(&mut Executor<'_>)>,
}

impl<'m> Executor<'m> {
    /// Prepares to run `entry` on `args`: memory holds the data image and the entry frame is pushed. If
    /// the entry frame does not fit on the stack the run has already faulted.
    pub fn new(module: &'m ValidModule, entry: FuncId, args: &[u64], options: Options) -> Result<Self, StartError> {
        let function = module.functions.get(entry.index()).ok_or(StartError::InvalidEntry(entry.0))?;
        check_arguments(function, args)?;
        let mut executor = Executor {
            module,
            memory: initial_memory(module)?,
            slots: Vec::new(),
            frames: Vec::new(),
            stack_pointer: module.memory_size,
            stack_floor: module.static_end(),
            tape: Vec::new(),
            quantum: QuantumState::default(),
            journal: None,
            outcome: None,
            steps: 0,
            charged: 0,
            charging: false,
            budget_spent: false,
            budget: options.budget,
            max_configurations: options.max_configurations,
            base_depth: 0,
            in_region: false,
            region_records: 0,
            materialised: false,
            apply_mode: ApplyMode::from_env().unwrap_or_default(),
            region_cost: RegionCost::default(),
            run_journal_peak: 0,
            run_overlay_peak: 0,
            spare_journal: Journal::default(),
            scratch: Vec::new(),
            rng: ChaCha8Rng::seed_from_u64(options.seed),
            region_steps: 0,
            allow_region_faults: options.allow_region_faults || options.machine_meaning,
            strict: !options.machine_meaning,
            region_faults: 0,
            region_fault_weight: 0.0,
            region_fault_max_weight: 0.0,
            journal_measurement: None,
            reversible: options.reversible,
            rewound_steps: 0,
            rerun_steps: 0,
            status: Status::Running,
            worlds: Vec::new(),
            current: 0,
            forked: Vec::new(),
            run_outcome: None,
            departed: Vec::new(),
            observed: false,
            mismatches: 0,
            splits: 0,
            merges: 0,
            observations: 0,
            world_steps: 0,
            peak_worlds: 1,
            split_memory_bytes: 0,
            split_journal_bytes: 0,
            peak_overlay_bytes: 0,
            peak_world_configurations: 0,
            others_configurations: 0,
            rerunning: 0,
            rewinding: 0,
            partial: false,
            splitting: false,
            deferring: false,
            deferred: Vec::new(),
            backups: Vec::new(),
            pending: false,
            #[cfg(test)]
            corrupt_history: None,
        };
        let args: Vec<u128> = args.iter().map(|&arg| u128::from(arg)).collect();
        if let Err(fault) = executor.push_frame(entry, &args) {
            executor.outcome = Some(Outcome::Faulted(fault));
        }
        executor.journal = options.reversible.then(Journal::default);
        Ok(executor)
    }

    /// Executes one step, unless the run has ended; returns the outcome once it has. With several worlds,
    /// one step is one tick: every running world takes a step.
    pub fn step(&mut self) -> Option<Outcome> {
        if self.in_region {
            return self.step_world();
        }
        if self.has_worlds() {
            return self.tick();
        }
        self.step_alone()
    }

    /// One step of the current world alone: the machine as it is without worlds.
    pub(crate) fn step_world(&mut self) -> Option<Outcome> {
        if self.outcome.is_some() {
            return self.outcome;
        }
        if self.budget_reached() {
            self.outcome = Some(Outcome::Faulted(Fault::Budget));
            return self.outcome;
        }
        let pending = self.measure_before_step();
        match self.execute() {
            Ok(()) => {
                if let Some(pending) = pending {
                    self.measure_after_step(pending);
                }
                self.steps += 1;
                if self.in_region {
                    let journal = self.journal.as_ref().map_or(0, Journal::bytes);
                    self.run_journal_peak = self.run_journal_peak.max(journal);
                    self.run_overlay_peak = self.run_overlay_peak.max(self.memory.overlay_len());
                } else {
                    self.world_steps += 1;
                }
            }
            Err(Halt::Fault(fault)) => self.outcome = Some(Outcome::Faulted(fault)),
            Err(Halt::Stop(error)) => self.outcome = Some(Outcome::Stopped(error)),
            Err(Halt::Discard) => unreachable!("a single world is never observed away"),
        }
        self.outcome
    }

    /// Whether the budget is spent: the steps taken and the steps run again charged to it reach it.
    pub(crate) fn budget_reached(&self) -> bool {
        self.budget.is_some_and(|budget| self.steps.saturating_add(self.charged) >= budget)
    }

    /// Steps until the run ends.
    pub fn run(&mut self) -> Outcome {
        loop {
            if let Some(outcome) = self.step() {
                return outcome;
            }
        }
    }

    /// Undoes the most recent step. A run that ended in a fault or was stopped at a limit is first returned
    /// to running, which changes nothing else; a run whose entry frame never fit has no frame and nothing to undo, so its
    /// fault is part of its entry state. Returns `false` when there is nothing left to undo.
    pub fn step_back(&mut self) -> Result<bool, ReverseError> {
        if !self.reversible {
            return Err(ReverseError::NotReversible);
        }
        if self.observed {
            return Err(ReverseError::PastMeasurement);
        }
        if self.has_worlds() {
            return self.step_back_worlds();
        }
        let journal = self.journal.as_ref().ok_or(ReverseError::NotReversible)?;
        if matches!(self.outcome, Some(Outcome::Faulted(_) | Outcome::Stopped(_))) && !self.frames.is_empty() {
            self.outcome = None;
        }
        match journal.last() {
            None => Ok(false),
            Some(Undo::Measure) => Err(ReverseError::PastMeasurement),
            Some(_) => {
                self.undo_last();
                // After several worlds, ticks and records need not match one for one; none is left once
                // the journal is empty.
                self.steps = self.steps.saturating_sub(1);
                if self.journal.as_ref().is_some_and(|journal| journal.last().is_none()) {
                    self.steps = 0;
                    self.charged = 0;
                }
                self.adopt_forks(Status::Running);
                Ok(true)
            }
        }
    }

    /// Undoes every step, back to the state `new` left; returns the number of steps undone.
    pub fn reverse(&mut self) -> Result<u64, ReverseError> {
        let mut undone = 0;
        while self.step_back()? {
            undone += 1;
        }
        Ok(undone)
    }

    /// Chooses how `apply` is computed from here on (`docs/DESIGN.md`, "`apply` as mark, call,
    /// rewind-keep"). The default is [`ApplyMode::History`], or the mode the environment variable
    /// `BENNETT_APPLY` names.
    pub fn set_apply_mode(&mut self, mode: ApplyMode) {
        self.apply_mode = mode;
    }

    /// How the run ended, or `None` while it is running.
    pub fn outcome(&self) -> Option<Outcome> {
        if self.has_worlds() { self.run_outcome } else { self.outcome }
    }

    /// The console output so far; with several worlds, the first world's.
    pub fn tape(&self) -> &[u8] {
        self.world_states()[0].tape
    }

    /// A copy of the whole state, for comparison by a test harness or a report.
    /// With several worlds, the first is `classical` and `quantum` and the others are in `worlds`.
    pub fn snapshot(&self) -> Snapshot {
        let mut worlds = self.world_states().into_iter().map(|world| Snapshot {
            classical: ClassicalSnapshot {
                memory: world.memory.to_vec(),
                frames: world.frames.to_vec(),
                slots: world.slots.to_vec(),
                stack_pointer: world.stack_pointer,
                tape: world.tape.to_vec(),
                outcome: world.outcome,
                steps: self.steps,
            },
            quantum: QuantumSnapshot::of(world.quantum),
            worlds: Vec::new(),
        });
        let mut first = worlds.next().expect("at least one world");
        first.worlds = worlds.collect();
        first
    }

    /// The counts of the run so far.
    pub fn stats(&self) -> Stats {
        Stats {
            steps: self.steps,
            replay_charged: self.charged,
            budget: self.budget,
            max_configurations: self.max_configurations,
            region_steps: self.region_steps,
            region_cost: self.region_cost,
            region_faults: self.region_faults,
            region_fault_weight: self.region_fault_weight,
            region_fault_max_weight: self.region_fault_max_weight,
            journal_bytes: self.journals().into_iter().flatten().map(Journal::bytes).reduce(|a, b| a + b),
            rewound_steps: self.rewound_steps,
            rerun_steps: self.rerun_steps,
            worlds: self.worlds.len().max(1),
            peak_worlds: self.peak_worlds,
            world_steps: self.world_steps,
            splits: self.splits,
            merges: self.merges,
            observations: self.observations,
            split_memory_bytes: self.split_memory_bytes,
            split_journal_bytes: self.split_journal_bytes,
            overlay_bytes: self.overlay_bytes(),
            peak_overlay_bytes: self.peak_overlay_bytes.max(self.overlay_bytes()),
            memory_qubit_bytes: self.quantum_states().iter().map(|q| q.cells().len()).max().unwrap_or(0),
            live_qubits: self.quantum_states().iter().map(|q| q.live_qubits()).max().unwrap_or(0),
            leftover_qubits: self.quantum_states().iter().map(|q| q.leftover_qubits()).max().unwrap_or(0),
            peak_live_qubits: self.quantum_states().iter().map(|q| q.peak_qubits()).max().unwrap_or(0),
            configurations: self.quantum_states().iter().map(|q| q.amplitudes().len()).sum(),
            peak_configurations: self
                .quantum_states()
                .iter()
                .map(|q| q.peak_configurations())
                .max()
                .unwrap_or(0)
                .max(self.peak_world_configurations),
        }
    }

    pub(crate) fn function(&self, id: FuncId) -> &'m Function {
        let module: &'m Module = self.module;
        module.function(id)
    }

    pub(crate) fn top(&self) -> &Frame {
        self.frames.last().expect("a running executor has a frame")
    }

    pub(crate) fn top_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a running executor has a frame")
    }

    /// The end of the live frames' slots.
    pub(crate) fn live_top(&self) -> usize {
        self.frames.last().map_or(0, |frame| frame.base + self.function(frame.func).slot_count())
    }

    pub(crate) fn slot(&self, slot: Slot) -> u128 {
        self.slots[self.top().base + slot.index()]
    }

    /// A register handle, which a slot of register type holds.
    pub(crate) fn handle(&self, slot: Slot) -> u64 {
        self.slot(slot) as u64
    }

    /// The value of an operand of 64 bits or fewer: an address, a size, a budget.
    pub(crate) fn read_word(&self, operand: &Operand) -> u64 {
        self.read(operand) as u64
    }

    pub(crate) fn read(&self, operand: &Operand) -> u128 {
        match *operand {
            Operand::Slot(slot) => self.slot(slot),
            Operand::Const(value) => value.value(),
        }
    }

    pub(crate) fn slot_type(&self, slot: Slot) -> Type {
        self.module.slot_type(self.top().func, slot)
    }

    /// Appends a record to the journal, if there is one.
    pub(crate) fn record(&mut self, undo: Undo) {
        if let Some(journal) = &mut self.journal {
            journal.push(undo);
        }
    }

    /// Appends a word to the journal, if there is one.
    pub(crate) fn record_word(&mut self, word: u128) {
        if let Some(journal) = &mut self.journal {
            journal.push_word(word);
        }
    }
}

fn check_arguments(entry: &Function, args: &[u64]) -> Result<(), StartError> {
    if entry.params.len() != args.len() {
        let (expected, found) = (entry.params.len(), args.len());
        return Err(StartError::ArgumentCount { function: entry.name.clone(), expected, found });
    }
    for (index, (param, &value)) in entry.params.iter().zip(args).enumerate() {
        match param.ty {
            Type::Qreg(_) => return Err(StartError::RegisterParameter { function: entry.name.clone(), index }),
            Type::Int(width) if u128::from(value) > width.mask() => {
                return Err(StartError::ArgumentRange { index, value, ty: param.ty });
            }
            Type::Int(_) => {}
        }
    }
    Ok(())
}

fn initial_memory(module: &Module) -> Result<Memory, StartError> {
    let too_large = StartError::MemoryTooLarge(module.memory_size);
    let size = usize::try_from(module.memory_size).map_err(|_| too_large.clone())?;
    let mut memory = Vec::new();
    memory.try_reserve_exact(size).map_err(|_| too_large)?;
    memory.resize(size, 0);
    for segment in &module.data {
        // The validator placed every segment inside memory.
        let start = segment.address as usize;
        let bytes = segment.encode();
        memory[start..start + bytes.len()].copy_from_slice(&bytes);
    }
    Ok(Memory::new(memory))
}
