//! Truncated unsigned products by reducing the columns of partial products.

use super::bits::{Bit, Circuit, Word, ZERO};
use super::ranges::{product_bits, unsigned_max};

pub(super) fn multiply(circuit: &mut Circuit, left: &[Bit], right: &[Bit]) -> Word {
    let left_max = unsigned_max(left);
    let right_max = unsigned_max(right);
    let width = product_bits(left_max, right_max, left.len());
    let mut result = circuit.clean_word(width, |circuit| {
        let mut columns = vec![Vec::new(); width];
        for (i, &a) in left.iter().take(width).enumerate() {
            for (j, &b) in right.iter().take(width - i).enumerate() {
                if !a.is_zero() && !b.is_zero() {
                    columns[i + j].push(circuit.and(a, b));
                }
            }
        }

        let mut word = Vec::with_capacity(width);
        for index in 0..width {
            let (current, rest) = columns[index..].split_at_mut(1);
            let column = &mut current[0];
            let mut next = rest.first_mut();
            while column.len() >= 2 {
                let a = column.pop().expect("two bits remain");
                let b = column.pop().expect("two bits remain");
                let different = circuit.xor(a, b);
                if let Some(c) = column.pop() {
                    column.push(circuit.xor(different, c));
                    if let Some(higher) = &mut next {
                        let both = circuit.and(a, b);
                        let propagated = circuit.and(different, c);
                        higher.push(circuit.xor(both, propagated));
                    }
                } else {
                    column.push(different);
                    if let Some(higher) = &mut next {
                        higher.push(circuit.and(a, b));
                    }
                }
            }
            word.push(column.pop().unwrap_or(ZERO));
        }
        word
    });
    result.resize(left.len(), ZERO);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::gadget_tests::check;

    // Independent column-height recurrence for disjoint, fully variable inputs.
    fn expected_toffolis(left: usize, right: usize, width: usize) -> usize {
        let mut incoming = 0;
        let mut count = 0;
        for column in 0..width {
            let partial = (column + 1).min(left).saturating_sub((column + 1).saturating_sub(right));
            count += partial;
            let height = partial + incoming;
            let full = height.saturating_sub(1) / 2;
            let half = usize::from(height != 0 && height.is_multiple_of(2));
            if column + 1 < width {
                count += 2 * full + half;
            }
            incoming = full + half;
        }
        2 * count
    }

    #[test]
    fn product_counts_and_clears_its_work_wires() {
        for (left, right, width) in [(1, 1, 2), (4, 4, 4), (4, 4, 8), (6, 3, 9), (6, 6, 12), (8, 8, 16), (64, 64, 128)]
        {
            let args = (left + right) as u64;
            let input = |start: u64, n| {
                (start..start + n as u64).map(Bit::input).chain(std::iter::repeat(ZERO)).take(width).collect::<Word>()
            };
            let mut circuit = Circuit::new(args);
            let result = multiply(&mut circuit, &input(0, left), &input(left as u64, right));
            let block = circuit.finish(0, "product").expect("valid gadget wires");
            let x = (1u128 << left) - 1;
            let y = (1u128 << right) - 1;
            let expected = (x * y) & (u128::MAX >> (128 - width));
            check(&block, &result, &[(left, x), (right, y)], expected, expected_toffolis(left, right, width));
        }
    }
}
