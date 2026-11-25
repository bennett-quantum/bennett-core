//! How `QuantumState::measure_bits` turns one uniform draw into an outcome: the outcomes, in order, own
//! consecutive half-open intervals `[start, start + weight)` of `[0, total)`, so that each is drawn with
//! exactly its weight. A draw on the boundary between two outcomes belongs to the second.

use std::collections::BTreeMap;

use bennett_ir::Width;
use num_complex::Complex64;
use rand::{Rng, RngCore};

use super::{Config, QuantumState, pick};

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

/// A weight below half the rounding unit of one half: added to one half it leaves the running sum unchanged.
const BELOW_ROUNDING: f64 = 1.0 / (1u64 << 60) as f64;

#[test]
fn an_outcome_whose_interval_is_empty_is_never_picked() {
    let weights = [0.5, BELOW_ROUNDING, 0.5];
    assert_eq!(0.5 + BELOW_ROUNDING, 0.5, "the middle interval is empty");
    let largest = Constant(u64::MAX).random::<f64>();
    assert_eq!(largest, 1.0 - f64::EPSILON / 2.0, "the largest draw the generator gives");
    for (draw, expected) in [(0.0, 0), (0.5 - f64::EPSILON / 4.0, 0), (0.5, 2), (largest, 2), (1.0, 2), (2.0, 2)] {
        assert_eq!(pick(&weights, draw), Some(expected), "draw {draw:e}");
    }
    // A draw at or past the last running sum takes the last outcome whose interval is not empty, not the last.
    let empty_last = [0.5, 0.5, BELOW_ROUNDING];
    assert_eq!(pick(&empty_last, 1.0), Some(1));
    assert_eq!(pick(&empty_last, 0.5), Some(1));
    assert_eq!(pick(&[], 0.0), None);
}

/// A weight below the rounding unit of the running sum can still change it: `0.5 + 3 * 2^-55` rounds up to
/// the next binary64 after one half, so the middle interval is `[0.5, 0.5 + 2^-53)`, not empty, and the draw
/// 0.5 is in it. A rule that skipped weights below some cutoff (such as `f64::EPSILON / 2`, the unit at one
/// half) would select the last outcome.
#[test]
fn a_small_weight_that_changes_the_running_sum_has_its_interval() {
    let small = 3.0 / (1u64 << 55) as f64;
    let weights = [0.5, small, 0.5 - f64::EPSILON / 2.0];
    assert!(0.5 + small > 0.5, "the middle interval is not empty");
    assert!(small < f64::EPSILON / 2.0, "below the rounding unit at one half");
    assert_eq!(pick(&weights, 0.5), Some(1));
    assert_eq!(pick(&weights, 0.5 + f64::EPSILON / 2.0), Some(2));
}

/// The same weights in a state: no draw the generator can give near the empty interval, nor the largest or
/// the smallest, measures the outcome of weight below the rounding unit.
#[test]
fn measurement_never_returns_an_outcome_with_an_empty_interval() {
    let state = || {
        let mut state = QuantumState::default();
        let handle = state.allocate(Width::new(2).expect("a width"));
        let column = state.resolve(handle).expect("live");
        let configs = [(0u64, 0.5), (1, BELOW_ROUNDING), (2, 0.5)].map(|(value, weight): (u64, f64)| {
            let mut config = vec![0u64; column + 1];
            config[column] = value;
            (Config::from(config), Complex64::new(weight.sqrt(), 0.0))
        });
        state.replace_amplitudes(BTreeMap::from(configs));
        (state, column)
    };
    // The generator's draw is the top 53 bits of its word: one step of a draw is 2^11 of the word.
    let step = 1u64 << 11;
    let near_half = (0..=2048u64).flat_map(|k| [(1u64 << 63) + k * step, (1u64 << 63) - k * step]);
    for word in near_half.chain([0, u64::MAX]) {
        let (mut state, column) = state();
        let outcome = state.measure_bits(&[(column, 3)], &mut Constant(word));
        assert_ne!(outcome, vec![1], "word {word:#x}");
    }
}

/// The witness: three rotations about one axis that return qubit 0 to a state of exact probability zero for
/// outcome 1 leave a computed weight of about 5.5e-32 there. At seed 0 the draw equals outcome 0's weight
/// exactly, which made the earlier rule return 1.
#[test]
fn rotations_that_cancel_do_not_measure_their_rounding_residue() {
    let module = bennett_ir::load(
        "memory 256
func @main() -> i2 {
entry:
    %q = qalloc 2
    ry(1.75) %q[0]
    ry(19.1875) %q[0]
    ry(-20.9375) %q[0]
    ry(1.139387646145412) %q[1]
    %m = measure %q
    ret %m
}",
    )
    .expect("valid");
    let entry = module.function_named("main").expect("entry");
    for seed in 0..64 {
        let options = crate::Options { seed, ..crate::Options::default() };
        let outcome = crate::Executor::new(&module, entry, &[], options).expect("starts").run();
        assert!(matches!(outcome, crate::Outcome::Returned(Some(0 | 2))), "seed {seed}: {outcome:?}");
    }
}

/// Two rotations about Y leave four outcomes, the smallest of weight above 0.12. At seed 0 the draw,
/// 0.7090754154265617, is the running sum after outcome 1 as the half-open intervals round it, so the
/// interval rule selects outcome 2. The earlier rule, subtracting weights from the draw, rounded differently
/// and returned 1: the change of rule changes seeded outcomes at a rounded boundary between two outcomes that
/// both have substantial weight, not only at an empty interval.
#[test]
fn a_draw_on_a_rounded_boundary_between_likely_outcomes_selects_the_later() {
    let module = bennett_ir::load(
        "memory 256
func @main() -> i2 {
entry:
    %q = qalloc 2
    ry(1.7421875) %q[0]
    ry(1.1393876461454118) %q[1]
    %m = measure %q
    ret %m
}",
    )
    .expect("valid");
    let entry = module.function_named("main").expect("entry");
    let outcome = crate::Executor::new(&module, entry, &[], crate::Options::default()).expect("starts").run();
    assert_eq!(outcome, crate::Outcome::Returned(Some(2)));
}
