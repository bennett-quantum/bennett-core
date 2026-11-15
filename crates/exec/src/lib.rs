//! The Bennett executor: runs a [`ValidModule`](bennett_ir::ValidModule) forward, and in reversible mode
//! backward, over flat byte memory, a frame stack and a sparse quantum state.
//!
//! The executor counts one step per executed instruction and enforces the budget itself; the program
//! carries no metering code. In reversible mode each step journals exactly what it destroys (see
//! `journal`), and [`Executor::reverse`] pops the journal back to the entry state, from a finished run or
//! a faulted one alike. Coherent regions (`apply`) are defined by branch enumeration: the callee runs
//! once per basis configuration, each run reversed afterwards so its memory is private.

mod arith;
mod backward;
mod executor;
mod forward;
mod gates;
mod journal;
mod measure;
mod quantum;
mod region;
mod snapshot;

pub use executor::{Executor, Frame, Options, Outcome, ReverseError, RunError, StartError, Stats};
pub use journal::Undo;
pub use measure::{
    Alignment, ApplyClassification, ApplyJournal, BranchJournal, CallPathStep, ClassifiedPosition, JournalMeasurement,
    LiveSource, PathId, ProducerOutput, RecomputeWitness, RecordInstance, RecoverableReason, Site, SitePos, SourceKind,
    WordClass, WordInstance, WordRole, WordWitness, classify, classify_with_recompute,
};
pub use quantum::RegisterState;
pub use snapshot::{ClassicalSnapshot, QuantumSnapshot, Snapshot};
