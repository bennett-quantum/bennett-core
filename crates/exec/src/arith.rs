//! Integer semantics. Values are `u128` holding the canonical (zero-extended) bits of their width. Widths
//! up to 64 bits are computed in `u64`, which is what nearly every value is, and gives the same bits.

use bennett_ir::{BinOp, CastOp, Fault, Predicate, Width};

/// The integer operations, at one machine width: `u64` or `u128`.
trait Machine: Copy + Eq + Ord {
    type Signed: Copy + Ord;
    fn wide(self) -> u128;
    fn signed(self, width: Width) -> Self::Signed;
    fn unsigned(value: Self::Signed) -> Self;
    fn binary(op: BinOp, width: Width, lhs: Self, rhs: Self) -> Result<Self, Fault>;
}

macro_rules! machine {
    ($unsigned:ty, $signed:ty) => {
        impl Machine for $unsigned {
            type Signed = $signed;

            fn wide(self) -> u128 {
                self as u128
            }

            /// The value read as a signed integer of `width`.
            fn signed(self, width: Width) -> $signed {
                let unused = <$unsigned>::BITS - width.bits();
                // Moving the sign bit to the top and shifting back arithmetically sign-extends.
                ((self << unused) as $signed) >> unused
            }

            fn unsigned(value: $signed) -> Self {
                value as $unsigned
            }

            fn binary(op: BinOp, width: Width, lhs: Self, rhs: Self) -> Result<Self, Fault> {
                let shift_in_range = rhs < width.bits() as $unsigned;
                Ok(match op {
                    BinOp::Add => lhs.wrapping_add(rhs),
                    BinOp::Sub => lhs.wrapping_sub(rhs),
                    BinOp::Mul => lhs.wrapping_mul(rhs),
                    BinOp::Udiv => lhs.checked_div(rhs).ok_or(Fault::DivideByZero)?,
                    BinOp::Urem => lhs.checked_rem(rhs).ok_or(Fault::DivideByZero)?,
                    BinOp::Sdiv | BinOp::Srem if rhs == 0 => return Err(Fault::DivideByZero),
                    BinOp::Sdiv => lhs.signed(width).wrapping_div(rhs.signed(width)) as $unsigned,
                    BinOp::Srem => lhs.signed(width).wrapping_rem(rhs.signed(width)) as $unsigned,
                    BinOp::And => lhs & rhs,
                    BinOp::Or => lhs | rhs,
                    BinOp::Xor => lhs ^ rhs,
                    BinOp::Shl | BinOp::Lshr | BinOp::Ashr if !shift_in_range => 0,
                    BinOp::Shl => lhs << rhs,
                    BinOp::Lshr => lhs >> rhs,
                    BinOp::Ashr => (lhs.signed(width) >> rhs) as $unsigned,
                })
            }
        }
    };
}

machine!(u64, i64);
machine!(u128, i128);

/// `lhs op rhs` at `width`; total except for division and remainder by zero.
pub(crate) fn binary(op: BinOp, width: Width, lhs: u128, rhs: u128) -> Result<u128, Fault> {
    let result = if width.bits() <= u64::BITS {
        <u64 as Machine>::binary(op, width, lhs as u64, rhs as u64)?.wide()
    } else {
        <u128 as Machine>::binary(op, width, lhs, rhs)?
    };
    Ok(result & width.mask())
}

/// Whether `lhs pred rhs` holds at `width`.
pub(crate) fn compare(pred: Predicate, width: Width, lhs: u128, rhs: u128) -> bool {
    if width.bits() <= u64::BITS {
        compare_at(pred, width, lhs as u64, rhs as u64)
    } else {
        compare_at(pred, width, lhs, rhs)
    }
}

fn compare_at<M: Machine>(pred: Predicate, width: Width, lhs: M, rhs: M) -> bool {
    let (slhs, srhs) = (lhs.signed(width), rhs.signed(width));
    match pred {
        Predicate::Eq => lhs == rhs,
        Predicate::Ne => lhs != rhs,
        Predicate::Ult => lhs < rhs,
        Predicate::Ule => lhs <= rhs,
        Predicate::Ugt => lhs > rhs,
        Predicate::Uge => lhs >= rhs,
        Predicate::Slt => slhs < srhs,
        Predicate::Sle => slhs <= srhs,
        Predicate::Sgt => slhs > srhs,
        Predicate::Sge => slhs >= srhs,
    }
}

/// `value` converted from width `from` to width `to`.
pub(crate) fn cast(op: CastOp, from: Width, to: Width, value: u128) -> u128 {
    match op {
        CastOp::Zext => value,
        CastOp::Sext => u128::unsigned(value.signed(from)) & to.mask(),
        CastOp::Trunc => value & to.mask(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(bits: u32) -> Width {
        Width::new(bits).unwrap_or(Width::WORD)
    }

    #[test]
    fn arithmetic_wraps_at_the_width() {
        assert_eq!(binary(BinOp::Add, w(8), 255, 1), Ok(0));
        assert_eq!(binary(BinOp::Sub, w(8), 0, 1), Ok(255));
        assert_eq!(binary(BinOp::Mul, w(4), 5, 5), Ok(9));
    }

    #[test]
    fn division_by_zero_faults_and_signed_overflow_wraps() {
        for op in [BinOp::Udiv, BinOp::Urem, BinOp::Sdiv, BinOp::Srem] {
            assert_eq!(binary(op, w(8), 7, 0), Err(Fault::DivideByZero));
        }
        assert_eq!(binary(BinOp::Sdiv, w(8), 0x80, 0xff), Ok(0x80));
        assert_eq!(binary(BinOp::Srem, w(8), 0x80, 0xff), Ok(0));
        assert_eq!(binary(BinOp::Sdiv, w(8), 0xf9, 2), Ok(0xfd)); // -7 / 2 = -3
        assert_eq!(binary(BinOp::Srem, w(8), 0xf9, 2), Ok(0xff)); // -7 % 2 = -1
    }

    #[test]
    fn shifts_by_the_width_or_more_give_zero() {
        assert_eq!(binary(BinOp::Shl, w(8), 1, 7), Ok(0x80));
        assert_eq!(binary(BinOp::Shl, w(8), 1, 8), Ok(0));
        assert_eq!(binary(BinOp::Ashr, w(8), 0x80, 7), Ok(0xff));
        assert_eq!(binary(BinOp::Ashr, w(8), 0x80, 8), Ok(0));
        assert_eq!(binary(BinOp::Lshr, w(64), u128::from(u64::MAX), 64), Ok(0));
    }

    #[test]
    fn widths_beyond_sixty_four_bits_compute_exactly() {
        let max = u128::MAX;
        assert_eq!(binary(BinOp::Mul, w(128), u128::from(u64::MAX), u128::from(u64::MAX)), Ok(max - (1 << 65) + 2));
        assert_eq!(binary(BinOp::Add, w(65), 1 << 64, 1 << 64), Ok(0));
        assert_eq!(binary(BinOp::Sub, w(72), 0, 1), Ok((1 << 72) - 1));
        assert_eq!(binary(BinOp::Sdiv, w(128), 1 << 127, max), Ok(1 << 127));
        assert_eq!(binary(BinOp::Sdiv, w(128), max - 6, 2), Ok(max - 2)); // -7 / 2 = -3
        assert_eq!(binary(BinOp::Udiv, w(128), max, 1 << 64), Ok(u128::from(u64::MAX)));
        assert_eq!(binary(BinOp::Ashr, w(100), 1 << 99, 99), Ok((1 << 100) - 1));
        assert_eq!(binary(BinOp::Shl, w(128), 1, 128), Ok(0));
        assert_eq!(binary(BinOp::Lshr, w(65), 1 << 64, 64), Ok(1));
        assert!(compare(Predicate::Slt, w(128), 1 << 127, 0));
        assert!(compare(Predicate::Ugt, w(128), 1 << 127, 0));
        assert!(compare(Predicate::Sgt, w(65), (1 << 64) - 1, 1 << 64));
        assert_eq!(cast(CastOp::Sext, w(64), w(128), u128::from(u64::MAX)), max);
        assert_eq!(cast(CastOp::Sext, w(65), w(128), 1 << 64), max << 64);
        assert_eq!(cast(CastOp::Trunc, w(128), w(64), max), u128::from(u64::MAX));
    }

    #[test]
    fn comparisons_and_casts_respect_sign() {
        assert!(compare(Predicate::Slt, w(8), 0xff, 0));
        assert!(!compare(Predicate::Ult, w(8), 0xff, 0));
        assert_eq!(cast(CastOp::Sext, w(4), w(8), 0b1000), 0xf8);
        assert_eq!(cast(CastOp::Zext, w(4), w(8), 0b1000), 0b1000);
        assert_eq!(cast(CastOp::Trunc, w(16), w(8), 0x1234), 0x34);
    }
}
