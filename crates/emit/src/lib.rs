//! Gate lowering and counts for a logical device with single-qubit gates, two-qubit gates and Toffolis.
#![deny(clippy::wildcard_enum_match_arm)]

mod count;
mod lower;
mod schedule;

pub use count::{CountingDevice, Counts, seeded_bits};
pub use lower::LowerControls;
