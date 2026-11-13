//! Integer semantics. Values are `u64` holding the canonical (zero-extended) bits of their width.

use bennett_ir::{BinOp, CastOp, Fault, Predicate, Width};

/// The value of `bits` read as a signed integer of `width`.
fn signed(width: Width, bits: u64) -> i64 {
    let unused = 64 - width.bits();
    // Moving the sign bit to the top and shifting back arithmetically sign-extends.
    ((bits << unused) as i64) >> unused
}

/// `lhs op rhs` at `width`; total except for division and remainder by zero.
pub(crate) fn binary(op: BinOp, width: Width, lhs: u64, rhs: u64) -> Result<u64, Fault> {
    let shift_in_range = rhs < u64::from(width.bits());
    let result = match op {
        BinOp::Add => lhs.wrapping_add(rhs),
        BinOp::Sub => lhs.wrapping_sub(rhs),
        BinOp::Mul => lhs.wrapping_mul(rhs),
        BinOp::Udiv => lhs.checked_div(rhs).ok_or(Fault::DivideByZero)?,
        BinOp::Urem => lhs.checked_rem(rhs).ok_or(Fault::DivideByZero)?,
        BinOp::Sdiv | BinOp::Srem if rhs == 0 => return Err(Fault::DivideByZero),
        BinOp::Sdiv => signed(width, lhs).wrapping_div(signed(width, rhs)) as u64,
        BinOp::Srem => signed(width, lhs).wrapping_rem(signed(width, rhs)) as u64,
        BinOp::And => lhs & rhs,
        BinOp::Or => lhs | rhs,
        BinOp::Xor => lhs ^ rhs,
        BinOp::Shl | BinOp::Lshr | BinOp::Ashr if !shift_in_range => 0,
        BinOp::Shl => lhs << rhs,
        BinOp::Lshr => lhs >> rhs,
        BinOp::Ashr => (signed(width, lhs) >> rhs) as u64,
    };
    Ok(result & width.mask())
}

/// Whether `lhs pred rhs` holds at `width`.
pub(crate) fn compare(pred: Predicate, width: Width, lhs: u64, rhs: u64) -> bool {
    let (slhs, srhs) = (signed(width, lhs), signed(width, rhs));
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
pub(crate) fn cast(op: CastOp, from: Width, to: Width, value: u64) -> u64 {
    match op {
        CastOp::Zext => value,
        CastOp::Sext => signed(from, value) as u64 & to.mask(),
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
        assert_eq!(binary(BinOp::Lshr, w(64), u64::MAX, 64), Ok(0));
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
