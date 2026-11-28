//! The step report of a measuring run (`docs/DESIGN.md`, "Step budgets"): for each `apply` the simulator
//! evaluates directly, how many steps its inputs used, so that a program can take a budget from a run that
//! measured it instead of guessing one.

use bennett_ir::{Fault, FuncId};

use crate::executor::Outcome;

/// One executed `apply`, evaluated directly on each basis configuration of nonzero amplitude at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApplyReport {
    /// Its position among the `apply` instructions the run executed, from 1.
    pub position: u64,
    /// The applied function.
    pub callee: FuncId,
    /// The inputs evaluated: the basis configurations of nonzero amplitude at the `apply`.
    pub inputs: u64,
    /// The largest number of steps an input used to return or to fault with a code other than `budget`;
    /// `None` if every input reached the budget.
    pub largest: Option<u64>,
    /// The inputs that reached the budget (the code `budget`): they have no count.
    pub at_budget: u64,
}

impl ApplyReport {
    /// The report of the `position`-th `apply`, from each input's outcome and the steps its run used.
    pub(crate) fn new(position: u64, callee: FuncId, runs: &[(Outcome, u64)]) -> Self {
        let finished = |&(outcome, steps): &(Outcome, u64)| match outcome {
            Outcome::Faulted(Fault::Budget) => None,
            Outcome::Returned(_) | Outcome::Faulted(_) | Outcome::Stopped(_) => Some(steps),
        };
        let largest = runs.iter().filter_map(finished).max();
        let at_budget = runs.iter().filter(|run| finished(run).is_none()).count() as u64;
        ApplyReport { position, callee, inputs: runs.len() as u64, largest, at_budget }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_count_leaves_out_inputs_that_reached_the_budget() {
        let runs = [
            (Outcome::Returned(Some(1)), 7),
            (Outcome::Faulted(Fault::DivideByZero), 9),
            (Outcome::Faulted(Fault::Budget), 20),
            (Outcome::Returned(Some(0)), 3),
        ];
        let report = ApplyReport::new(2, FuncId(1), &runs);
        assert_eq!(report, ApplyReport { position: 2, callee: FuncId(1), inputs: 4, largest: Some(9), at_budget: 1 });
        let all = ApplyReport::new(1, FuncId(0), &[(Outcome::Faulted(Fault::Budget), 5)]);
        assert_eq!((all.largest, all.at_budget), (None, 1));
    }
}
