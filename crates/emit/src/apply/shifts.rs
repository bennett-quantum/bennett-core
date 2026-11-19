//! Conditional power-of-two shifts with a full-width range check.

use bennett_ir::{BinOp, Predicate};

use super::arithmetic;
use super::bits::{Bit, Circuit, Word, ZERO, constant, definite};

pub(super) fn fixed(op: BinOp, word: &[Bit], amount: u128) -> Word {
    let width = word.len();
    if amount >= width as u128 {
        return vec![ZERO; width];
    }
    let amount = amount as usize;
    let mut shifted = word.to_vec();
    if op == BinOp::Shl {
        shifted.copy_within(..width - amount, amount);
        shifted[..amount].fill(ZERO);
    } else {
        shifted.copy_within(amount.., 0);
        shifted[width - amount..].fill(if op == BinOp::Ashr { word[width - 1] } else { ZERO });
    }
    shifted
}

pub(super) fn shift(circuit: &mut Circuit, op: BinOp, word: &[Bit], amount: &[Bit]) -> Word {
    if let Some(amount) = definite(amount) {
        return fixed(op, word, amount);
    }

    let width = word.len();
    circuit.clean_word(width, |circuit| {
        let in_range = arithmetic::compare(circuit, Predicate::Ult, amount, &constant(width as u128, width));
        if in_range == ZERO {
            return vec![ZERO; width];
        }
        let mut result = word.to_vec();
        for (index, &enabled) in amount.iter().take(width.next_power_of_two().ilog2() as usize).enumerate() {
            let shifted = fixed(op, &result, 1 << index);
            for (bit, shifted) in result.iter_mut().zip(shifted) {
                *bit = circuit.select(enabled, shifted, *bit);
            }
        }
        // The executor gives zero for all out-of-range amounts, including ashr.
        result.into_iter().map(|bit| circuit.and(in_range, bit)).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::gadget_tests::check;

    #[test]
    fn shift_counts_and_clears_its_work_wires() {
        for width in [1usize, 2, 3, 4, 8, 65, 128] {
            for op in [BinOp::Shl, BinOp::Lshr, BinOp::Ashr] {
                let input = |start| (start..start + width as u64).map(|wire| Bit::Wire(wire, false)).collect::<Word>();
                let mut circuit = Circuit::new(2 * width as u64);
                let result = shift(&mut circuit, op, &input(0), &input(width as u64));
                let block = circuit.finish(0, "variable shift").expect("valid gadget wires");
                let stages: usize = (0..width.next_power_of_two().ilog2())
                    .map(|index| if op == BinOp::Ashr { 2 * (width - 1) } else { 2 * width - (1 << index) })
                    .sum();
                let range_test = width - width.trailing_zeros() as usize - 1;
                let toffolis = 2 * (stages + range_test + width);
                let high = 1 << (width - 1);
                check(&block, &result, &[(width, high), (width, 0)], high, toffolis);
                check(&block, &result, &[(width, high), (width, width as u128)], 0, toffolis);
            }
        }
    }
}
