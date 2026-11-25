//! Summarize which functions can return with unconstrained arguments.

use std::collections::BTreeSet;

use bennett_ir::{FuncId, ValidModule};

use super::symbolic::Frame;

#[derive(Default)]
struct Summary {
    analyzed: bool,
    can_return: bool,
    callers: BTreeSet<FuncId>,
}

pub(super) struct Returns<'a> {
    module: &'a ValidModule,
    summaries: Vec<Summary>,
}

impl<'a> Returns<'a> {
    pub(super) fn new(module: &'a ValidModule) -> Self {
        let summaries = module.functions.iter().map(|_| Summary::default()).collect();
        Self { module, summaries }
    }

    pub(super) fn can_return(&mut self, callee: FuncId) -> bool {
        if !self.summaries[callee.index()].analyzed {
            self.resolve(callee);
        }
        self.summaries[callee.index()].can_return
    }

    fn resolve(&mut self, callee: FuncId) {
        // Start without a return witness. When one is found, recheck callers
        // that could reach another instruction by following that return.
        let mut pending = BTreeSet::from([callee]);
        while let Some(function) = pending.pop_first() {
            let index = function.index();
            if self.summaries[index].can_return {
                continue;
            }
            self.summaries[index].analyzed = true;
            let mut dependencies = BTreeSet::new();
            let can_return = Frame::may_return(self.module, function, |target| {
                let can_return = self.summaries[target.index()].can_return;
                if !can_return {
                    dependencies.insert(target);
                }
                can_return
            });
            if can_return {
                let summary = &mut self.summaries[index];
                summary.can_return = true;
                pending.append(&mut summary.callers);
            } else {
                for target in dependencies {
                    let dependency = &mut self.summaries[target.index()];
                    dependency.callers.insert(function);
                    if !dependency.analyzed {
                        pending.insert(target);
                    }
                }
            }
        }
    }
}
