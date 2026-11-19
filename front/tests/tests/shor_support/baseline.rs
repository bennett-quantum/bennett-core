//! A hand-built modular exponentiation using controlled modular additions.

use bennett_exec::{BlockOp, GateBlock, GateOp, Wire};
use bennett_ir::{FixedGate, Gate};

use super::{EXPONENT_BITS, INTERFACE};

fn flip(gates: &mut Vec<GateOp>, target: Wire, controls: &[Wire]) {
    gates.push(GateOp { kind: Gate::Fixed(FixedGate::X), target, controls: controls.to_vec() });
}

fn xor_constant(gates: &mut Vec<GateOp>, word: &[Wire], value: u64, controls: &[Wire]) {
    for (bit, &wire) in word.iter().enumerate() {
        if value >> bit & 1 == 1 {
            flip(gates, wire, controls);
        }
    }
}

struct ModularAdder {
    sum: Vec<Wire>,
    addend: Vec<Wire>,
    carry: Wire,
    negative: Wire,
    modulus: u64,
}

impl ModularAdder {
    // Carry travels through the addend, then the reverse sweep restores the
    // addend and writes the sum bits. The incoming carry returns to zero.
    fn add(&self, gates: &mut Vec<GateOp>) {
        let lines = self.addend.iter().copied().zip(self.sum.iter().copied()).enumerate();
        let carry_into = |bit| if bit == 0 { self.carry } else { self.addend[bit - 1] };
        for (bit, (a, sum)) in lines.clone() {
            let carry = carry_into(bit);
            flip(gates, sum, &[a]);
            flip(gates, carry, &[a]);
            flip(gates, a, &[carry, sum]);
        }
        for (bit, (a, sum)) in lines.rev() {
            let carry = carry_into(bit);
            flip(gates, a, &[carry, sum]);
            flip(gates, carry, &[a]);
            flip(gates, sum, &[carry]);
        }
    }

    fn subtract(&self, gates: &mut Vec<GateOp>) {
        let invert = |gates: &mut Vec<GateOp>| {
            for &wire in &self.sum {
                flip(gates, wire, &[]);
            }
        };
        invert(gates);
        self.add(gates);
        invert(gates);
    }

    fn add_mod(&self, gates: &mut Vec<GateOp>, value: u64, controls: [Wire; 2]) {
        if value == 0 {
            return;
        }
        xor_constant(gates, &self.addend, value, &controls);
        self.add(gates);
        xor_constant(gates, &self.addend, value, &controls);

        let sign = *self.sum.last().expect("modular sum has a sign bit");
        xor_constant(gates, &self.addend, self.modulus, &[]);
        self.subtract(gates);
        xor_constant(gates, &self.addend, self.modulus, &[]);
        flip(gates, self.negative, &[sign]);
        xor_constant(gates, &self.addend, self.modulus, &[self.negative]);
        self.add(gates);
        xor_constant(gates, &self.addend, self.modulus, &[self.negative]);

        // Removing the effective addend underflows exactly when the modular
        // subtraction was kept. Toggle the flag on the other case, then undo
        // this trial subtraction.
        xor_constant(gates, &self.addend, value, &controls);
        self.subtract(gates);
        flip(gates, sign, &[]);
        flip(gates, self.negative, &[sign]);
        flip(gates, sign, &[]);
        self.add(gates);
        xor_constant(gates, &self.addend, value, &controls);
    }

    fn multiply(&self, gates: &mut Vec<GateOp>, control: Wire, factor: u64, residue: &[Wire]) {
        for (bit, &wire) in residue.iter().enumerate() {
            self.add_mod(gates, (factor << bit) % self.modulus, [control, wire]);
        }
        for (&a, &b) in residue.iter().zip(&self.sum) {
            flip(gates, a, &[b]);
            flip(gates, b, &[control, a]);
            flip(gates, a, &[b]);
        }
        let inverse = (1..self.modulus)
            .find(|candidate| candidate * factor % self.modulus == 1)
            .expect("factor is coprime to the modulus");
        for (bit, &wire) in residue.iter().enumerate() {
            let undo = (self.modulus - ((inverse << bit) % self.modulus)) % self.modulus;
            self.add_mod(gates, undo, [control, wire]);
        }
    }
}

pub(super) fn powmod(base: u64, modulus: u64) -> GateBlock {
    let width = u64::from(u64::BITS - (modulus - 1).leading_zeros());
    let residue = (INTERFACE..INTERFACE + width).collect::<Vec<_>>();
    let sum_start = INTERFACE + width;
    let addend_start = sum_start + width + 1;
    let carry = addend_start + width + 1;
    let adder = ModularAdder {
        sum: (sum_start..addend_start).collect(),
        addend: (addend_start..carry).collect(),
        carry,
        negative: carry + 1,
        modulus,
    };
    let mut gates = Vec::new();
    flip(&mut gates, residue[0], &[]);
    let mut factor = base;
    for control in 0..EXPONENT_BITS {
        if factor != 1 {
            adder.multiply(&mut gates, control, factor, &residue);
        }
        factor = factor * factor % modulus;
    }

    // Use the same XOR output interface and clean work qubits as apply.
    let compute = gates.len();
    for (&wire, output) in residue.iter().zip(EXPONENT_BITS..) {
        flip(&mut gates, output, &[wire]);
    }
    gates.extend((0..compute).rev().map(|index| gates[index].clone()).collect::<Vec<_>>());
    let scratch = INTERFACE..carry + 2;
    GateBlock::from_ops(
        "hand-built powmod",
        [BlockOp::Allocate(scratch.clone())]
            .into_iter()
            .chain(gates.into_iter().map(BlockOp::Gate))
            .chain([BlockOp::ReleaseZero(scratch)]),
    )
}
