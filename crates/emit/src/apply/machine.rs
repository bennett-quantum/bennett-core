//! One clock for instruction dispatch after the definite prefix.

use std::collections::{BTreeMap, btree_map::Entry};

use bennett_ir::{Fault, Predicate, Slot, Target};

use super::arithmetic;
use super::bits::{Bit, Circuit, Word, constant};
use super::calls::Program;
use super::memory::Memory;
use super::symbolic::{Point, Step};
use super::work::{self, Work};
use super::{ApplyError, Copies};

type Dispatch = BTreeMap<Point, Bit>;
type Writes = BTreeMap<Slot, Vec<(Bit, Word)>>;

pub(super) fn run(
    circuit: &mut Circuit,
    program: &Program<'_>,
    slots: &mut [Word],
    memory: &mut Memory<'_>,
    start: (Point, Bit),
    remaining: u64,
) -> Result<(Copies, u64), ApplyError> {
    let mut copies = Copies::default();
    let mut dispatch = Dispatch::new();
    activate(circuit, &mut dispatch, start.0, start.1);
    let mut ticks = 0;
    for _ in 0..remaining {
        if dispatch.is_empty() {
            break;
        }

        let mut next = Dispatch::new();
        let mut writes = Writes::new();
        for (point, enabled) in dispatch {
            match program.step_in(circuit, slots, point, memory, enabled)? {
                Step::Write(successor, dst, word) => {
                    writes.entry(dst).or_default().push((enabled, word));
                    activate(circuit, &mut next, successor, enabled);
                }
                Step::Jump(target) => {
                    jump(circuit, program, slots, &mut next, &mut writes, (point, target), enabled);
                }
                Step::Branch(condition, yes, no) => {
                    let take_yes = circuit.and(enabled, condition);
                    let take_no = circuit.and(enabled, condition.not());
                    jump(circuit, program, slots, &mut next, &mut writes, (point, yes), take_yes);
                    jump(circuit, program, slots, &mut next, &mut writes, (point, no), take_no);
                }
                Step::Switch(selector, edges) => {
                    let mut unmatched = enabled;
                    for case in edges.cases {
                        work::add(Work::TargetChecks, 1);
                        let pattern = constant(case.value, selector.len());
                        let equal = arithmetic::compare(circuit, Predicate::Eq, &selector, &pattern);
                        let take = circuit.and(enabled, equal);
                        if edges.default.is_some() {
                            // Case values are distinct, so removing a matching dispatch is an XOR.
                            unmatched = circuit.xor(unmatched, take);
                        }
                        jump(circuit, program, slots, &mut next, &mut writes, (point, &case.target), take);
                    }
                    if let Some(default) = edges.default {
                        jump(circuit, program, slots, &mut next, &mut writes, (point, default), unmatched);
                    }
                }
                Step::Indirect(call) => {
                    let mut unmatched = enabled;
                    for (address, index) in call.targets {
                        work::add(Work::TargetChecks, 1);
                        let pattern = constant(u128::from(address), call.address.len());
                        let equal = arithmetic::compare(circuit, Predicate::Eq, &call.address, &pattern);
                        let take = circuit.and(enabled, equal);
                        if call.invalid_possible {
                            unmatched = circuit.xor(unmatched, take);
                        }
                        if take.is_zero() {
                            continue;
                        }
                        match program.enter(circuit, slots, point, index, call.args) {
                            Ok(entry) => {
                                if !entry.failed.is_zero() {
                                    copies.faulted.push((circuit.and(take, entry.failed), Fault::StackOverflow));
                                }
                                let succeeded = circuit.and(take, entry.failed.not());
                                transfer(circuit, &mut next, &mut writes, entry.next, entry.writes, succeeded);
                            }
                            Err(fault) => copies.faulted.push((take, fault)),
                        }
                    }
                    if call.invalid_possible {
                        copies.faulted.push((unmatched, Fault::BadIndirect));
                    }
                }
                Step::Return(word) => copies.returned.push((enabled, word)),
                Step::CheckedTransfer(successor, incoming, succeeded, faults) => {
                    copies
                        .faulted
                        .extend(faults.into_iter().map(|(failed, fault)| (circuit.and(enabled, failed), fault)));
                    let succeeded = circuit.and(enabled, succeeded);
                    transfer(circuit, &mut next, &mut writes, successor, incoming, succeeded);
                }
                Step::Inactive => {}
                Step::Fault(fault) => copies.faulted.push((enabled, fault)),
                Step::Transfer(successor, incoming) => {
                    transfer(circuit, &mut next, &mut writes, successor, incoming, enabled);
                }
            }
        }

        // All operands, including every incoming block argument, were read from the same
        // pre-tick frame. Updates leave every other input's slots unchanged.
        for (dst, alternatives) in writes {
            let word = &mut slots[dst.index()];
            for (enabled, update) in alternatives {
                for (old, value) in word.iter_mut().zip(update) {
                    work::add(Work::FactJoins, 1);
                    *old = circuit.select(enabled, value, *old);
                }
            }
        }
        memory.commit(circuit);
        dispatch = next;
        ticks += 1;
    }
    copies.faulted.extend(dispatch.into_values().map(|enabled| (enabled, Fault::Budget)));
    Ok((copies, ticks))
}

fn activate(circuit: &mut Circuit, dispatch: &mut Dispatch, point: Point, enabled: Bit) {
    work::add(Work::Successors, 1);
    if enabled.is_zero() {
        return;
    }
    match dispatch.entry(point) {
        Entry::Vacant(entry) => {
            entry.insert(enabled);
        }
        Entry::Occupied(mut entry) => {
            // An input can come from only one point and take only one edge in a tick.
            let joined = circuit.xor(*entry.get(), enabled);
            if joined.is_zero() {
                entry.remove();
            } else {
                entry.insert(joined);
            }
        }
    }
}

fn jump(
    circuit: &mut Circuit,
    program: &Program<'_>,
    slots: &[Word],
    dispatch: &mut Dispatch,
    writes: &mut Writes,
    (point, target): (Point, &Target),
    enabled: Bit,
) {
    if enabled.is_zero() {
        return;
    }
    for (dst, value) in program.incoming(slots, point, target) {
        writes.entry(dst).or_default().push((enabled, value));
    }
    activate(circuit, dispatch, Point { copy: point.copy, ..Point::entry(target.block) }, enabled);
}

fn transfer(
    circuit: &mut Circuit,
    dispatch: &mut Dispatch,
    writes: &mut Writes,
    successor: Point,
    incoming: Vec<(Slot, Word)>,
    enabled: Bit,
) {
    if enabled.is_zero() {
        return;
    }
    for (dst, word) in incoming {
        writes.entry(dst).or_default().push((enabled, word));
    }
    activate(circuit, dispatch, successor, enabled);
}
