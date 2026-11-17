//! Memory: a flat byte array.

use std::ops::Range;

/// The machine's memory.
#[derive(Clone, Debug, Default)]
pub(crate) struct Memory {
    bytes: Vec<u8>,
}

impl Memory {
    /// A memory holding `bytes`.
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Memory { bytes }
    }

    /// The size in bytes.
    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    /// The bytes at `range`, which must be in bounds, into `out` (as long as the range).
    pub(crate) fn read_into(&self, range: Range<usize>, out: &mut [u8]) {
        out.copy_from_slice(&self.bytes[range]);
    }

    /// Writes `bytes` at `start`; the range must be in bounds.
    pub(crate) fn write(&mut self, start: usize, bytes: &[u8]) {
        self.bytes[start..start + bytes.len()].copy_from_slice(bytes);
    }

    /// A copy of every byte.
    pub(crate) fn to_vec(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}
