//! Lower choices and proved loops using saved control bits instead of instruction dispatch.

use std::collections::BTreeSet;

use bennett_ir::{BlockId, Fault, Target};

use super::bits::{Bit, Circuit, ONE, Word, ZERO};
use super::calls::Program;
use super::flow::{BranchEquality, Flow, ShiftLoop};
use super::symbolic::{Point, Step};
use super::{ApplyError, Copies};
use facts::Facts;

mod facts;

pub(super) enum Failure {
    Control(Refusal),
    Lowering(ApplyError),
}

impl From<ApplyError> for Failure {
    fn from(error: ApplyError) -> Self {
        Self::Lowering(error)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    UnprovedLoop,
    IncompatibleJoins,
    RepeatedContinuation,
    UnequalCharges,
    MultiwayDispatch,
    IndirectDispatch,
}

impl Refusal {
    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::UnprovedLoop => "qubit-dependent loop without a program-proved pass count",
            Self::IncompatibleJoins => "qubit-dependent routes do not share a supported continuation",
            Self::RepeatedContinuation => "conditional routes would emit shared code more than once",
            Self::MultiwayDispatch => "qubit-dependent switch uses multiway dispatch",
            Self::IndirectDispatch => "qubit-dependent indirect call uses multiway dispatch",
            Self::UnequalCharges => "unequal route lengths can exhaust the budget after a join",
        }
    }
}

#[derive(Clone)]
struct Route {
    point: Point,
    enabled: Bit,
    slots: Vec<Word>,
    facts: Facts,
    // Inclusive bounds on steps spent since the definite prefix, for enabled inputs only.
    first: u64,
    last: u64,
}

struct Boundary {
    point: Point,
    proved_loop: bool,
}

struct Controls<'a, 'b> {
    circuit: &'b mut Circuit,
    program: &'b Program<'a>,
    flows: Vec<Flow<'a>>,
    budget: u64,
    copies: Copies,
    proved_passes: usize,
    choices: BTreeSet<(usize, BlockId)>,
    boundaries: Vec<Boundary>,
}

pub(super) fn run(
    circuit: &mut Circuit,
    program: &Program<'_>,
    slots: &[Word],
    start: Point,
    remaining: u64,
) -> Result<(Copies, usize), Failure> {
    let mut controls = Controls {
        circuit,
        program,
        flows: program.frames().map(Flow::analyze).collect(),
        budget: remaining,
        copies: Copies::default(),
        proved_passes: 0,
        choices: BTreeSet::new(),
        boundaries: Vec::new(),
    };
    let route = Route { point: start, enabled: ONE, slots: slots.to_vec(), facts: Facts::default(), first: 0, last: 0 };
    let end = controls.follow(route)?;
    debug_assert!(end.is_none(), "no enclosing continuation");
    Ok((controls.copies, controls.proved_passes))
}

impl Controls<'_, '_> {
    fn can_revisit(&self, point: Point, proved_boundaries_only: bool) -> bool {
        self.flows[point.copy].can_revisit(point.block, |block| {
            self.boundaries.iter().any(|stop| {
                stop.point.copy == point.copy
                    && stop.point.block == block
                    && (!proved_boundaries_only || stop.proved_loop)
            })
        })
    }

    fn charge(&mut self, route: &mut Route) -> Result<bool, Failure> {
        if route.first == self.budget {
            self.copies.faulted.push((route.enabled, Fault::Budget));
            return Ok(false);
        }
        // This state has merged routes. Spending the next step for all of them
        // could change an outcome, so the step machine must preserve their clocks.
        if route.last == self.budget {
            return Err(Failure::Control(Refusal::UnequalCharges));
        }
        route.first += 1;
        route.last += 1;
        Ok(true)
    }

    fn follow(&mut self, mut route: Route) -> Result<Option<Route>, Failure> {
        if route.enabled.is_zero() {
            return Ok(None);
        }
        loop {
            if self.boundaries.iter().any(|stop| stop.point == route.point) {
                return Ok(Some(route));
            }
            if !self.charge(&mut route)? {
                // A route can spend its budget in an ordinary loop whose test was
                // evaluated during lowering. Only a program proof permits a loop
                // schedule here. In particular, a proved outer loop cannot copy an
                // inner schedule whose only stopping condition is the budget.
                let mut frame = Some(route.point);
                while let Some(point) = frame {
                    if self.can_revisit(point, true) {
                        return Err(Failure::Control(Refusal::UnprovedLoop));
                    }
                    frame = self.program.resume(point);
                }
                return Ok(None);
            }
            if !self.flows[route.point.copy].can_finish(route.point.block) {
                return Err(Failure::Control(Refusal::UnprovedLoop));
            }
            let point = route.point;
            let step = self.circuit.under(route.enabled, |circuit| {
                self.program.step_with(circuit, &route.slots, point, &[], |word| route.facts.resolve_word(word))
            })?;
            match step {
                Step::Write(next, dst, word) => {
                    route.slots[dst.index()] = word;
                    route.point = next;
                }
                Step::Jump(target) => route.point = self.program.jump(&mut route.slots, point, target),
                Step::Return(word) => {
                    self.copies.returned.push((route.enabled, word));
                    return Ok(None);
                }
                Step::Transfer(next, writes) => {
                    for (slot, word) in writes {
                        route.slots[slot.index()] = word;
                    }
                    route.point = next;
                }
                Step::Fault(fault) => {
                    self.copies.faulted.push((route.enabled, fault));
                    return Ok(None);
                }
                Step::CheckedTransfer(next, writes, succeeded, faults) => {
                    self.copies.faulted.extend(
                        faults.into_iter().map(|(failed, fault)| (self.circuit.and(route.enabled, failed), fault)),
                    );
                    route.enabled = self.circuit.and(route.enabled, succeeded);
                    if route.enabled.is_zero() {
                        return Ok(None);
                    }
                    for (slot, word) in writes {
                        route.slots[slot.index()] = word;
                    }
                    route.point = next;
                }
                Step::Inactive => unreachable!("this lowering does not access memory"),
                Step::Switch(..) => return Err(Failure::Control(Refusal::MultiwayDispatch)),
                Step::Indirect(..) => return Err(Failure::Control(Refusal::IndirectDispatch)),
                Step::Branch(condition, yes, no) => {
                    let choice = (point.copy, point.block);
                    if !self.choices.insert(choice) {
                        return Err(Failure::Control(Refusal::UnprovedLoop));
                    }
                    let next = if let Some(plan) = self.flows[point.copy].shift_loop(point.block) {
                        self.repeat(route, condition, plan)?
                    } else {
                        self.choose(route, condition, yes, no)?
                    };
                    self.choices.remove(&choice);
                    let Some(next) = next else { return Ok(None) };
                    route = next;
                }
            }
        }
    }

    fn choose(&mut self, route: Route, condition: Bit, yes: &Target, no: &Target) -> Result<Option<Route>, Failure> {
        let point = route.point;
        let flow = &self.flows[point.copy];
        let join = flow.join(point.block).map(|join| Point { copy: point.copy, ..join });
        let yes_mask = self.circuit.and(route.enabled, condition);
        let no_mask = self.circuit.and(route.enabled, condition.not());
        let parent_mask = route.enabled;
        let outer_boundaries = self.boundaries.len();
        // Returning from a copy is also a boundary. A route that leaves the
        // callee must not lower the caller's continuation before its sibling.
        self.boundaries.extend(
            [join, self.program.resume(point)]
                .into_iter()
                .flatten()
                .map(|point| Boundary { point, proved_loop: false }),
        );
        if self.can_revisit(point, false) {
            return Err(Failure::Control(Refusal::UnprovedLoop));
        }
        if !yes_mask.is_zero()
            && !no_mask.is_zero()
            && flow.shares_code(yes.block, no.block, |block| {
                self.boundaries
                    .iter()
                    .filter(|stop| stop.point.copy == point.copy && stop.point.block == block)
                    .map(|stop| stop.point.index)
                    .min()
            })
        {
            return Err(Failure::Control(Refusal::RepeatedContinuation));
        }
        let (mut yes_route, mut no_route) = (route.clone(), route);
        if let Some(equality) = BranchEquality::in_block(self.program.block(point)) {
            let equal_route = if equality.when_true { &mut yes_route } else { &mut no_route };
            let slot = self.program.slot(point, equality.slot);
            equal_route.facts.assume(&equal_route.slots[slot.index()], equality.value.value());
        }
        let yes = self.transfer(yes_route, yes, yes_mask);
        let no = self.transfer(no_route, no, no_mask);
        let yes = self.follow(yes)?;
        let no = self.follow(no)?;
        self.boundaries.truncate(outer_boundaries);
        let same_mask = yes.as_ref().is_some_and(|yes| yes.enabled.same_value(yes_mask))
            && no.as_ref().is_some_and(|no| no.enabled.same_value(no_mask));
        self.merge(yes, no, same_mask.then_some(parent_mask))
    }

    fn repeat(&mut self, mut route: Route, mut condition: Bit, plan: ShiftLoop<'_>) -> Result<Option<Route>, Failure> {
        let test = route.point;
        let exit_point = Point { copy: test.copy, ..Point::entry(plan.exiting.block) };
        let mut exits = None;
        for pass in 0..=plan.width {
            let continuation = if pass == plan.width {
                // Each back edge removed one bit, so the tested word is now zero.
                ZERO
            } else if plan.continue_when_true {
                condition
            } else {
                condition.not()
            };
            let exit_mask = self.circuit.and(route.enabled, continuation.not());
            if !exit_mask.is_zero() {
                let mut exiting = route.clone();
                // Every enabled exit saw zero. Carry that fact through the
                // transfer, including when the tested word is an exit argument.
                let zeroed = self.program.slot(test, plan.zero_on_exit);
                exiting.slots[zeroed.index()].fill(ZERO);
                let exit = self.transfer(exiting, plan.exiting, exit_mask);
                exits = self.merge(exits, Some(exit), None)?;
            }
            route.enabled = self.circuit.and(route.enabled, continuation);
            if route.enabled.is_zero() {
                break;
            }
            let enabled = route.enabled;
            route = self.transfer(route, plan.continuing, enabled);
            self.proved_passes += 1;
            let outer_boundaries = self.boundaries.len();
            self.boundaries.extend([test, exit_point].map(|point| Boundary { point, proved_loop: true }));
            self.boundaries.extend(self.program.resume(test).map(|point| Boundary { point, proved_loop: false }));
            let next = self.follow(route)?;
            self.boundaries.truncate(outer_boundaries);
            let Some(next) = next else { break };
            route = next;
            if route.point != test {
                return Err(Failure::Control(Refusal::IncompatibleJoins));
            }
            if !self.charge(&mut route)? {
                break;
            }
            let tested = self.program.read(&route.slots, route.point, plan.condition)[0];
            condition = route.facts.resolve(tested);
        }
        Ok(exits)
    }

    fn transfer(&self, mut route: Route, target: &Target, enabled: Bit) -> Route {
        route.point = self.program.jump(&mut route.slots, route.point, target);
        route.enabled = enabled;
        route
    }

    fn merge(
        &mut self,
        left: Option<Route>,
        right: Option<Route>,
        parent: Option<Bit>,
    ) -> Result<Option<Route>, Failure> {
        let (mut left, right) = match (left, right) {
            (Some(left), Some(right)) => (left, right),
            (None, route) | (route, None) => return Ok(route),
        };
        if left.point != right.point {
            return Err(Failure::Control(Refusal::IncompatibleJoins));
        }
        // Each enabled input took exactly one of these routes. Slots outside the union
        // need no meaningful value. Keeping common bits also keeps ordinary loop counters definite.
        for (old, update) in left.slots.iter_mut().zip(right.slots) {
            for (old, update) in old.iter_mut().zip(update) {
                *old = left.facts.join(*old, &right.facts, update, right.enabled, self.circuit);
            }
        }
        left.facts.intersect(&right.facts);
        left.enabled = parent.unwrap_or_else(|| self.circuit.xor(left.enabled, right.enabled));
        left.first = left.first.min(right.first);
        left.last = left.last.max(right.last);
        Ok(Some(left))
    }
}
