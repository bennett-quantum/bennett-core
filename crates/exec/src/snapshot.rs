//! Copies of the whole executor state, for a harness to compare. The executor never judges its own
//! reversal; a snapshot is only data.

use num_complex::Complex64;

use crate::executor::{Frame, Outcome};
use crate::quantum::{QuantumState, RegisterState};

/// The whole state of an executor.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Everything but the quantum state; compared exactly.
    pub classical: ClassicalSnapshot,
    /// The quantum state; compared within a numerical tolerance.
    pub quantum: QuantumSnapshot,
    /// Every world after the first, in the executor's order, when there are several
    /// (`docs/DESIGN.md`, "Worlds"); each has no worlds of its own.
    pub worlds: Vec<Snapshot>,
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
}

/// The register table and every nonzero amplitude.
#[derive(Clone, Debug, PartialEq)]
pub struct QuantumSnapshot {
    /// The state of each register id.
    pub registers: Vec<RegisterState>,
    /// Each basis configuration (one value per register id in use) with its amplitude, in order.
    pub amplitudes: Vec<(Vec<u64>, Complex64)>,
}

impl QuantumSnapshot {
    pub(crate) fn of(state: &QuantumState) -> Self {
        QuantumSnapshot {
            registers: state.registers().to_vec(),
            amplitudes: state.amplitudes().iter().map(|(config, amplitude)| (config.to_vec(), *amplitude)).collect(),
        }
    }

    /// The amplitude of `config`, zero if it is absent.
    pub fn amplitude(&self, config: &[u64]) -> Complex64 {
        let found = self.amplitudes.binary_search_by(|(c, _)| c.as_slice().cmp(config));
        found.map_or(Complex64::new(0.0, 0.0), |index| self.amplitudes[index].1)
    }

    /// The largest difference between corresponding amplitudes, or `None` if the register tables differ.
    pub fn distance(&self, other: &QuantumSnapshot) -> Option<f64> {
        if self.registers != other.registers {
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
            amplitudes: amplitudes.iter().map(|&(config, amplitude)| (vec![config], amplitude)).collect(),
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
