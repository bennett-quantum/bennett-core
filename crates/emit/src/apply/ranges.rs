//! Unsigned bounds derived from definite bits in a little-endian word.

use super::bits::Bit;

pub(super) fn unsigned_max(word: &[Bit]) -> u128 {
    word.iter().enumerate().fold(0, |maximum, (index, &bit)| maximum | (u128::from(!bit.is_zero()) << index))
}

pub(super) fn bits(maximum: u128) -> usize {
    (u128::BITS - maximum.leading_zeros()) as usize
}

pub(super) fn product_bits(left: u128, right: u128, width: usize) -> usize {
    left.checked_mul(right).map_or(width, |maximum| bits(maximum).min(width))
}
