//! The Bennett device program: one data structure, one text reader, one text writer and one validator.
//!
//! A [`Module`] comes from [`read`] or is built directly, is checked by [`validate`] into a
//! [`ValidModule`], and is printed by [`write`]. Every name is resolved to a dense index when the
//! module is read; nothing downstream looks anything up by name. [`load`] reads and validates in one go.

mod fault;
mod module;
mod ops;
mod reader;
mod types;
mod validate;
mod writer;

pub use fault::Fault;
pub use module::{
    Block, DataItem, DataSegment, Function, Inst, Module, Param, Qubit, Signature, SwitchCase, Target, Terminator,
};
pub use ops::{BinOp, CastOp, FixedGate, Gate, Predicate, Rotation};
pub use reader::{NameKind, ReadError, ReadErrorKind, read};
pub use types::{BlockId, Const, FuncId, Operand, Slot, Type, Width};
pub use validate::{Expected, Location, Problem, ValidModule, ValidationError, ValidationErrors, validate};
pub use writer::write;

use thiserror::Error;

/// Why text could not be loaded as a valid module.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LoadError {
    /// The text is not a module.
    #[error("{0}")]
    Read(#[from] ReadError),
    /// The module breaks the validator's rules.
    #[error("{0}")]
    Invalid(#[from] ValidationErrors),
}

/// Reads and validates a module.
pub fn load(text: &str) -> Result<ValidModule, LoadError> {
    Ok(validate(read(text)?)?)
}
