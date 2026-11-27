//! Errors of the validator.

use std::fmt;

use thiserror::Error;

use crate::Type;

/// Everything wrong with a module, one error per problem found.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))]
pub struct ValidationErrors(pub Vec<ValidationError>);

/// One problem and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{location}: {problem}")]
pub struct ValidationError {
    /// Where the problem is.
    pub location: Location,
    /// What the problem is.
    pub problem: Problem,
}

/// A place in a module. Instruction numbers count from 1 within their block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// The data segment with this index.
    Data(usize),
    /// A function as a whole.
    Function(String),
    /// A block as a whole, or its parameters.
    Block {
        /// The function.
        function: String,
        /// The block label.
        block: String,
    },
    /// An instruction.
    Inst {
        /// The function.
        function: String,
        /// The block label.
        block: String,
        /// The instruction's position in the block, from 1.
        number: usize,
    },
    /// A block's terminator.
    Terminator {
        /// The function.
        function: String,
        /// The block label.
        block: String,
    },
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Location::Data(index) => write!(f, "data segment {}", index + 1),
            Location::Function(function) => write!(f, "@{function}"),
            Location::Block { function, block } => write!(f, "@{function}, block {block}"),
            Location::Inst { function, block, number } => {
                write!(f, "@{function}, block {block}, instruction {number}")
            }
            Location::Terminator { function, block } => write!(f, "@{function}, block {block}, terminator"),
        }
    }
}

/// What an operand must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    /// Exactly this type.
    Type(Type),
    /// A register of any width.
    AnyRegister,
    /// A register of any width, or an `i64` handle.
    RegisterOrHandle,
    /// A register of at least this many qubits.
    RegisterOfAtLeast(u32),
    /// An integer or a register of this width.
    IntOrRegister(u32),
}

impl fmt::Display for Expected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expected::Type(ty) => write!(f, "{ty}"),
            Expected::AnyRegister => write!(f, "a register"),
            Expected::RegisterOrHandle => write!(f, "a register or an i64 handle"),
            Expected::RegisterOfAtLeast(bits) => write!(f, "a register of at least {bits} qubits"),
            Expected::IntOrRegister(bits) => write!(f, "i{bits} or q{bits}"),
        }
    }
}

/// A problem the validator found.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Problem {
    /// A whole-run exit needs an integer status.
    #[error("exit requires an integer operand")]
    ExitOperand,
    /// A data segment reaching past the end of memory.
    #[error("{len} bytes at address {address} reach past the end of memory ({memory_size} bytes)")]
    DataOutOfBounds {
        /// The segment's address.
        address: u64,
        /// The segment's length.
        len: u64,
        /// The memory size.
        memory_size: u64,
    },
    /// A data segment whose items add up to more bytes than a `u64` counts.
    #[error("the items add up to more than 2^64 - 1 bytes")]
    DataTooLong,
    /// Two data segments sharing bytes.
    #[error("overlaps data segment {}", .0 + 1)]
    DataOverlap(usize),
    /// A function address stored in fewer bits than it needs.
    #[error("the address of @{function} does not fit in i{bits}")]
    AddressTooWide {
        /// The function.
        function: String,
        /// The width it is stored at.
        bits: u32,
    },
    /// A function index past the end of the module.
    #[error("there is no function {0}")]
    UnknownFunction(u32),
    /// Two functions with one name.
    #[error("another function has the same name")]
    DuplicateFunction,
    /// A function without blocks.
    #[error("a function needs at least one block")]
    NoBlocks,
    /// An entry block with parameters.
    #[error("the entry block cannot take parameters")]
    EntryParams,
    /// A block index past the end of the function.
    #[error("there is no block {0}")]
    UnknownBlock(u32),
    /// A slot index past the end of the frame.
    #[error("there is no slot {0}")]
    UnknownSlot(u32),
    /// A slot defined twice.
    #[error("`%{0}` is defined more than once")]
    Redefined(String),
    /// A slot used but never defined.
    #[error("`%{0}` is used but never defined")]
    Undefined(String),
    /// A use that its definition does not dominate.
    #[error("`%{0}` is used where its definition does not dominate")]
    NotDominated(String),
    /// An operand of the wrong type.
    #[error("{operand} is {found}, expected {expected}")]
    TypeMismatch {
        /// The operand: a `%name` or a constant.
        operand: String,
        /// What it must be.
        expected: Expected,
        /// What it is.
        found: Type,
    },
    /// A call or branch with the wrong number of arguments.
    #[error("expected {expected} arguments, found {found}")]
    ArgumentCount {
        /// The number of parameters.
        expected: usize,
        /// The number of arguments.
        found: usize,
    },
    /// A call result named although the callee returns nothing.
    #[error("the callee returns no value to name")]
    NoResult,
    /// A return that does not match the function's result type.
    #[error("`ret` must return exactly the function's result type")]
    ReturnMismatch,
    /// A qubit index outside its register.
    #[error("qubit {bit} is outside a register of {bits} qubits")]
    QubitOutOfRange {
        /// The index.
        bit: u64,
        /// The register width.
        bits: u32,
    },
    /// The same qubit twice in one gate.
    #[error("`%{0}[{1}]` appears twice in one gate")]
    RepeatedQubit(String, u32),
    /// A non-finite rotation angle.
    #[error("the rotation angle is not finite")]
    AngleNotFinite,
    /// A cast that does not widen (`zext`, `sext`) or narrow (`trunc`).
    #[error("`{op}` cannot convert i{from} to i{to}")]
    CastWidths {
        /// The cast mnemonic.
        op: &'static str,
        /// The operand width.
        from: u32,
        /// The result width.
        to: u32,
    },
    /// A switch case outside the switch's width, or repeated.
    #[error("case {0} is repeated or does not fit the switch's width")]
    BadCase(u128),
    /// `measure` of a whole register wider than an integer: copy at most 128 of its qubits into a fresh
    /// register with controlled X and measure that (`docs/DESIGN.md`, "The device program").
    #[error(
        "`measure` of a whole register of {0} qubits gives an integer of at most 128 bits; copy at most 128 of \
         its qubits into a fresh register with controlled X and measure that"
    )]
    MeasureTooWide(u32),
    /// An `apply` argument or output register wider than an integer: copy at most 128 of its qubits into a
    /// fresh register with controlled X and apply to that.
    #[error(
        "`%{0}`, a register of {1} qubits, cannot be an `apply` argument or output: an applied function's \
         parameters and result are integers of at most 128 bits"
    )]
    RegionOperandTooWide(String, u32),
    /// An `apply` callee that does not return an integer.
    #[error("a coherent region must return an integer")]
    RegionResult,
    /// An `apply` callee with a non-integer parameter.
    #[error("a coherent region takes integer parameters only")]
    RegionParams,
    /// A register that is both an `apply` target and an argument, or both targets.
    #[error("`%{0}` is used twice among the arguments, output and fault register")]
    RegionAlias(String),
    /// A memory qubit's bit index at or above `MEMORY_QUBIT_BITS`.
    #[error("bit {0} of a memory qubit is not below 128")]
    MemoryBitOutOfRange(u8),
    /// The same memory qubit, by the same address operand and bit, twice in one gate.
    #[error("the memory qubit at bit {0} of one address appears twice in one gate")]
    RepeatedMemoryQubit(u8),
    /// A `GateAt` whose qubits are all register qubits at literal positions below 2^32: it is written, and
    /// read back, as a `Gate`. A `GateAt` has a qubit in memory or a position given as an `i64` value.
    #[error("a gate with every qubit in a register at a literal position is a `Gate`, not a `GateAt`")]
    GateAtWithoutMemory,
}
