//! Restoring unsigned remainder, one trial subtraction per dividend bit.

use bennett_ir::Predicate;

use super::arithmetic;
use super::bits::{Bit, Circuit, ONE, Word, ZERO, constant, definite};
use super::ranges::{bits, unsigned_max};

pub(super) fn is_zero(circuit: &mut Circuit, divisor: &[Bit]) -> Bit {
    if divisor.contains(&ONE) {
        return ZERO;
    }
    let tested = divisor.iter().copied().filter(|&bit| bit != ZERO).collect::<Word>();
    if tested.is_empty() {
        ONE
    } else if tested.len() == 1 {
        tested[0].not()
    } else {
        circuit.clean_word(1, |circuit| vec![tested.into_iter().fold(ONE, |zero, bit| circuit.and(zero, bit.not()))])[0]
    }
}

// Keep even a definite bit on a wire when it is part of a trial subtraction.
// This provides a fixed topology and lets callers recount the native gates.
fn materialize(circuit: &mut Circuit, bit: Bit) -> Bit {
    let wire = circuit.fresh();
    circuit.xor_into(wire, bit, ONE);
    Bit::Wire(wire, false)
}

pub(super) fn result_bits(dividend: &[Bit], divisor: &[Bit]) -> usize {
    // For a successful division the remainder is at most the dividend and
    // strictly below the divisor.
    bits(unsigned_max(dividend).min(unsigned_max(divisor).saturating_sub(1)))
}

pub(super) fn reduce(circuit: &mut Circuit, dividend: &[Bit], divisor: &[Bit]) -> Word {
    let modulus = definite(divisor);
    if let (Some(left), Some(right)) = (definite(dividend), modulus)
        && let Some(value) = left.checked_rem(right)
    {
        return constant(value, dividend.len());
    }
    let maximum = unsigned_max(dividend);
    let significant = bits(maximum);
    let divisor_width = bits(unsigned_max(divisor));
    let width = result_bits(dividend, divisor);
    let mut word = if width == 0 {
        Vec::new()
    } else if modulus.is_some_and(|value| value > maximum || value.is_power_of_two()) {
        dividend[..width].to_vec()
    } else if modulus == Some(maximum) {
        // The only possible nonzero quotient is one, when the two words
        // are equal. In that case the remainder has every bit clear.
        let keep = arithmetic::compare(circuit, Predicate::Ne, &dividend[..significant], &divisor[..significant]);
        dividend[..width].iter().map(|&bit| circuit.and(keep, bit)).collect()
    } else {
        circuit.clean_word(width, |circuit| {
            let denominator = divisor[..divisor_width].iter().map(|&bit| materialize(circuit, bit)).collect::<Word>();
            let zero = materialize(circuit, ZERO);
            let mut residue = vec![zero; divisor_width];
            for &bit in dividend[..significant].iter().rev() {
                residue = bring_down(circuit, &residue, bit, &denominator);
            }
            residue.truncate(width);
            residue
        })
    };
    word.resize(dividend.len(), ZERO);
    word
}

fn bring_down(circuit: &mut Circuit, residue: &[Bit], bit: Bit, divisor: &[Bit]) -> Word {
    let (&top, lower) = residue.split_last().expect("nonempty divisor");
    let candidate = std::iter::once(materialize(circuit, bit)).chain(lower.iter().copied()).collect::<Word>();
    let mut borrow = ZERO;
    let difference = candidate
        .iter()
        .zip(divisor)
        .map(|(&a, &b)| {
            let different = circuit.xor(a, b);
            let digit = circuit.xor(different, borrow);
            let generated = circuit.and(a.not(), b);
            let propagated = circuit.and(different.not(), borrow);
            borrow = circuit.xor(generated, propagated);
            digit
        })
        .collect::<Word>();

    // The doubled residue has one extra high bit. Subtract if that bit is set
    // or if the remaining bits subtracted without a borrow.
    let fits = circuit.and(top.not(), borrow).not();
    candidate
        .into_iter()
        .zip(difference)
        .map(|(original, subtracted)| {
            let change = circuit.xor(original, subtracted);
            let selected = circuit.and(fits, change);
            circuit.xor(original, selected)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::gadget_tests::check;

    #[test]
    fn remainder_counts_and_clears_its_work_wires() {
        for (p, m) in [(3, 2), (8, 3), (8, 4), (12, 6), (16, 8), (128, 64)] {
            let width = p.max(m);
            let inputs = |start, count| {
                (start..start + count as u64)
                    .map(|wire| Bit::Wire(wire, false))
                    .chain(std::iter::repeat(ZERO))
                    .take(width)
                    .collect::<Word>()
            };
            let mut circuit = Circuit::new((p + m) as u64);
            let result = reduce(&mut circuit, &inputs(0, p), &inputs(p as u64, m));
            let block = circuit.finish(0, "remainder").expect("valid gadget wires");
            let x = u128::MAX >> (128 - p);
            let y = (1u128 << m) - 1;
            check(&block, &result, &[(p, x), (m, y)], x % y, 6 * p * m);
            // The emitter ignores this value and copies the fault code when the divisor is zero.
            check(&block, &result, &[(p, x), (m, 0)], x & y, 6 * p * m);
        }
    }

    #[test]
    fn zero_predicate_counts_and_clears_its_work_wires() {
        for width in [2, 3, 4, 6, 8, 64] {
            let input = (0..width as u64).map(|wire| Bit::Wire(wire, false)).collect::<Word>();
            let mut circuit = Circuit::new(width as u64);
            let result = vec![is_zero(&mut circuit, &input)];
            let block = circuit.finish(0, "zero check").expect("valid wires");
            check(&block, &result, &[(width, 0)], 1, 2 * (width - 1));
            check(&block, &result, &[(width, 1 << (width - 1))], 0, 2 * (width - 1));
        }
    }

    #[test]
    fn known_modulus_caps_the_result_and_powers_of_two_need_no_gates() {
        let mut circuit = Circuit::new(8);
        let input = (0..8).map(|wire| Bit::Wire(wire, false)).collect::<Word>();
        let power = reduce(&mut circuit, &input, &constant(8, 8));
        assert_eq!(&power[..3], &input[..3]);
        assert_eq!(&power[3..], &[ZERO; 5]);
        assert_eq!(circuit.forward_len(), 0);

        let bounded = reduce(&mut circuit, &input, &constant(11, 8));
        assert_eq!(&bounded[4..], &[ZERO; 4]);
        let block = circuit.finish(0, "bounded remainder").expect("valid wires");
        check(&block, &bounded, &[(8, 21)], 10, 6 * 8 * 4);
    }

    #[test]
    fn remainder_at_the_dividend_bound_only_clears_equality() {
        let mut circuit = Circuit::new(4);
        let input = (0..4).map(|wire| Bit::Wire(wire, false)).collect::<Word>();
        let result = reduce(&mut circuit, &input, &constant(15, 4));
        let block = circuit.finish(0, "remainder at bound").expect("valid wires");
        for value in 0..16 {
            check(&block, &result, &[(4, value)], value % 15, 7);
        }
    }
}
