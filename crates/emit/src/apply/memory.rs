//! Private byte versions, retained through the inverse of the applied call.

use std::collections::BTreeMap;
use std::fmt;

use bennett_exec::ApplyMemory;
use bennett_ir::{Fault, Width};

use super::bits::{Bit, Circuit, Word, ZERO, definite};

pub(super) const DISPATCH_REQUIRED: &str = "private memory in the step machine";

/// Additional circuit resources used by the private byte map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryCounts {
    /// New memory-version wires retained through reversal, excluding other scratch.
    pub retained_scratch_qubits: usize,
    /// X, CNOT and Toffoli gates for memory versions, including their inverse.
    pub gates: usize,
    /// Largest number of symbolic bits in the current byte versions at one time.
    pub peak_nonconstant_bits: usize,
}

impl fmt::Display for MemoryCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} retained memory scratch qubits, {} memory gates, {} peak nonconstant memory bits",
            self.retained_scratch_qubits, self.gates, self.peak_nonconstant_bits
        )
    }
}

type Byte = [Bit; 8];

pub(super) enum AccessError {
    MissingMemory,
    Address,
    Fault(Fault),
}

pub(super) struct Memory<'a> {
    base: Option<ApplyMemory<'a>>,
    bytes: BTreeMap<u64, Byte>,
    counts: MemoryCounts,
    current_bits: usize,
}

impl<'a> Memory<'a> {
    pub(super) fn new(base: Option<ApplyMemory<'a>>) -> Self {
        Self { base, bytes: BTreeMap::new(), counts: MemoryCounts::default(), current_bits: 0 }
    }

    pub(super) fn counts(&self) -> MemoryCounts {
        self.counts
    }

    fn access(&self, address: &[Bit], width: Width) -> Result<(u64, &'a [u8]), AccessError> {
        let base = self.base.ok_or(AccessError::MissingMemory)?;
        let address = definite(address).ok_or(AccessError::Address)? as u64;
        Ok((address, base.read(address, width).map_err(AccessError::Fault)?))
    }

    pub(super) fn load(&self, address: &[Bit], width: Width) -> Result<Word, AccessError> {
        let (address, base) = self.access(address, width)?;
        Ok(base
            .iter()
            .enumerate()
            .flat_map(|(offset, &byte)| {
                self.bytes
                    .get(&(address + offset as u64))
                    .copied()
                    .unwrap_or_else(|| std::array::from_fn(|bit| Bit::Constant(byte >> bit & 1 != 0)))
            })
            .take(width.bits() as usize)
            .collect())
    }

    pub(super) fn store(
        &mut self,
        circuit: &mut Circuit,
        address: &[Bit],
        width: Width,
        value: &[Bit],
    ) -> Result<(), AccessError> {
        let (address, base) = self.access(address, width)?;
        let start = circuit.forward_len();
        let mut value = value.iter().copied();
        for offset in 0..base.len() {
            let version = std::array::from_fn(|_| match value.next().unwrap_or(ZERO) {
                bit @ Bit::Constant(_) => bit,
                bit @ Bit::Wire(..) => {
                    self.counts.retained_scratch_qubits += 1;
                    circuit.retain_copy(bit)
                }
            });
            if let Some(previous) = self.bytes.insert(address + offset as u64, version) {
                self.current_bits -= symbolic_bits(&previous);
            }
            self.current_bits += symbolic_bits(&version);
        }
        self.counts.peak_nonconstant_bits = self.counts.peak_nonconstant_bits.max(self.current_bits);
        self.counts.gates += 2 * (circuit.forward_len() - start);
        Ok(())
    }
}

fn symbolic_bits(byte: &Byte) -> usize {
    byte.iter().filter(|bit| matches!(bit, Bit::Wire(..))).count()
}
