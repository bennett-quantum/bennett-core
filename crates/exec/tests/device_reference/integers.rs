//! Integer rules from the device language and the definite apply step specification.

use bennett_ir::{BinOp, Fault, Predicate, Width};

pub fn signed(value: u128, width: Width) -> i128 {
    let padding = 128 - width.bits();
    ((value << padding) as i128) >> padding
}

pub fn binary(op: BinOp, width: Width, a: u128, b: u128) -> Result<u128, Fault> {
    let signed_a = || signed(a, width);
    let signed_b = || signed(b, width);
    let value = match op {
        BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem if b == 0 => {
            return Err(Fault::DivideByZero);
        }
        BinOp::Shl | BinOp::Lshr | BinOp::Ashr if b >= u128::from(width.bits()) => 0,
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::Mul => a.wrapping_mul(b),
        BinOp::Udiv => a / b,
        BinOp::Urem => a % b,
        BinOp::Sdiv => signed_a().wrapping_div(signed_b()) as u128,
        BinOp::Srem => signed_a().wrapping_rem(signed_b()) as u128,
        BinOp::And => a & b,
        BinOp::Or => a | b,
        BinOp::Xor => a ^ b,
        BinOp::Shl => a << b,
        BinOp::Lshr => a >> b,
        BinOp::Ashr => (signed_a() >> b) as u128,
    };
    Ok(value & width.mask())
}

pub fn compare(pred: Predicate, width: Width, a: u128, b: u128) -> u128 {
    let signed_order = || signed(a, width).cmp(&signed(b, width));
    u128::from(match pred {
        Predicate::Eq => a == b,
        Predicate::Ne => a != b,
        Predicate::Ult => a < b,
        Predicate::Ule => a <= b,
        Predicate::Ugt => a > b,
        Predicate::Uge => a >= b,
        Predicate::Slt => signed_order().is_lt(),
        Predicate::Sle => signed_order().is_le(),
        Predicate::Sgt => signed_order().is_gt(),
        Predicate::Sge => signed_order().is_ge(),
    })
}
