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
    if matches!(pred, Predicate::Eq | Predicate::Ne)
        && lhs.iter().zip(rhs).any(|(&left, &right)| left.same_value(right.not()))
    {
        return Bit::Constant(pred == Predicate::Ne);
    }
    let checkpoint = circuit.checkpoint();
    let result = if matches!(pred, Predicate::Eq | Predicate::Ne) {
        let equal = lhs.iter().zip(rhs).fold(ONE, |equal, (&a, &b)| {
            let different = circuit.xor(a, b);
            circuit.and(equal, different.not())
        });
        if pred == Predicate::Eq { equal } else { equal.not() }
    } else {
        order(circuit, pred, lhs, rhs)
    };
    if let Some(value) = result.literal()
        && circuit.fits()
    {
        circuit.rewind(checkpoint);
        return Bit::Constant(value);
    }
    result
}

fn order(circuit: &mut Circuit, pred: Predicate, lhs: &[Bit], rhs: &[Bit]) -> Bit {
    let (mut lhs, mut rhs) = (lhs, rhs);
    if matches!(pred, Predicate::Ugt | Predicate::Ule | Predicate::Sgt | Predicate::Sle) {
        std::mem::swap(&mut lhs, &mut rhs);
    }
    let signed = matches!(pred, Predicate::Slt | Predicate::Sle | Predicate::Sgt | Predicate::Sge);
    // Complementing the sign bits puts negative numbers before nonnegative numbers.
    let pairs = lhs
        .iter()
        .zip(rhs)
        .enumerate()
        .map(|(index, (&a, &b))| if signed && index + 1 == lhs.len() { (a.not(), b.not()) } else { (a, b) });
    // Any known difference decides the comparison of its bit and all lower bits.
    let decisive = pairs.clone().enumerate().rfind(|&(_, (a, b))| a.same_value(b.not()));
    let (first, mut less) = decisive.map_or((0, ZERO), |(index, (_, b))| (index + 1, b));
    for (a, b) in pairs.skip(first) {
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
            let input = |start| (start..start + width as u64).map(Bit::input).collect::<Word>();
            let mut circuit = Circuit::new(2 * width as u64);
            let result = binary(&mut circuit, BinOp::Or, &input(0), &input(width as u64)).expect("or");
            let block = circuit.finish(0, "or").expect("valid gadget wires");
            check(&block, &result, &[(width, 1), (width, 1 << (width - 1))], 1 | 1 << (width - 1), width);
        }
    }

    #[test]
    fn proved_comparisons_skip_the_unneeded_low_bits() {
        let mut left = (0..127).map(Bit::input).collect::<Word>();
        left.push(ONE);
        let right = [ZERO; 128];
        for (predicate, expected) in [
            (Predicate::Eq, false),
            (Predicate::Ne, true),
            (Predicate::Ult, false),
            (Predicate::Uge, true),
            (Predicate::Slt, true),
            (Predicate::Sge, false),
        ] {
            let mut circuit = Circuit::new(127);
            assert_eq!(compare(&mut circuit, predicate, &left, &right).literal(), Some(expected));
            assert_eq!(circuit.forward_len(), 0, "{predicate:?}");
            assert_eq!(circuit.scratch_end(), Some(127));
        }
    }

    #[test]
    fn fixed_products_keep_proved_high_bits_zero() {
        let mut circuit = Circuit::new(4);
        let mut input = (0..4).map(Bit::input).collect::<Word>();
        input.resize(128, ZERO);
        let product = multiply(&mut circuit, &input, 11);
        // 15 * 11 is 165. The high 120 bits are definite even though the
        // arithmetic instruction and its result are both 128 bits wide.
        assert!(crate::apply::identity::same_word(&product[8..], &[ZERO; 120]));
        assert!(!product[7].is_zero());
        assert_eq!(product_bits(1 << 127, 2, 128), 128);
    }
}
