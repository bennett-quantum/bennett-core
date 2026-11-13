//! The fixed vocabularies of the instruction set, each variant with its text mnemonic.

/// Declares a field-less enum together with its mnemonics, so that each name is written once.
macro_rules! mnemonic_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($(#[$vmeta])* $variant,)* }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant,)*];

            /// The word naming this variant in the text form.
            pub fn mnemonic(self) -> &'static str {
                match self { $($name::$variant => $text,)* }
            }

            /// The variant named by `text`, if any.
            pub fn from_mnemonic(text: &str) -> Option<$name> {
                $name::ALL.iter().copied().find(|v| v.mnemonic() == text)
            }
        }
    };
}

mnemonic_enum! {
    /// A two-operand integer operation. Arithmetic wraps at the width; division or remainder by zero
    /// faults; a shift by at least the width gives 0.
    BinOp {
        /// Wrapping addition.
        Add = "add",
        /// Wrapping subtraction.
        Sub = "sub",
        /// Wrapping multiplication.
        Mul = "mul",
        /// Unsigned division.
        Udiv = "udiv",
        /// Signed division, truncating; `MIN / -1` wraps to `MIN`.
        Sdiv = "sdiv",
        /// Unsigned remainder.
        Urem = "urem",
        /// Signed remainder, with the sign of the dividend; `MIN % -1` is 0.
        Srem = "srem",
        /// Bitwise and.
        And = "and",
        /// Bitwise or.
        Or = "or",
        /// Bitwise exclusive or.
        Xor = "xor",
        /// Shift left.
        Shl = "shl",
        /// Logical shift right.
        Lshr = "lshr",
        /// Arithmetic shift right.
        Ashr = "ashr",
    }
}

mnemonic_enum! {
    /// An integer comparison; the result is an `i1`.
    Predicate {
        /// Equal.
        Eq = "eq",
        /// Not equal.
        Ne = "ne",
        /// Unsigned less than.
        Ult = "ult",
        /// Unsigned less than or equal.
        Ule = "ule",
        /// Unsigned greater than.
        Ugt = "ugt",
        /// Unsigned greater than or equal.
        Uge = "uge",
        /// Signed less than.
        Slt = "slt",
        /// Signed less than or equal.
        Sle = "sle",
        /// Signed greater than.
        Sgt = "sgt",
        /// Signed greater than or equal.
        Sge = "sge",
    }
}

mnemonic_enum! {
    /// A change of integer width.
    CastOp {
        /// Zero-extend to a wider type.
        Zext = "zext",
        /// Sign-extend to a wider type.
        Sext = "sext",
        /// Keep the low bits, to a narrower type.
        Trunc = "trunc",
    }
}

mnemonic_enum! {
    /// A single-qubit gate without parameters.
    FixedGate {
        /// Pauli X (NOT).
        X = "x",
        /// Hadamard.
        H = "h",
        /// Pauli Z.
        Z = "z",
        /// Phase gate, `diag(1, i)`.
        S = "s",
        /// Inverse phase gate, `diag(1, -i)`.
        Sdg = "sdg",
        /// T gate, `diag(1, e^{iπ/4})`.
        T = "t",
        /// Inverse T gate, `diag(1, e^{-iπ/4})`.
        Tdg = "tdg",
    }
}

mnemonic_enum! {
    /// The axis of a single-qubit rotation `exp(-iθσ/2)`.
    Rotation {
        /// Rotation about X.
        Rx = "rx",
        /// Rotation about Y.
        Ry = "ry",
        /// Rotation about Z.
        Rz = "rz",
    }
}

/// A single-qubit gate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gate {
    /// A gate without parameters.
    Fixed(FixedGate),
    /// A rotation by an angle in radians.
    Rotate(Rotation, f64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonics_round_trip() {
        for &op in BinOp::ALL {
            assert_eq!(BinOp::from_mnemonic(op.mnemonic()), Some(op));
        }
        for &gate in FixedGate::ALL {
            assert_eq!(FixedGate::from_mnemonic(gate.mnemonic()), Some(gate));
        }
        assert_eq!(Predicate::from_mnemonic("lt"), None);
    }
}
