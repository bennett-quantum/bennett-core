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
    /// A register handle that names no allocated register: its register was freed, or no allocation made
    /// it (an `i64` handle is the number of the allocation that made it, from 1).
    DanglingRegister = 9 "dangling_register",
    /// One gate or `apply` names the same qubit or register twice; a gate's target among its controls.
    QubitAlias = 10 "qubit_alias",
    // Code 11 was `dirty_free`, removed on 2026-11-17 when `qfree` became the partial trace; it stays unused.
    /// `stackrestore` to a stack pointer below the current one or above the current frame's allocations.
    BadStackRestore = 12 "bad_stack_restore",
    /// A whole-run exit was reached inside a coherent region.
    ExitInRegion = 13 "exit_in_region",
    /// The exit operand's type differs from the selected entry's result type.
    ExitTypeMismatch = 14 "exit_type_mismatch",
    // Code 15 was `bad_rewind`, removed with `rewind` on 2026-11-17; codes are stable, so it stays unused.
    /// `longjmp` through a buffer that holds no jump record to a live frame of the current run (inside a
    /// coherent region: of the region).
    BadLongjmp = 16 "bad_longjmp",
    /// Ordinary code read or wrote a memory byte that holds qubits (a `load`, a `store`, a `setjmp` record
    /// written over it, a `longjmp` record read from it), whatever the qubits' values; inside an applied
    /// function too. Gates and measurements name such bytes; ordinary code does not.
    QubitMemoryAccess = 17 "qubit_memory_access",
    /// A rotation whose angle operand, read as the bits of an IEEE 754 binary64, is not finite (an
    /// infinity or a NaN): angles are finite `f64` values (`docs/DESIGN.md`, "The model").
    BadAngle = 18 "bad_angle",
    // Code 19 is `out_of_memory`, the C runtime's allocator stop, added with that work.
    /// `qalloc i64` with a width outside 1 to 2^32 - 1, the widths a register may have.
    BadWidth = 20 "bad_width",
    /// `qalloc i64` when the run has made 2^64 - 1 allocations, as many as an `i64` handle can number.
    OutOfHandles = 21 "out_of_handles",
    /// A qubit position at or past its register's width, given at run time.
    BadRange = 22 "bad_range",
    /// A gate or rotation code outside its set; the device's gate kinds are literals, so only a program that
    /// switches on a code reaches it, through `fault bad_gate`.
    BadGate = 23 "bad_gate",
    /// A list read from memory that cannot be read, or an argument of the handle interface that is malformed.
    BadArgument = 24 "bad_argument",
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
