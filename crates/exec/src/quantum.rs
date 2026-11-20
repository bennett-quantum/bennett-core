//! The sparse quantum state: a map from basis configuration to complex amplitude.
//!
//! A configuration holds one column per register id: the register's value as an integer. A register
//! released while zero in every configuration has its column removed or reused; one released while not zero
//! keeps its column, unnamed and never reused, so the stored state stays the exact purification of the
//! state after the partial trace (`docs/DESIGN.md`, "Releasing a register"). A register of more than 64
//! qubits is stored in consecutive columns from its head, whose id its handle names, 64 qubits each; it is
//! kept or removed whole. The map is ordered, so iteration, and
//! therefore seeded measurement, is deterministic.

use std::collections::{BTreeMap, TryReserveError};
use std::ops::Range;

use bennett_ir::{Fault, Qubits, Width};
use num_complex::Complex64;
use rand::Rng;

use crate::device::Wire;
use crate::device_io::{wire, wires};
use crate::executor::RunError;
use crate::gates::Matrix;
use crate::wire_storage::WireStorage;

/// The values of every register column, one basis state.
pub(crate) type Config = Box<[u64]>;

/// The most qubits one column holds: a column's value in a configuration is a `u64`.
pub(crate) const COLUMN_QUBITS: u32 = u64::BITS;

/// The widths of the columns that store a register of `qubits` qubits, least significant first: 64 each
/// but the last.
pub(crate) fn column_widths(qubits: Qubits) -> impl Iterator<Item = Width> {
    let count = qubits.count();
    (0..count.div_ceil(COLUMN_QUBITS))
        .map(move |k| Width::new((count - k * COLUMN_QUBITS).min(COLUMN_QUBITS)).expect("1 to 64 qubits"))
}

/// An amplitude no larger than this is treated as zero and dropped, by the simulator: amplitudes of a
/// normalised state are at most 1, so this is one unit of rounding, what cancellation leaves behind. A
/// dropped configuration has a weight of at most its square (about 5e-32), so it can never be a measurement's
/// outcome; nothing else of the program reads amplitudes.
const NEGLIGIBLE_AMPLITUDE: f64 = f64::EPSILON;

/// The state of one register id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegisterState {
    /// Counts the allocations of this id, so a handle to an earlier allocation is recognised as stale.
    pub generation: u32,
    /// The register's width while it is allocated; `None` while the id is free.
    pub width: Option<Width>,
}

/// A qubit resolved to its column and bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QubitRef {
    pub(crate) column: usize,
    pub(crate) bit: u8,
}

impl QubitRef {
    pub(crate) fn is_set(self, config: &[u64]) -> bool {
        config[self.column] >> self.bit & 1 == 1
    }

    pub(crate) fn assign(self, config: &mut [u64], value: bool) {
        let mask = 1 << self.bit;
        config[self.column] = if value { config[self.column] | mask } else { config[self.column] & !mask };
    }
}

#[derive(Clone, Debug)]
pub(crate) struct QuantumState {
    registers: Vec<RegisterState>,
    columns: usize,
    pub(crate) device_wires: WireStorage,
    amplitudes: BTreeMap<Config, Complex64>,
    peak_qubits: u64,
    peak_configurations: usize,
    pub(crate) nonzero_releases: u64,
    pub(crate) nonzero_release_weight: f64,
    /// The memory bytes that hold qubits ("cells", `docs/DESIGN.md`, "Qubits in memory"): byte address to
    /// the column of the 8-qubit register that holds the byte's value. Part of the register table.
    cells: BTreeMap<u64, usize>,
    /// The ids of registers left over, in the order they were left: released by `qfree` while not zero, so
    /// their columns are kept, with no live handle (`docs/DESIGN.md`, "Releasing a register").
    leftovers: Vec<usize>,
    /// The number of columns of each register of more than 64 qubits, by its head: it occupies the
    /// consecutive columns `head..head + n`. Kept while the head is live or left over.
    spans: BTreeMap<usize, usize>,
    /// Whether the state stores columns (the simulator); an external device's run only names registers.
    stores_columns: bool,
}

impl Default for QuantumState {
    /// No registers: one empty configuration with amplitude 1.
    fn default() -> Self {
        Self {
            amplitudes: BTreeMap::from([(Config::default(), Complex64::new(1.0, 0.0))]),
            peak_configurations: 1,
            stores_columns: true,
            ..Self::without_simulator()
        }
    }
}

impl QuantumState {
    /// Register and cell bookkeeping for a run using an external device.
    pub(crate) fn without_simulator() -> Self {
        Self {
            registers: Vec::new(),
            columns: 0,
            device_wires: WireStorage::default(),
            amplitudes: BTreeMap::new(),
            peak_qubits: 0,
            peak_configurations: 0,
            nonzero_releases: 0,
            nonzero_release_weight: 0.0,
            cells: BTreeMap::new(),
            leftovers: Vec::new(),
            spans: BTreeMap::new(),
            stores_columns: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn allocate(&mut self, width: Width) -> u64 {
        let handle = self.reserve(width);
        self.ensure_column(handle_id(handle));
        handle
    }

    /// Reserves a register of `qubits` qubits and returns the handle naming its head. The simulator gives a
    /// register of more than 64 qubits the first run of consecutive free columns long enough, growing the
    /// table and every configuration fallibly: a register the host cannot hold is `QubitStorage`, with the
    /// state unchanged. A run on an external device reserves the head alone.
    pub(crate) fn reserve_register(&mut self, qubits: Qubits) -> Result<u64, RunError> {
        let count = qubits.count().div_ceil(COLUMN_QUBITS) as usize;
        let head_width = Width::new(qubits.count().min(COLUMN_QUBITS)).expect("1 to 64 qubits");
        if !self.stores_columns {
            return Ok(self.reserve(head_width));
        }
        self.admit(u64::from(qubits.count()))?;
        if count == 1 {
            return Ok(self.reserve(head_width));
        }
        let unstorable = |_| RunError::QubitStorage { qubits: u64::from(qubits.count()) };
        let head = self.free_run(count);
        let end = head + count;
        if let Some(missing) = end.checked_sub(self.registers.len()).filter(|&missing| missing > 0) {
            self.registers.try_reserve(missing).map_err(unstorable)?;
        }
        self.try_grow_columns(end).map_err(unstorable)?;
        if end > self.registers.len() {
            self.registers.resize(end, RegisterState { generation: 0, width: None });
        }
        for (register, width) in self.registers[head..end].iter_mut().zip(column_widths(qubits)) {
            register.generation = register.generation.wrapping_add(1);
            register.width = Some(width);
        }
        self.spans.insert(head, count);
        self.note_qubit_peak(0);
        Ok(u64::from(self.registers[head].generation) << 32 | head as u64)
    }

    /// Fails with `QubitCount` if holding `more` qubits besides those held now would take the simulator's
    /// total past what its counts hold.
    pub(crate) fn admit(&self, more: u64) -> Result<(), RunError> {
        admitted(self.live_qubits(), more)
    }

    /// The first id of `count` consecutive free ids, those beyond the table included.
    fn free_run(&self, count: usize) -> usize {
        let mut start = 0;
        for (id, register) in self.registers.iter().enumerate() {
            if register.width.is_some() {
                start = id + 1;
            } else if id + 1 - start == count {
                return start;
            }
        }
        start
    }

    /// The columns of the register whose head is `id`, least significant first.
    pub(crate) fn columns_of(&self, id: usize) -> Range<usize> {
        id..id + self.spans.get(&id).copied().unwrap_or(1)
    }

    /// Reserves a register of `width` qubits and returns its handle: the generation in the high
    /// half, the id in the low half.
    pub(crate) fn reserve(&mut self, width: Width) -> u64 {
        let id = self.registers.iter().position(|r| r.width.is_none()).unwrap_or(self.registers.len());
        if id == self.registers.len() {
            self.registers.push(RegisterState { generation: 0, width: None });
        }
        let register = &mut self.registers[id];
        register.generation = register.generation.wrapping_add(1);
        register.width = Some(width);
        let handle = u64::from(register.generation) << 32 | id as u64;
        self.note_qubit_peak(0);
        handle
    }

    pub(crate) fn ensure_column(&mut self, id: usize) {
        if id >= self.columns {
            self.set_columns(id + 1);
        }
    }

    /// The simulator reports the probability that any of the selected bits is nonzero.
    /// Each configuration contributes at most once, even when several columns are dirty. A register of
    /// several columns is kept or removed whole. Fails, with nothing changed, if the host cannot hold the
    /// record of the columns kept.
    pub(crate) fn discard(&mut self, masks: BTreeMap<usize, u64>) -> Result<(), RunError> {
        if masks.is_empty() {
            return Ok(());
        }
        let mut selected: Vec<_> = masks.into_iter().map(|(column, mask)| (column, mask, false)).collect();
        let mut any_dirty = false;
        let mut weight = 0.0;
        for (config, amplitude) in &self.amplitudes {
            let mut nonzero = false;
            for (column, mask, dirty) in &mut selected {
                if config[*column] & *mask != 0 {
                    *dirty = true;
                    nonzero = true;
                }
            }
            if nonzero {
                any_dirty = true;
                weight += amplitude.norm_sqr();
            }
        }
        let kept: u64 =
            selected.iter().filter(|(.., dirty)| *dirty).map(|(_, mask, _)| u64::from(mask.count_ones())).sum();
        self.leftovers.try_reserve(selected.len()).map_err(|_| RunError::QubitStorage { qubits: kept })?;
        if any_dirty {
            self.nonzero_releases += 1;
            // Rounding can place the sum marginally above one.
            self.nonzero_release_weight += weight.min(1.0);
        }
        let groups: Vec<Range<usize>> = self
            .spans
            .iter()
            .filter(|&(head, _)| selected.binary_search_by_key(head, |&(column, ..)| column).is_ok())
            .map(|(&head, &count)| head..head + count)
            .collect();
        let (grouped, single): (Vec<_>, Vec<_>) =
            selected.into_iter().partition(|(column, ..)| groups.iter().any(|group| group.contains(column)));
        for (column, mask, dirty) in single {
            self.release_bits(column, mask, dirty);
        }
        for group in groups {
            if grouped.iter().any(|(column, _, dirty)| *dirty && group.contains(column)) {
                self.leave_group(group);
            } else {
                self.free_group(group);
            }
        }
        Ok(())
    }

    pub(crate) fn release_zero(&mut self, id: usize, mask: u64) {
        self.release_bits(id, mask, false);
    }

    fn release_bits(&mut self, id: usize, mask: u64, dirty: bool) {
        if mask == self.device_wires.mask(id) {
            if dirty {
                self.leave(id);
            } else {
                self.free(id);
            }
        } else {
            self.release_part(id, mask, dirty);
        }
    }

    /// Unbind just these bits. Preserve a dirty subset in its own unnamed column so the remaining
    /// live bits keep their values and correlations, and the released wire names can be used again.
    fn release_part(&mut self, id: usize, mask: u64, keep: bool) {
        let width = self.registers[id].width.expect("live column").bits();
        let removed = mask.count_ones();
        self.registers[id].width = Width::new(width - removed);
        self.device_wires.unbind(id, mask);
        if keep {
            let handle = self.reserve(Width::new(removed).expect("released bits"));
            let hidden = handle_id(handle);
            self.ensure_column(hidden);
            let positions: Vec<_> = (0..64).filter(|bit| mask >> bit & 1 == 1).collect();
            self.amplitudes = self
                .take_amplitudes()
                .into_iter()
                .map(|(mut config, amplitude)| {
                    let old = config[id];
                    config[hidden] = positions
                        .iter()
                        .enumerate()
                        .fold(0, |value, (bit, &source)| value | ((old >> source & 1) << bit));
                    config[id] &= !mask;
                    (config, amplitude)
                })
                .collect();
            self.leave(hidden);
        }
    }

    pub(crate) fn just_left_over(&self, id: usize) -> bool {
        self.leftovers.last() == Some(&id)
    }

    /// Undoes `allocate`: the register's columns are dropped and its ids get their previous generation.
    /// Configurations where the register is not zero are dropped: once every later step is undone they can
    /// only be rounding residue.
    pub(crate) fn unallocate(&mut self, handle: u64) {
        let id = self.resolve(handle).expect("an allocation being undone is live");
        let group = self.columns_of(id);
        self.amplitudes.retain(|config, _| config[group.clone()].iter().all(|&word| word == 0));
        for register in &mut self.registers[group.clone()] {
            register.generation = register.generation.wrapping_sub(1);
        }
        self.free_group(group);
        // Ids that were never allocated before are forgotten again, as `allocate` found them.
        while self.registers.last().is_some_and(|r| r.width.is_none() && r.generation == 0) {
            self.registers.pop();
        }
    }

    /// Undoes `allocate` of the live register in `column`, as `unallocate` does for its handle.
    pub(crate) fn unallocate_column(&mut self, column: usize) {
        let handle = u64::from(self.registers[column].generation) << 32 | column as u64;
        self.unallocate(handle);
    }

    /// The column of the cell holding the memory byte at `address`, if the byte holds qubits.
    pub(crate) fn cell(&self, address: u64) -> Option<usize> {
        self.cells.get(&address).copied()
    }

    /// Whether any of the bytes of `range` holds qubits.
    pub(crate) fn has_cells_in(&self, range: std::ops::Range<u64>) -> bool {
        self.cells.range(range).next().is_some()
    }

    /// The cells among the bytes of `range`, in address order.
    pub(crate) fn cells_in(&self, range: std::ops::Range<u64>) -> Vec<(u64, usize)> {
        self.cells.range(range).map(|(&address, &column)| (address, column)).collect()
    }

    /// Every cell, in address order.
    pub(crate) fn cells(&self) -> &BTreeMap<u64, usize> {
        &self.cells
    }

    /// Binds the byte at `address` to register `column`, or unbinds it (`None`).
    pub(crate) fn bind_cell(&mut self, address: u64, column: Option<usize>) {
        match column {
            Some(column) => self.cells.insert(address, column),
            None => self.cells.remove(&address),
        };
    }

    /// Whether register `id` is zero in every configuration.
    pub(crate) fn is_zero(&self, id: usize) -> bool {
        self.amplitudes.keys().all(|config| config[id] == 0)
    }

    /// Whether the selected bits are zero in every stored configuration.
    pub(crate) fn mask_is_zero(&self, id: usize, mask: u64) -> bool {
        self.amplitudes.keys().all(|config| config[id] & mask == 0)
    }

    /// Frees the register, which is zero in every configuration: its column goes, which is exact.
    pub(crate) fn free(&mut self, id: usize) {
        debug_assert!(self.is_zero(id), "only a register zero in every configuration is freed");
        self.retire(id);
    }

    /// Frees the consecutive columns `group`, a register of several columns zero in every configuration, at
    /// once; the table and the configurations shrink once.
    fn free_group(&mut self, group: Range<usize>) {
        self.spans.remove(&group.start);
        for column in group.clone() {
            self.device_wires.unbind(column, u64::MAX);
            self.registers[column].width = None;
        }
        self.shrink_columns();
    }

    /// Keeps the register's qubits as left over instead of freeing them: its handle goes stale (the
    /// generation moves on, so no handle names it), its column and amplitudes stay, and its id is not reused.
    /// The stored state is then a purification of the state after the partial trace over the register.
    fn leave(&mut self, id: usize) {
        self.device_wires.unbind(id, u64::MAX);
        let register = &mut self.registers[id];
        register.generation = register.generation.wrapping_add(1);
        self.leftovers.push(id);
    }

    /// Leaves every column of `group`, the head last, so that the head is the last register left over.
    fn leave_group(&mut self, group: Range<usize>) {
        for column in group.clone().skip(1).chain([group.start]) {
            self.leave(column);
        }
    }

    /// Takes back the column `id` left over, its qubits named again by the wires from `first`.
    fn reclaim(&mut self, id: usize, first: Wire) {
        let index = self.leftovers.iter().rposition(|&left| left == id).expect("a register left over is taken back");
        self.leftovers.remove(index);
        let register = &mut self.registers[id];
        register.generation = register.generation.wrapping_sub(1);
        let count = Wire::from(register.width.expect("kept column").bits());
        self.device_wires.bind(id, first..first + count).expect("storage the register held");
    }

    /// Undoes `leave` of a whole register: its handle is live again. A register of several columns was
    /// the last left over, so its columns are the last entries of the list.
    pub(crate) fn take_back(&mut self, handle: u64) {
        let id = handle_id(handle);
        let group = self.columns_of(id);
        if group.len() == 1 {
            return self.reclaim(id, wire(id, 0));
        }
        let kept = self.leftovers.len() - group.len();
        debug_assert!(self.leftovers[kept..].iter().all(|column| group.contains(column)));
        self.leftovers.truncate(kept);
        let mut qubits = 0;
        for register in &mut self.registers[group.clone()] {
            register.generation = register.generation.wrapping_sub(1);
            qubits += register.width.expect("kept column").bits();
        }
        self.device_wires.bind(id, wires(id, qubits)).expect("storage the register held");
    }

    /// The qubits of registers left over.
    pub(crate) fn leftover_qubits(&self) -> u64 {
        qubit_total(self.leftovers.iter().filter_map(|&id| self.registers[id].width))
    }

    /// Undoes `free` of a whole register of `qubits` qubits: it is live again in the columns it had, zero in
    /// every configuration, with the same handle.
    pub(crate) fn revive(&mut self, handle: u64, qubits: Qubits) {
        let id = handle_id(handle);
        let count = qubits.count().div_ceil(COLUMN_QUBITS) as usize;
        for (register, width) in self.registers[id..id + count].iter_mut().zip(column_widths(qubits)) {
            register.width = Some(width);
        }
        self.device_wires.bind(id, wires(id, qubits.count())).expect("storage the register held");
        self.ensure_column(id + count - 1);
        if count > 1 {
            self.spans.insert(id, count);
        }
    }

    /// Frees the column `id`; the head of a register of several columns forgets its span.
    pub(crate) fn retire(&mut self, id: usize) {
        self.spans.remove(&id);
        self.device_wires.unbind(id, u64::MAX);
        self.registers[id].width = None;
        self.shrink_columns();
    }

    /// Drops the columns after the last live one.
    fn shrink_columns(&mut self) {
        let columns = self.registers.iter().rposition(|r| r.width.is_some()).map_or(0, |last| last + 1);
        if columns != self.columns {
            self.set_columns(columns);
        }
    }

    /// Resizes every configuration to `columns` columns. Dropped columns are zero, so no two
    /// configurations merge.
    fn set_columns(&mut self, columns: usize) {
        self.columns = columns;
        let resize = |(config, amplitude): (Config, Complex64)| {
            let mut config = config.into_vec();
            config.resize(columns, 0);
            (config.into_boxed_slice(), amplitude)
        };
        self.amplitudes = std::mem::take(&mut self.amplitudes).into_iter().map(resize).collect();
    }

    /// Grows every configuration to at least `columns` columns, or fails with the state unchanged if the
    /// host cannot hold them.
    fn try_grow_columns(&mut self, columns: usize) -> Result<(), TryReserveError> {
        if columns <= self.columns {
            return Ok(());
        }
        let mut grown = BTreeMap::new();
        for (config, &amplitude) in &self.amplitudes {
            let mut words = Vec::new();
            words.try_reserve_exact(columns)?;
            words.extend_from_slice(config);
            words.resize(columns, 0);
            grown.insert(words.into_boxed_slice(), amplitude);
        }
        self.amplitudes = grown;
        self.columns = columns;
        Ok(())
    }

    /// The column of a live register's handle.
    pub(crate) fn resolve(&self, handle: u64) -> Result<usize, Fault> {
        let id = handle_id(handle);
        let generation = (handle >> 32) as u32;
        match self.registers.get(id) {
            Some(register) if register.width.is_some() && register.generation == generation => Ok(id),
            _ => Err(Fault::DanglingRegister),
        }
    }

    /// Applies `matrix` to `target` in every configuration where all `controls` are 1. If the result
    /// would have more than `limit` configurations, the state is left unchanged and the run stops.
    pub(crate) fn apply_gate(
        &mut self,
        matrix: &Matrix,
        target: QubitRef,
        controls: &[QubitRef],
        limit: Option<usize>,
    ) -> Result<(), RunError> {
        let mut next = BTreeMap::new();
        for (config, &amplitude) in &self.amplitudes {
            if !controls.iter().all(|&control| control.is_set(config)) {
                accumulate(&mut next, config.clone(), amplitude);
                continue;
            }
            let column = usize::from(target.is_set(config));
            let mut zero = config.clone();
            target.assign(&mut zero, false);
            let mut one = config.clone();
            target.assign(&mut one, true);
            accumulate(&mut next, zero, matrix[0][column] * amplitude);
            accumulate(&mut next, one, matrix[1][column] * amplitude);
        }
        next.retain(|_, amplitude| amplitude.norm() > NEGLIGIBLE_AMPLITUDE);
        if let Some(limit) = limit.filter(|&limit| next.len() > limit) {
            return Err(RunError::ConfigurationLimit { limit });
        }
        self.replace_amplitudes(next);
        Ok(())
    }

    /// XORs `value` into register `column` on every configuration: a permutation of configurations.
    pub(crate) fn xor_register(&mut self, column: usize, value: u64) {
        if value == 0 {
            return;
        }
        let xor = |(mut config, amplitude): (Config, Complex64)| {
            config[column] ^= value;
            (config, amplitude)
        };
        self.amplitudes = std::mem::take(&mut self.amplitudes).into_iter().map(xor).collect();
    }

    /// Samples, jointly, the bits `mask` of each column `column` in `picks` and collapses the state onto the
    /// values found, one per pick: one draw, the values in the order of the picks compared as a sequence (for
    /// one whole column, the order of its values, as `measure` always had). Other bits keep their
    /// superposition.
    pub(crate) fn measure_bits(&mut self, picks: &[(usize, u64)], rng: &mut impl Rng) -> Vec<u64> {
        let key = |config: &[u64]| -> Vec<u64> { picks.iter().map(|&(column, mask)| config[column] & mask).collect() };
        let mut probabilities = BTreeMap::<Vec<u64>, f64>::new();
        for (config, amplitude) in &self.amplitudes {
            *probabilities.entry(key(config)).or_default() += amplitude.norm_sqr();
        }
        let total: f64 = probabilities.values().sum();
        let mut threshold = rng.random::<f64>() * total;
        let mut outcome = probabilities.last_key_value().map_or((vec![0; picks.len()], 1.0), |(v, &p)| (v.clone(), p));
        for (value, &probability) in &probabilities {
            if threshold < probability {
                outcome = (value.clone(), probability);
                break;
            }
            threshold -= probability;
        }
        let (value, probability) = outcome;
        let scale = probability.sqrt().recip();
        self.amplitudes.retain(|config, _| key(config) == value);
        self.amplitudes.values_mut().for_each(|amplitude| *amplitude *= scale);
        value
    }

    /// Removes the amplitudes, for a caller that rebuilds them by a permutation of configurations.
    pub(crate) fn take_amplitudes(&mut self) -> BTreeMap<Config, Complex64> {
        std::mem::take(&mut self.amplitudes)
    }

    /// Installs amplitudes built from `take_amplitudes`.
    pub(crate) fn replace_amplitudes(&mut self, amplitudes: BTreeMap<Config, Complex64>) {
        self.amplitudes = amplitudes;
        self.peak_configurations = self.peak_configurations.max(self.amplitudes.len());
    }

    pub(crate) fn registers(&self) -> &[RegisterState] {
        &self.registers
    }

    /// The registers stored in more than one column: head and number of columns, in head order.
    pub(crate) fn spans(&self) -> &BTreeMap<usize, usize> {
        &self.spans
    }

    pub(crate) fn amplitudes(&self) -> &BTreeMap<Config, Complex64> {
        &self.amplitudes
    }

    pub(crate) fn live_qubits(&self) -> u64 {
        qubit_total(self.registers.iter().filter_map(|r| r.width))
    }

    /// Include private circuit scratch in the peak even when the simulator packs it separately.
    pub(crate) fn note_qubit_peak(&mut self, scratch: usize) {
        let scratch = u64::try_from(scratch).unwrap_or(u64::MAX);
        self.peak_qubits = self.peak_qubits.max(self.live_qubits().saturating_add(scratch));
    }

    pub(crate) fn peak_qubits(&self) -> u64 {
        self.peak_qubits
    }

    /// The simulator's count of stored basis configurations.
    pub(crate) fn configurations(&self) -> usize {
        self.amplitudes.len()
    }

    pub(crate) fn peak_configurations(&self) -> usize {
        self.peak_configurations
    }
}

/// Whether the simulator may hold `more` qubits besides `live`: its qubit counts in `Stats` are `u32`, and
/// registers of up to 2^32 - 1 qubits each could total more.
fn admitted(live: u64, more: u64) -> Result<(), RunError> {
    match live.checked_add(more) {
        Some(total) if total <= u64::from(u32::MAX) => Ok(()),
        Some(_) | None => Err(RunError::QubitCount { live, more }),
    }
}

/// The qubits of columns of these widths: a `u64`, since registers of up to 2^32 - 1 qubits each can total
/// more than a `u32` counts.
fn qubit_total(widths: impl Iterator<Item = Width>) -> u64 {
    widths.map(|width| u64::from(width.bits())).sum()
}

pub(crate) fn handle_id(handle: u64) -> usize {
    (handle & u64::from(u32::MAX)) as usize
}

fn accumulate(map: &mut BTreeMap<Config, Complex64>, config: Config, amplitude: Complex64) {
    if amplitude != Complex64::new(0.0, 0.0) {
        *map.entry(config).or_default() += amplitude;
    }
}

#[cfg(test)]
#[path = "measure_sampling_tests.rs"]
mod measure_sampling_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The columns of the widest register and of one more qubit total 2^32 qubits, one more than a
    /// `u32` holds; the widths are generated lazily, so no table of 2^26 columns is built.
    #[test]
    fn qubit_totals_count_past_32_bits() {
        let widest = Qubits::new(u32::MAX).expect("the widest register");
        let one = Qubits::new(1).expect("one qubit");
        assert_eq!(qubit_total(column_widths(widest).chain(column_widths(one))), 1 << 32);
    }

    /// The simulator holds at most 2^32 - 1 qubits at once, its counts' limit, and stops by name past it.
    #[test]
    fn holding_more_qubits_than_the_counts_hold_is_refused_by_name() {
        let most = u64::from(u32::MAX);
        assert_eq!(admitted(most - 1, 1), Ok(()));
        assert_eq!(admitted(0, most), Ok(()));
        assert_eq!(admitted(most, 1), Err(RunError::QubitCount { live: most, more: 1 }));
        assert_eq!(admitted(1, most), Err(RunError::QubitCount { live: 1, more: most }));
        assert_eq!(admitted(u64::MAX, 1), Err(RunError::QubitCount { live: u64::MAX, more: 1 }));
    }

    /// A register of several columns takes the first run of free columns long enough, beyond the table if
    /// need be, and leaves narrower gaps to narrow registers.
    #[test]
    fn a_wide_register_takes_consecutive_columns() {
        let mut state = QuantumState::default();
        let byte = Width::BYTE;
        let [a, b, c] = [(); 3].map(|()| handle_id(state.allocate(byte)));
        state.free(a);
        state.free(c);
        let wide = Qubits::new(129).expect("a width");
        let head = handle_id(state.reserve_register(wide).expect("stored"));
        assert_eq!((b, head, state.columns_of(head)), (1, 2, 2..5));
        assert_eq!(handle_id(state.reserve(byte)), 0, "the gap before the wide register");
        assert_eq!(state.live_qubits(), 8 + 129 + 8);
    }
}
