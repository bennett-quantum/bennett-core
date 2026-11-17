//! The sparse quantum state: a map from basis configuration to complex amplitude.
//!
//! A configuration holds one column per register id: the register's value as an integer. A register
//! released while zero in every configuration has its column removed or reused; one released while not zero
//! keeps its column, unnamed and never reused, so the stored state stays the exact purification of the
//! state after the partial trace (`docs/DESIGN.md`, "Releasing a register"). The map is ordered, so
//! iteration, and therefore seeded measurement, is deterministic.

use std::collections::BTreeMap;

use bennett_ir::{Fault, Width};
use num_complex::Complex64;
use rand::Rng;

use crate::executor::RunError;
use crate::gates::Matrix;

/// The values of every register column, one basis state.
pub(crate) type Config = Box<[u64]>;

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
    fn is_set(self, config: &[u64]) -> bool {
        config[self.column] >> self.bit & 1 == 1
    }

    fn assign(self, config: &mut [u64], value: bool) {
        let mask = 1 << self.bit;
        config[self.column] = if value { config[self.column] | mask } else { config[self.column] & !mask };
    }
}

#[derive(Clone, Debug)]
pub(crate) struct QuantumState {
    registers: Vec<RegisterState>,
    columns: usize,
    amplitudes: BTreeMap<Config, Complex64>,
    peak_qubits: u32,
    peak_configurations: usize,
    /// The memory bytes that hold qubits ("cells", `docs/DESIGN.md`, "Qubits in memory"): byte address to
    /// the column of the 8-qubit register that holds the byte's value. Part of the register table.
    cells: BTreeMap<u64, usize>,
    /// The ids of registers left over, in the order they were left: released by `qfree` while not zero, so
    /// their columns are kept, with no live handle (`docs/DESIGN.md`, "Releasing a register").
    leftovers: Vec<usize>,
}

impl Default for QuantumState {
    /// No registers: one empty configuration with amplitude 1.
    fn default() -> Self {
        let amplitudes = BTreeMap::from([(Config::default(), Complex64::new(1.0, 0.0))]);
        QuantumState {
            registers: Vec::new(),
            columns: 0,
            amplitudes,
            peak_qubits: 0,
            peak_configurations: 1,
            cells: BTreeMap::new(),
            leftovers: Vec::new(),
        }
    }
}

impl QuantumState {
    /// Allocates a register of `width` qubits in |0⟩ and returns its handle: the generation in the high
    /// half, the id in the low half.
    pub(crate) fn allocate(&mut self, width: Width) -> u64 {
        let id = self.registers.iter().position(|r| r.width.is_none()).unwrap_or(self.registers.len());
        if id == self.registers.len() {
            self.registers.push(RegisterState { generation: 0, width: None });
        }
        let register = &mut self.registers[id];
        register.generation = register.generation.wrapping_add(1);
        register.width = Some(width);
        let handle = u64::from(register.generation) << 32 | id as u64;
        if id >= self.columns {
            self.set_columns(id + 1);
        }
        self.peak_qubits = self.peak_qubits.max(self.live_qubits());
        handle
    }

    /// Undoes `allocate`: the register's column is dropped and the id gets its previous generation.
    /// Configurations where the register is not zero are dropped: once every later step is undone they can
    /// only be rounding residue.
    pub(crate) fn unallocate(&mut self, handle: u64) {
        let id = self.resolve(handle).expect("an allocation being undone is live");
        self.amplitudes.retain(|config, _| config[id] == 0);
        let register = &mut self.registers[id];
        register.generation = register.generation.wrapping_sub(1);
        self.release(id);
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

    /// Whether any memory byte holds qubits.
    pub(crate) fn has_cells(&self) -> bool {
        !self.cells.is_empty()
    }

    /// The cells among the bytes of `range`, in address order.
    pub(crate) fn cells_in(&self, range: std::ops::Range<u64>) -> Vec<(u64, usize)> {
        if self.cells.is_empty() {
            return Vec::new();
        }
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

    /// The value of register `column` in the first configuration: its value in every configuration, where
    /// they agree.
    pub(crate) fn first_value(&self, column: usize) -> u64 {
        self.amplitudes.keys().next().map_or(0, |config| config[column])
    }

    /// Whether register `id` is zero in every configuration.
    pub(crate) fn is_zero(&self, id: usize) -> bool {
        self.amplitudes.keys().all(|config| config[id] == 0)
    }

    /// The weight (summed squared amplitude) of the configurations where register `id` is not zero.
    pub(crate) fn nonzero_weight(&self, id: usize) -> f64 {
        self.amplitudes.iter().filter(|(config, _)| config[id] != 0).map(|(_, amplitude)| amplitude.norm_sqr()).sum()
    }

    /// Frees the register, which is zero in every configuration: its column goes, which is exact.
    pub(crate) fn free(&mut self, id: usize) {
        debug_assert!(self.is_zero(id), "only a register zero in every configuration is freed");
        self.release(id);
    }

    /// Keeps the register's qubits as left over instead of freeing them: its handle goes stale (the
    /// generation moves on, so no handle names it), its column and amplitudes stay, and its id is not reused.
    /// The stored state is then a purification of the state after the partial trace over the register.
    pub(crate) fn leave(&mut self, id: usize) {
        let register = &mut self.registers[id];
        register.generation = register.generation.wrapping_add(1);
        self.leftovers.push(id);
    }

    /// Takes back the register `id` left over: its handle is live again. Returns the handle.
    pub(crate) fn reclaim(&mut self, id: usize) -> u64 {
        let index = self.leftovers.iter().rposition(|&left| left == id).expect("a register left over is taken back");
        self.leftovers.remove(index);
        let register = &mut self.registers[id];
        register.generation = register.generation.wrapping_sub(1);
        u64::from(register.generation) << 32 | id as u64
    }

    /// Undoes `leave`: the register's handle is live again.
    pub(crate) fn take_back(&mut self, handle: u64) {
        self.reclaim(handle_id(handle));
    }

    /// The qubits of registers left over.
    pub(crate) fn leftover_qubits(&self) -> u32 {
        self.leftovers.iter().filter_map(|&id| self.registers[id].width).map(Width::bits).sum()
    }

    /// Undoes `free`: the register is live again, zero in every configuration, with the same handle.
    pub(crate) fn revive(&mut self, handle: u64, width: Width) {
        let id = handle_id(handle);
        self.registers[id].width = Some(width);
        if id >= self.columns {
            self.set_columns(id + 1);
        }
    }

    fn release(&mut self, id: usize) {
        self.registers[id].width = None;
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
            if !controls.iter().all(|control| control.is_set(config)) {
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

    /// Samples the value of register `id` and collapses the state onto it.
    pub(crate) fn measure(&mut self, id: usize, rng: &mut impl Rng) -> u64 {
        self.measure_bits(&[(id, u64::MAX)], rng)[0]
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

    pub(crate) fn live_qubits(&self) -> u32 {
        self.registers.iter().filter_map(|r| r.width).map(Width::bits).sum()
    }

    pub(crate) fn peak_qubits(&self) -> u32 {
        self.peak_qubits
    }

    pub(crate) fn peak_configurations(&self) -> usize {
        self.peak_configurations
    }

    /// The configurations grouped by the values of `columns`, in order of those values.
    pub(crate) fn partition(&self, columns: &[usize]) -> Vec<(Vec<u64>, BTreeMap<Config, Complex64>)> {
        let mut parts = BTreeMap::<Vec<u64>, BTreeMap<Config, Complex64>>::new();
        for (config, &amplitude) in &self.amplitudes {
            let key = columns.iter().map(|&column| config[column]).collect();
            parts.entry(key).or_default().insert(config.clone(), amplitude);
        }
        parts.into_iter().collect()
    }
}

fn handle_id(handle: u64) -> usize {
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
