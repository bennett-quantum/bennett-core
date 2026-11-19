//! Gate lowering and counts for a logical device with single-qubit gates, two-qubit gates and Toffolis.
#![deny(clippy::wildcard_enum_match_arm)]

mod apply;
mod count;
mod lower;
mod schedule;

pub use apply::{
    ApplyError, ApplyInput, ApplyMemory, ApplyMode, Argument, CompiledApply, Construction, MemoryCounts, compile_apply,
    compile_apply_with,
};
pub use count::{CountingDevice, Counts, seeded_bits};
pub use lower::LowerControls;
