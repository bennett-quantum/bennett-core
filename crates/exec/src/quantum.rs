//! The sparse quantum state: a map from basis configuration to complex amplitude.
//!
//! A configuration holds one column per register id: the register's value as an integer. Freed
//! registers are zero in every configuration, so their columns can be reused or trimmed. The map is
//! ordered, so iteration, and therefore seeded measurement, is deterministic.

use std::collections::BTreeMap;

use bennett_ir::{Fault, Width};
use num_complex::Complex64;
use rand::Rng;

use crate::executor::RunError;
use crate::gates::Matrix;

/// The values of every register column, one basis state.
pub(crate) type Config = Box<[u64]>;

/// An amplitude no larger than this is treated as zero and dropped. Amplitudes of a normalised state
/// are at most 1, so this is one unit of rounding: what cancellation leaves behind.
const NEGLIGIBLE_AMPLITUDE: f64 = f64::EPSILON;

/// The state of one register id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl Default for QuantumState {
    /// No registers: one empty configuration with amplitude 1.
    fn default() -> Self {
        let amplitudes = BTreeMap::from([(Config::default(), Complex64::new(1.0, 0.0))]);
        QuantumState { registers: Vec::new(), columns: 0, amplitudes, peak_qubits: 0, peak_configurations: 1 }
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
    /// Configurations where the register is not zero can only be rounding residue and are dropped.
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

    /// Frees the register, which must be zero in every configuration.
    pub(crate) fn free(&mut self, id: usize) -> Result<(), Fault> {
        if self.amplitudes.keys().any(|config| config[id] != 0) {
            return Err(Fault::DirtyFree);
        }
        self.release(id);
        Ok(())
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

    /// Samples the value of register `id` and collapses the state onto it.
    pub(crate) fn measure(&mut self, id: usize, rng: &mut impl Rng) -> u64 {
        let mut probabilities = BTreeMap::<u64, f64>::new();
        for (config, amplitude) in &self.amplitudes {
            *probabilities.entry(config[id]).or_default() += amplitude.norm_sqr();
        }
        let total: f64 = probabilities.values().sum();
        let mut threshold = rng.random::<f64>() * total;
        let mut outcome = probabilities.last_key_value().map_or((0, 1.0), |(&v, &p)| (v, p));
        for (&value, &probability) in &probabilities {
            if threshold < probability {
                outcome = (value, probability);
                break;
            }
            threshold -= probability;
        }
        let (value, probability) = outcome;
        let scale = probability.sqrt().recip();
        self.amplitudes.retain(|config, _| config[id] == value);
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
}

fn handle_id(handle: u64) -> usize {
    (handle & u64::from(u32::MAX)) as usize
}

fn accumulate(map: &mut BTreeMap<Config, Complex64>, config: Config, amplitude: Complex64) {
    if amplitude != Complex64::new(0.0, 0.0) {
        *map.entry(config).or_default() += amplitude;
    }
}
