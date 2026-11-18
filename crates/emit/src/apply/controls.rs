//! Lower choices and proved loops using saved control bits instead of instruction dispatch.

use std::collections::BTreeSet;

use bennett_ir::{BlockId, Fault, Target};

use super::bits::{Bit, Circuit, ONE, Word, ZERO};
use super::flow::{Flow, ShiftLoop};
use super::symbolic::{Frame, Point, Step};
use super::{ApplyError, Copies};

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
}

impl Refusal {
    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::UnprovedLoop => "qubit-dependent loop without a program-proved pass count",
            Self::IncompatibleJoins => "qubit-dependent routes do not share a supported continuation",
            Self::RepeatedContinuation => "conditional routes would emit shared code more than once",
            Self::UnequalCharges => "unequal route lengths can exhaust the budget after a join",
        }
    }
}

#[derive(Clone)]
struct Route {
    point: Point,
    enabled: Bit,
    slots: Vec<Word>,
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
    frame: &'b Frame<'a>,
    flow: Flow<'a>,
    budget: u64,
    copies: Copies,
    proved_passes: usize,
    choices: BTreeSet<BlockId>,
    boundaries: Vec<Boundary>,
}

pub(super) fn run(
    circuit: &mut Circuit,
    frame: &Frame<'_>,
    slots: &[Word],
    start: Point,
    remaining: u64,
) -> Result<(Copies, usize), Failure> {
    let mut controls = Controls {
        circuit,
        frame,
        flow: Flow::analyze(frame),
        budget: remaining,
        copies: Copies::default(),
        proved_passes: 0,
        choices: BTreeSet::new(),
        boundaries: Vec::new(),
    };
    let route = Route { point: start, enabled: ONE, slots: slots.to_vec(), first: 0, last: 0 };
    let end = controls.follow(route)?;
    debug_assert!(end.is_none(), "no enclosing continuation");
    Ok((controls.copies, controls.proved_passes))
}

impl Controls<'_, '_> {
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
        if route.enabled == ZERO {
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
                if self.flow.can_revisit(route.point.block, |block| {
                    self.boundaries.iter().any(|stop| stop.proved_loop && stop.point.block == block)
                }) {
                    return Err(Failure::Control(Refusal::UnprovedLoop));
                }
                return Ok(None);
            }
            if !self.flow.can_return(route.point.block) {
                return Err(Failure::Control(Refusal::UnprovedLoop));
            }
            let point = route.point;
            match self.circuit.under(route.enabled, |circuit| self.frame.step(circuit, &route.slots, point))? {
                Step::Write(next, dst, word) => {
                    route.slots[dst.index()] = word;
                    route.point = next;
                }
                Step::Jump(target) => route.point = self.frame.jump(&mut route.slots, target),
                Step::Return(word) => {
                    self.copies.returned.push((route.enabled, word));
                    return Ok(None);
                }
                Step::Transfer(..) | Step::Fault(_) => unreachable!("calls use the step machine"),
                Step::Branch(condition, yes, no) => {
                    if !self.choices.insert(point.block) {
                        return Err(Failure::Control(Refusal::UnprovedLoop));
                    }
                    let next = if let Some(plan) = self.flow.shift_loop(point.block) {
                        self.repeat(route, condition, plan)?
                    } else {
                        self.choose(route, condition, yes, no)?
                    };
                    self.choices.remove(&point.block);
                    let Some(next) = next else { return Ok(None) };
                    route = next;
                }
            }
        }
    }

    fn choose(&mut self, route: Route, condition: Bit, yes: &Target, no: &Target) -> Result<Option<Route>, Failure> {
        let route_block = route.point.block;
        let join = self.flow.join(route_block);
        let yes_mask = self.circuit.and(route.enabled, condition);
        let no_mask = self.circuit.and(route.enabled, condition.not());
        let parent_mask = route.enabled;
        self.boundaries.extend(join.map(|point| Boundary { point, proved_loop: false }));
        if self.flow.can_revisit(route_block, |block| self.boundaries.iter().any(|stop| stop.point.block == block)) {
            return Err(Failure::Control(Refusal::UnprovedLoop));
        }
        if yes_mask != ZERO
            && no_mask != ZERO
            && self.flow.shares_code(yes.block, no.block, |block| {
                self.boundaries.iter().filter(|stop| stop.point.block == block).map(|stop| stop.point.index).min()
            })
        {
            return Err(Failure::Control(Refusal::RepeatedContinuation));
        }
        let yes = self.transfer(route.clone(), yes, yes_mask);
        let no = self.transfer(route, no, no_mask);
        let yes = self.follow(yes)?;
        let no = self.follow(no)?;
        self.boundaries.truncate(self.boundaries.len() - usize::from(join.is_some()));
        let same_mask = yes.as_ref().is_some_and(|yes| yes.enabled == yes_mask)
            && no.as_ref().is_some_and(|no| no.enabled == no_mask);
        self.merge(yes, no, same_mask.then_some(parent_mask))
    }

    fn repeat(&mut self, mut route: Route, mut condition: Bit, plan: ShiftLoop<'_>) -> Result<Option<Route>, Failure> {
        let test = route.point;
        let exit_point = Point::entry(plan.exiting.block);
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
            if exit_mask != ZERO {
                let mut exiting = route.clone();
                // Every enabled exit saw zero. Carry that fact through the
                // transfer, including when the tested word is an exit argument.
                exiting.slots[plan.tested.index()].fill(ZERO);
                let exit = self.transfer(exiting, plan.exiting, exit_mask);
                exits = self.merge(exits, Some(exit), None)?;
            }
            route.enabled = self.circuit.and(route.enabled, continuation);
            if route.enabled == ZERO {
                break;
            }
            let enabled = route.enabled;
            route = self.transfer(route, plan.continuing, enabled);
            self.proved_passes += 1;
            self.boundaries.extend([test, exit_point].map(|point| Boundary { point, proved_loop: true }));
            let next = self.follow(route)?;
            self.boundaries.truncate(self.boundaries.len() - 2);
            let Some(next) = next else { break };
            route = next;
            if route.point != test {
                return Err(Failure::Control(Refusal::IncompatibleJoins));
            }
            if !self.charge(&mut route)? {
                break;
            }
            condition = Frame::read(&route.slots, plan.condition)[0];
        }
        Ok(exits)
    }

    fn transfer(&self, mut route: Route, target: &Target, enabled: Bit) -> Route {
        route.point = self.frame.jump(&mut route.slots, target);
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
                *old = self.circuit.select(right.enabled, update, *old);
            }
        }
        left.enabled = parent.unwrap_or_else(|| self.circuit.xor(left.enabled, right.enabled));
        left.first = left.first.min(right.first);
        left.last = left.last.max(right.last);
        Ok(Some(left))
    }
}
