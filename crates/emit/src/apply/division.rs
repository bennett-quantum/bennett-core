//! Restoring division and remainder, one trial subtraction per dividend bit.

use bennett_ir::{BinOp, Predicate};

use super::arithmetic;
use super::bits::{Bit, Circuit, ONE, Word, ZERO, constant, definite};
use super::ranges::{bits, unsigned_max};
use super::shifts;

pub(super) fn is_zero(circuit: &mut Circuit, divisor: &[Bit]) -> Bit {
    if divisor.iter().any(|bit| bit.is_one()) {
        return ZERO;
    }
    let tested = divisor.iter().copied().filter(|&bit| !bit.is_zero()).collect::<Word>();
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
    circuit.value(wire)
}

pub(super) fn result_bits(dividend: &[Bit], divisor: &[Bit]) -> usize {
    // For a successful division the remainder is at most the dividend and
    // strictly below the divisor.
    bits(unsigned_max(dividend).min(unsigned_max(divisor).saturating_sub(1)))
}

fn signed_value(value: u128, width: usize) -> i128 {
    ((value << (128 - width)) as i128) >> (128 - width)
}

// The most negative input has the sign bit set and every unproved low
// bit clear. The most positive input has the sign bit clear and every
// unproved low bit set. Correlations can only tighten these bounds.
fn maximum_signed_magnitude(word: &[Bit]) -> u128 {
    let (&sign, low) = word.split_last().expect("integer has a sign bit");
    let positive = if sign.is_one() { 0 } else { unsigned_max(low) };
    let negative = if sign.is_zero() {
        0
    } else {
        let minimum_low =
            low.iter().enumerate().fold(0, |value, (index, bit)| value | (u128::from(bit.is_one()) << index));
        (1 << low.len()) - minimum_low
    };
    positive.max(negative)
}

struct RemainderBounds {
    dividend: u128,
    modulus: u128,
    can_be_negative: bool,
}

impl RemainderBounds {
    fn constant_divisor(op: BinOp, dividend: &[Bit], divisor: &[Bit]) -> Option<Self> {
        let value = definite(divisor).filter(|&value| value != 0)?;
        match op {
            BinOp::Urem => Some(Self { dividend: unsigned_max(dividend), modulus: value, can_be_negative: false }),
            BinOp::Srem => Some(Self {
                dividend: maximum_signed_magnitude(dividend),
                modulus: signed_value(value, divisor.len()).unsigned_abs(),
                can_be_negative: dividend.last().is_none_or(|bit| !bit.is_zero()),
            }),
            BinOp::Udiv
            | BinOp::Sdiv
            | BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::And
            | BinOp::Or
            | BinOp::Xor
            | BinOp::Shl
            | BinOp::Lshr
            | BinOp::Ashr => None,
        }
    }

    fn maximum(&self) -> u128 {
        self.dividend.min(self.modulus - 1)
    }

    fn width(&self) -> usize {
        let maximum = self.maximum();
        bits(maximum) + usize::from(maximum != 0 && self.can_be_negative)
    }

    fn sign_extension(&self, word: &[Bit]) -> Bit {
        if self.can_be_negative { word.last().copied().unwrap_or(ZERO) } else { ZERO }
    }
}

pub(super) fn apply_remainder_facts(op: BinOp, dividend: &[Bit], divisor: &[Bit], result: &mut [Bit]) {
    if let Some(bounds) = RemainderBounds::constant_divisor(op, dividend, divisor) {
        let (value, high) = result.split_at_mut(bounds.width());
        high.fill(bounds.sign_extension(value));
    } else if op == BinOp::Urem && definite(divisor) != Some(0) {
        result[result_bits(dividend, divisor)..].fill(ZERO);
    }
}

pub(super) fn fold(op: BinOp, dividend: &[Bit], divisor: &[Bit]) -> Option<Word> {
    let width = dividend.len();
    let right = definite(divisor).filter(|&value| value != 0)?;
    let Some(left) = definite(dividend) else {
        let bounds = RemainderBounds::constant_divisor(op, dividend, divisor)?;
        return if bounds.maximum() == 0 {
            Some(vec![ZERO; width])
        } else if bounds.dividend < bounds.modulus {
            Some(dividend.to_vec())
        } else {
            None
        };
    };
    let signed = |value| signed_value(value, width);
    let value = match op {
        BinOp::Udiv => left / right,
        BinOp::Urem => left % right,
        BinOp::Sdiv => signed(left).wrapping_div(signed(right)).cast_unsigned(),
        BinOp::Srem => signed(left).wrapping_rem(signed(right)).cast_unsigned(),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::And
        | BinOp::Or
        | BinOp::Xor
        | BinOp::Shl
        | BinOp::Lshr
        | BinOp::Ashr => unreachable!("division or remainder"),
    };
    Some(constant(value, width))
}

pub(super) fn calculate(circuit: &mut Circuit, op: BinOp, dividend: &[Bit], divisor: &[Bit]) -> Word {
    if let Some(result) = fold(op, dividend, divisor) {
        return result;
    }
    match op {
        BinOp::Udiv => quotient(circuit, dividend, divisor),
        BinOp::Urem => reduce(circuit, dividend, divisor),
        BinOp::Sdiv => signed(circuit, dividend, divisor, true),
        BinOp::Srem => {
            if let Some(bounds) = RemainderBounds::constant_divisor(op, dividend, divisor) {
                signed_constant_remainder(circuit, dividend, &bounds)
            } else {
                signed(circuit, dividend, divisor, false)
            }
        }
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::And
        | BinOp::Or
        | BinOp::Xor
        | BinOp::Shl
        | BinOp::Lshr
        | BinOp::Ashr => unreachable!("division or remainder"),
    }
}

fn quotient(circuit: &mut Circuit, dividend: &[Bit], divisor: &[Bit]) -> Word {
    let maximum = unsigned_max(dividend);
    if let Some(divisor) = definite(divisor) {
        if divisor > maximum {
            return vec![ZERO; dividend.len()];
        }
        if divisor.is_power_of_two() {
            return shifts::fixed(BinOp::Lshr, dividend, u128::from(divisor.trailing_zeros()));
        }
    }
    if maximum == 0 {
        return vec![ZERO; dividend.len()];
    }
    let p = bits(maximum);
    let m = bits(unsigned_max(divisor));
    let mut result = circuit.clean_word(p, |circuit| restore(circuit, &dividend[..p], &divisor[..m], true));
    result.resize(dividend.len(), ZERO);
    result
}

fn signed(circuit: &mut Circuit, dividend: &[Bit], divisor: &[Bit], quotient: bool) -> Word {
    let width = dividend.len();
    let dividend_sign = dividend[width - 1];
    let divisor_sign = divisor[width - 1];
    circuit.clean_word(width, |circuit| {
        let numerator = negate_if(circuit, dividend, dividend_sign);
        let denominator = negate_if(circuit, divisor, divisor_sign);
        let magnitude = restore(circuit, &numerator, &denominator, quotient);
        let sign = if quotient { circuit.xor(dividend_sign, divisor_sign) } else { dividend_sign };
        negate_if(circuit, &magnitude, sign)
    })
}

fn signed_constant_remainder(circuit: &mut Circuit, dividend: &[Bit], bounds: &RemainderBounds) -> Word {
    let width = bounds.width();
    let significant = bits(bounds.dividend);
    let divisor = constant(bounds.modulus, bits(bounds.modulus));
    let sign = dividend[dividend.len() - 1];
    let mut result = circuit.clean_word(width, |circuit| {
        let magnitude = negate_if(circuit, &dividend[..significant], sign);
        let mut remainder = restore(circuit, &magnitude, &divisor, false);
        remainder.truncate(bits(bounds.maximum()));
        remainder.resize(width, ZERO);
        negate_if(circuit, &remainder, sign)
    });
    let high = bounds.sign_extension(&result);
    result.resize(dividend.len(), high);
    result
}

// In two's complement negation, a bit flips exactly when a lower bit is one.
fn negate_if(circuit: &mut Circuit, word: &[Bit], enabled: Bit) -> Word {
    if enabled.is_zero() {
        return word.to_vec();
    }
    let mut nonzero = ZERO;
    let mut negated = Vec::with_capacity(word.len());
    for (index, &bit) in word.iter().enumerate() {
        let flip = circuit.and(enabled, nonzero);
        negated.push(circuit.xor(bit, flip));
        if index + 1 < word.len() {
            nonzero = circuit.and(nonzero.not(), bit.not()).not();
        }
    }
    negated
}

fn reduce(circuit: &mut Circuit, dividend: &[Bit], divisor: &[Bit]) -> Word {
    let modulus = definite(divisor);
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
            let mut residue = restore(circuit, &dividend[..significant], &divisor[..divisor_width], false);
            residue.truncate(width);
            residue
        })
    };
    word.resize(dividend.len(), ZERO);
    word
}

fn restore(circuit: &mut Circuit, dividend: &[Bit], divisor: &[Bit], quotient: bool) -> Word {
    let denominator = divisor.iter().map(|&bit| materialize(circuit, bit)).collect::<Word>();
    let zero = materialize(circuit, ZERO);
    let mut residue = vec![zero; divisor.len()];
    let mut result = vec![ZERO; dividend.len()];
    for (index, &bit) in dividend.iter().enumerate().rev() {
        let need_difference = !quotient || index != 0;
        let (candidate, difference, fits) = trial(circuit, &residue, bit, &denominator, need_difference);
        if quotient {
            result[index] = fits;
            if index == 0 {
                break;
            }
        }
        residue = candidate
            .into_iter()
            .zip(difference)
            .map(|(original, subtracted)| {
                let change = circuit.xor(original, subtracted);
                let selected = circuit.and(fits, change);
                circuit.xor(original, selected)
            })
            .collect();
    }
    if quotient { result } else { residue }
}

fn trial(
    circuit: &mut Circuit,
    residue: &[Bit],
    bit: Bit,
    divisor: &[Bit],
    need_difference: bool,
) -> (Word, Word, Bit) {
    let (&top, lower) = residue.split_last().expect("nonempty divisor");
    let candidate = std::iter::once(materialize(circuit, bit)).chain(lower.iter().copied()).collect::<Word>();
    let mut borrow = ZERO;
    let mut difference = Vec::new();
    for (&a, &b) in candidate.iter().zip(divisor) {
        let different = circuit.xor(a, b);
        if need_difference {
            difference.push(circuit.xor(different, borrow));
        }
        let generated = circuit.and(a.not(), b);
        let propagated = circuit.and(different.not(), borrow);
        borrow = circuit.xor(generated, propagated);
    }

    // The doubled residue has one extra high bit. Subtract if that bit is set
    // or if the remaining bits subtracted without a borrow.
    let fits = circuit.and(top.not(), borrow).not();
    (candidate, difference, fits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::gadget_tests::check;
    use crate::apply::identity::same_word;

    #[test]
    fn remainder_counts_and_clears_its_work_wires() {
        for (p, m) in [(3, 2), (8, 3), (8, 4), (12, 6), (16, 8), (128, 64)] {
            let width = p.max(m);
            let inputs = |start, count| {
                (start..start + count as u64)
                    .map(Bit::input)
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
    fn quotient_and_signed_counts_include_local_uncomputation() {
        for width in [1usize, 2, 4, 8, 64, 128] {
            for op in [BinOp::Udiv, BinOp::Sdiv, BinOp::Srem] {
                let input = |start| (start..start + width as u64).map(Bit::input).collect::<Word>();
                let mut circuit = Circuit::new(2 * width as u64);
                let result = calculate(&mut circuit, op, &input(0), &input(width as u64));
                let block = circuit.finish(0, "division").expect("valid gadget wires");

                let mut trial_gates = 3 * width * width;
                if op != BinOp::Srem {
                    trial_gates -= width; // The final quotient bit uses only the comparison.
                }
                let signs = if op == BinOp::Udiv { 0 } else { 3 * (2 * width).saturating_sub(3) };
                let toffolis = 2 * (trial_gates + signs);
                let min = 1 << (width - 1);
                let mask = u128::MAX >> (128 - width);
                let expected = if op == BinOp::Udiv {
                    min / mask
                } else if op == BinOp::Sdiv {
                    min
                } else {
                    0
                };
                check(&block, &result, &[(width, min), (width, mask)], expected, toffolis);
            }
        }
    }

    #[test]
    fn unsigned_quotient_counts_unequal_significant_widths() {
        let (p, m) = (3, 5);
        let dividend = (0..p as u64).map(Bit::input).chain([ZERO; 2]).collect::<Word>();
        let divisor = (p as u64..(p + m) as u64).map(Bit::input).collect::<Word>();
        let mut circuit = Circuit::new((p + m) as u64);
        let result = calculate(&mut circuit, BinOp::Udiv, &dividend, &divisor);
        let block = circuit.finish(0, "unequal-width quotient").expect("valid gadget wires");
        let toffolis = 6 * p * m - 2 * m;
        check(&block, &result, &[(p, 7), (m, 3)], 2, toffolis);
        check(&block, &result, &[(p, 7), (m, 31)], 0, toffolis);
    }

    #[test]
    fn fixed_quotients_preserve_rewiring_and_wrapping() {
        for (width, distance) in [(8, 3), (128, 65)] {
            let dividend = (0..width as u64).map(Bit::input).collect::<Word>();
            let mut circuit = Circuit::new(width as u64);
            let divisor = 1u128 << distance;
            let result = calculate(&mut circuit, BinOp::Udiv, &dividend, &constant(divisor, width));
            assert!(same_word(&result[..width - distance], &dividend[distance..]));
            assert!(result[width - distance..].iter().all(|bit| bit.is_zero()));
            assert_eq!(circuit.forward_len(), 0);
            let block = circuit.finish(0, "fixed quotient").expect("valid gadget wires");
            check(&block, &result, &[(width, divisor)], 1, 0);
        }

        for width in [1, 4, 64, 128] {
            let min = 1u128 << (width - 1);
            let dividend = constant(min, width);
            let divisor = constant(u128::MAX, width);
            assert!(same_word(&fold(BinOp::Sdiv, &dividend, &divisor).expect("identity"), &dividend));
            assert!(same_word(&fold(BinOp::Srem, &dividend, &divisor).expect("zero remainder"), &vec![ZERO; width]));
            assert!(fold(BinOp::Sdiv, &dividend, &constant(0, width)).is_none());
        }
    }

    #[test]
    fn zero_predicate_counts_and_clears_its_work_wires() {
        for width in [2, 3, 4, 6, 8, 64] {
            let input = (0..width as u64).map(Bit::input).collect::<Word>();
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
        let input = (0..8).map(Bit::input).collect::<Word>();
        let power = reduce(&mut circuit, &input, &constant(8, 8));
        assert!(same_word(&power[..3], &input[..3]));
        assert!(same_word(&power[3..], &[ZERO; 5]));
        assert_eq!(circuit.forward_len(), 0);

        let bounded = reduce(&mut circuit, &input, &constant(11, 8));
        assert!(same_word(&bounded[4..], &[ZERO; 4]));
        let block = circuit.finish(0, "bounded remainder").expect("valid wires");
        check(&block, &bounded, &[(8, 21)], 10, 6 * 8 * 4);
    }

    #[test]
    fn remainder_at_the_dividend_bound_only_clears_equality() {
        let input = (0..4).map(Bit::input).collect::<Word>();
        // A smaller modulus needs reduction even when the words are unequal.
        for (modulus, toffolis) in [(15, 7), (11, 6 * 4 * 4)] {
            let mut circuit = Circuit::new(4);
            let result = reduce(&mut circuit, &input, &constant(modulus, 4));
            let block = circuit.finish(0, "remainder at or below bound").expect("valid wires");
            for value in 0..16 {
                check(&block, &result, &[(4, value)], value % modulus, toffolis);
            }
        }
    }
}
