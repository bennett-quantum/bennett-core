//! Return word proofs indexed by the arguments known at a copied call.

use std::collections::{BTreeSet, HashMap, HashSet};

use bennett_exec::Wire;
use bennett_ir::{FuncId, ValidModule};

use super::{Constants, normalize_arguments};
use crate::apply::bits::{Bit, Word};
use crate::apply::calls::work;
use crate::apply::returns::Returns;
use crate::apply::symbolic::Frame;

pub(super) type ReturnWord = Vec<Option<Bit>>;

pub(in crate::apply) enum CallResult {
    Stops,
    Returns(ReturnWord),
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct Context {
    callee: FuncId,
    arguments: Vec<Word>,
}

struct Query {
    context: Context,
    sources: Vec<Option<Wire>>,
}

impl Query {
    fn new(frame: &Frame<'_>, args: &[Option<Word>]) -> Self {
        let (arguments, sources) = normalize_arguments(frame, args.iter().cloned());
        Self { context: Context { callee: frame.callee, arguments }, sources }
    }

    fn translate(&self, word: &ReturnWord) -> ReturnWord {
        word.iter()
            .map(|bit| match *bit {
                Some(bit @ Bit::Constant(_)) => Some(bit),
                Some(Bit::Wire(wire, inverted) | Bit::Shared { wire, inverted, .. }) => {
                    self.sources[wire as usize].map(|source| Bit::Wire(source, inverted))
                }
                None => None,
            })
            .collect()
    }
}

struct Pending {
    context: Context,
    dependencies: Vec<Context>,
}

pub(in crate::apply) struct CallProofs<'a> {
    pub(in crate::apply) returns: Returns<'a>,
    words: CopyResults,
}

impl<'a> CallProofs<'a> {
    pub(in crate::apply) fn new(module: &'a ValidModule, work_limit: usize) -> Self {
        Self { returns: Returns::new(module), words: CopyResults::new(work_limit) }
    }

    pub(super) fn call(&mut self, module: &'a ValidModule, target: FuncId, args: &[Option<Word>]) -> CallResult {
        self.words.call(module, &mut self.returns, target, args)
    }

    pub(super) fn stops_with_arguments(&self, callee: FuncId, arguments: &[Word]) -> bool {
        let context = Context { callee, arguments: arguments.to_vec() };
        matches!(self.words.complete.get(&context), Some(CallResult::Stops))
    }
}

struct CopyResults {
    complete: HashMap<Context, CallResult>,
    remaining: usize,
}

impl CopyResults {
    fn new(work_limit: usize) -> Self {
        Self { complete: HashMap::new(), remaining: work_limit }
    }

    fn call<'a>(
        &mut self,
        module: &'a ValidModule,
        returns: &mut Returns<'a>,
        target: FuncId,
        args: &[Option<Word>],
    ) -> CallResult {
        if !returns.can_return(target) {
            return CallResult::Stops;
        }
        let query = Query::new(&Frame::for_analysis(module, target), args);
        self.resolve(module, returns, &query.context);
        self.lookup(&query).unwrap_or_else(|| CallResult::Returns(Vec::new()))
    }

    fn lookup(&self, query: &Query) -> Option<CallResult> {
        self.complete.get(&query.context).map(|result| match result {
            CallResult::Stops => CallResult::Stops,
            CallResult::Returns(word) => CallResult::Returns(query.translate(word)),
        })
    }

    fn resolve<'a>(&mut self, module: &'a ValidModule, returns: &mut Returns<'a>, root: &Context) {
        if self.complete.contains_key(root) {
            return;
        }
        let mut active = BTreeSet::from([root.callee]);
        let mut pending = vec![Pending { context: root.clone(), dependencies: Vec::new() }];
        while let Some(task) = pending.last_mut() {
            if let Some(context) = task.dependencies.pop() {
                if !self.complete.contains_key(&context) {
                    active.insert(context.callee);
                    pending.push(Pending { context, dependencies: Vec::new() });
                }
                continue;
            }

            let context = &task.context;
            let Some(remaining) = self.remaining.checked_sub(work(module.function(context.callee))) else {
                return;
            };
            self.remaining = remaining;
            let frame = Frame::for_analysis(module, context.callee);
            let mut dependencies = Vec::new();
            let mut requested = HashSet::new();
            let plan = Constants::solve(&frame, &context.arguments, |target, args| {
                if !returns.can_return(target) {
                    return CallResult::Stops;
                }
                // The copy planner still checks recursive call paths. Use an
                // unknown word for a query to a function already on this path.
                if active.contains(&target) {
                    return CallResult::Returns(Vec::new());
                }
                let query = Query::new(&Frame::for_analysis(module, target), args);
                self.lookup(&query).unwrap_or_else(|| {
                    if requested.insert(query.context.clone()) {
                        dependencies.push(query.context);
                    }
                    CallResult::Returns(Vec::new())
                })
            });
            if dependencies.is_empty() {
                let task = pending.pop().expect("current query exists");
                active.remove(&task.context.callee);
                let result = if plan.may_return {
                    CallResult::Returns(plan.returned.unwrap_or_default())
                } else {
                    CallResult::Stops
                };
                self.complete.insert(task.context, result);
            } else {
                // The provisional pass discovers independent calls together.
                // Recompute after their results are available so refined
                // arguments and selectors can request more precise proofs.
                task.dependencies = dependencies;
            }
        }
    }
}
