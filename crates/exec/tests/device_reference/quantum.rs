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
    pub cells: BTreeMap<u64, usize>,
    pub density: Density,
}

impl Quantum {
    pub fn new() -> Self {
        Self {
            next_handle: 1,
            live: Vec::new(),
            cells: BTreeMap::new(),
            density: BTreeMap::from([((Vec::new(), Vec::new()), C::ONE)]),
        }
    }

    pub fn allocate(&mut self, width: Width) -> u64 {
        let handle = self.next_handle;
        self.next_handle += 1;
        // Register columns precede cells, whose positions are tracked independently of address order.
        let column = self.live.len();
        self.live.push((handle, width));
        for position in self.cells.values_mut() {
            *position += 1;
        }
        self.insert_column(column, 0);
        handle
    }

    pub fn promote(&mut self, address: u64, value: u8) -> usize {
        let column = self.live.len() + self.cells.len();
        assert!(self.cells.insert(address, column).is_none());
        self.insert_column(column, u64::from(value));
        column
    }

    fn insert_column(&mut self, column: usize, value: u64) {
        self.permute(|mut basis| {
            basis.insert(column, value);
            basis
        });
    }

    pub fn position(&self, handle: u64) -> Option<usize> {
        self.live.iter().position(|&(id, _)| id == handle)
    }

    pub fn release(&mut self, register: usize) {
        self.live.remove(register);
        self.trace_column(register);
    }

    pub fn demote(&mut self, address: u64) {
        let column = self.cells.remove(&address).expect("measured cell");
        self.trace_column(column);
    }

    fn trace_column(&mut self, column: usize) {
        for position in self.cells.values_mut() {
            if *position > column {
                *position -= 1;
            }
        }
        let mut reduced = Density::new();
        for ((mut row, mut col), value) in std::mem::take(&mut self.density) {
            if row.remove(column) == col.remove(column) {
                *reduced.entry((row, col)).or_default() += value;
            }
        }
        self.density = reduced;
    }

    pub fn gate(&mut self, gate: Gate, (column, bit): (usize, u8), controls: &[(usize, u8)]) {
        let matrix = unitary(gate);
        let image = |basis: Basis| {
            if controls.iter().any(|&(reg, bit)| basis[reg] & (1 << bit) == 0) {
                return vec![(basis, C::ONE)];
            }
            let input = ((basis[column] >> bit) & 1) as usize;
            matrix
                .iter()
                .enumerate()
                .filter(|(_, row)| row[input] != C::ZERO)
                .map(|(output, row)| {
                    let mut new = basis.clone();
                    new[column] ^= ((input ^ output) as u64) << bit;
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

    pub fn probabilities(&self, read: impl Fn(&Basis) -> u128) -> BTreeMap<u128, f64> {
        let mut probabilities = BTreeMap::new();
        for ((row, col), value) in &self.density {
            if row == col {
                *probabilities.entry(read(row)).or_default() += value.re;
            }
        }
        probabilities
    }

    pub fn project(&mut self, matches: impl Fn(&Basis) -> bool, probability: f64) {
        self.density.retain(|(row, col), value| {
            if matches(row) && matches(col) {
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
