//! Independent small-state calculations for wide registers and computed rotations.

mod common;

use std::cmp::Ordering;
use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use bennett_exec::{Options, Outcome, QuantumSnapshot, ReverseError};
use bennett_ir::Fault;
use common::{TOLERANCE, bounded, executor};
use num_complex::Complex64 as C;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const WIDTHS: [usize; 6] = [63, 64, 65, 127, 128, 129];

fn assert_amplitudes(state: &QuantumSnapshot, expected: &[(Vec<u64>, C)], context: &str) {
    for (basis, amplitude) in expected {
        let observed = state.amplitude(basis);
        assert!((observed - amplitude).norm() < TOLERANCE, "{context}, {basis:?}: {observed} != {amplitude}");
    }
    for (basis, amplitude) in &state.amplitudes {
        assert!(
            amplitude.norm() < TOLERANCE || expected.iter().any(|(key, _)| key == basis),
            "{context}: unexpected amplitude {amplitude} for {basis:?}"
        );
    }
}

fn preparation(width: usize) -> String {
    let top = width - 1;
    format!(
        "    %q = qalloc {width}
    %witness = qalloc 1
    h %q
    x %q[{top}] ctrl %q
    x %witness ctrl %q[{top}]
    s %q[{top}]
"
    )
}

/// The two occupied configurations are all zero and the one with qubits 0, top, and witness set.
fn basis_pair(width: usize) -> [Vec<u64>; 2] {
    let top = width - 1;
    let zero = vec![0; width.div_ceil(64) + 1];
    let mut one = zero.clone();
    one[0] = 1;
    one[top / 64] |= 1 << (top % 64);
    *one.last_mut().expect("witness column") = 1;
    [zero, one]
}

#[test]
fn a_wide_measurement_projects_onto_the_predicted_configuration() {
    for width in WIDTHS {
        let (result_width, measurement) = if width <= 128 {
            (width, "    %m = measure %q\n".to_owned())
        } else {
            // SYNTAX.md's measuring-through-a-copy expansion for a register that cannot be read whole.
            let top = width - 1;
            (
                2,
                format!(
                    "    %copy = qalloc 2
    x %copy ctrl %q
    x %copy[1] ctrl %q[{top}]
    %m = measure %copy
    x %copy[1] ctrl %q[{top}]
    x %copy ctrl %q
    qfree %copy
"
                ),
            )
        };
        let module = common::load(&format!(
            "memory 128\nfunc @main() -> i{result_width} {{\nentry:\n{}{measurement}    ret %m\n}}",
            preparation(width)
        ));
        let expected_one = if width <= 128 { 1 | (1u128 << (width - 1)) } else { 3 };
        let [zero, one] = basis_pair(width);
        let mut seen = [false; 2];
        for seed in 0..12 {
            let (outcome, executor) = common::run(&module, "main", &[], Options { seed, ..bounded() });
            let context = format!("q{width}, seed {seed}");
            let (index, expected) = match outcome {
                Outcome::Returned(Some(0)) => (0, (zero.clone(), C::ONE)),
                Outcome::Returned(Some(value)) if value == expected_one => (1, (one.clone(), C::I)),
                other => panic!("{context}: wrong measurement {other:?}"),
            };
            seen[index] = true;
            assert_amplitudes(&executor.snapshot().quantum, &[expected], &context);
            assert_eq!(executor.stats().live_qubits, width as u32 + 1, "{context}");
            assert_eq!(executor.stats().leftover_qubits, 0, "{context}: the copy was cleared before release");
        }
        assert_eq!(seen, [true, true], "q{width}");
    }
}

#[test]
fn discarding_a_wide_register_removes_interference_and_can_be_undone() {
    for width in WIDTHS {
        let module = common::load(&format!(
            "memory 128
func @main() -> i1 {{
entry:
{}    qfree %q
    h %witness
    %m = measure %witness
    ret %m
}}",
            preparation(width)
        ));
        let context = format!("q{width}");
        let [zero, one] = basis_pair(width);
        let entangled = [(zero.clone(), C::ONE * FRAC_1_SQRT_2), (one.clone(), C::I * FRAC_1_SQRT_2)];
        // Trace out q: the two witness values are paired with orthogonal states of q. After H, each
        // contributes 1/4 to each witness probability, so both outcomes still have probability 1/2.
        let mut zero_with_one = zero.clone();
        *zero_with_one.last_mut().expect("witness column") = 1;
        let mut one_with_zero = one.clone();
        *one_with_zero.last_mut().expect("witness column") = 0;
        let after_h = [
            (zero.clone(), C::ONE / 2.0),
            (zero_with_one.clone(), C::ONE / 2.0),
            (one_with_zero.clone(), C::I / 2.0),
            (one.clone(), -C::I / 2.0),
        ];
        let mut seen = [false; 2];
        for seed in 0..12 {
            let mut executor = executor(&module, "main", &[], Options { seed, reversible: true, ..bounded() });
            let entry = executor.snapshot();
            for _ in 0..6 {
                assert_eq!(executor.step(), None);
            }
            let before_release = executor.snapshot();
            assert_amplitudes(&before_release.quantum, &entangled, &context);
            assert_eq!(executor.step(), None);
            let after_release = executor.snapshot();
            assert_eq!(after_release.quantum.wires.len(), 1, "{context}: only the witness stays bound");
            let stats = executor.stats();
            assert_eq!((stats.nonzero_releases, stats.leftover_qubits), (1, width as u32), "{context}");
            assert!((stats.nonzero_release_weight - 0.5).abs() < TOLERANCE, "{context}");
            assert_eq!(executor.step(), None);
            assert_amplitudes(&executor.snapshot().quantum, &after_h, &context);
            assert!(executor.step_back().expect("undo H"));
            common::assert_same_state(&executor.snapshot(), &after_release, &context);
            assert!(executor.step_back().expect("undo qfree"));
            common::assert_same_state(&executor.snapshot(), &before_release, &context);
            assert_eq!(executor.reverse(), Ok(6));
            common::assert_same_state(&executor.snapshot(), &entry, &context);

            let (witness, expected) = match executor.run() {
                Outcome::Returned(Some(0)) => {
                    (0, [(zero.clone(), C::ONE * FRAC_1_SQRT_2), (one_with_zero.clone(), C::I * FRAC_1_SQRT_2)])
                }
                Outcome::Returned(Some(1)) => {
                    (1, [(zero_with_one.clone(), C::ONE * FRAC_1_SQRT_2), (one.clone(), -C::I * FRAC_1_SQRT_2)])
                }
                other => panic!("{context}, seed {seed}: {other:?}"),
            };
            seen[witness] = true;
            assert_amplitudes(&executor.snapshot().quantum, &expected, &context);
            assert!(executor.step_back().expect("undo ret"));
            assert_eq!(executor.step_back(), Err(ReverseError::PastMeasurement), "{context}");
        }
        assert_eq!(seen, [true, true], "{context}");
    }
}

#[test]
fn controlled_computed_rotations_match_their_matrix_and_reverse() {
    for axis in ["rx", "ry", "rz"] {
        let module = common::load(&format!(
            "memory 128
func @main(i64 %bits) {{
entry:
    %q = qalloc 65
    h %q
    x %q[64]
    %angle = xor i64 %bits, 0x8000000000000000
    {axis}(%angle) %q[64] ctrl %q
    %later = xor i64 %angle, 1
    ret
}}"
        ));
        for angle in [0.0, -0.0, TAU, -2.0 * TAU, -0.7, 1e200, -f64::MAX] {
            let context = format!("{axis}({angle:e})");
            let mut executor =
                executor(&module, "main", &[(-angle).to_bits()], Options { reversible: true, ..bounded() });
            let entry = executor.snapshot();
            for _ in 0..4 {
                assert_eq!(executor.step(), None, "{context}");
            }
            let before = executor.snapshot();
            assert_eq!(executor.step(), None, "{context}");
            let half = angle / 2.0;
            let (c, s) = (half.cos(), half.sin());
            // The control is in |+>, and the target is in |1>. Its enabled component is the second
            // column of exp(-i angle Pauli/2). A full turn changes that component's relative phase.
            let (target_zero, target_one) = match axis {
                "rx" => (-C::I * s, C::from(c)),
                "ry" => (C::from(-s), C::from(c)),
                "rz" => (C::ZERO, C::new(c, s)),
                _ => unreachable!("three axes in the case table"),
            };
            assert_amplitudes(
                &executor.snapshot().quantum,
                &[
                    (vec![0, 1], C::from(FRAC_1_SQRT_2)),
                    (vec![1, 0], target_zero * FRAC_1_SQRT_2),
                    (vec![1, 1], target_one * FRAC_1_SQRT_2),
                ],
                &context,
            );
            assert_eq!(executor.run(), Outcome::Returned(None), "{context}");
            for _ in 0..3 {
                assert!(executor.step_back().expect("undo through the rotation"), "{context}");
            }
            common::assert_same_state(&executor.snapshot(), &before, &context);
            assert_eq!(executor.reverse(), Ok(4));
            common::assert_same_state(&executor.snapshot(), &entry, &context);
        }
    }
}

#[test]
fn a_nonfinite_computed_rotation_does_not_promote_either_memory_byte() {
    let module = common::load(
        "memory 128
data 8 = i8 0x04, i8 0x80
func @main(i64 %bits) {
entry:
    %q = qalloc 65
    h %q[64]
    %angle = xor i64 %bits, 0x8000000000000000
    rx(%angle) [8:2] ctrl [9:7], %q[64]
    ret
}",
    );
    for bits in [f64::INFINITY.to_bits(), f64::NEG_INFINITY.to_bits(), 0x7ff8_0000_0000_abcd, 0xfff0_0000_0000_0001] {
        let mut executor = executor(&module, "main", &[bits], Options { reversible: true, ..bounded() });
        let entry = executor.snapshot();
        for _ in 0..3 {
            assert_eq!(executor.step(), None);
        }
        let before = executor.snapshot();
        let stats = executor.stats();
        assert_eq!(executor.step(), Some(Outcome::Faulted(Fault::BadAngle)), "{bits:#x}");
        assert_eq!(Fault::BadAngle.code(), 18);
        assert_eq!(Fault::BadAngle.name(), "bad_angle");
        let mut after = executor.snapshot();
        after.classical.outcome = None;
        assert_eq!(after, before, "{bits:#x}");
        assert_eq!(executor.stats(), stats, "{bits:#x}: no byte was promoted");
        assert_eq!(executor.reverse(), Ok(3));
        common::assert_same_state(&executor.snapshot(), &entry, &format!("angle bits {bits:#x}"));
    }
}

#[test]
fn seeded_draws_straddle_a_boundary_with_an_empty_interval() {
    // A small rotation creates nonzero amplitudes for outcomes 1 and 3, but their weights
    // round away when added after 0 and 2. The first draw at the boundary must land on 2.
    let tiny = 2.0f64.powi(-29);
    let module = common::load(&format!(
        "memory 64
func @main(i64 %angle) -> i2 {{
entry:
    %q = qalloc 2
    ry(%angle) %q[1]
    ry({tiny}) %q
    %m = measure %q
    ret %m
}}"
    ));
    let tiny_half = tiny / 2.0;
    let mut exact_seeds = 0;
    for seed in 0..32 {
        let uniform = ChaCha8Rng::seed_from_u64(seed).random::<f64>();
        let center = 2.0 * uniform.sqrt().acos();
        let mut cases = [None; 3];
        // Search adjacent binary64 angles, since rounding the analytic inverse can put its draw
        // just to one side. Some seeds cannot land exactly on this boundary with these gates.
        for bits in center.to_bits() - 32..=center.to_bits() + 32 {
            let angle = f64::from_bits(bits) / 2.0;
            let c = angle.cos();
            let s = angle.sin();
            let [p0, p1, p2, p3] =
                [c * tiny_half.cos(), c * tiny_half.sin(), s * tiny_half.cos(), s * tiny_half.sin()].map(|a| a * a);
            let total = p0 + p2;
            assert!(p1 > 0.0 && p3 > 0.0);
            assert_eq!(p0 + p1, p0);
            assert_eq!(total + p3, total);
            let index = match (uniform * total).total_cmp(&p0) {
                Ordering::Less => 0,
                Ordering::Equal => 1,
                Ordering::Greater => 2,
            };
            cases[index] = Some(bits);
        }
        let [Some(below), boundary, Some(above)] = cases else { panic!("seed {seed}: angle search never crossed") };
        exact_seeds += usize::from(boundary.is_some());
        for (bits, expected) in [(Some(below), 0), (boundary, 2), (Some(above), 2)] {
            let Some(bits) = bits else { continue };
            let mut executor = executor(&module, "main", &[bits], Options { seed, ..bounded() });
            for _ in 0..3 {
                assert_eq!(executor.step(), None);
            }
            let state = executor.snapshot().quantum;
            assert_eq!(state.amplitudes.len(), 4, "seed {seed}: the two empty intervals have nonzero amplitudes");
            assert_eq!(executor.run(), Outcome::Returned(Some(expected)), "seed {seed}, angle {bits:#x}");
        }
    }
    assert!(exact_seeds >= 4, "only {exact_seeds} seeds reached the boundary exactly");
}
