//! Reversible integer expressions. Every newly computed wire starts in zero.

use bennett_ir::{BinOp, CastOp, Predicate};

use super::bits::{Bit, Circuit, ONE, Word, ZERO, definite};
use super::ranges::{product_bits, unsigned_max};
use super::{division, products, shifts};

pub(super) fn binary(circuit: &mut Circuit, op: BinOp, left: &[Bit], right: &[Bit]) -> Option<Word> {
    Some(match op {
        BinOp::Add => add(circuit, left, right.iter().copied(), ZERO),
        BinOp::Sub => add(circuit, left, right.iter().map(|bit| bit.not()), ONE),
        BinOp::And => left.iter().zip(right).map(|(&a, &b)| circuit.and(a, b)).collect(),
        BinOp::Or => left.iter().zip(right).map(|(&a, &b)| circuit.and(a.not(), b.not()).not()).collect(),
        BinOp::Xor => left.iter().zip(right).map(|(&a, &b)| circuit.xor(a, b)).collect(),
        BinOp::Mul => {
            let (word, factor) = match (definite(left), definite(right)) {
                (Some(factor), _) => (right, factor),
                (None, Some(factor)) => (left, factor),
                (None, None) => return Some(products::multiply(circuit, left, right)),
            };
            multiply(circuit, word, factor)
        }
        BinOp::Shl | BinOp::Lshr | BinOp::Ashr => shifts::shift(circuit, op, left, right),
        BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem => return None,
    })
}

// The operand proof needs constant folding, without emitting a general product.
pub(super) fn fold_binary(circuit: &mut Circuit, op: BinOp, left: &[Bit], right: &[Bit]) -> Option<Word> {
    if matches!(op, BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem) {
        return division::fold(op, left, right);
    }
    if matches!(op, BinOp::Shl | BinOp::Lshr | BinOp::Ashr) && definite(right).is_none() {
        return None;
    }
    if op == BinOp::Mul && definite(left).is_none() && definite(right).is_none() {
        return None;
    }
    binary(circuit, op, left, right)
}

fn add(circuit: &mut Circuit, left: &[Bit], right: impl Iterator<Item = Bit>, mut carry: Bit) -> Word {
    left.iter()
        .zip(right)
        .enumerate()
        .map(|(index, (&a, b))| {
            let different = circuit.xor(a, b);
            let sum = circuit.xor(different, carry);
            if index + 1 < left.len() {
                let both = circuit.and(a, b);
                let propagated = circuit.and(different, carry);
                carry = circuit.xor(both, propagated);
            }
            sum
        })
        .collect()
}

fn multiply(circuit: &mut Circuit, word: &[Bit], mut factor: u128) -> Word {
    let width = product_bits(unsigned_max(word), factor, word.len());
    let mut product = vec![ZERO; width];
    while factor != 0 {
        let shifted = shifts::fixed(BinOp::Shl, &word[..width], u128::from(factor.trailing_zeros()));
        product = add(circuit, &product, shifted.into_iter(), ZERO);
        factor &= factor - 1;
    }
    product.resize(word.len(), ZERO);
    product
}

pub(super) fn compare(circuit: &mut Circuit, pred: Predicate, lhs: &[Bit], rhs: &[Bit]) -> Bit {
    if matches!(pred, Predicate::Eq | Predicate::Ne) {
        let equal = lhs.iter().zip(rhs).fold(ONE, |equal, (&a, &b)| {
            let different = circuit.xor(a, b);
            circuit.and(equal, different.not())
        });
        return if pred == Predicate::Eq { equal } else { equal.not() };
    }

    let (mut lhs, mut rhs) = (lhs, rhs);
    if matches!(pred, Predicate::Ugt | Predicate::Ule | Predicate::Sgt | Predicate::Sle) {
        std::mem::swap(&mut lhs, &mut rhs);
    }
    let signed = matches!(pred, Predicate::Slt | Predicate::Sle | Predicate::Sgt | Predicate::Sge);
    let mut less = ZERO;
    // A more significant difference decides the comparison. Complementing the sign bits
    // puts negative numbers before nonnegative numbers in the same unsigned ordering.
    for (index, (&mut_a, &mut_b)) in lhs.iter().zip(rhs).enumerate() {
        let (a, b) = if signed && index + 1 == lhs.len() { (mut_a.not(), mut_b.not()) } else { (mut_a, mut_b) };
        let different = circuit.xor(a, b);
        less = circuit.select(different, b, less);
    }
    if matches!(pred, Predicate::Ule | Predicate::Uge | Predicate::Sle | Predicate::Sge) { less.not() } else { less }
}

pub(super) fn cast(op: CastOp, mut value: Word, width: usize) -> Word {
    let pad = if op == CastOp::Sext { value[value.len() - 1] } else { ZERO };
    value.resize(width, pad);
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn or_uses_one_toffoli_per_distinct_bit_pair() {
        use crate::apply::gadget_tests::check;

        for width in [1, 4, 8, 64, 128] {
            let input = |start| (start..start + width as u64).map(|wire| Bit::Wire(wire, false)).collect::<Word>();
            let mut circuit = Circuit::new(2 * width as u64);
            let result = binary(&mut circuit, BinOp::Or, &input(0), &input(width as u64)).expect("or");
            let block = circuit.finish(0, "or").expect("valid gadget wires");
            check(&block, &result, &[(width, 1), (width, 1 << (width - 1))], 1 | 1 << (width - 1), width);
        }
    }

    #[test]
    fn fixed_products_keep_proved_high_bits_zero() {
        let mut circuit = Circuit::new(4);
        let mut input = (0..4).map(|wire| Bit::Wire(wire, false)).collect::<Word>();
        input.resize(128, ZERO);
        let product = multiply(&mut circuit, &input, 11);
        // 15 * 11 is 165. The high 120 bits are definite even though the
        // arithmetic instruction and its result are both 128 bits wide.
        assert_eq!(&product[8..], &[ZERO; 120]);
        assert_ne!(product[7], ZERO);
        assert_eq!(product_bits(1 << 127, 2, 128), 128);
    }
}
