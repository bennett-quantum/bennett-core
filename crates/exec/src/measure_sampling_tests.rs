//! How `QuantumState::measure_bits` turns one uniform draw into an outcome: the outcomes, in order, own
//! consecutive half-open intervals `[start, start + weight)` of `[0, total)`, so that each is drawn with
//! exactly its weight. A draw on the boundary between two outcomes belongs to the second.

use std::collections::BTreeMap;

use bennett_ir::Width;
use num_complex::Complex64;
use rand::{Rng, RngCore};

use super::{Config, QuantumState};

/// A generator that always gives the same 64 bits.
struct Constant(u64);

impl RngCore for Constant {
    fn next_u32(&mut self) -> u32 {
        (self.0 >> 32) as u32
    }
    fn next_u64(&mut self) -> u64 {
        self.0
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for (byte, source) in dest.iter_mut().zip(self.0.to_le_bytes().into_iter().cycle()) {
            *byte = source;
        }
    }
}

/// A one-qubit register holding 0 and 1 with equal amplitudes, and its column. The two weights are equal, so
/// the total is exactly twice one and a draw of one half lands exactly on the boundary between them.
fn two_equal_outcomes() -> (QuantumState, usize) {
    let mut state = QuantumState::default();
    let handle = state.allocate(Width::new(1).expect("a width"));
    let column = state.resolve(handle).expect("live");
    let amplitude = Complex64::new(std::f64::consts::FRAC_1_SQRT_2, 0.0);
    let configs = [0u64, 1].map(|value| {
        let mut config = vec![0u64; column + 1];
        config[column] = value;
        (Config::from(config), amplitude)
    });
    state.replace_amplitudes(BTreeMap::from(configs));
    (state, column)
}

#[test]
fn a_draw_on_the_boundary_between_two_outcomes_belongs_to_the_second() {
    // The top bit alone: rand's uniform f64 takes the top 53 bits, so the draw is exactly one half.
    let half = 1u64 << 63;
    assert_eq!(Constant(half).random::<f64>(), 0.5);
    let (mut state, column) = two_equal_outcomes();
    assert_eq!(state.measure_bits(&[(column, 1)], &mut Constant(half)), vec![1]);
    assert_eq!(state.amplitudes().len(), 1, "collapsed onto the outcome drawn");
    // The draw one step below the boundary is the first outcome's.
    let (mut state, column) = two_equal_outcomes();
    assert_eq!(state.measure_bits(&[(column, 1)], &mut Constant(half - (1 << 11))), vec![0]);
}
