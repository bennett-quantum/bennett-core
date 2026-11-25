//! Private byte versions, retained through the inverse of the applied call.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use bennett_exec::{ApplyMemory, Wire};
use bennett_ir::{DataSegment, Fault, Predicate, Width};

use super::bits::{Bit, Circuit, ONE, Word, ZERO, constant, definite};
use super::{ApplyInput, Argument, arithmetic, reach};

pub(super) const DISPATCH_REQUIRED: &str = "private memory in the step machine";

/// Additional circuit resources used by the private byte map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryCounts {
    /// Scratch wires for accesses and versions retained through the inverse.
    pub retained_scratch_qubits: usize,
    /// X, CNOT and Toffoli gates for accesses and versions, including their inverse.
    pub gates: usize,
    /// Largest number of symbolic bits in the current byte versions at one time.
    pub peak_nonconstant_bits: usize,
    /// Load dispatches with more than one possible address.
    pub varying_loads: usize,
    /// Store dispatches with more than one possible address.
    pub varying_stores: usize,
    /// Sum of the in-bounds candidate span sizes, counting overlapping spans separately.
    pub candidate_bytes: usize,
    /// Toffolis for varying-address checks and selection, including their inverse.
    pub address_toffolis: usize,
    /// Scratch wires retained by varying-address checks and selection.
    pub address_scratch_qubits: usize,
}

impl fmt::Display for MemoryCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            concat!(
                "{} retained memory scratch qubits, {} memory gates, {} peak nonconstant memory bits, ",
                "{} varying loads, {} varying stores, {} candidate bytes, ",
                "{} address selection Toffolis, {} retained address selection scratch qubits"
            ),
            self.retained_scratch_qubits,
            self.gates,
            self.peak_nonconstant_bits,
            self.varying_loads,
            self.varying_stores,
            self.candidate_bytes,
            self.address_toffolis,
            self.address_scratch_qubits
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

pub(super) struct Access<T> {
    pub(super) value: T,
    pub(super) succeeded: Bit,
    pub(super) faults: Vec<(Bit, Fault)>,
}

impl<T> Access<T> {
    fn fixed(value: T) -> Self {
        Self { value, succeeded: ONE, faults: Vec::new() }
    }
}

struct AddressChoices<'a> {
    valid: Vec<(u64, &'a [u8])>,
    held: Vec<u64>,
    last_if_outside: Option<u64>,
}

struct ByteWrites {
    initial: u8,
    alternatives: Vec<(Bit, Byte)>,
    varying: bool,
}

pub(super) struct Memory<'a> {
    base: Option<ApplyMemory<'a>>,
    declarations: &'a [DataSegment],
    bytes: BTreeMap<u64, Byte>,
    counts: MemoryCounts,
    current_bits: usize,
    pending: BTreeMap<u64, ByteWrites>,
    proof: Option<AddressProof>,
}

impl<'a> Memory<'a> {
    pub(super) fn new(input: &ApplyInput<'a>) -> Self {
        Self {
            base: input.memory,
            declarations: &input.module.data,
            bytes: BTreeMap::new(),
            counts: MemoryCounts::default(),
            current_bits: 0,
            pending: BTreeMap::new(),
            proof: input.memory.and_then(|_| AddressProof::new(input.args)),
        }
    }

    pub(super) fn counts(&self) -> MemoryCounts {
        self.counts
    }

    // A speculative exit uses arbitrary symbolic residue wires. Address replay
    // cannot evaluate those wires, so it may only use the symbolic address bounds.
    pub(super) fn fork_for_exit(&self) -> Self {
        debug_assert!(self.pending.is_empty());
        Self {
            base: self.base,
            declarations: self.declarations,
            bytes: self.bytes.clone(),
            counts: MemoryCounts::default(),
            current_bits: self.current_bits,
            pending: BTreeMap::new(),
            proof: None,
        }
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
        circuit: &mut Circuit,
        address: &[Bit],
        guard: Bit,
        width: Width,
    ) -> Result<Access<Word>, AccessError> {
        match self.access(circuit, address, guard, width) {
            Ok((address, bytes)) => return Ok(Access::fixed(self.word(address, bytes, width))),
            Err(AccessError::Address) => {}
            Err(error) => return Err(error),
        }
        self.counts.varying_loads += 1;
        let choices = self.choices(circuit, address, guard, width)?;
        let gates = circuit.forward_len();
        let scratch = circuit.scratch_end();
        let (succeeded, faults) = choices.status(circuit, address);
        let candidates: Vec<_> = choices
            .valid
            .into_iter()
            .map(|(candidate, bytes)| (candidate, self.word(candidate, bytes, width)))
            .collect();
        let mut copies = Vec::new();
        let word = if let Some((_, first)) = candidates.first() {
            first
                .iter()
                .enumerate()
                .map(|(index, &bit)| {
                    if matches!(bit, Bit::Constant(_)) && candidates.iter().all(|(_, word)| word[index] == bit) {
                        return bit;
                    }
                    let wire = circuit.fresh();
                    copies.push((index, wire));
                    Bit::Wire(wire, false)
                })
                .collect()
        } else {
            vec![ZERO; width.bits() as usize]
        };
        for (candidate, value) in candidates {
            if copies.iter().any(|&(index, _)| value[index] != ZERO) {
                let selected = address_equal(circuit, address, candidate);
                for &(index, target) in &copies {
                    circuit.xor_into(target, value[index], selected);
                }
            }
        }
        self.account(circuit, scratch, gates, true);
        Ok(Access { value: word, succeeded, faults })
    }

    fn word(&self, address: u64, bytes: &[u8], width: Width) -> Word {
        bytes
            .iter()
            .enumerate()
            .flat_map(|(offset, &byte)| self.byte(address + offset as u64, byte))
            .take(width.bits() as usize)
            .collect()
    }

    pub(super) fn store(
        &mut self,
        circuit: &mut Circuit,
        address: &[Bit],
        guard: Bit,
        width: Width,
        mut value: Word,
    ) -> Result<Access<()>, AccessError> {
        value.resize(width.bytes() as usize * 8, ZERO);
        match self.access(circuit, address, guard, width) {
            Ok((address, bytes)) => {
                self.queue(address, bytes, guard, &value, false);
                return Ok(Access::fixed(()));
            }
            Err(AccessError::Address) => {}
            Err(error) => return Err(error),
        }
        self.counts.varying_stores += 1;
        let choices = self.choices(circuit, address, guard, width)?;
        let gates = circuit.forward_len();
        let scratch = circuit.scratch_end();
        let (succeeded, faults) = choices.status(circuit, address);
        for (candidate, bytes) in choices.valid {
            let selected = address_equal(circuit, address, candidate);
            let selected = circuit.and(guard, selected);
            self.queue(candidate, bytes, selected, &value, true);
        }
        self.account(circuit, scratch, gates, true);
        Ok(Access { value: (), succeeded, faults })
    }

    fn choices(
        &mut self,
        circuit: &Circuit,
        address: &[Bit],
        guard: Bit,
        width: Width,
    ) -> Result<AddressChoices<'a>, AccessError> {
        let base = self.base.ok_or(AccessError::MissingMemory)?;
        let last = base.byte_len().checked_sub(width.bytes()).ok_or(AccessError::Fault(Fault::OutOfBounds))?;
        let (addresses, may_be_outside) = self.lookup_addresses(circuit, address, guard, width, last)?;
        if addresses.is_empty() {
            return Err(AccessError::Fault(Fault::OutOfBounds));
        }
        self.counts.candidate_bytes += addresses.len() * width.bytes() as usize;
        let mut choices =
            AddressChoices { valid: Vec::new(), held: Vec::new(), last_if_outside: may_be_outside.then_some(last) };
        for candidate in addresses {
            match base.read(candidate, width) {
                Ok(bytes) => {
                    if !self.declared(candidate, candidate + width.bytes()) {
                        return Err(AccessError::Address);
                    }
                    choices.valid.push((candidate, bytes));
                }
                Err(Fault::QubitMemoryAccess) => choices.held.push(candidate),
                Err(fault) => return Err(AccessError::Fault(fault)),
            }
        }
        if choices.valid.is_empty() && !may_be_outside {
            return Err(AccessError::Fault(Fault::QubitMemoryAccess));
        }
        Ok(choices)
    }

    fn declared(&self, first: u64, end: u64) -> bool {
        self.declarations.iter().any(|segment| first >= segment.address && end <= segment.address + segment.len())
    }

    fn lookup_addresses(
        &mut self,
        circuit: &Circuit,
        address: &[Bit],
        guard: Bit,
        width: Width,
        last: u64,
    ) -> Result<(Vec<u64>, bool), AccessError> {
        if let Some(proof) = &mut self.proof
            && let Some(addresses) = proof.addresses_under(circuit, address, guard)?
        {
            let outside = addresses.last().is_some_and(|&address| address > last);
            return Ok((addresses.into_iter().take_while(|&address| address <= last).collect(), outside));
        }
        // Known bits bound all possible addresses, including those on inactive inputs.
        let (fixed, mask) = address.iter().enumerate().fold((0u64, 0u64), |(fixed, mask), (index, bit)| match bit {
            Bit::Constant(value) => (fixed | u64::from(*value) << index, mask | 1 << index),
            Bit::Wire(..) => (fixed, mask),
        });
        let maximum = fixed | !mask;
        let upper = maximum.min(last);
        if fixed > upper {
            return Ok((Vec::new(), true));
        }
        if !self.declared(fixed, upper + width.bytes()) {
            return Err(AccessError::Address);
        }
        Ok(((fixed..=upper).filter(|&candidate| candidate & mask == fixed).collect(), maximum > last))
    }

    fn byte(&self, address: u64, initial: u8) -> Byte {
        self.bytes
            .get(&address)
            .copied()
            .unwrap_or_else(|| std::array::from_fn(|bit| Bit::Constant(initial >> bit & 1 != 0)))
    }

    fn queue(&mut self, address: u64, bytes: &[u8], guard: Bit, value: &[Bit], varying: bool) {
        if guard != ZERO {
            for (offset, (&data, &initial)) in value.as_chunks::<8>().0.iter().zip(bytes).enumerate() {
                let updates = self.pending.entry(address + offset as u64).or_insert_with(|| ByteWrites {
                    initial,
                    alternatives: Vec::new(),
                    varying: false,
                });
                updates.alternatives.push((guard, data));
                updates.varying |= varying;
            }
        }
    }

    // Reads see the pre-tick map. Each touched byte gets one version at commit.
    pub(super) fn commit(&mut self, circuit: &mut Circuit) {
        for (address, pending) in std::mem::take(&mut self.pending) {
            let gates = circuit.forward_len();
            let scratch = circuit.scratch_end();
            let old = self.byte(address, pending.initial);
            let new = match pending.alternatives.as_slice() {
                // Every value bit stays available through the inverse. The
                // new version can point directly to the stored bits.
                [(ONE, value)] => value.map(|bit| match bit {
                    Bit::Wire(source, true) => circuit.retain_complement(source),
                    Bit::Constant(_) | Bit::Wire(_, false) => bit,
                }),
                [(guard, value)] => std::array::from_fn(|i| circuit.select(*guard, value[i], old[i])),
                alternatives => select_byte(circuit, old, alternatives),
            };
            self.current_bits -= symbolic_bits(&old);
            self.current_bits += symbolic_bits(&new);
            self.bytes.insert(address, new);
            self.account(circuit, scratch, gates, pending.varying);
        }
        self.counts.peak_nonconstant_bits = self.counts.peak_nonconstant_bits.max(self.current_bits);
    }

    fn account(&mut self, circuit: &Circuit, first: Option<Wire>, gates: usize, varying: bool) {
        let retained = first.zip(circuit.scratch_end()).map_or(0, |(first, end)| (end - first) as usize);
        let emitted = &circuit.forward_gates()[gates..];
        self.counts.retained_scratch_qubits += retained;
        self.counts.gates += 2 * emitted.len();
        if varying {
            self.counts.address_scratch_qubits += retained;
            self.counts.address_toffolis += 2 * emitted.iter().filter(|gate| gate.controls.len() == 2).count();
        }
    }
}

fn select_byte(circuit: &mut Circuit, old: Byte, alternatives: &[(Bit, Byte)]) -> Byte {
    // A definite input dispatches one instruction per tick and has one address.
    // Its write guards are therefore disjoint even when the byte spans overlap.
    let keep_old = alternatives.iter().fold(ZERO, |touched, (select, _)| circuit.xor(touched, *select)).not();
    std::array::from_fn(|index| {
        let first = if keep_old == ZERO { alternatives[0].1[index] } else { old[index] };
        if alternatives.iter().all(|(_, value)| value[index] == first) {
            return first;
        }
        let wire = circuit.fresh();
        circuit.xor_into(wire, old[index], keep_old);
        for &(select, value) in alternatives {
            circuit.xor_into(wire, value[index], select);
        }
        Bit::Wire(wire, false)
    })
}

fn address_equal(circuit: &mut Circuit, address: &[Bit], candidate: u64) -> Bit {
    arithmetic::compare(circuit, Predicate::Eq, address, &constant(u128::from(candidate), address.len()))
}

impl AddressChoices<'_> {
    fn status(&self, circuit: &mut Circuit, address: &[Bit]) -> (Bit, Vec<(Bit, Fault)>) {
        let outside = self.last_if_outside.map_or(ZERO, |last| {
            arithmetic::compare(circuit, Predicate::Ugt, address, &constant(u128::from(last), address.len()))
        });
        let held = if self.valid.is_empty() {
            outside.not()
        } else {
            self.held.iter().fold(ZERO, |held, &candidate| {
                let selected = address_equal(circuit, address, candidate);
                circuit.xor(held, selected)
            })
        };
        let succeeded = if self.valid.is_empty() { ZERO } else { circuit.and(outside.not(), held.not()) };
        let faults = [(outside, Fault::OutOfBounds), (held, Fault::QubitMemoryAccess)]
            .into_iter()
            .filter(|&(guard, _)| guard != ZERO)
            .collect();
        (succeeded, faults)
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
                    let pattern = (0..64).filter(|&i| (pack * 64 + i) >> bit & 1 != 0).fold(0, |mask, i| mask | 1 << i);
                    // Every wire mask is zero beyond the real argument assignments.
                    pattern & domain[pack]
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

    fn active(&mut self, circuit: &Circuit, guard: Bit) -> Result<Option<Assignments>, AccessError> {
        if circuit.forward_len() > reach::MAX_ATTEMPTS {
            return Ok(None);
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
        Ok(Some(mask))
    }

    fn addresses_under(
        &mut self,
        circuit: &Circuit,
        address: &[Bit],
        guard: Bit,
    ) -> Result<Option<BTreeSet<u64>>, AccessError> {
        let Some(mask) = self.active(circuit, guard)? else { return Ok(None) };
        let values: Vec<_> = address.iter().map(|&bit| self.values(bit)).collect();
        let mut addresses = BTreeSet::new();
        for (pack, mut enabled) in mask.into_iter().enumerate() {
            while enabled != 0 {
                let assignment = 1u64 << enabled.trailing_zeros();
                let addr = values
                    .iter()
                    .enumerate()
                    .fold(0, |address, (bit, value)| address | u64::from(value[pack] & assignment != 0) << bit);
                addresses.insert(addr);
                enabled &= !assignment;
            }
        }
        Ok(Some(addresses))
    }

    fn constant_under(&mut self, circuit: &Circuit, word: &[Bit], guard: Bit) -> Result<u64, AccessError> {
        let mask = self.active(circuit, guard)?.ok_or(AccessError::Address)?;
        word.iter().enumerate().try_fold(0, |acc, (bit, &value)| {
            let values = self.values(value);
            let one = values.iter().zip(mask).any(|(value, enabled)| value & enabled != 0);
            let zero = values.iter().zip(mask).any(|(value, enabled)| !value & enabled != 0);
            if one && zero { Err(AccessError::Address) } else { Ok(acc | u64::from(one) << bit) }
        })
    }
}
