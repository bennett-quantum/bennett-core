//! Integer rules from the device language and the definite apply step specification.

use bennett_ir::{BinOp, Fault, Predicate, Width};

pub fn mask(width: Width) -> u64 {
    assert!(width.bits() <= 64, "wide integers are outside this reference");
    width.mask() as u64
}

pub fn signed(value: u64, width: Width) -> i128 {
    let sign = 1_u64 << (width.bits() - 1);
    i128::from(value ^ sign) - i128::from(sign)
}

pub fn binary(op: BinOp, width: Width, a: u64, b: u64) -> Result<u64, Fault> {
    let signed_a = || signed(a, width);
    let signed_b = || signed(b, width);
    let value = match op {
        BinOp::Udiv | BinOp::Sdiv | BinOp::Urem | BinOp::Srem if b == 0 => {
            return Err(Fault::DivideByZero);
        }
        BinOp::Shl | BinOp::Lshr | BinOp::Ashr if b >= u64::from(width.bits()) => 0,
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::Mul => a.wrapping_mul(b),
        BinOp::Udiv => a / b,
        BinOp::Urem => a % b,
        BinOp::Sdiv => (signed_a() / signed_b()) as u64,
        BinOp::Srem => (signed_a() % signed_b()) as u64,
        BinOp::And => a & b,
        BinOp::Or => a | b,
        BinOp::Xor => a ^ b,
        BinOp::Shl => a << b,
        BinOp::Lshr => a >> b,
        BinOp::Ashr => (signed_a() >> b) as u64,
    };
    Ok(value & mask(width))
}

pub fn compare(pred: Predicate, width: Width, a: u64, b: u64) -> u64 {
    let signed_order = || signed(a, width).cmp(&signed(b, width));
    u64::from(match pred {
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
