//! The sparse quantum state: a map from basis configuration to complex amplitude.
//!
//! A configuration holds one column per register id: the register's value as an integer. A register
//! released while zero in every configuration has its column removed or reused; one released while not zero
//! keeps its column, unnamed and never reused, so the stored state stays the exact purification of the
//! state after the partial trace (`docs/DESIGN.md`, "Releasing a register"). A register of more than 64
//! qubits is stored in consecutive columns from its head, whose id its handle names, 64 qubits each; it is
//! kept or removed whole. Which wire names which bit of which column is the wire table's
//! (`wire_storage.rs`): allocation and release work on wires, for the controller's registers as for any
//! client of the device interface. The map is ordered, so iteration, and therefore seeded measurement, is
//! deterministic.

use std::collections::{BTreeMap, TryReserveError};
use std::ops::Range;

use bennett_ir::{Fault, Qubits, Width};
use num_complex::Complex64;
use rand::Rng;

use crate::device::Wire;
use crate::device_io::wires;
use crate::executor::RunError;
use crate::gates::Matrix;
use crate::wire_storage::{WireStorage, column_bit};

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

/// The columns of the register of `qubits` qubits whose head is `head`: consecutive, 64 qubits each.
pub(crate) fn register_columns(head: usize, qubits: Qubits) -> Range<usize> {
    head..head + qubits.count().div_ceil(COLUMN_QUBITS) as usize
}

/// The bits of `value` that `mask` selects, packed from bit 0 in their order.
fn gather(value: u64, mask: u64) -> u64 {
    let mut packed = 0;
    let mut rest = mask;
    let mut bit = 0;
    while rest != 0 {
        packed |= (value >> rest.trailing_zeros() & 1) << bit;
        bit += 1;
        rest &= rest - 1;
    }
    packed
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
    /// The column table: one entry per column, a register of several columns having one per column.
    registers: Vec<RegisterState>,
    columns: usize,
    /// Which wire names which bit of which column (`wire_storage.rs`).
    pub(crate) device_wires: WireStorage,
    amplitudes: BTreeMap<Config, Complex64>,
    peak_qubits: u64,
    peak_configurations: usize,
    pub(crate) nonzero_releases: u64,
    pub(crate) nonzero_release_weight: f64,
    /// The memory bytes that hold qubits ("cells", `docs/DESIGN.md`, "Qubits in memory"): byte address to
    /// the column of the 8-qubit register that holds the byte's value. Part of the register table.
    cells: BTreeMap<u64, usize>,
    /// The columns left over, in the order they were left: kept by a release that was not zero, with no
    /// bound wire and no live handle (`docs/DESIGN.md`, "Releasing a register").
    leftovers: Vec<usize>,
    /// Whether the state stores columns (the simulator); an external device's run only names registers.
    stores_columns: bool,
}

/// What a release of wires does with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Release {
    /// The partial trace over them (`qfree`, a device's `discard`).
    Trace,
    /// Their release in zero, refused unless they are zero in every stored configuration.
    Zero,
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
            stores_columns: false,
        }
    }

    /// A register of `width` qubits, allocated and bound as the controller does it.
    #[cfg(test)]
    pub(crate) fn allocate(&mut self, width: Width) -> u64 {
        let qubits = Qubits::from(width);
        let handle = self.reserve_register(qubits).expect("a small register");
        let head = handle_id(handle);
        self.bind(head, wires(head, qubits.count()));
        handle
    }

    /// Reserves a register of `qubits` qubits and returns the handle naming its head. The simulator gives it
    /// the columns `reserve_columns` finds; a run on an external device reserves the head alone.
    pub(crate) fn reserve_register(&mut self, qubits: Qubits) -> Result<u64, RunError> {
        let head = if self.stores_columns {
            self.admit(u64::from(qubits.count()))?;
            self.reserve_columns(qubits)?
        } else {
            self.reserve_head(Width::new(qubits.count().min(COLUMN_QUBITS)).expect("1 to 64 qubits"))
        };
        Ok(u64::from(self.registers[head].generation) << 32 | head as u64)
    }

    /// Fails with `QubitCount` if holding `more` qubits besides those held now would take the simulator's
    /// total past what its counts hold.
    pub(crate) fn admit(&self, more: u64) -> Result<(), RunError> {
        admitted(self.live_qubits(), more)
    }

    /// Reserves the columns for `qubits` qubits, wires allocated together: the first run of consecutive
    /// free columns long enough, 64 qubits to a column but the last, beyond the table if need be. The table,
    /// the wire table's masks and every configuration grow fallibly: if the host cannot hold them the result
    /// is `QubitStorage`, with the state unchanged. Returns the first column.
    pub(crate) fn reserve_columns(&mut self, qubits: Qubits) -> Result<usize, RunError> {
        let count = qubits.count().div_ceil(COLUMN_QUBITS) as usize;
        self.reserve_run(count, column_widths(qubits), u64::from(qubits.count()))
    }

    /// Reserves `count` consecutive free columns of the given widths, as `reserve_columns`; `qubits` is what
    /// a refusal names.
    fn reserve_run(
        &mut self,
        count: usize,
        widths: impl Iterator<Item = Width>,
        qubits: u64,
    ) -> Result<usize, RunError> {
        let unstorable = |_| RunError::QubitStorage { qubits };
        let head = self.free_run(count);
        let end = head + count;
        if let Some(missing) = end.checked_sub(self.registers.len()).filter(|&missing| missing > 0) {
            self.registers.try_reserve_exact(missing).map_err(unstorable)?;
        }
        self.device_wires.grow(end).map_err(unstorable)?;
        // The last fallible growth, and the only one a snapshot sees.
        self.try_grow_columns(end).map_err(unstorable)?;
        if end > self.registers.len() {
            self.registers.resize(end, RegisterState { generation: 0, width: None });
        }
        for (register, width) in self.registers[head..end].iter_mut().zip(widths) {
            register.generation = register.generation.wrapping_add(1);
            register.width = Some(width);
        }
        Ok(head)
    }

    /// Reserves a register id for a run on an external device, which stores no columns.
    fn reserve_head(&mut self, width: Width) -> usize {
        let id = self.registers.iter().position(|r| r.width.is_none()).unwrap_or(self.registers.len());
        if id == self.registers.len() {
            self.registers.push(RegisterState { generation: 0, width: None });
        }
        let register = &mut self.registers[id];
        register.generation = register.generation.wrapping_add(1);
        register.width = Some(width);
        id
    }

    /// Binds `wires` to the columns reserved for them from `head`, from its bit 0, and counts them in the
    /// peak.
    pub(crate) fn bind(&mut self, head: usize, wires: Range<Wire>) {
        self.device_wires.bind(wires, column_bit(head));
        self.note_qubit_peak(0);
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

    /// Releases `wires`, every one of them bound, by the partial trace or in zero (`Release`). A release is
    /// kept or removed whole. If any of its bits is nonzero in some stored configuration, every one is kept:
    /// a column all of whose bound bits it names is left over, and the bits it names of any other column
    /// move to a fresh column, left over. Otherwise its wires are unbound and a column left with no bound
    /// bit is freed. The simulator reports the probability that any of the bits is nonzero, each
    /// configuration counted once. Fails with nothing changed if a wire is not bound, if `Zero` finds a
    /// nonzero bit, or if the host cannot hold the record of what is kept (`QubitStorage`).
    pub(crate) fn release(&mut self, wires: Range<Wire>, how: Release) -> Result<(), RunError> {
        let table = &self.device_wires;
        if !table.covers(wires.clone()) {
            return Err(RunError::Device("wire is not allocated"));
        }
        // A column named whole by one piece needs nothing more; the pieces of other columns are gathered.
        let mut gathered = BTreeMap::<usize, u64>::new();
        for piece in table.pieces(wires.clone()).filter(|piece| piece.mask() != table.mask(piece.column)) {
            *gathered.entry(piece.column).or_default() |= piece.mask();
        }
        let parts: Vec<(usize, u64)> = gathered
            .iter()
            .map(|(&column, &mask)| (column, mask))
            .filter(|&(column, mask)| mask != table.mask(column))
            .collect();
        let mut weight = 0.0;
        let mut dirty = false;
        for (config, amplitude) in &self.amplitudes {
            if table.pieces(wires.clone()).any(|piece| config[piece.column] & piece.mask() != 0) {
                dirty = true;
                weight += amplitude.norm_sqr();
            }
        }
        if dirty && how == Release::Zero {
            return Err(RunError::Device("release_zero requires zero wires"));
        }
        let mut hidden = None;
        if dirty {
            let unstorable = |_| RunError::QubitStorage { qubits: wires.end - wires.start };
            let whole = table.pieces(wires.clone()).filter(|piece| piece.mask() == table.mask(piece.column)).count();
            self.leftovers.try_reserve(whole + gathered.len()).map_err(unstorable)?;
            if !parts.is_empty() {
                let widths = parts.iter().map(|&(_, mask)| Width::new(mask.count_ones()).expect("named bits"));
                hidden = Some(self.reserve_run(parts.len(), widths, wires.end - wires.start)?);
            }
            self.nonzero_releases += 1;
            // Rounding can place the sum marginally above one.
            self.nonzero_release_weight += weight.min(1.0);
        }
        if let Some(hidden) = hidden {
            let moves: Vec<(usize, u64, usize)> =
                parts.iter().zip(hidden..).map(|(&(column, mask), to)| (column, mask, to)).collect();
            self.amplitudes = self
                .take_amplitudes()
                .into_iter()
                .map(|(mut config, amplitude)| {
                    for &(column, mask, to) in &moves {
                        config[to] = gather(config[column], mask);
                        config[column] &= !mask;
                    }
                    (config, amplitude)
                })
                .collect();
            self.leftovers.extend(hidden..hidden + parts.len());
            for register in &mut self.registers[hidden..hidden + parts.len()] {
                register.generation = register.generation.wrapping_add(1);
            }
        }
        // The columns named whole are left over or freed; the wire table is read, not changed, meanwhile.
        let Self { device_wires, registers, leftovers, .. } = self;
        let complete = |column: usize| gathered.get(&column).is_some_and(|&mask| mask == device_wires.mask(column));
        let named_whole = device_wires
            .pieces(wires.clone())
            .filter(|piece| piece.mask() == device_wires.mask(piece.column))
            .map(|piece| piece.column)
            .chain(gathered.keys().copied().filter(|&column| complete(column)));
        for column in named_whole {
            if dirty {
                registers[column].generation = registers[column].generation.wrapping_add(1);
                leftovers.push(column);
            } else {
                registers[column].width = None;
            }
        }
        device_wires.unbind(wires);
        for &(column, _) in &parts {
            registers[column].width = Width::new(device_wires.mask(column).count_ones());
        }
        if !dirty {
            self.shrink_columns();
        }
        Ok(())
    }

    /// Whether the register whose head is `id` was just released and kept (its head is a column left over).
    pub(crate) fn is_left_over(&self, id: usize) -> bool {
        self.registers[id].width.is_some() && self.device_wires.mask(id) == 0
    }

    /// Undoes `allocate` of the register of `qubits` qubits with this handle: its columns are dropped and get
    /// their previous generation. Configurations where the register is not zero are dropped: once every
    /// later step is undone they can only be rounding residue.
    pub(crate) fn unallocate(&mut self, handle: u64, qubits: Qubits) {
        let head = self.resolve(handle).expect("an allocation being undone is live");
        // A run on an external device reserved the head alone.
        let columns = if self.stores_columns { register_columns(head, qubits) } else { head..head + 1 };
        self.amplitudes.retain(|config, _| config[columns.clone()].iter().all(|&word| word == 0));
        self.device_wires.unbind(wires(head, qubits.count()));
        for register in &mut self.registers[columns] {
            register.generation = register.generation.wrapping_sub(1);
            register.width = None;
        }
        self.shrink_columns();
        // Ids that were never allocated before are forgotten again, as `allocate` found them.
        while self.registers.last().is_some_and(|r| r.width.is_none() && r.generation == 0) {
            self.registers.pop();
        }
    }

    /// Undoes `allocate` of the live register of `qubits` qubits whose head is `column`.
    pub(crate) fn unallocate_column(&mut self, column: usize, qubits: Qubits) {
        let handle = u64::from(self.registers[column].generation) << 32 | column as u64;
        self.unallocate(handle, qubits);
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

    /// Undoes the release of a register of `qubits` qubits with this handle that kept it: its columns, the
    /// last ones left over, are live again, bound to its wires.
    pub(crate) fn take_back(&mut self, handle: u64, qubits: Qubits) {
        let head = handle_id(handle);
        let columns = register_columns(head, qubits);
        let kept = self.leftovers.len() - columns.len();
        debug_assert!(self.leftovers[kept..].iter().all(|column| columns.contains(column)));
        self.leftovers.truncate(kept);
        for register in &mut self.registers[columns] {
            register.generation = register.generation.wrapping_sub(1);
        }
        self.device_wires.bind(wires(head, qubits.count()), column_bit(head));
    }

    /// The qubits of registers left over.
    pub(crate) fn leftover_qubits(&self) -> u64 {
        qubit_total(self.leftovers.iter().filter_map(|&id| self.registers[id].width))
    }

    /// Undoes the release of a register of `qubits` qubits with this handle that freed it: it is live again
    /// in the columns it had, zero in every configuration. Undoing returns to storage the run held, so its
    /// growth is not checked.
    pub(crate) fn revive(&mut self, handle: u64, qubits: Qubits) {
        let head = handle_id(handle);
        let columns = register_columns(head, qubits);
        for (register, width) in self.registers[columns.clone()].iter_mut().zip(column_widths(qubits)) {
            register.width = Some(width);
        }
        self.device_wires.grow(columns.end).expect("storage the register held");
        if columns.end > self.columns {
            self.set_columns(columns.end);
        }
        self.device_wires.bind(wires(head, qubits.count()), column_bit(head));
    }

    /// Frees the register id `id` of a run on an external device, which stores no columns.
    pub(crate) fn retire(&mut self, id: usize) {
        self.registers[id].width = None;
        self.shrink_columns();
    }

    /// Drops the columns after the last one in use.
    fn shrink_columns(&mut self) {
        let columns = self.registers.iter().rposition(|r| r.width.is_some()).map_or(0, |last| last + 1);
        if columns < self.columns {
            self.set_columns(columns);
        }
        self.device_wires.truncate(columns);
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
        for id in [a, c] {
            state.release(wires(id, 8), Release::Zero).expect("zero");
        }
        let wide = Qubits::new(129).expect("a width");
        let head = handle_id(state.reserve_register(wide).expect("stored"));
        assert_eq!((b, head, register_columns(head, wide)), (1, 2, 2..5));
        assert_eq!(
            handle_id(state.reserve_register(byte.into()).expect("stored")),
            0,
            "the gap before the wide register"
        );
        assert_eq!(state.live_qubits(), 8 + 129 + 8);
    }

    #[test]
    fn gathering_packs_the_selected_bits() {
        assert_eq!(gather(0b1010_0110, 0b1110_0100), 0b1011);
        assert_eq!(gather(u64::MAX, u64::MAX), u64::MAX);
    }
}
