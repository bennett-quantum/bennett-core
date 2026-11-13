//! The executor's state, its construction, and the public stepping interface.

use bennett_ir::{BlockId, Fault, FuncId, Function, Module, Operand, Slot, Type, ValidModule};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

use crate::journal::{Journal, Undo};
use crate::quantum::QuantumState;
use crate::snapshot::{ClassicalSnapshot, QuantumSnapshot, Snapshot};

/// How to run.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// The most steps the run may take; the next step faults with [`Fault::Budget`]. `None` is unbounded.
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
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The entry function returned, with its value if it has one.
    Returned(Option<u64>),
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
}

/// Why a step did not complete: a fault of the program or a limit of the simulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Halt {
    Fault(Fault),
    Stop(RunError),
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
    /// The run's step budget, if it has one.
    pub budget: Option<u64>,
    /// The run's limit on basis configurations, if it has one.
    pub max_configurations: Option<usize>,
    /// Steps taken inside coherent regions, summed over every basis configuration they ran on.
    pub region_steps: u64,
    /// Executions of `apply` (forward or, when reversing, backward) that faulted on at least one basis
    /// configuration. The configurations are those of nonzero amplitude: negligible ones are dropped.
    pub region_faults: u64,
    /// The weight (summed squared amplitude) of the faulting configurations, summed over those executions.
    pub region_fault_weight: f64,
    /// The largest weight of the faulting configurations in any one of those executions.
    pub region_fault_max_weight: f64,
    /// The size of the undo journal, in reversible mode.
    pub journal_bytes: Option<usize>,
    /// Qubits allocated now.
    pub live_qubits: u32,
    /// The most qubits allocated at once.
    pub peak_live_qubits: u32,
    /// Basis configurations with nonzero amplitude now.
    pub configurations: usize,
    /// The most basis configurations at once.
    pub peak_configurations: usize,
}

/// One activation of a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Bytes of stack a frame reserves per slot: a slot holds one 64-bit value.
pub(crate) const SLOT_BYTES: u64 = 8;

/// Executes a valid module, forward and, in reversible mode, backward.
pub struct Executor<'m> {
    pub(crate) module: &'m ValidModule,
    pub(crate) memory: Vec<u8>,
    /// The slots of every frame, each frame a contiguous window. Values above the live frames are dead.
    pub(crate) slots: Vec<u64>,
    pub(crate) frames: Vec<Frame>,
    pub(crate) stack_pointer: u64,
    pub(crate) stack_floor: u64,
    pub(crate) tape: Vec<u8>,
    pub(crate) quantum: QuantumState,
    pub(crate) journal: Option<Journal>,
    pub(crate) outcome: Option<Outcome>,
    pub(crate) steps: u64,
    pub(crate) budget: Option<u64>,
    pub(crate) max_configurations: Option<usize>,
    /// The depth of the frames belonging to whoever started the current run: 0 for the program, the
    /// caller's depth inside a coherent region.
    pub(crate) base_depth: usize,
    pub(crate) in_region: bool,
    /// A journal kept for reuse by coherent regions, which always run reversibly.
    pub(crate) spare_journal: Journal,
    /// A buffer for argument values, kept to avoid allocating per call.
    pub(crate) scratch: Vec<u64>,
    pub(crate) rng: ChaCha8Rng,
    pub(crate) region_steps: u64,
    pub(crate) allow_region_faults: bool,
    pub(crate) region_faults: u64,
    pub(crate) region_fault_weight: f64,
    pub(crate) region_fault_max_weight: f64,
}

impl<'m> Executor<'m> {
    /// Prepares to run `entry` on `args`: memory holds the data image and the entry frame is pushed. If
    /// the entry frame does not fit on the stack the run has already faulted.
    pub fn new(module: &'m ValidModule, entry: FuncId, args: &[u64], options: Options) -> Result<Self, StartError> {
        check_arguments(module.function(entry), args)?;
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
            budget: options.budget,
            max_configurations: options.max_configurations,
            base_depth: 0,
            in_region: false,
            spare_journal: Journal::default(),
            scratch: Vec::new(),
            rng: ChaCha8Rng::seed_from_u64(options.seed),
            region_steps: 0,
            allow_region_faults: options.allow_region_faults,
            region_faults: 0,
            region_fault_weight: 0.0,
            region_fault_max_weight: 0.0,
        };
        if let Err(fault) = executor.push_frame(entry, args) {
            executor.outcome = Some(Outcome::Faulted(fault));
        }
        executor.journal = options.reversible.then(Journal::default);
        Ok(executor)
    }

    /// Executes one step, unless the run has ended; returns the outcome once it has.
    pub fn step(&mut self) -> Option<Outcome> {
        if self.outcome.is_some() {
            return self.outcome;
        }
        if self.budget.is_some_and(|budget| self.steps >= budget) {
            self.outcome = Some(Outcome::Faulted(Fault::Budget));
            return self.outcome;
        }
        match self.execute() {
            Ok(()) => self.steps += 1,
            Err(Halt::Fault(fault)) => self.outcome = Some(Outcome::Faulted(fault)),
            Err(Halt::Stop(error)) => self.outcome = Some(Outcome::Stopped(error)),
        }
        self.outcome
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
        let journal = self.journal.as_ref().ok_or(ReverseError::NotReversible)?;
        if matches!(self.outcome, Some(Outcome::Faulted(_) | Outcome::Stopped(_))) && !self.frames.is_empty() {
            self.outcome = None;
        }
        match journal.last() {
            None => Ok(false),
            Some(Undo::Measure) => Err(ReverseError::PastMeasurement),
            Some(_) => {
                self.undo_last();
                self.steps -= 1;
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

    /// How the run ended, or `None` while it is running.
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// The console output so far.
    pub fn tape(&self) -> &[u8] {
        &self.tape
    }

    /// A copy of the whole state, for comparison by a test harness or a report.
    pub fn snapshot(&self) -> Snapshot {
        let live = self.live_top();
        Snapshot {
            classical: ClassicalSnapshot {
                memory: self.memory.clone(),
                frames: self.frames.clone(),
                slots: self.slots[..live].to_vec(),
                stack_pointer: self.stack_pointer,
                tape: self.tape.clone(),
                outcome: self.outcome,
                steps: self.steps,
            },
            quantum: QuantumSnapshot::of(&self.quantum),
        }
    }

    /// The counts of the run so far.
    pub fn stats(&self) -> Stats {
        Stats {
            steps: self.steps,
            budget: self.budget,
            max_configurations: self.max_configurations,
            region_steps: self.region_steps,
            region_faults: self.region_faults,
            region_fault_weight: self.region_fault_weight,
            region_fault_max_weight: self.region_fault_max_weight,
            journal_bytes: self.journal.as_ref().map(Journal::bytes),
            live_qubits: self.quantum.live_qubits(),
            peak_live_qubits: self.quantum.peak_qubits(),
            configurations: self.quantum.amplitudes().len(),
            peak_configurations: self.quantum.peak_configurations(),
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

    pub(crate) fn slot(&self, slot: Slot) -> u64 {
        self.slots[self.top().base + slot.index()]
    }

    pub(crate) fn read(&self, operand: &Operand) -> u64 {
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
    pub(crate) fn record_word(&mut self, word: u64) {
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
            Type::Int(width) if value > width.mask() => {
                return Err(StartError::ArgumentRange { index, value, ty: param.ty });
            }
            Type::Int(_) => {}
        }
    }
    Ok(())
}

fn initial_memory(module: &Module) -> Result<Vec<u8>, StartError> {
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
    Ok(memory)
}
