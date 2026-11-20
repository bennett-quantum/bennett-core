//! Private byte versions, retained through the inverse of the applied call.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use bennett_exec::{ApplyMemory, Wire};
use bennett_ir::{Fault, Width};

use super::bits::{Bit, Circuit, ONE, Word, ZERO, definite};
use super::{ApplyInput, Argument, reach};

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
    Inactive,
    MissingMemory,
    Address,
    Fault(Fault),
}

struct Store<'a> {
    address: u64,
    base: &'a [u8],
    guard: Bit,
    value: Word,
}

pub(super) struct Memory<'a> {
    base: Option<ApplyMemory<'a>>,
    bytes: BTreeMap<u64, Byte>,
    counts: MemoryCounts,
    current_bits: usize,
    pending: Vec<Store<'a>>,
    proof: Option<AddressProof>,
}

impl<'a> Memory<'a> {
    pub(super) fn new(input: &ApplyInput<'a>) -> Self {
        Self {
            base: input.memory,
            bytes: BTreeMap::new(),
            counts: MemoryCounts::default(),
            current_bits: 0,
            pending: Vec::new(),
            proof: input.memory.and_then(|_| AddressProof::new(input.args)),
        }
    }

    pub(super) fn counts(&self) -> MemoryCounts {
        self.counts
    }

    fn access(
        &mut self,
        circuit: &Circuit,
        address: &[Bit],
        guard: Bit,
        width: Width,
    ) -> Result<(u64, &'a [u8]), AccessError> {
        let base = self.base.ok_or(AccessError::MissingMemory)?;
        let address = if let Some(address) = definite(address) {
            address as u64
        } else {
            self.proof.as_mut().ok_or(AccessError::Address)?.constant_under(circuit, address, guard)?
        };
        Ok((address, base.read(address, width).map_err(AccessError::Fault)?))
    }

    pub(super) fn load(
        &mut self,
        circuit: &Circuit,
        address: &[Bit],
        guard: Bit,
        width: Width,
    ) -> Result<Word, AccessError> {
        let (address, base) = self.access(circuit, address, guard, width)?;
        Ok(base
            .iter()
            .enumerate()
            .flat_map(|(offset, &byte)| self.byte(address + offset as u64, byte))
            .take(width.bits() as usize)
            .collect())
    }

    pub(super) fn store(
        &mut self,
        circuit: &Circuit,
        address: &[Bit],
        guard: Bit,
        width: Width,
        mut value: Word,
    ) -> Result<(), AccessError> {
        let (address, base) = self.access(circuit, address, guard, width)?;
        value.resize(base.len() * 8, ZERO);
        self.pending.push(Store { address, base, guard, value });
        Ok(())
    }

    fn byte(&self, address: u64, initial: u8) -> Byte {
        self.bytes
            .get(&address)
            .copied()
            .unwrap_or_else(|| std::array::from_fn(|bit| Bit::Constant(initial >> bit & 1 != 0)))
    }

    // All accesses in a tick read the pre-tick map. Store guards are disjoint.
    pub(super) fn commit(&mut self, circuit: &mut Circuit) {
        if self.pending.is_empty() {
            return;
        }
        let start = circuit.forward_len();
        let first = circuit.scratch_end();
        for store in std::mem::take(&mut self.pending) {
            for (offset, (chunk, &initial)) in store.value.as_chunks::<8>().0.iter().zip(store.base).enumerate() {
                let address = store.address + offset as u64;
                let old = self.byte(address, initial);
                let new = std::array::from_fn(|i| {
                    if store.guard == ONE {
                        match chunk[i] {
                            bit @ Bit::Constant(_) => bit,
                            bit @ Bit::Wire(..) => circuit.retain_copy(bit),
                        }
                    } else {
                        circuit.select(store.guard, chunk[i], old[i])
                    }
                });
                self.current_bits -= symbolic_bits(&old);
                self.current_bits += symbolic_bits(&new);
                self.bytes.insert(address, new);
            }
        }
        if let Some((first, end)) = first.zip(circuit.scratch_end()) {
            self.counts.retained_scratch_qubits += (end - first) as usize;
        }
        self.counts.peak_nonconstant_bits = self.counts.peak_nonconstant_bits.max(self.current_bits);
        self.counts.gates += 2 * (circuit.forward_len() - start);
    }
}

fn symbolic_bits(byte: &Byte) -> usize {
    byte.iter().filter(|bit| matches!(bit, Bit::Wire(..))).count()
}

// Replay forward gates in parallel over every assignment of a small interface.
// This can prove an address constant only on the inputs executing its access.
const PROOF_BITS: usize = 8;
const PROOF_WORDS: usize = (1 << PROOF_BITS) / u64::BITS as usize;
type Assignments = [u64; PROOF_WORDS];

struct AddressProof {
    wires: BTreeMap<Wire, Assignments>,
    domain: Assignments,
    read_gates: usize,
}

impl AddressProof {
    fn new(args: &[Argument]) -> Option<Self> {
        let mut inputs = BTreeSet::new();
        for arg in args {
            if let Argument::Register(wires) = arg {
                for wire in wires.clone() {
                    inputs.insert(wire);
                    if inputs.len() > PROOF_BITS {
                        return None;
                    }
                }
            }
        }
        let mut domain = [0; PROOF_WORDS];
        for assignment in 0..1 << inputs.len() {
            domain[assignment / 64] |= 1 << (assignment % 64);
        }
        let wires = inputs
            .into_iter()
            .enumerate()
            .map(|(bit, wire)| {
                let pattern = std::array::from_fn(|pack| {
                    (0..64).filter(|&i| (pack * 64 + i) >> bit & 1 != 0).fold(0, |mask, i| mask | 1 << i)
                });
                (wire, pattern)
            })
            .collect();
        Some(Self { wires, domain, read_gates: 0 })
    }

    fn values(&self, bit: Bit) -> Assignments {
        match bit {
            Bit::Constant(true) => self.domain,
            Bit::Constant(false) => [0; PROOF_WORDS],
            Bit::Wire(wire, inverted) => {
                let values = self.wires.get(&wire).copied().unwrap_or_default();
                std::array::from_fn(|i| values[i] ^ if inverted { self.domain[i] } else { 0 })
            }
        }
    }

    fn constant_under(&mut self, circuit: &Circuit, word: &[Bit], guard: Bit) -> Result<u64, AccessError> {
        if circuit.forward_len() > reach::MAX_ATTEMPTS {
            return Err(AccessError::Address);
        }
        for gate in &circuit.forward_gates()[self.read_gates..] {
            let mask = gate.controls.iter().fold(self.domain, |mask, &wire| {
                let values = self.values(Bit::Wire(wire, false));
                std::array::from_fn(|i| mask[i] & values[i])
            });
            for (value, toggles) in self.wires.entry(gate.target).or_default().iter_mut().zip(mask) {
                *value ^= toggles;
            }
        }
        self.read_gates = circuit.forward_len();
        let mask = self.values(guard);
        if mask == [0; PROOF_WORDS] {
            return Err(AccessError::Inactive);
        }
        word.iter().enumerate().try_fold(0, |acc, (bit, &value)| {
            let values = self.values(value);
            let one = values.iter().zip(mask).any(|(value, enabled)| value & enabled != 0);
            let zero = values.iter().zip(mask).any(|(value, enabled)| !value & enabled != 0);
            if one && zero { Err(AccessError::Address) } else { Ok(acc | u64::from(one) << bit) }
        })
    }
}
