//! Widths, value types, dense indices and operands.

use std::fmt;

/// The bit width of an integer value or a quantum register: 1 to 64 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Width(pub(crate) u8);

impl Width {
    /// One bit: the width of branch conditions and comparison results.
    pub const BIT: Width = Width(1);
    /// Eight bits: the width of a console byte.
    pub const BYTE: Width = Width(8);
    /// Sixty-four bits: the width of addresses, allocation sizes and budgets.
    pub const WORD: Width = Width(64);

    /// The width of `bits` bits, or `None` outside 1 to 64.
    pub const fn new(bits: u32) -> Option<Width> {
        if matches!(bits, 1..=64) { Some(Width(bits as u8)) } else { None }
    }

    /// The number of bits.
    pub const fn bits(self) -> u32 {
        self.0 as u32
    }

    /// All ones in the low `bits()` bits; every value of this width is at most this.
    pub const fn mask(self) -> u64 {
        u64::MAX >> (64 - self.0 as u32)
    }

    /// The number of bytes a value of this width occupies in memory.
    pub const fn bytes(self) -> u64 {
        (self.0 as u64).div_ceil(8)
    }
}

/// What a value is: an integer, or a handle naming a quantum register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// An integer of the given width. Pointers are `i64` addresses.
    Int(Width),
    /// A handle to a quantum register of the given number of qubits.
    Qreg(Width),
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int(width) => write!(f, "i{}", width.bits()),
            Type::Qreg(width) => write!(f, "q{}", width.bits()),
        }
    }
}

macro_rules! dense_index {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            /// The index, for indexing the vector it refers into.
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

dense_index!(
    /// A value slot of a function's frame. Every parameter, block parameter and instruction result has
    /// its own slot, numbered densely per function.
    Slot
);
dense_index!(
    /// A block of a function. Block 0 is the entry block.
    BlockId
);
dense_index!(
    /// A function of a module.
    FuncId
);

/// An integer constant, always stored in canonical form: zero-extended from its width.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Const {
    width: Width,
    value: u64,
}

impl Const {
    /// The constant `value` of `width`, if `value` fits in `width` bits.
    pub fn new(width: Width, value: u64) -> Option<Const> {
        (value <= width.mask()).then_some(Const { width, value })
    }

    /// The constant a source literal denotes at `width`: either an unsigned value below `2^bits` or a
    /// negative value of at least `-2^(bits-1)`, stored as its two's complement.
    pub fn from_literal(width: Width, literal: i128) -> Option<Const> {
        let unsigned_max = i128::from(width.mask());
        let signed_min = -(1i128 << (width.bits() - 1));
        if (signed_min..=unsigned_max).contains(&literal) {
            // Truncating to the low 64 bits of the two's complement, then masking, is the intent.
            let value = (literal as u64) & width.mask();
            Some(Const { width, value })
        } else {
            None
        }
    }

    /// The width of the constant.
    pub fn width(self) -> Width {
        self.width
    }

    /// The value, zero-extended to 64 bits.
    pub fn value(self) -> u64 {
        self.value
    }
}

/// An instruction input: a slot of the current frame or a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operand {
    /// The value in a slot.
    Slot(Slot),
    /// A constant.
    Const(Const),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_outside_one_to_sixty_four_are_rejected() {
        assert_eq!(Width::new(0), None);
        assert_eq!(Width::new(65), None);
        assert_eq!(Width::new(64).map(Width::mask), Some(u64::MAX));
        assert_eq!(Width::new(1).map(Width::bytes), Some(1));
        assert_eq!(Width::new(9).map(Width::bytes), Some(2));
    }

    #[test]
    fn literals_accept_signed_and_unsigned_ranges() {
        let byte = Width::BYTE;
        assert_eq!(Const::from_literal(byte, 255).map(Const::value), Some(255));
        assert_eq!(Const::from_literal(byte, -1).map(Const::value), Some(255));
        assert_eq!(Const::from_literal(byte, -128).map(Const::value), Some(128));
        assert_eq!(Const::from_literal(byte, 256), None);
        assert_eq!(Const::from_literal(byte, -129), None);
        assert_eq!(Const::from_literal(Width::WORD, -1).map(Const::value), Some(u64::MAX));
        assert_eq!(Const::from_literal(Width::BIT, -1).map(Const::value), Some(1));
    }
}
