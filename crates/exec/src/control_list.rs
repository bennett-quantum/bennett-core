//! Control lists in memory (`docs/SYNTAX.md`, "Control lists"): the controls of a gate of the handle
//! interface, `ctrl list(address, count)`, as `count` entries of [`CONTROL_ENTRY_BYTES`] bytes, each a
//! little-endian `u64` handle at offset 0 and a `u64` qubit position at offset 8.

use std::collections::BTreeSet;

use bennett_ir::{CONTROL_ENTRY_BYTES, Fault};

use crate::device::Wire;
use crate::executor::Executor;

/// The byte offset of an entry's position.
const POSITION_OFFSET: usize = 8;

impl Executor<'_> {
    /// The control qubits of the list of `count` entries at `address`, checked in this order: the list is
    /// readable as ordinary memory, its byte count computed as a mathematical integer (else `bad_argument`;
    /// a list of no entry reads no byte, at any address); then, entry by entry in list order, its handle
    /// names a register (`dangling_register`) and its position is below the register's width (`bad_range`).
    /// A qubit listed twice is one control: each appears once, where it was first listed.
    pub(crate) fn control_list(&self, address: u64, count: u64) -> Result<Vec<Wire>, Fault> {
        let bytes = self.list_bytes(address, count, CONTROL_ENTRY_BYTES)?;
        let word = |at: &[u8]| u64::from_le_bytes(at.try_into().expect("eight bytes"));
        let (mut controls, mut listed) = (Vec::new(), BTreeSet::new());
        let (entries, rest) = bytes.as_chunks::<{ CONTROL_ENTRY_BYTES as usize }>();
        debug_assert!(rest.is_empty(), "whole entries");
        for entry in entries {
            let (handle, position) = (word(&entry[..POSITION_OFFSET]), word(&entry[POSITION_OFFSET..]));
            let control = self.wire_of(handle, position)?;
            if listed.insert(control) {
                controls.push(control);
            }
        }
        Ok(controls)
    }

    /// The bytes of a list of `count` entries of `entry_bytes` at `address` in the interface's memory: inside
    /// memory, the byte count computed as a mathematical integer, and none holding qubits (else
    /// `bad_argument`); a list of no entry reads no byte, at any address.
    pub(crate) fn list_bytes(&self, address: u64, count: u64, entry_bytes: u64) -> Result<Vec<u8>, Fault> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let end = u128::from(address) + u128::from(count) * u128::from(entry_bytes);
        if end > self.memory.len() as u128 {
            return Err(Fault::BadArgument);
        }
        let range = address as usize..end as usize;
        if self.quantum.has_cells_in(range.start as u64..range.end as u64) {
            return Err(Fault::BadArgument);
        }
        let mut bytes = vec![0; range.len()];
        self.memory.read_into(range, &mut bytes);
        Ok(bytes)
    }
}
