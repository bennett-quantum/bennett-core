#![cfg_attr(not(test), deny(clippy::wildcard_enum_match_arm))]

//! The Bennett executor: runs a [`ValidModule`](bennett_ir::ValidModule) forward, and in reversible mode
//! backward, over flat byte memory, a frame stack and a sparse quantum state.
//!
//! The executor counts one step per executed instruction and enforces the budget itself; the program
//! carries no metering code. In reversible mode each step journals exactly what it destroys (see
//! `journal`), and [`Executor::reverse`] pops the journal back to the entry state, from a finished run or
//! a faulted one alike. A coherent region (`apply`) runs its callee once for each definite value of its
//! argument registers, on private memory, and XORs the result into its target (`region`).

mod apply_device;
mod arith;
mod backward;
mod cells;
mod device;
mod device_io;
mod executor;
mod forward;
mod gates;
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
mod wire_storage;

pub use apply_device::{ApplyCompiler, ApplyError, ApplyInput, ApplyMemory, Argument, frame_bytes};

pub use device::{BlockOp, Device, GateBlock, GateBlockKey, GateOp, Wire};
pub use executor::{DeviceRun, Executor, Frame, Options, Outcome, ReverseError, RunError, StartError, Stats};
pub use journal::Undo;
pub use measure::{
    Alignment, ApplyClassification, ApplyJournal, BranchJournal, CallPathStep, ChainRecovery, ClassifiedPosition,
    JournalMeasurement, LiveSource, PathId, ProducerOutput, RecomputeWitness, RecordInstance, RecoverableReason, Site,
    SitePos, SourceKind, WordClass, WordInstance, WordRole, WordWitness, classify, classify_with_chains,
    classify_with_recompute,
};
pub use quantum::RegisterState;
pub use region::RegionCost;
pub use snapshot::{ClassicalSnapshot, QuantumSnapshot, Snapshot};
