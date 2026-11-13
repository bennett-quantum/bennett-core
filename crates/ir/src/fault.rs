//! Named faults: how an execution ends when it ends without a result.
//!
//! The numeric codes are stable. A coherent region XORs the code into its fault register, so programs
//! and tests depend on the numbers. They are defined here and nowhere else.

use std::fmt;

use crate::Width;

/// Declares the fault enum, its codes and its names from one table.
macro_rules! faults {
    ($($(#[doc = $doc:literal])* $variant:ident = $code:literal $name:literal,)*) => {
        /// A named fault: part of an execution's result, never a crash.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u8)]
        pub enum Fault { $($(#[doc = $doc])* $variant = $code,)* }

        impl Fault {
            /// Every fault, in code order.
            pub const ALL: &'static [Fault] = &[$(Fault::$variant,)*];

            /// The word naming this fault in the text form and in reports.
            pub fn name(self) -> &'static str {
                match self { $(Fault::$variant => $name,)* }
            }
        }
    };
}

faults! {
    /// The step budget ran out.
    Budget = 1 "budget",
    /// Integer division or remainder by zero.
    DivideByZero = 2 "divide_by_zero",
    /// A load or store outside memory.
    OutOfBounds = 3 "out_of_bounds",
    /// `call_indirect` through a value that is not the address of a function of the expected signature.
    BadIndirect = 4 "bad_indirect",
    /// Control reached code marked unreachable.
    Unreachable = 5 "unreachable",
    /// The stack (frames and `alloca`) would grow into the static data.
    StackOverflow = 6 "stack_overflow",
    /// An effect was reached inside a coherent region.
    EffectInRegion = 7 "effect_in_region",
    /// A quantum instruction was reached inside a coherent region.
    QuantumInRegion = 8 "quantum_in_region",
    /// A register handle was used after its register was freed.
    DanglingRegister = 9 "dangling_register",
    /// One gate or `apply` names the same qubit or register twice.
    QubitAlias = 10 "qubit_alias",
    /// `qfree` of a register that is not zero in every basis configuration.
    DirtyFree = 11 "dirty_free",
}

impl Fault {
    /// The width of every fault code. A fault register must be at least this wide.
    pub const CODE_WIDTH: Width = Width(4);

    /// The stable numeric code.
    pub fn code(self) -> u64 {
        self as u64
    }

    /// The fault with the given code, if any.
    pub fn from_code(code: u64) -> Option<Fault> {
        Fault::ALL.iter().copied().find(|fault| fault.code() == code)
    }

    /// The fault with the given name, if any.
    pub fn from_name(name: &str) -> Option<Fault> {
        Fault::ALL.iter().copied().find(|fault| fault.name() == name)
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct_nonzero_and_fit_the_code_width() {
        for (i, fault) in Fault::ALL.iter().enumerate() {
            assert_ne!(fault.code(), 0);
            assert!(fault.code() <= Fault::CODE_WIDTH.mask());
            assert_eq!(Fault::from_code(fault.code()), Some(*fault));
            assert_eq!(Fault::from_name(fault.name()), Some(*fault));
            assert!(Fault::ALL[..i].iter().all(|other| other.name() != fault.name()));
        }
    }
}
