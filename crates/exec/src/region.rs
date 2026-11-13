//! `apply`: a coherent region, defined by branch enumeration.
//!
//! On each basis configuration the callee runs on the argument values, on the caller's memory, with
//! its own budget. Each run is Bennett's construction in miniature: compute forward keeping the journal,
//! copy the result (or fault code) out by XOR, then run the journal backward to erase every trace, so
//! the memory is private to the run and the region is a permutation that is its own inverse.

use std::collections::BTreeMap;

use bennett_ir::{Fault, FuncId, Operand, Slot, Type};

use crate::executor::{Executor, Outcome};
use crate::journal::Journal;

/// Where an argument's value comes from on each configuration.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Input {
    /// A register, read as an integer: a column of the configuration.
    Register(usize),
    /// A classical value, the same on every configuration.
    Value(u64),
}

impl Executor<'_> {
    /// XORs `callee(args)` into `out`, or its fault code into `fault`, on every configuration.
    pub(crate) fn apply(
        &mut self,
        callee: FuncId,
        args: &[Operand],
        out: Slot,
        fault: Slot,
        budget: &Operand,
    ) -> Result<(), Fault> {
        let out = self.quantum.resolve(self.slot(out))?;
        let fault = self.quantum.resolve(self.slot(fault))?;
        let inputs = args.iter().map(|arg| self.input(arg)).collect::<Result<Vec<_>, _>>()?;
        let targets = [Input::Register(out), Input::Register(fault)];
        if out == fault || inputs.iter().any(|input| targets.contains(input)) {
            return Err(Fault::QubitAlias);
        }
        let budget = self.read(budget);
        let mut values = Vec::with_capacity(inputs.len());
        let mut permuted = BTreeMap::new();
        for (mut config, amplitude) in self.quantum.take_amplitudes() {
            values.clear();
            values.extend(inputs.iter().map(|input| match *input {
                Input::Register(column) => config[column],
                Input::Value(value) => value,
            }));
            match self.evaluate(callee, &values, budget) {
                Outcome::Returned(result) => config[out] ^= result.expect("a region callee returns an integer"),
                Outcome::Faulted(code) => config[fault] ^= code.code(),
                Outcome::Stopped(_) => unreachable!("a region reaches no gate, so no limit of the quantum state"),
            }
            permuted.insert(config, amplitude);
        }
        self.quantum.replace_amplitudes(permuted);
        Ok(())
    }

    fn input(&self, arg: &Operand) -> Result<Input, Fault> {
        match arg {
            Operand::Slot(slot) if matches!(self.slot_type(*slot), Type::Qreg(_)) => {
                Ok(Input::Register(self.quantum.resolve(self.slot(*slot))?))
            }
            _ => Ok(Input::Value(self.read(arg))),
        }
    }

    /// Runs `callee(args)` within `budget` steps on top of the current frames, then undoes the run.
    fn evaluate(&mut self, callee: FuncId, args: &[u64], budget: u64) -> Outcome {
        let region_journal = std::mem::take(&mut self.spare_journal);
        let outer = Saved {
            journal: self.journal.replace(region_journal),
            outcome: self.outcome.take(),
            steps: std::mem::take(&mut self.steps),
            budget: self.budget.replace(budget),
            base_depth: std::mem::replace(&mut self.base_depth, self.frames.len()),
            in_region: std::mem::replace(&mut self.in_region, true),
        };
        let outcome = match self.push_frame(callee, args) {
            Ok(()) => self.run(),
            Err(fault) => Outcome::Faulted(fault),
        };
        self.region_steps += self.steps;
        self.unwind();
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

/// The run state of the caller of a region, set aside while the region runs.
struct Saved {
    journal: Option<Journal>,
    outcome: Option<Outcome>,
    steps: u64,
    budget: Option<u64>,
    base_depth: usize,
    in_region: bool,
}
