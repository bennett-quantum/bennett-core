//! Summarize which functions can return with unconstrained arguments.

use std::collections::BTreeSet;
use std::rc::Rc;

use bennett_ir::{FuncId, Inst, ValidModule};

use super::bits::Word;
use super::symbolic::{Frame, before_instruction_fault};

#[derive(Default)]
enum ReturnStatus {
    #[default]
    Pending,
    Stops(Rc<[Option<Word>]>),
    MayReturn,
}

#[derive(Default)]
struct Summary {
    scheduled: bool,
    result: ReturnStatus,
    callers: BTreeSet<FuncId>,
}

pub(super) struct Returns<'a> {
    module: &'a ValidModule,
    summaries: Vec<Summary>,
    #[cfg(test)]
    pub(super) work: usize,
}

impl<'a> Returns<'a> {
    pub(super) fn new(module: &'a ValidModule) -> Self {
        let summaries = module.functions.iter().map(|_| Summary::default()).collect();
        Self {
            module,
            summaries,
            #[cfg(test)]
            work: 0,
        }
    }

    pub(super) fn can_return(&mut self, callee: FuncId) -> bool {
        if matches!(self.summaries[callee.index()].result, ReturnStatus::Pending) {
            self.resolve(callee);
        }
        matches!(self.summaries[callee.index()].result, ReturnStatus::MayReturn)
    }

    pub(super) fn stopping_proof(&self, callee: FuncId) -> Option<Rc<[Option<Word>]>> {
        match &self.summaries[callee.index()].result {
            ReturnStatus::Stops(proof) => Some(Rc::clone(proof)),
            ReturnStatus::Pending | ReturnStatus::MayReturn => None,
        }
    }

    fn resolve(&mut self, callee: FuncId) {
        // Start without a return witness. When one is found, recheck callers
        // that could reach another instruction by following that return.
        let mut pending = Vec::new();
        self.schedule(callee, &mut pending);
        while let Some(function) = pending.pop() {
            let index = function.index();
            if matches!(self.summaries[index].result, ReturnStatus::MayReturn) {
                continue;
            }
            let mut dependencies = BTreeSet::new();
            let proof = Frame::return_analysis(self.module, function, |target| {
                #[cfg(test)]
                {
                    self.work += 1;
                }
                let can_return = matches!(self.summaries[target.index()].result, ReturnStatus::MayReturn);
                if !can_return {
                    dependencies.insert(target);
                }
                can_return
            });
            let summary = &mut self.summaries[index];
            if let Some(proof) = proof {
                summary.result = ReturnStatus::Stops(proof.into());
                for target in dependencies {
                    self.summaries[target.index()].callers.insert(function);
                    self.schedule(target, &mut pending);
                }
            } else {
                summary.result = ReturnStatus::MayReturn;
                pending.extend(std::mem::take(&mut summary.callers));
            }
        }
    }

    fn schedule(&mut self, callee: FuncId, pending: &mut Vec<FuncId>) {
        // Discover direct dependencies once, in callee-first order. The
        // analysis still decides which calls are reachable. Indirect targets
        // and cycles can add dependencies as their proofs are evaluated.
        let mut traversal = vec![(callee, false)];
        let mut order = Vec::new();
        while let Some((function, visited)) = traversal.pop() {
            if visited {
                order.push(function);
                continue;
            }
            let summary = &mut self.summaries[function.index()];
            if summary.scheduled {
                continue;
            }
            summary.scheduled = true;
            traversal.push((function, true));
            for block in &self.module.function(function).blocks {
                let (instructions, _) = before_instruction_fault(block);
                traversal.extend(instructions.iter().filter_map(|inst| {
                    if let Inst::Call { callee, .. } = inst { Some((*callee, false)) } else { None }
                }));
            }
        }
        pending.extend(order.into_iter().rev());
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use bennett_ir::load;

    use super::super::calls::Program;
    use super::super::{ApplyInput, Argument};

    #[test]
    fn a_straight_line_summary_checks_each_call_once() {
        const CALLS: usize = 1_024;
        let mut source = String::from(
            "memory 65536\nfunc @f(i1 %x) -> i8 {\nentry:\n call @sequence()\n ret 9\n}\nfunc @sequence() {\nentry:\n",
        );
        for index in 0..CALLS {
            writeln!(source, " call @leaf{index}()").expect("write call");
        }
        source.push_str(" ret\n}\n");
        for index in 0..CALLS {
            writeln!(source, "func @leaf{index}() {{\nentry:\n ret\n}}").expect("write leaf");
        }
        let module = load(&source).expect("valid call sequence");
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("entry"),
            args: &[Argument::Register(0..1)],
            out: 1..9,
            fault: 9..14,
            budget: 0,
            entry_fault: None,
            stack_available: 0,
            memory: None,
            scratch_start: 14,
        };
        let (program, _) = Program::check(&input).expect("plan the sequence");
        assert_eq!(program.frames().count(), CALLS + 2);
        assert_eq!(program.return_analysis_work, CALLS);
    }
}
