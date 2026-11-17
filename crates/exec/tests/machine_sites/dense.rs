//! Direct dense tensor contractions; no executor arithmetic or world machinery.
use num_complex::Complex64 as C;
use std::f64::consts::FRAC_1_SQRT_2;

pub(super) type Matrix = [[C; 2]; 2];
pub(super) const ZERO: C = C::new(0.0, 0.0);
pub(super) const ONE: C = C::new(1.0, 0.0);

#[derive(Clone)]
pub(super) struct Circuit(pub(super) Vec<C>);
impl Circuit {
    pub(super) fn new(bits: usize) -> Self {
        let mut state = vec![ZERO; 1 << bits];
        state[0] = ONE;
        Self(state)
    }
    pub(super) fn gate(&mut self, m: Matrix, bit: usize, controls: usize) {
        for b in 0..self.0.len() {
            if b & (1 << bit) == 0 && b & controls == controls {
                let (a, z) = (self.0[b], self.0[b | (1 << bit)]);
                self.0[b] = m[0][0] * a + m[0][1] * z;
                self.0[b | (1 << bit)] = m[1][0] * a + m[1][1] * z;
            }
        }
    }
}
pub(super) fn matrix(name: &str) -> Matrix {
    let i = C::new(0.0, 1.0);
    let c = C::new((0.73_f64 / 2.0).cos(), 0.0);
    let s = C::new((0.73_f64 / 2.0).sin(), 0.0);
    match name {
        "x" => [[ZERO, ONE], [ONE, ZERO]],
        "h" => [[ONE * FRAC_1_SQRT_2, ONE * FRAC_1_SQRT_2], [ONE * FRAC_1_SQRT_2, -ONE * FRAC_1_SQRT_2]],
        "z" => [[ONE, ZERO], [ZERO, -ONE]],
        "s" => [[ONE, ZERO], [ZERO, i]],
        "t" => [[ONE, ZERO], [ZERO, C::from_polar(1.0, std::f64::consts::FRAC_PI_4)]],
        "rx(0.73)" => [[c, -i * s], [-i * s, c]],
        "ry(0.73)" => [[c, -s], [s, c]],
        "rz(0.73)" => [[c - i * s, ZERO], [ZERO, c + i * s]],
        _ => panic!("unknown gate {name}"),
    }
}
pub(super) fn adjoint(m: Matrix) -> Matrix {
    [[m[0][0].conj(), m[1][0].conj()], [m[0][1].conj(), m[1][1].conj()]]
}
pub(super) fn prepare(circuit: &mut Circuit, input: usize) -> &'static str {
    match input {
        0 => "",
        1 => {
            circuit.gate(matrix("x"), 0, 0);
            circuit.gate(matrix("x"), 1, 0);
            "x %c[0]\nx %c[1]\n"
        }
        2 => {
            circuit.gate(matrix("h"), 0, 0);
            circuit.gate(matrix("h"), 1, 0);
            "h %c[0]\nh %c[1]\n"
        }
        3 => {
            circuit.gate(matrix("h"), 0, 0);
            circuit.gate(matrix("s"), 0, 0);
            circuit.gate(matrix("h"), 1, 0);
            "h %c[0]\ns %c[0]\nh %c[1]\n"
        }
        4 => {
            circuit.gate(matrix("ry(0.73)"), 0, 0);
            circuit.gate(matrix("rx(0.73)"), 1, 0);
            "ry(0.73) %c[0]\nrx(0.73) %c[1]\n"
        }
        5 => {
            circuit.gate(matrix("h"), 0, 0);
            circuit.gate(matrix("x"), 1, 1);
            circuit.gate(matrix("t"), 1, 0);
            "h %c[0]\nx %c[1] ctrl %c[0]\nt %c[1]\n"
        }
        _ => unreachable!(),
    }
}
