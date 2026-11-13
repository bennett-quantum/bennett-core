//! Errors of the text reader.

use std::fmt;

use thiserror::Error;

use crate::Type;

/// Why the text is not a module, and where.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("line {line}, column {column}: {kind}")]
pub struct ReadError {
    /// The line, from 1.
    pub line: usize,
    /// The column in bytes, from 1.
    pub column: usize,
    /// What is wrong.
    pub kind: ReadErrorKind,
}

/// What kind of thing a name names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    /// A `@function`.
    Function,
    /// A block label.
    Block,
    /// A `%value`.
    Value,
}

impl fmt::Display for NameKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NameKind::Function => "function",
            NameKind::Block => "block",
            NameKind::Value => "value",
        })
    }
}

/// What is wrong with the text.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReadErrorKind {
    /// A character that starts no token.
    #[error("unexpected character")]
    BadCharacter,
    /// A token other than the grammar allows.
    #[error("expected {expected}, found {found}")]
    Unexpected {
        /// What the grammar allows here.
        expected: &'static str,
        /// What the text has.
        found: String,
    },
    /// A statement starting with a word that is no instruction.
    #[error("unknown instruction `{0}`")]
    UnknownInstruction(String),
    /// A type word with a width outside 1 to 64, or of the wrong kind.
    #[error("`{0}` is not a valid type here")]
    BadType(String),
    /// A malformed number.
    #[error("`{0}` is not a valid number here")]
    BadNumber(String),
    /// An integer literal that does not fit its type.
    #[error("`{literal}` does not fit in {ty}")]
    OutOfRange {
        /// The literal as written.
        literal: String,
        /// The type it must fit.
        ty: Type,
    },
    /// A string with an unknown escape sequence.
    #[error("unknown escape sequence in string")]
    BadEscape,
    /// A name used but never defined.
    #[error("{kind} `{name}` is never defined")]
    Undefined {
        /// What the name names.
        kind: NameKind,
        /// The name.
        name: String,
    },
    /// A name defined twice.
    #[error("{kind} `{name}` is defined twice")]
    Redefined {
        /// What the name names.
        kind: NameKind,
        /// The name.
        name: String,
    },
    /// A label while the previous block has no terminator yet.
    #[error("block `{0}` has no terminator")]
    MissingTerminator(String),
    /// An instruction outside any block.
    #[error("instruction outside a block: start a block with a label")]
    MissingLabel,
    /// A value instruction without `%name =`.
    #[error("`{0}` produces a value: write `%name = {0} ...`")]
    ResultRequired(&'static str),
    /// `%name =` before an instruction that produces no value.
    #[error("`{0}` does not produce a value")]
    ResultNotAllowed(&'static str),
    /// A second `memory` line.
    #[error("`memory` is declared twice")]
    DuplicateMemory,
    /// More names than fit a dense 32-bit index.
    #[error("too many names")]
    TooManyNames,
}

/// A read failure before it is placed on a line: the byte offset and the problem.
#[derive(Debug)]
pub(super) struct Failure {
    pub(super) offset: usize,
    pub(super) kind: ReadErrorKind,
}

impl Failure {
    /// Places the failure in `text` by line and column.
    pub(super) fn locate(self, text: &str) -> ReadError {
        let before = &text[..self.offset.min(text.len())];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        ReadError { line: before.matches('\n').count() + 1, column: before.len() - line_start + 1, kind: self.kind }
    }
}
