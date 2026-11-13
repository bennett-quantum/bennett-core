//! Dense indices for names that may be used before they are defined.

use std::collections::HashMap;

use super::error::{Failure, NameKind, ReadErrorKind};

/// Assigns each distinct name the next dense index on first mention, and remembers where it was first
/// used and in which order names were defined.
pub(super) struct Names<'s> {
    kind: NameKind,
    indices: HashMap<&'s str, u32>,
    entries: Vec<Entry<'s>>,
    defined: u32,
}

struct Entry<'s> {
    name: &'s str,
    first_use: usize,
    /// The position of the definition among all definitions so far.
    definition: Option<u32>,
}

impl<'s> Names<'s> {
    pub(super) fn new(kind: NameKind) -> Self {
        Names { kind, indices: HashMap::new(), entries: Vec::new(), defined: 0 }
    }

    /// The index of `name`, used at `offset`.
    pub(super) fn mention(&mut self, name: &'s str, offset: usize) -> Result<u32, Failure> {
        if let Some(&index) = self.indices.get(name) {
            return Ok(index);
        }
        let index =
            u32::try_from(self.entries.len()).map_err(|_| Failure { offset, kind: ReadErrorKind::TooManyNames })?;
        self.indices.insert(name, index);
        self.entries.push(Entry { name, first_use: offset, definition: None });
        Ok(index)
    }

    /// The index of `name`, defined at `offset`; an error if it was defined before.
    pub(super) fn define(&mut self, name: &'s str, offset: usize) -> Result<u32, Failure> {
        let index = self.mention(name, offset)?;
        let entry = &mut self.entries[index as usize];
        if entry.definition.is_some() {
            let kind = ReadErrorKind::Redefined { kind: self.kind, name: name.to_owned() };
            return Err(Failure { offset, kind });
        }
        entry.definition = Some(self.defined);
        self.defined += 1;
        Ok(index)
    }

    /// For each index, the position of its definition among all definitions; or an error at the first use
    /// of the first name that was never defined.
    pub(super) fn definition_order(&self) -> Result<Vec<u32>, Failure> {
        let undefined = |entry: &Entry<'_>| Failure {
            offset: entry.first_use,
            kind: ReadErrorKind::Undefined { kind: self.kind, name: entry.name.to_owned() },
        };
        self.entries.iter().map(|entry| entry.definition.ok_or_else(|| undefined(entry))).collect()
    }

    /// The names in index order.
    pub(super) fn into_names(self) -> Vec<String> {
        self.entries.into_iter().map(|entry| entry.name.to_owned()).collect()
    }
}
