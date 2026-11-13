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
            Expected::RegisterOfAtLeast(bits) => write!(f, "a register of at least {bits} qubits"),
            Expected::IntOrRegister(bits) => write!(f, "i{bits} or q{bits}"),
        }
    }
}

/// A problem the validator found.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Problem {
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
        bit: u8,
        /// The register width.
        bits: u32,
    },
    /// The same qubit twice in one gate.
    #[error("`%{0}[{1}]` appears twice in one gate")]
    RepeatedQubit(String, u8),
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
    BadCase(u64),
    /// An `apply` callee that does not return an integer.
    #[error("a coherent region must return an integer")]
    RegionResult,
    /// An `apply` callee with a non-integer parameter.
    #[error("a coherent region takes integer parameters only")]
    RegionParams,
    /// A register that is both an `apply` target and an argument, or both targets.
    #[error("`%{0}` is used twice among the arguments, output and fault register")]
    RegionAlias(String),
}
