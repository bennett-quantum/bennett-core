//! The unitary of each single-qubit gate.

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4};

use bennett_ir::{FixedGate, Gate, Rotation};
use num_complex::Complex64;

/// A 2×2 unitary, indexed `[row][column]` in the basis |0⟩, |1⟩.
pub(crate) type Matrix = [[Complex64; 2]; 2];

fn real(x: f64) -> Complex64 {
    Complex64::new(x, 0.0)
}

fn diagonal(phase: Complex64) -> Matrix {
    [[real(1.0), real(0.0)], [real(0.0), phase]]
}

/// The matrix of `gate`. Rotations are `exp(-iθσ/2)`.
pub(crate) fn matrix(gate: Gate) -> Matrix {
    let (zero, one, i) = (real(0.0), real(1.0), Complex64::i());
    match gate {
        Gate::Fixed(FixedGate::X) => [[zero, one], [one, zero]],
        Gate::Fixed(FixedGate::H) => {
            let h = real(FRAC_1_SQRT_2);
            [[h, h], [h, -h]]
        }
        Gate::Fixed(FixedGate::Z) => diagonal(-one),
        Gate::Fixed(FixedGate::S) => diagonal(i),
        Gate::Fixed(FixedGate::Sdg) => diagonal(-i),
        Gate::Fixed(FixedGate::T) => diagonal(Complex64::from_polar(1.0, FRAC_PI_4)),
        Gate::Fixed(FixedGate::Tdg) => diagonal(Complex64::from_polar(1.0, -FRAC_PI_4)),
        Gate::Rotate(axis, angle) => {
            let (c, s) = ((angle / 2.0).cos(), (angle / 2.0).sin());
            match axis {
                Rotation::Rx => [[real(c), -i * s], [-i * s, real(c)]],
                Rotation::Ry => [[real(c), real(-s)], [real(s), real(c)]],
                Rotation::Rz => {
                    [[Complex64::from_polar(1.0, -angle / 2.0), zero], [zero, Complex64::from_polar(1.0, angle / 2.0)]]
                }
            }
        }
    }
}

/// The gate that undoes `gate`.
pub(crate) fn inverse(gate: Gate) -> Gate {
    match gate {
        Gate::Fixed(FixedGate::S) => Gate::Fixed(FixedGate::Sdg),
        Gate::Fixed(FixedGate::Sdg) => Gate::Fixed(FixedGate::S),
        Gate::Fixed(FixedGate::T) => Gate::Fixed(FixedGate::Tdg),
        Gate::Fixed(FixedGate::Tdg) => Gate::Fixed(FixedGate::T),
        Gate::Fixed(self_inverse) => Gate::Fixed(self_inverse),
        Gate::Rotate(axis, angle) => Gate::Rotate(axis, -angle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(a: &Matrix, b: &Matrix) -> Matrix {
        let entry = |r: usize, c: usize| a[r][0] * b[0][c] + a[r][1] * b[1][c];
        [[entry(0, 0), entry(0, 1)], [entry(1, 0), entry(1, 1)]]
    }

    #[test]
    fn every_gate_times_its_inverse_is_the_identity() {
        let mut gates: Vec<Gate> = FixedGate::ALL.iter().map(|&g| Gate::Fixed(g)).collect();
        gates.extend(Rotation::ALL.iter().map(|&axis| Gate::Rotate(axis, 0.7)));
        for gate in gates {
            let p = product(&matrix(gate), &matrix(inverse(gate)));
            for (r, row) in p.iter().enumerate() {
                for (c, value) in row.iter().enumerate() {
                    let expected = if r == c { 1.0 } else { 0.0 };
                    assert!((value - real(expected)).norm() < 1e-15, "{gate:?}");
                }
            }
        }
    }
}
