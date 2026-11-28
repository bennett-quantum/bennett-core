//! Private byte versions, retained through the inverse of the applied call.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use bennett_exec::{ApplyMemory, Wire};
use bennett_ir::{DataSegment, Fault, Predicate, Width};

use super::bits::{Bit, Circuit, ONE, Word, ZERO, constant, definite};
use super::identity::{Generation, Identity};
use super::work::{self, Work};
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
    fixed_stores: bool,
    bytes: BTreeMap<u64, Byte>,
    counts: MemoryCounts,
    current_bits: usize,
    shared_version: u32,
    pending: BTreeMap<u64, ByteWrites>,
    proof: Option<AddressProof>,
}

impl<'a> Memory<'a> {
    pub(super) fn new(input: &ApplyInput<'a>, fixed_stores: bool) -> Self {
        Self {
            base: input.memory,
            declarations: &input.module.data,
            fixed_stores,
            bytes: BTreeMap::new(),
            counts: MemoryCounts::default(),
            current_bits: 0,
            shared_version: 0,
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
        work::add(Work::SnapshotWords, self.bytes.len().saturating_mul(9));
        Self {
            base: self.base,
            declarations: self.declarations,
            fixed_stores: self.fixed_stores,
            bytes: self.bytes.clone(),
            counts: MemoryCounts::default(),
            current_bits: self.current_bits,
            shared_version: self.shared_version,
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
                    if bit.literal().is_some()
                        && candidates.iter().all(|(_, word)| {
                            work::add(Work::MemoryBits, 1);
                            word[index].same_value(bit)
                        })
                    {
                        return bit;
                    }
                    let wire = circuit.fresh();
                    copies.push((index, wire));
                    circuit.value(wire)
                })
                .collect()
        } else {
            vec![ZERO; width.bits() as usize]
        };
        for (candidate, value) in candidates {
            if copies.iter().any(|&(index, _)| {
                work::add(Work::MemoryBits, 1);
                !value[index].is_zero()
            }) {
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
        work::add(Work::MemoryBits, width.bits() as usize);
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
            work::add(Work::MemoryCandidates, 1);
            work::add(Work::CandidateBytes, width.bytes() as usize);
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
        self.declarations.iter().any(|segment| {
            work::add(Work::DeclarationChecks, 1);
            first >= segment.address && end <= segment.address + segment.len()
        })
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
        work::add(Work::AddressBoundBits, address.len());
        let (fixed, mask) =
            address.iter().enumerate().fold((0u64, 0u64), |(fixed, mask), (index, bit)| match bit.literal() {
                Some(value) => (fixed | u64::from(value) << index, mask | 1 << index),
                None => (fixed, mask),
            });
        let maximum = fixed | !mask;
        let upper = maximum.min(last);
        if fixed > upper {
            return Ok((Vec::new(), true));
        }
        if !self.declared(fixed, upper + width.bytes()) {
            return Err(AccessError::Address);
        }
        let addresses = (fixed..=upper)
            .filter(|&candidate| {
                work::add(Work::IntervalAddresses, 1);
                candidate & mask == fixed
            })
            .collect();
        Ok((addresses, maximum > last))
    }

    fn byte(&self, address: u64, initial: u8) -> Byte {
        work::add(Work::MemoryBytes, 1);
        self.bytes
            .get(&address)
            .copied()
            .unwrap_or_else(|| std::array::from_fn(|bit| Bit::Constant(initial >> bit & 1 != 0)))
    }

    fn queue(&mut self, address: u64, bytes: &[u8], guard: Bit, value: &[Bit], varying: bool) {
        if !guard.is_zero() {
            for (offset, (&data, &initial)) in value.as_chunks::<8>().0.iter().zip(bytes).enumerate() {
                let updates = self.pending.entry(address + offset as u64).or_insert_with(|| ByteWrites {
                    initial,
                    alternatives: Vec::new(),
                    varying: false,
                });
                work::add(Work::MemoryBytes, 1);
                work::add(Work::FactStores, 9);
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
                [(guard, value)] if guard.literal() == Some(true) => value.map(|bit| self.store_bit(circuit, bit)),
                [(guard, value)] => std::array::from_fn(|i| circuit.select(*guard, value[i], old[i])),
                alternatives => {
                    // A varying store can only reach declared bytes. Keep
                    // those bytes on the existing selection path if a store
                    // in any copy has an unproved address.
                    let fixed_only = !pending.varying && (self.fixed_stores || !self.declared(address, address + 1));
                    if fixed_only && let Some(folded) = self.fold_literal_writes(circuit, &old, alternatives) {
                        folded
                    } else {
                        select_byte(circuit, old, alternatives)
                    }
                }
            };
            self.current_bits -= symbolic_bits(&old);
            self.current_bits += symbolic_bits(&new);
            self.bytes.insert(address, new);
            self.account(circuit, scratch, gates, pending.varying);
        }
        self.counts.peak_nonconstant_bits = self.counts.peak_nonconstant_bits.max(self.current_bits);
    }

    fn store_bit(&mut self, circuit: &mut Circuit, bit: Bit) -> Bit {
        if let Some(value) = bit.literal() {
            return Bit::Constant(value);
        }
        if let (Some((_, false)), Some(version)) = (bit.wire(), self.shared_version.checked_add(1)) {
            // Keep the identity of the omitted copy for guarded selection.
            self.shared_version = version;
            bit.stored(version)
        } else {
            circuit.retain_copy(bit)
        }
    }

    // For disjoint write guards, a literal bit changes precisely when one
    // of the active writes differs from the old bit.
    fn fold_literal_writes(&mut self, circuit: &mut Circuit, old: &Byte, alternatives: &[(Bit, Byte)]) -> Option<Byte> {
        let old = definite(old)?;
        let writes = alternatives
            .iter()
            .map(|(guard, byte)| Some((*guard, old ^ definite(byte)?)))
            .collect::<Option<Vec<_>>>()?;
        let mut parities: Vec<(bool, Vec<usize>, Bit)> = Vec::new();
        Some(std::array::from_fn(|index| {
            let mask = 1 << index;
            let initial = old & mask != 0;
            let changes = writes
                .iter()
                .enumerate()
                .filter_map(|(write, &(_, delta))| (delta & mask != 0).then_some(write))
                .collect::<Vec<_>>();
            if let Some((_, _, bit)) = parities.iter().find(|(old, writes, _)| *old == initial && *writes == changes) {
                return self.store_bit(circuit, *bit);
            }
            let bit = self.guard_parity(circuit, initial, changes.iter().map(|&write| writes[write].0));
            parities.push((initial, changes, bit));
            bit
        }))
    }

    fn guard_parity(&mut self, circuit: &mut Circuit, mut inverted: bool, guards: impl Iterator<Item = Bit>) -> Bit {
        let mut terms = BTreeMap::new();
        for guard in guards {
            let (source, polarity) = match guard.identity() {
                Identity::Literal(value) => {
                    inverted ^= value;
                    continue;
                }
                Identity::Source(source, polarity) => (source, polarity),
            };
            if let Some((previous, _)) = terms.remove(&source) {
                inverted ^= previous ^ polarity;
            } else {
                terms.insert(source, (polarity, guard));
            }
        }
        // Resolve identities for cancellation and keep the original wires for emission.
        let bits = terms
            .into_values()
            .map(|(_, guard)| {
                let (_, complement) = guard.wire().expect("a symbolic guard");
                inverted ^= complement;
                guard.with_flip(complement)
            })
            .collect::<Vec<_>>();
        if bits.len() > 1 {
            let target = circuit.fresh();
            circuit.xor_into(target, Bit::Constant(inverted), ONE);
            for source in bits {
                circuit.xor_into(target, source, ONE);
            }
            circuit.value(target)
        } else if let Some(&source) = bits.first() {
            if inverted { circuit.retain_copy(source.not()) } else { self.store_bit(circuit, source) }
        } else {
            Bit::Constant(inverted)
        }
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
        let first = if keep_old.is_zero() { alternatives[0].1[index] } else { old[index] };
        if alternatives.iter().all(|(_, value)| value[index].same_version(first)) {
            return first;
        }
        let wire = circuit.fresh();
        circuit.xor_into(wire, old[index], keep_old);
        for &(select, value) in alternatives {
            circuit.xor_into(wire, value[index], select);
        }
        circuit.value(wire)
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
            .filter(|&(guard, _)| !guard.is_zero())
            .collect();
        (succeeded, faults)
    }
}

fn symbolic_bits(byte: &Byte) -> usize {
    byte.iter().filter(|bit| bit.wire().is_some()).count()
}

// Replay forward gates in parallel over every assignment of a small interface.
// This can prove an address constant only on the inputs executing its access.
const PROOF_BITS: usize = 8;
const PROOF_WORDS: usize = (1 << PROOF_BITS) / u64::BITS as usize;
type Assignments = [u64; PROOF_WORDS];

pub(super) struct AddressProof {
    inputs: BTreeMap<Wire, Assignments>,
    wires: BTreeMap<Wire, Assignments>,
    domain: Assignments,
    read_gates: usize,
    revision: Option<Generation>,
    opaque: bool,
}

impl AddressProof {
    pub(super) fn new(args: &[Argument]) -> Option<Self> {
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
        let inputs: BTreeMap<_, _> = inputs
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
        Some(Self { wires: inputs.clone(), inputs, domain, read_gates: 0, revision: None, opaque: false })
    }

    fn values(&self, circuit: &Circuit, bit: Bit) -> Option<Assignments> {
        work::add(Work::PackReads, PROOF_WORDS);
        match bit.wire() {
            None if bit.literal() == Some(true) => Some(self.domain),
            None => Some([0; PROOF_WORDS]),
            Some((wire, inverted)) => {
                let values = self
                    .wires
                    .get(&wire)
                    .copied()
                    .or_else(|| circuit.allocated_scratch(wire).then_some([0; PROOF_WORDS]))?;
                Some(std::array::from_fn(|i| values[i] ^ if inverted { self.domain[i] } else { 0 }))
            }
        }
    }

    fn active(&mut self, circuit: &Circuit, guard: Bit) -> Result<Option<Assignments>, AccessError> {
        work::add(Work::AddressQueries, 1);
        if circuit.forward_len() > reach::MAX_ATTEMPTS {
            return Ok(None);
        }
        let revision = circuit.observe_replay_prefix();
        if let Some(previous) = self.revision
            && previous != revision
        {
            work::add(Work::SnapshotWords, self.inputs.len() * (1 + PROOF_WORDS));
            self.wires.clone_from(&self.inputs);
            self.read_gates = 0;
            self.opaque = false;
        }
        self.revision = Some(revision);
        if self.opaque {
            return Ok(None);
        }
        for gate in &circuit.forward_gates()[self.read_gates..] {
            work::add(Work::ReplayGates, 1);
            let mut mask = self.domain;
            for &wire in &gate.controls {
                let values = self
                    .wires
                    .get(&wire)
                    .copied()
                    .or_else(|| circuit.starts_as_scratch(wire).then_some([0; PROOF_WORDS]));
                let Some(values) = values else {
                    self.opaque = true;
                    return Ok(None);
                };
                work::add(Work::ControlPacks, PROOF_WORDS);
                for (acc, value) in mask.iter_mut().zip(values) {
                    *acc &= value;
                }
            }
            if !self.wires.contains_key(&gate.target) && !circuit.starts_as_scratch(gate.target) {
                self.opaque = true;
                return Ok(None);
            }
            work::add(Work::TargetPacks, PROOF_WORDS);
            for (value, toggles) in self.wires.entry(gate.target).or_default().iter_mut().zip(mask) {
                *value ^= toggles;
            }
        }
        self.read_gates = circuit.forward_len();
        let Some(mask) = self.values(circuit, guard) else { return Ok(None) };
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
        let Some(values) = address.iter().map(|&bit| self.values(circuit, bit)).collect::<Option<Vec<_>>>() else {
            return Ok(None);
        };
        let mut addresses = BTreeSet::new();
        for (pack, mut enabled) in mask.into_iter().enumerate() {
            while enabled != 0 {
                work::add(Work::AddressBits, values.len());
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

    pub(super) fn constant_under(&mut self, circuit: &Circuit, word: &[Bit], guard: Bit) -> Result<u64, AccessError> {
        let mask = self.active(circuit, guard)?.ok_or(AccessError::Address)?;
        word.iter().enumerate().try_fold(0, |acc, (bit, &value)| {
            let values = self.values(circuit, value).ok_or(AccessError::Address)?;
            let one = values.iter().zip(mask).any(|(value, enabled)| {
                work::add(Work::AddressBits, 1);
                value & enabled != 0
            });
            let zero = values.iter().zip(mask).any(|(value, enabled)| {
                work::add(Work::AddressBits, 1);
                !value & enabled != 0
            });
            if one && zero { Err(AccessError::Address) } else { Ok(acc | u64::from(one) << bit) }
        })
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
