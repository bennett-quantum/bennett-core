//! Resolve normalized call contexts without replaying suspended callers.

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use bennett_ir::{FuncId, ValidModule};

use super::{ArgumentIdentities, Constants, Progress, Solver, normalize_arguments};
use crate::apply::bits::{Bit, Word};
use crate::apply::identity::Identity;
use crate::apply::symbolic::Frame;
use crate::apply::work::{self, Work};

pub(super) type ReturnWord = Vec<Option<Bit>>;

pub(super) enum CallResult {
    Stops,
    Returns(ReturnWord),
}

#[derive(Clone, Eq)]
struct Context {
    callee: FuncId,
    arguments: ArgumentIdentities,
    independent: bool,
}

impl Context {
    fn new(callee: FuncId, arguments: ArgumentIdentities) -> Self {
        let independent = arguments.iter().flatten().enumerate().all(|(index, &bit)| {
            work::add(Work::ContextBits, 1);
            bit == Identity::input(index as u64, false)
        });
        Self { callee, arguments, independent }
    }
}

impl Hash for Context {
    fn hash<H: Hasher>(&self, state: &mut H) {
        work::add(Work::KeyHashWords, 2 + self.arguments.iter().map(|word| 1 + word.len()).sum::<usize>());
        self.callee.hash(state);
        self.arguments.hash(state);
    }
}

impl PartialEq for Context {
    fn eq(&self, other: &Self) -> bool {
        work::add(Work::KeyCompareWords, 1);
        if self.callee != other.callee {
            return false;
        }
        work::add(Work::KeyCompareWords, 1);
        self.arguments.len() == other.arguments.len()
            && self.arguments.iter().zip(&other.arguments).all(|(left, right)| {
                work::add(Work::KeyCompareWords, 1);
                left.len() == right.len()
                    && left.iter().zip(right).all(|(left, right)| {
                        work::add(Work::KeyCompareWords, 1);
                        left == right
                    })
            })
    }
}

struct Query {
    context: Context,
    sources: Vec<Option<Bit>>,
}

enum Continuation {
    Exact { sources: Vec<Option<Bit>>, charged: usize },
    Unconstrained,
}

impl Continuation {
    fn answer(self, plan: &Constants<'_>) -> CallResult {
        match self {
            Self::Exact { sources, .. } => Query::answer(&sources, plan),
            Self::Unconstrained if plan.may_return => CallResult::Returns(Vec::new()),
            Self::Unconstrained => CallResult::Stops,
        }
    }
}

impl Query {
    fn new(frame: &Frame<'_>, args: impl Iterator<Item = Option<Word>>) -> Self {
        let (arguments, sources) = normalize_arguments(frame, args);
        Self { context: Context::new(frame.callee, arguments), sources }
    }

    fn answer(sources: &[Option<Bit>], plan: &Constants<'_>) -> CallResult {
        if !plan.may_return {
            return CallResult::Stops;
        }
        let returned = plan.returned.as_deref().unwrap_or_default();
        work::add(Work::CacheResultWords, returned.len());
        CallResult::Returns(
            returned
                .iter()
                .map(|bit| match bit.map(Bit::identity) {
                    Some(literal @ Identity::Literal(_)) => Some(literal.argument_bit()),
                    Some(Identity::Source(source, inverted)) => source
                        .argument_index()
                        .and_then(|index| sources[index])
                        .map(|argument| argument.with_flip(inverted)),
                    None => None,
                })
                .collect(),
        )
    }
}

pub(in crate::apply) struct CallProofs<'a> {
    module: &'a ValidModule,
    independent: Vec<Option<Rc<Constants<'a>>>>,
    specific: HashMap<Context, Rc<Constants<'a>>>,
    has_completed_contexts: bool,
    // Charge each new return proof for the cells reached in its context.
    // Looking up a completed proof or planning a copy does not charge it again.
    remaining: usize,
}

impl<'a> CallProofs<'a> {
    pub(in crate::apply) fn new(module: &'a ValidModule, work_limit: usize) -> Self {
        Self {
            module,
            independent: vec![None; module.functions.len()],
            specific: HashMap::new(),
            has_completed_contexts: false,
            remaining: work_limit,
        }
    }

    fn get(&self, context: &Context) -> Option<&Rc<Constants<'a>>> {
        work::add(Work::SummaryTableReads, 1);
        let independent = self.independent[context.callee.index()].as_ref();
        if context.independent || independent.is_some_and(|plan| !plan.may_return) {
            independent
        } else {
            work::add(Work::CallLookups, 1);
            self.specific.get(context)
        }
    }

    pub(super) fn plan(
        &mut self,
        callee: FuncId,
        arguments: ArgumentIdentities,
        needs_analysis: bool,
    ) -> Rc<Constants<'a>> {
        let context = Context::new(callee, arguments);
        if let Some(result) = self.has_completed_contexts.then(|| self.get(&context)).flatten() {
            if needs_analysis || !result.may_return {
                return Rc::clone(result);
            }
        } else if needs_analysis {
            return self.resolve(context);
        }
        Rc::new(Constants::with_arguments(&Frame::for_analysis(self.module, callee), &context.arguments))
    }

    fn resolve(&mut self, root: Context) -> Rc<Constants<'a>> {
        let module = self.module;
        let mut active = BTreeSet::from([root.callee]);
        let mut pending = vec![(
            Solver::new(Frame::for_analysis(module, root.callee), &root.arguments, false),
            root,
            None::<Continuation>,
        )];
        let mut resumed = None;
        while !pending.is_empty() {
            let nested = pending.len() > 1;
            let (solver, _, continuation) = pending.last_mut().expect("current proof");
            let allowance = match continuation {
                Some(Continuation::Exact { charged, .. }) => self.remaining + *charged,
                Some(Continuation::Unconstrained) | None => usize::MAX,
            };
            let mut requested = None;
            let progress = solver.advance(allowance, |callee, args| {
                let mut lookup = || {
                    if let Some(answer) = resumed.take() {
                        return Some(answer);
                    }
                    if active.contains(&callee) {
                        // An unfinished recursive context cannot exclude a normal
                        // continuation. Copy planning still rejects a reached cycle.
                        return Some(CallResult::Returns(Vec::new()));
                    }
                    let frame = Frame::for_analysis(module, callee);
                    let Query { context, sources } = Query::new(&frame, args.iter().cloned());
                    if let Some(plan) = self.get(&context) {
                        return Some(Query::answer(&sources, plan));
                    }
                    if self.remaining != 0 {
                        requested = Some((context, Continuation::Exact { sources, charged: 0 }));
                        return None;
                    }
                    // Keep the unconstrained stopping proof when the old copied-result
                    // allowance is exhausted. Its word is insufficient for this query.
                    let general = Query::new(&frame, module.function(callee).params.iter().map(|_| None));
                    if let Some(plan) = self.get(&general.context) {
                        Some(Continuation::Unconstrained.answer(plan))
                    } else {
                        requested = Some((general.context, Continuation::Unconstrained));
                        None
                    }
                };
                let result = lookup();
                if nested && result.is_some() {
                    work::add(Work::SummaryLookups, 1);
                }
                result
            });
            if let Some(Continuation::Exact { charged, .. }) = continuation {
                let charge = (solver.reached_cells - *charged).min(self.remaining);
                *charged += charge;
                self.remaining -= charge;
            }
            match progress {
                Progress::Complete => {
                    let (mut solver, context, continuation) = pending.pop().expect("current proof");
                    active.remove(&context.callee);
                    let Some(continuation) = continuation else { return Rc::new(solver.plan) };
                    let independent = context.independent || solver.generalize_stop();
                    let plan = Rc::new(solver.plan);
                    let answer = continuation.answer(&plan);
                    if pending.len() > 1 && matches!(answer, CallResult::Returns(_)) {
                        work::add(Work::Dependencies, 1);
                    }
                    resumed = Some(answer);
                    self.has_completed_contexts = true;
                    if independent {
                        work::add(Work::SummaryTableWrites, 1);
                        self.independent[context.callee.index()] = Some(plan);
                    } else {
                        work::add(Work::CacheWrites, 1);
                        self.specific.insert(context, plan);
                    }
                }
                Progress::NeedsCall => {
                    let (context, continuation) = requested.expect("suspended call requested a proof");
                    if self.remaining == 0 && matches!(continuation, Continuation::Exact { .. }) {
                        continue;
                    }
                    active.insert(context.callee);
                    let solver = Solver::new(Frame::for_analysis(module, context.callee), &context.arguments, true);
                    work::add(Work::ContextSchedules, 1);
                    pending.push((solver, context, Some(continuation)));
                }
                Progress::Exhausted => {
                    let (_, context, _) = pending.pop().expect("exhausted copied-result proof");
                    active.remove(&context.callee);
                    resumed = None;
                    // A caller retries the unresolved site using the same
                    // worklist with unconstrained arguments.
                }
            }
        }
        unreachable!("the root proof always settles")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::bits::Circuit;

    #[test]
    fn specific_call_cache_counts_key_and_result_elements() {
        let module = bennett_ir::load("func @f(i8 %a, i4 %b) -> i3 {\nentry:\n ret 3\n}\n").expect("valid program");
        let callee = module.function_named("f").expect("function");
        let frame = Frame::for_analysis(&module, callee);
        let query =
            Query::new(&frame, [Some(vec![Bit::Constant(false); 8]), Some(vec![Bit::Constant(true); 4])].into_iter());
        let mut cache = CallProofs::new(&module, 0);
        cache.specific.insert(
            query.context.clone(),
            Rc::new(Constants { may_return: true, returned: Some(vec![None; 3]), ..Constants::default() }),
        );
        let meter = work::Scope::start();
        let plan = cache.get(&query.context).expect("cached proof");
        assert!(matches!(Query::answer(&query.sources, plan), CallResult::Returns(word) if word.len() == 3));
        let counts = meter.counts();
        let key_words = 2 + (1 + 8) + (1 + 4);
        assert_eq!(counts.call_lookups, 1);
        assert_eq!(counts.key_hash_words, key_words);
        assert_eq!(counts.key_compare_words, key_words);
        assert_eq!(counts.cache_result_words, 3);
    }

    #[test]
    fn retained_argument_identity_survives_copied_call_proofs() {
        let module = bennett_ir::load(
            r#"
func @top(i1 %a, i1 %b) -> i2 {
entry:
    %result = call @wrapper(%a, %b)
    ret %result
}
func @wrapper(i1 %a, i1 %b) -> i2 {
entry:
    %result = call @leaf(%a, %b)
    ret %result
}
func @leaf(i1 %a, i1 %b) -> i2 {
entry:
    %different = icmp ne i1 %a, %b
    %high = select i2 %different, 2, 0
    %low = zext i1 %b to i2
    %result = or i2 %high, %low
    ret %result
}
"#,
        )
        .expect("valid program");
        let top = module.function_named("top").expect("entry");
        let wrapper = module.function_named("wrapper").expect("wrapper");
        let mut proofs = CallProofs::new(&module, 128);
        let mut circuit = Circuit::new(2);
        for wire in 0..2 {
            let source = Bit::input(wire);
            let complemented = circuit.retain_copy(source.not());
            let frame = Frame::for_analysis(&module, wrapper);
            let query = Query::new(&frame, [Some(vec![source]), Some(vec![complemented])].into_iter());
            proofs.plan(top, query.context.arguments.clone(), true);
            let plan = proofs.get(&query.context).expect("cached wrapper return proof");
            let CallResult::Returns(word) = Query::answer(&query.sources, plan) else {
                panic!("an ordinary copied call returns");
            };
            assert!(word[0].is_some_and(|value| value.same_value(complemented)));
            assert!(word[1].is_some_and(Bit::is_one));
        }
    }
}
