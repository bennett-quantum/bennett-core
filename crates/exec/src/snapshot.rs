//! Copies of the whole executor state, for a harness to compare. The executor never judges its own
//! reversal; a snapshot is only data. It is the simulator's view, for tests and reports: the amplitudes it
//! lists are no observation a device could make, and no instruction reads a snapshot.

use std::ops::Range;

use num_complex::Complex64;

use crate::device::Wire;
use crate::executor::{Frame, Outcome};
use crate::quantum::{QuantumState, RegisterState};

/// The whole state of an executor.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Everything but the quantum state; compared exactly.
    pub classical: ClassicalSnapshot,
    /// The quantum state; compared within a numerical tolerance.
    pub quantum: QuantumSnapshot,
}

/// Memory, frames, live slots, stack pointer, console tape, outcome and step count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassicalSnapshot {
    /// All of memory.
    pub memory: Vec<u8>,
    /// The frames, outermost first.
    pub frames: Vec<Frame>,
    /// The slots of the live frames.
    pub slots: Vec<u128>,
    /// The stack pointer.
    pub stack_pointer: u64,
    /// The console output.
    pub tape: Vec<u8>,
    /// How the run ended, if it has.
    pub outcome: Option<Outcome>,
    /// The step count.
    pub steps: u64,
    /// The allocation count, the last handle given, and each live handle with its register's width, in
    /// handle order (`docs/DESIGN.md`, "Running a program").
    pub handles: (u64, Vec<(u64, u32)>),
}

/// The register table and every nonzero amplitude.
#[derive(Clone, Debug, PartialEq)]
pub struct QuantumSnapshot {
    /// The state of each register id: of each column, for a register stored in several.
    pub registers: Vec<RegisterState>,
    /// The wire table: each run of consecutive live wires bound to consecutive bits of the columns, with
    /// the bit of its first wire (bit `b` of column `c` is `64 * c + b`), in wire order (`docs/DESIGN.md`,
    /// "Running a program"). It says which qubits form which register: without it, two states whose
    /// columns hold the same words but form different registers would compare equal.
    pub wires: Vec<(Range<Wire>, u64)>,
    /// Each basis configuration (one value per register id in use) with its amplitude, in order.
    pub amplitudes: Vec<(Vec<u64>, Complex64)>,
    /// The memory bytes that hold qubits, in address order, each with the register id holding its value
    /// (`docs/DESIGN.md`, "Qubits in memory"); the classical memory holds 0 there.
    pub cells: Vec<(u64, usize)>,
}

impl QuantumSnapshot {
    pub(crate) fn of(state: &QuantumState) -> Self {
        QuantumSnapshot {
            registers: state.registers().to_vec(),
            wires: state.device_wires.segments().collect(),
            amplitudes: state.amplitudes().iter().map(|(config, amplitude)| (config.to_vec(), *amplitude)).collect(),
            cells: state.cells().iter().map(|(&address, &column)| (address, column)).collect(),
        }
    }

    /// The amplitude of `config`, zero if it is absent.
    pub fn amplitude(&self, config: &[u64]) -> Complex64 {
        let found = self.amplitudes.binary_search_by(|(c, _)| c.as_slice().cmp(config));
        found.map_or(Complex64::new(0.0, 0.0), |index| self.amplitudes[index].1)
    }

    /// The largest difference between corresponding amplitudes, or `None` if the register tables (cells
    /// and the columns of each register included) differ.
    pub fn distance(&self, other: &QuantumSnapshot) -> Option<f64> {
        if self.registers != other.registers || self.wires != other.wires || self.cells != other.cells {
            return None;
        }
        let configs = self.amplitudes.iter().chain(&other.amplitudes).map(|(config, _)| config);
        Some(configs.map(|config| (self.amplitude(config) - other.amplitude(config)).norm()).fold(0.0, f64::max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bennett_ir::Width;

    fn snapshot(amplitudes: &[(u64, Complex64)]) -> QuantumSnapshot {
        QuantumSnapshot {
            registers: vec![RegisterState { generation: 1, width: Width::new(2) }],
            wires: Vec::new(),
            amplitudes: amplitudes.iter().map(|&(config, amplitude)| (vec![config], amplitude)).collect(),
            cells: Vec::new(),
        }
    }

    #[test]
    fn distance_is_the_largest_amplitude_difference() {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let plus = snapshot(&[(0, Complex64::new(h, 0.0)), (1, Complex64::new(h, 0.0))]);
        let minus = snapshot(&[(0, Complex64::new(h, 0.0)), (1, Complex64::new(-h, 0.0))]);
        let three = snapshot(&[(3, Complex64::new(0.0, 1.0))]);
        assert_eq!(plus.distance(&plus), Some(0.0));
        // Equal probabilities, opposite phase on |1⟩: |h - (-h)| = 2h.
        assert!((plus.distance(&minus).unwrap_or(0.0) - 2.0 * h).abs() < 1e-15);
        // Disjoint supports: each side's amplitude against zero, the larger being 1.
        assert_eq!(three.distance(&plus), Some(1.0));
        assert_eq!(plus.distance(&three), Some(1.0));
        let mut other_table = plus.clone();
        other_table.registers[0].generation = 2;
        assert_eq!(plus.distance(&other_table), None);
    }
}
