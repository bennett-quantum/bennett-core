//! Controlled multiplication in place on a residue known to be below a fixed modulus.

use bennett_exec::Wire;

use super::bits::{Bit, Circuit, ONE, ZERO};

pub(super) fn width(modulus: u64) -> usize {
    (u64::BITS - (modulus - 1).leading_zeros()) as usize
}

#[derive(Clone, Copy)]
pub(super) struct Factor {
    value: u64,
    inverse: u64,
}

impl Factor {
    pub(super) fn prove(value: u128, modulus: u64) -> Option<Self> {
        if modulus < 2 {
            return None;
        }
        let modulus = u128::from(modulus);
        let value = value % modulus;
        // Extended Euclid, with the coefficients of value reduced modulo N.
        let (mut left, mut right) = (modulus, value);
        let (mut a, mut b) = (0, 1);
        while right != 0 {
            let quotient = left / right;
            (left, right) = (right, left % right);
            (a, b) = (b, (a + modulus - quotient * b % modulus) % modulus);
        }
        (left == 1 && value * a % modulus == 1).then_some(Self { value: value as u64, inverse: a as u64 })
    }
}

pub(super) struct Multiplier {
    modulus: u64,
    sum: Vec<Wire>,
    addend: Vec<Wire>,
    carry: Wire,
    below: Wire,
}

fn wire(id: Wire) -> Bit {
    Bit::Wire(id, false)
}

impl Multiplier {
    pub(super) fn new(circuit: &mut Circuit, modulus: u64) -> Self {
        let size = width(modulus) + 1; // An extra digit for the sign after subtraction.
        Self {
            modulus,
            sum: (0..size).map(|_| circuit.fresh()).collect(),
            addend: (0..size).map(|_| circuit.fresh()).collect(),
            carry: circuit.fresh(),
            below: circuit.fresh(),
        }
    }

    // Ripple the carry through the addend, then restore its digits while
    // completing the sum. The top digit does not produce a carry.
    fn add(&self, circuit: &mut Circuit) {
        let positions = self.addend.iter().copied().zip(self.sum.iter().copied()).enumerate();
        let carry = |index: usize| {
            if index == 0 { self.carry } else { self.addend[index - 1] }
        };
        for (index, (a, sum)) in positions.clone() {
            circuit.xor_into(sum, wire(a), ONE);
            if index + 1 < self.sum.len() {
                let carry = carry(index);
                circuit.xor_into(carry, wire(a), ONE);
                circuit.xor_into(a, wire(carry), wire(sum));
            }
        }
        for (index, (a, sum)) in positions.rev() {
            let carry = carry(index);
            if index + 1 < self.sum.len() {
                circuit.xor_into(a, wire(carry), wire(sum));
                circuit.xor_into(carry, wire(a), ONE);
            }
            circuit.xor_into(sum, wire(carry), ONE);
        }
    }

    fn subtract(&self, circuit: &mut Circuit) {
        for &digit in &self.sum {
            circuit.xor_into(digit, ONE, ONE);
        }
        self.add(circuit);
        for &digit in &self.sum {
            circuit.xor_into(digit, ONE, ONE);
        }
    }

    fn load(&self, circuit: &mut Circuit, value: u64, controls: [Bit; 2]) {
        for (index, &digit) in self.addend.iter().take(u64::BITS as usize).enumerate() {
            if value >> index & 1 != 0 {
                circuit.xor_into(digit, controls[0], controls[1]);
            }
        }
    }

    fn add_mod(&self, circuit: &mut Circuit, value: u64, controls: [Bit; 2]) {
        if value == 0 {
            return;
        }
        self.load(circuit, value, controls);
        self.add(circuit);
        self.load(circuit, value, controls);

        self.load(circuit, self.modulus, [ONE, ONE]);
        self.subtract(circuit);
        self.load(circuit, self.modulus, [ONE, ONE]);
        let sign = wire(*self.sum.last().expect("the sum has a sign bit"));
        circuit.xor_into(self.below, sign, ONE);
        self.load(circuit, self.modulus, [wire(self.below), ONE]);
        self.add(circuit);
        self.load(circuit, self.modulus, [wire(self.below), ONE]);

        // Subtracting the effective addend from the reduced sum is negative
        // exactly when N was subtracted. The other case clears the saved flag.
        self.load(circuit, value, controls);
        self.subtract(circuit);
        circuit.xor_into(self.below, sign.not(), ONE);
        self.add(circuit);
        self.load(circuit, value, controls);
    }

    fn digit_product(&self, factor: u64, bit: usize) -> u64 {
        ((u128::from(factor) << bit) % u128::from(self.modulus)) as u64
    }

    // The residue is in [0,N), and the factor and inverse multiply to 1 mod N.
    // The accumulator, addend, carry and flag start and finish at zero.
    pub(super) fn multiply(&self, circuit: &mut Circuit, residue: &[Wire], factor: Factor, enabled: Bit) {
        if enabled == ZERO || factor.value == 1 {
            return;
        }
        for (bit, &digit) in residue.iter().enumerate() {
            self.add_mod(circuit, self.digit_product(factor.value, bit), [enabled, wire(digit)]);
        }
        for (&y, &t) in residue.iter().zip(&self.sum) {
            circuit.xor_into(y, wire(t), ONE);
            circuit.xor_into(t, wire(y), enabled);
            circuit.xor_into(y, wire(t), ONE);
        }
        for (bit, &digit) in residue.iter().enumerate() {
            let value = self.digit_product(factor.inverse, bit);
            let subtract = (self.modulus - value) % self.modulus;
            self.add_mod(circuit, subtract, [enabled, wire(digit)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::gadget_tests::check;

    #[test]
    fn controlled_products_restore_every_work_wire() {
        for (modulus, factor) in [(7, 6), (15, 7), (21, 2), (10, 3), (17, 16)] {
            let factor = Factor::prove(factor, modulus).expect("invertible");
            let n = width(modulus);
            let mut circuit = Circuit::new(n as Wire + 1);
            let residue = (0..n).map(|_| circuit.fresh()).collect::<Vec<_>>();
            for (bit, &digit) in residue.iter().enumerate() {
                circuit.xor_into(digit, wire(bit as Wire + 1), ONE);
            }
            let multiplier = Multiplier::new(&mut circuit, modulus);
            multiplier.multiply(&mut circuit, &residue, factor, wire(0));
            let block = circuit.finish(0, "controlled in-place product").expect("valid wires");
            let result = residue.into_iter().map(wire).collect();
            let cost = |value: u64| 10 * n + 4 * value.count_ones() as usize;
            let additions = (0..n).flat_map(|bit| {
                [
                    multiplier.digit_product(factor.value, bit),
                    (modulus - multiplier.digit_product(factor.inverse, bit)) % modulus,
                ]
            });
            let toffolis = n + additions.filter(|&value| value != 0).map(cost).sum::<usize>();
            for value in 0..modulus {
                for enabled in [0, 1] {
                    let expected = if enabled == 0 { value } else { value * factor.value % modulus };
                    check(&block, &result, &[(1, enabled), (n, u128::from(value))], u128::from(expected), toffolis);
                }
            }
        }
    }

    #[test]
    fn nonunits_have_no_inverse() {
        assert!(Factor::prove(7, 21).is_none());
        assert!(Factor::prove(0, 21).is_none());
        assert!(Factor::prove(3, 0).is_none());
        assert!(Factor::prove(3, 1).is_none());
        let factor = Factor::prove(u128::from(u64::MAX - 1), u64::MAX).expect("minus one is its own inverse");
        assert_eq!(factor.inverse, u64::MAX - 1);
    }
}
