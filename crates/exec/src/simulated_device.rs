//! The sparse simulator as a device of its own, for a controller run on another device.
//!
//! A writer that records a run's commands, such as the OpenQASM exporter, has no quantum state from which to
//! draw measurement outcomes. It can forward every command to a [`SimulatedDevice`] and take each outcome from
//! it: the outcomes then follow the run's own simulation, sampled with a seeded generator as the simulator
//! samples them, and a request to release wires in zero is checked against the simulated state.

use std::ops::Range;

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::device::{Device, GateOp, Wire};
use crate::device_io::Simulator;
use crate::executor::RunError;
use crate::quantum::QuantumState;
use crate::snapshot::QuantumSnapshot;

/// The executor's sparse state-vector simulator behind the [`Device`] interface: wires are bound to storage
/// columns as they are allocated, gates act on the amplitudes, and measurements sample the state with a
/// generator seeded like the simulator's own (`Options::seed`).
#[derive(Clone, Debug)]
pub struct SimulatedDevice {
    state: QuantumState,
    rng: ChaCha8Rng,
    limit: Option<usize>,
}

impl SimulatedDevice {
    /// A device with no wires, sampling measurements from `seed`, stopping a gate that would make more
    /// than `limit` basis configurations.
    pub fn new(seed: u64, limit: Option<usize>) -> Self {
        Self { state: QuantumState::default(), rng: ChaCha8Rng::seed_from_u64(seed), limit }
    }

    /// The simulated state, for a report or a test harness.
    pub fn snapshot(&self) -> QuantumSnapshot {
        QuantumSnapshot::of(&self.state)
    }

    fn simulator(&mut self) -> Simulator<'_> {
        Simulator { state: &mut self.state, rng: &mut self.rng, limit: self.limit, reserved_column: None }
    }
}

impl Device for SimulatedDevice {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.simulator().allocate(wires)
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        self.simulator().gate(gate)
    }

    fn measure(&mut self, wire: Wire) -> Result<bool, RunError> {
        self.simulator().measure(wire)
    }

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.simulator().discard(wires)
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.simulator().release_zero(wires)
    }

    fn measure_many(&mut self, ranges: &[Range<Wire>]) -> Result<Vec<u64>, RunError> {
        self.simulator().measure_many(ranges)
    }
}
