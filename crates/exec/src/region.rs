//! `apply`: a coherent region (`docs/DESIGN.md`, "`apply`: a coherent region").
//!
//! `apply @f(args) -> out, fault budget B` is one step of the run that called it. For each definite value
//! of the argument registers (each basis configuration of nonzero amplitude), `f` runs on those values, on
//! private memory, within `B` steps of its own, and its result is XORed into `out` (or its fault code into
//! `fault`); then the run is undone, so nothing of it remains but the XOR. A region cannot touch a
//! register, so the configurations are permuted and their amplitudes carried along unchanged.

use std::collections::BTreeMap;

use bennett_ir::{Fault, FuncId, Operand, Slot, Type};

use crate::apply_device::{AppliedCircuit, ApplyInput, Argument};
use crate::device_io::{DeviceContext, wire, wires};
use crate::executor::{Executor, Halt, Outcome, RunError};
use crate::journal::Journal;
use crate::quantum::Config;

/// What computing coherent regions cost the executor, summed over every `apply` executed, forward or
/// backward. A run is one evaluation of the callee on one basis configuration. A report of the simulator,
/// not part of the program's meaning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegionCost {
    /// Runs of region callees.
    pub runs: u64,
    /// Steps executed for regions, forward and back (each run undone after its result is XORed out).
    pub work: u64,
    /// The most journal bytes each run held, summed over runs.
    pub journal_bytes: u64,
}

/// Where an argument's value comes from on each configuration.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Input {
    /// A register, read as an integer: a column of the configuration.
    Register(usize),
    /// A classical value, the same on every configuration.
    Value(u128),
}

impl<'m> Executor<'m> {
    /// XORs `callee(args)` into `out`, or its fault code into `fault`, on every configuration (module
    /// documentation).
    ///
    /// Every execution that faults on some configuration is counted, with the weight of the faulting
    /// configurations. Unless the run allows region faults, such an execution stops the run instead, with
    /// the state as it was before the `apply`.
    pub(crate) fn apply(
        &mut self,
        callee: FuncId,
        args: &[Operand],
        out: Slot,
        fault: Slot,
        budget: &Operand,
    ) -> Result<(), Halt> {
        let (out, fault, inputs) = self.region_operands(args, out, fault)?;
        let budget = self.read_word(budget);
        let mut values = Vec::with_capacity(inputs.len());
        // This evaluation is the simulator's implementation of apply. It requires amplitudes.
        let original = self.quantum.take_amplitudes();
        self.measure_begin_apply(callee);
        let mut outcomes = Vec::with_capacity(original.len());
        for config in original.keys() {
            values.clear();
            values.extend(inputs.iter().map(|input| match *input {
                Input::Register(column) => u128::from(config[column]),
                Input::Value(value) => value,
            }));
            outcomes.push(self.evaluate(callee, &values, budget));
        }
        let faulting: Vec<(&Config, f64, Fault)> = original
            .iter()
            .zip(&outcomes)
            .filter_map(|((config, amplitude), outcome)| match *outcome {
                Outcome::Faulted(code) => Some((config, amplitude.norm_sqr(), code)),
                Outcome::Returned(_) | Outcome::Stopped(_) => None,
            })
            .collect();
        if let Some(error) = self.count_region_faults(callee, &faulting, original.len()) {
            self.quantum.replace_amplitudes(original);
            return Err(error.into());
        }
        let mut permuted = BTreeMap::new();
        for ((mut config, amplitude), outcome) in original.into_iter().zip(outcomes) {
            match outcome {
                // The validator made the result as wide as the register, at most 64 bits.
                Outcome::Returned(result) => config[out] ^= result.expect("a region callee returns an integer") as u64,
                Outcome::Faulted(code) => config[fault] ^= code.code(),
                // The simulator's stops concern gates and `apply`, which fault `quantum_in_region` in a region
                // first: a stop of the simulator never becomes a fault code the program could read.
                Outcome::Stopped(_) => unreachable!("a region reaches no gate or apply, so no stop of the simulator"),
            }
            permuted.insert(config, amplitude);
        }
        self.quantum.replace_amplitudes(permuted);
        Ok(())
    }

    /// Resolves and compiles the whole operation before sending any commands for it.
    pub(crate) fn apply_gates(
        &mut self,
        callee: FuncId,
        args: &[Operand],
        out: Slot,
        fault: Slot,
        budget: &Operand,
        context: &mut DeviceContext<'_>,
    ) -> Result<(), Halt> {
        let (out, fault, inputs) = self.region_operands(args, out, fault)?;
        let range = |column: usize| {
            let register = &self.quantum.registers()[column];
            wires(column, register.width.expect("resolved register is live").bits())
        };
        let args = inputs
            .into_iter()
            .map(|input| match input {
                Input::Value(value) => Argument::Value(value),
                Input::Register(column) => Argument::Register(range(column)),
            })
            .collect::<Vec<_>>();
        let input = ApplyInput {
            module: self.module,
            callee,
            args: &args,
            out: range(out),
            fault: range(fault),
            budget: self.read_word(budget),
            entry_fault: self.frame_fits(callee).err(),
            scratch_start: wire(self.quantum.registers().len(), 0),
        };
        let compiler = context.compiler.as_deref_mut().expect("gate compiler selected");
        let block = compiler.compile(input.clone())?;
        let circuit = AppliedCircuit::check(&input, &block)?;
        if let Some(device) = context.external.as_deref_mut() {
            AppliedCircuit::send(&block, device)?;
        } else {
            circuit.simulate(&mut self.quantum)?;
        }
        Ok(())
    }

    /// The columns of `out` and `fault` and where each argument comes from; `qubit_alias` if a target is
    /// an argument or both are one register.
    fn region_operands(&self, args: &[Operand], out: Slot, fault: Slot) -> Result<(usize, usize, Vec<Input>), Halt> {
        let out = self.quantum.resolve(self.handle(out))?;
        let fault = self.quantum.resolve(self.handle(fault))?;
        let inputs = args.iter().map(|arg| self.input(arg)).collect::<Result<Vec<_>, _>>()?;
        let targets = [Input::Register(out), Input::Register(fault)];
        if out == fault || inputs.iter().any(|input| targets.contains(input)) {
            return Err(Fault::QubitAlias.into());
        }
        Ok((out, fault, inputs))
    }

    fn input(&self, arg: &Operand) -> Result<Input, Fault> {
        match arg {
            Operand::Slot(slot) if matches!(self.slot_type(*slot), Type::Qreg(_)) => {
                Ok(Input::Register(self.quantum.resolve(self.handle(*slot))?))
            }
            Operand::Slot(_) | Operand::Const(_) => Ok(Input::Value(self.read(arg))),
        }
    }

    /// The fault counts of an `apply` whose faulting configurations are `faulting` (configuration, its
    /// squared amplitude, the fault), in configuration order: counted, and unless the run allows region
    /// faults, the stop to return.
    pub(crate) fn count_region_faults(
        &mut self,
        callee: FuncId,
        faulting: &[(&Config, f64, Fault)],
        configurations: usize,
    ) -> Option<RunError> {
        let &(_, _, code) = faulting.first()?;
        let weight: f64 = faulting.iter().map(|&(_, weight, _)| weight).sum();
        self.region_faults += 1;
        self.region_fault_weight += weight;
        self.region_fault_max_weight = self.region_fault_max_weight.max(weight);
        (!self.allow_region_faults).then_some(RunError::RegionFault {
            callee,
            fault: code,
            faulting: faulting.len(),
            configurations,
        })
    }

    /// Runs `callee(args)` within `budget` steps on top of the current frames, then undoes the run.
    fn evaluate(&mut self, callee: FuncId, args: &[u128], budget: u64) -> Outcome {
        let region_journal = std::mem::take(&mut self.spare_journal);
        let outer = Saved {
            journal: self.journal.replace(region_journal),
            outcome: self.outcome.take(),
            steps: std::mem::take(&mut self.steps),
            budget: self.budget.replace(budget),
            base_depth: std::mem::replace(&mut self.base_depth, self.frames.len()),
            in_region: std::mem::replace(&mut self.in_region, true),
        };
        self.run_journal_peak = 0;
        self.measure_begin_branch(args);
        let outcome = match self.push_frame(callee, args) {
            Ok(()) => {
                self.measure_entry_call(callee);
                self.run()
            }
            Err(fault) => Outcome::Faulted(fault),
        };
        self.region_steps += self.steps;
        let journal = self.journal.as_ref().expect("a region runs with a journal");
        let (records, words) = (journal.records(), journal.words().len());
        if self.journal_measurement.is_some() {
            self.measure_end_branch(outcome, self.steps, records, words);
        }
        let undone = self.unwind();
        self.region_cost.runs += 1;
        self.region_cost.work += self.steps + undone;
        self.region_cost.journal_bytes += self.run_journal_peak as u64;
        self.restore(outer);
        outcome
    }

    fn restore(&mut self, outer: Saved) {
        let mut region_journal = std::mem::replace(&mut self.journal, outer.journal).unwrap_or_default();
        region_journal.clear();
        self.spare_journal = region_journal;
        self.outcome = outer.outcome;
        self.steps = outer.steps;
        self.budget = outer.budget;
        self.base_depth = outer.base_depth;
        self.in_region = outer.in_region;
    }
}

/// The run's state set aside while a region runs for one basis configuration.
struct Saved {
    journal: Option<Journal>,
    outcome: Option<Outcome>,
    steps: u64,
    budget: Option<u64>,
    base_depth: usize,
    in_region: bool,
}
