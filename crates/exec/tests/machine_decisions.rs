//! Independent probes of the 2026-11-16 machine decisions, with explicit expected results.
mod common;

#[path = "machine_decisions/faults.rs"]
mod faults;
#[path = "machine_decisions/jumps.rs"]
mod jumps;
#[path = "machine_decisions/measurement.rs"]
mod measurement;
