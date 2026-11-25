//! The live state is a density matrix in the computational basis, stored by its nonzero entries.

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4};

use bennett_ir::{FixedGate, Gate, Rotation, Width};
use num_complex::Complex64 as C;

pub type Basis = Vec<u64>;
pub type Density = BTreeMap<(Basis, Basis), C>;

pub struct Quantum {
    // Handles are monotonic. A register's position in a basis vector can change on release.
    next_handle: u64,
    pub live: Vec<(u64, Width)>,
    pub density: Density,
}

impl Quantum {
    pub fn new() -> Self {
        Self { next_handle: 1, live: Vec::new(), density: BTreeMap::from([((Vec::new(), Vec::new()), C::ONE)]) }
    }

    pub fn allocate(&mut self, width: Width) -> u64 {
        let handle = self.next_handle;
        self.next_handle += 1;
        self.live.push((handle, width));
        self.density = std::mem::take(&mut self.density)
            .into_iter()
            .map(|((mut row, mut col), value)| {
                row.push(0);
                col.push(0);
                ((row, col), value)
            })
            .collect();
        handle
    }

    pub fn position(&self, handle: u64) -> Option<usize> {
        self.live.iter().position(|&(id, _)| id == handle)
    }

    pub fn release(&mut self, register: usize) {
        self.live.remove(register);
        let mut reduced = Density::new();
        for ((mut row, mut col), value) in std::mem::take(&mut self.density) {
            if row.remove(register) == col.remove(register) {
                *reduced.entry((row, col)).or_default() += value;
            }
        }
        self.density = reduced;
    }

    pub fn gate(&mut self, gate: Gate, (register, bit): (usize, u8), controls: &[(usize, u8)]) {
        let matrix = unitary(gate);
        let image = |basis: Basis| {
            if controls.iter().any(|&(reg, bit)| basis[reg] & (1 << bit) == 0) {
                return vec![(basis, C::ONE)];
            }
            let input = ((basis[register] >> bit) & 1) as usize;
            matrix
                .iter()
                .enumerate()
                .filter(|(_, row)| row[input] != C::ZERO)
                .map(|(output, row)| {
                    let mut new = basis.clone();
                    new[register] ^= ((input ^ output) as u64) << bit;
                    (new, row[input])
                })
                .collect()
        };
        let mut next = Density::new();
        // U rho U†: expand each side of the entry separately.
        for ((row, col), value) in std::mem::take(&mut self.density) {
            let cols = image(col);
            for (row, left) in image(row) {
                for (col, right) in &cols {
                    *next.entry((row.clone(), col.clone())).or_default() += left * value * right.conj();
                }
            }
        }
        next.retain(|_, value| *value != C::ZERO);
        self.density = next;
    }

    pub fn probabilities(&self, register: usize) -> BTreeMap<u64, f64> {
        let mut probabilities = BTreeMap::new();
        for ((row, col), value) in &self.density {
            if row == col {
                *probabilities.entry(row[register]).or_default() += value.re;
            }
        }
        probabilities
    }

    pub fn project(&mut self, register: usize, outcome: u64, probability: f64) {
        self.density.retain(|(row, col), value| {
            if row[register] == outcome && col[register] == outcome {
                *value /= probability;
                true
            } else {
                false
            }
        });
    }

    pub fn permute(&mut self, image: impl Fn(Basis) -> Basis) {
        self.density = std::mem::take(&mut self.density)
            .into_iter()
            .map(|((row, col), value)| ((image(row), image(col)), value))
            .collect();
    }
}

/// In the |0>, |1> basis, rotations are cos(theta/2) I - i sin(theta/2) sigma.
fn unitary(gate: Gate) -> [[C; 2]; 2] {
    let real = |value| C::new(value, 0.0);
    let phase = |value| [[C::ONE, C::ZERO], [C::ZERO, value]];
    match gate {
        Gate::Fixed(FixedGate::X) => [[C::ZERO, C::ONE], [C::ONE, C::ZERO]],
        Gate::Fixed(FixedGate::H) => {
            let h = real(FRAC_1_SQRT_2);
            [[h, h], [h, -h]]
        }
        Gate::Fixed(FixedGate::Z) => phase(-C::ONE),
        Gate::Fixed(FixedGate::S) => phase(C::I),
        Gate::Fixed(FixedGate::Sdg) => phase(-C::I),
        Gate::Fixed(FixedGate::T) => phase(C::from_polar(1.0, FRAC_PI_4)),
        Gate::Fixed(FixedGate::Tdg) => phase(C::from_polar(1.0, -FRAC_PI_4)),
        Gate::Rotate(axis, theta) => {
            let (s, c) = (theta / 2.0).sin_cos();
            let c = real(c);
            match axis {
                Rotation::Rx => [[c, -C::I * s], [-C::I * s, c]],
                Rotation::Ry => [[c, real(-s)], [real(s), c]],
                Rotation::Rz => [[c - C::I * s, C::ZERO], [C::ZERO, c + C::I * s]],
                Rotation::P => panic!("the phase rotation is outside this reference"),
            }
        }
    }
}
