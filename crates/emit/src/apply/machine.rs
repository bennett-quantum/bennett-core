//! One clock for all instructions after the first qubit-dependent jump.

use std::collections::{BTreeMap, btree_map::Entry};

use bennett_ir::{Fault, Slot, Target};

use super::bits::{Bit, Circuit, ONE, Word, ZERO};
use super::symbolic::{Frame, Point, Step};
use super::{ApplyError, Copies};

type Dispatch = BTreeMap<Point, Bit>;
type Writes = BTreeMap<Slot, Vec<(Bit, Word)>>;

pub(super) fn run(
    circuit: &mut Circuit,
    frame: &Frame<'_>,
    slots: &mut [Word],
    start: Point,
    remaining: u64,
) -> Result<(Copies, u64), ApplyError> {
    let mut copies = Copies::default();
    let mut dispatch = Dispatch::from([(start, ONE)]);
    let mut ticks = 0;
    for _ in 0..remaining {
        if dispatch.is_empty() {
            break;
        }

        let mut next = Dispatch::new();
        let mut writes = Writes::new();
        for (point, enabled) in dispatch {
            match frame.step(circuit, slots, point)? {
                Step::Write(successor, dst, word) => {
                    writes.entry(dst).or_default().push((enabled, word));
                    activate(circuit, &mut next, successor, enabled);
                }
                Step::Jump(target) => {
                    jump(circuit, frame, slots, &mut next, &mut writes, target, enabled);
                }
                Step::Branch(condition, yes, no) => {
                    let take_yes = circuit.and(enabled, condition);
                    let take_no = circuit.and(enabled, condition.not());
                    jump(circuit, frame, slots, &mut next, &mut writes, yes, take_yes);
                    jump(circuit, frame, slots, &mut next, &mut writes, no, take_no);
                }
                Step::Return(word) => copies.returned.push((enabled, word)),
            }
        }

        // All operands, including every incoming block argument, were read from the same
        // pre-tick frame. Updates leave every other input's slots unchanged.
        for (dst, alternatives) in writes {
            let word = &mut slots[dst.index()];
            for (enabled, update) in alternatives {
                for (old, value) in word.iter_mut().zip(update) {
                    *old = circuit.select(enabled, value, *old);
                }
            }
        }
        dispatch = next;
        ticks += 1;
    }
    copies.faulted.extend(dispatch.into_values().map(|enabled| (enabled, Fault::Budget)));
    Ok((copies, ticks))
}

fn activate(circuit: &mut Circuit, dispatch: &mut Dispatch, point: Point, enabled: Bit) {
    if enabled == ZERO {
        return;
    }
    match dispatch.entry(point) {
        Entry::Vacant(entry) => {
            entry.insert(enabled);
        }
        Entry::Occupied(mut entry) => {
            // An input can come from only one point and take only one edge in a tick.
            let joined = circuit.xor(*entry.get(), enabled);
            if joined == ZERO {
                entry.remove();
            } else {
                entry.insert(joined);
            }
        }
    }
}

fn jump(
    circuit: &mut Circuit,
    frame: &Frame<'_>,
    slots: &[Word],
    dispatch: &mut Dispatch,
    writes: &mut Writes,
    target: &Target,
    enabled: Bit,
) {
    if enabled == ZERO {
        return;
    }
    for (dst, value) in frame.incoming(slots, target) {
        writes.entry(dst).or_default().push((enabled, value));
    }
    activate(circuit, dispatch, Point::entry(target.block), enabled);
}
