//! The step report of a measuring run (`docs/DESIGN.md`, "The step report"): for each `apply` the simulator
//! evaluates directly, how many steps its inputs used, so that a program can take a budget from a run that
//! measured it instead of guessing one. The counts are the executor's own step accounting of each input's
//! run, read where the run ends: a run that returned counts its steps, a run that faulted counts the
//! faulting step too (it needs that step of budget to fault), and a run that the budget ended has no count.

use bennett_ir::FuncId;

/// How one input's run of an applied function ended, in steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunSteps {
    /// It returned or faulted after this many steps of the budget, the faulting step included; a run whose
    /// entry frame did not fit faulted before any step, with 0.
    Finished(u64),
    /// The budget ended it: it had taken every step the budget allowed and was still running. An explicit
    /// `fault budget` is a fault, not this.
    Exhausted,
}

/// One executed `apply`, evaluated directly on each basis configuration of nonzero amplitude at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApplyReport {
    /// Its position among the `apply` instructions the run executed, from 1, whether evaluated directly or
    /// compiled to gates; an `apply` undone and executed again keeps its position.
    pub position: u64,
    /// The applied function.
    pub callee: FuncId,
    /// The budget operand's value at the `apply`: the ceiling a measuring run gave it.
    pub budget: u64,
    /// The inputs evaluated: the basis configurations of nonzero amplitude at the `apply`.
    pub inputs: u64,
    /// The largest step count of an input that returned or faulted; `None` if every input reached the budget.
    pub largest: Option<u64>,
    /// The inputs the budget ended: they have no count.
    pub at_budget: u64,
}

impl ApplyReport {
    /// The report of the `position`-th `apply`, from the way each input's run ended.
    pub(crate) fn new(position: u64, callee: FuncId, budget: u64, runs: &[RunSteps]) -> Self {
        let finished = |run: &RunSteps| match *run {
            RunSteps::Finished(steps) => Some(steps),
            RunSteps::Exhausted => None,
        };
        let largest = runs.iter().filter_map(finished).max();
        let at_budget = runs.iter().filter(|run| finished(run).is_none()).count() as u64;
        ApplyReport { position, callee, budget, inputs: runs.len() as u64, largest, at_budget }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_count_leaves_out_inputs_the_budget_ended() {
        let runs = [RunSteps::Finished(7), RunSteps::Finished(9), RunSteps::Exhausted, RunSteps::Finished(3)];
        let report = ApplyReport::new(2, FuncId(1), 20, &runs);
        let expected =
            ApplyReport { position: 2, callee: FuncId(1), budget: 20, inputs: 4, largest: Some(9), at_budget: 1 };
        assert_eq!(report, expected);
        let all = ApplyReport::new(1, FuncId(0), 5, &[RunSteps::Exhausted]);
        assert_eq!((all.largest, all.at_budget), (None, 1));
    }
}
