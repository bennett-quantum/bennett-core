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
    /// `call_indirect` through a value that is not the address of a function of the expected signature: a
    /// call through a value that names no valid target. (An invalid jump record is `bad_longjmp`.)
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
    /// `stackrestore` to a stack pointer below the current one or above the current frame's allocations.
    BadStackRestore = 12 "bad_stack_restore",
    /// A whole-run exit was reached inside a coherent region.
    ExitInRegion = 13 "exit_in_region",
    /// The exit operand's type differs from the selected entry's result type.
    ExitTypeMismatch = 14 "exit_type_mismatch",
    /// A `rewind` that cannot un-run: its handle is not a mark of the running function's activation, the
    /// history since the mark holds a measurement, a kept value goes into a register allocated since, or
    /// the un-run meets a state its history does not describe (a register read as an integer since the
    /// mark that no longer holds the value read, a register allocated since that is not zero).
    BadRewind = 15 "bad_rewind",
    /// `longjmp` through a buffer that holds no jump record to a live frame of the current run (inside a
    /// coherent region: of the region), or whose record is held in qubits that disagree on it.
    BadLongjmp = 16 "bad_longjmp",
}

impl Fault {
    /// The width of every fault code. A fault register must be at least this wide.
    pub const CODE_WIDTH: Width = Width(5);

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
            assert!(u128::from(fault.code()) <= Fault::CODE_WIDTH.mask());
            assert_eq!(Fault::from_code(fault.code()), Some(*fault));
            assert_eq!(Fault::from_name(fault.name()), Some(*fault));
            assert!(Fault::ALL[..i].iter().all(|other| other.name() != fault.name()));
        }
    }

    /// The decision of 2026-11-16: codes are 5 bits wide and an invalid jump record has a code of its own,
    /// one that a 4-bit register could not hold.
    #[test]
    fn an_invalid_jump_record_has_its_own_five_bit_code() {
        assert_eq!(Fault::CODE_WIDTH, Width(5));
        assert_eq!(Fault::from_name("bad_longjmp"), Some(Fault::BadLongjmp));
        assert_eq!(Fault::BadLongjmp.code(), 16);
        assert_ne!(Fault::BadLongjmp, Fault::BadIndirect);
        assert!(u128::from(Fault::BadLongjmp.code()) > Width(4).mask());
    }
}
