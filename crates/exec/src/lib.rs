//! The Bennett executor: runs a [`ValidModule`](bennett_ir::ValidModule) forward, and in reversible mode
//! backward, over flat byte memory, a frame stack and a sparse quantum state.
//!
//! The executor counts one step per executed instruction and enforces the budget itself; the program
//! carries no metering code. In reversible mode each step journals exactly what it destroys (see
//! `journal`), and [`Executor::reverse`] pops the journal back to the entry state, from a finished run or
//! a faulted one alike. A coherent region (`apply`) is mark, call, rewind-keep on worlds: the callee runs
//! once per value of its arguments, each run un-run after its result is kept; the earlier branch enumeration
//! is kept as a comparison (`ApplyMode`).

mod arith;
mod backward;
mod cells;
#[cfg(test)]
mod compare_tests;
mod executor;
mod forward;
mod gates;
mod history;
mod journal;
mod measure;
mod memory;
#[cfg(test)]
mod permutation_tests;
mod quantum;
#[cfg(test)]
mod recompute_independent_tests;
mod region;
mod snapshot;
mod worlds;

pub use executor::{Executor, Frame, Options, Outcome, ReverseError, RunError, StartError, Stats};
pub use journal::Undo;
pub use measure::{
    Alignment, ApplyClassification, ApplyJournal, BranchJournal, CallPathStep, ChainRecovery, ClassifiedPosition,
    JournalMeasurement, LiveSource, PathId, ProducerOutput, RecomputeWitness, RecordInstance, RecoverableReason, Site,
    SitePos, SourceKind, WordClass, WordInstance, WordRole, WordWitness, classify, classify_with_chains,
    classify_with_recompute,
};
pub use quantum::RegisterState;
pub use region::{ApplyMode, RegionCost};
pub use snapshot::{ClassicalSnapshot, QuantumSnapshot, Snapshot};
