//! Bound unsigned results by tracing all assignments of a small input register.

use std::collections::BTreeSet;

use bennett_ir::BlockId;

use super::ApplyInput;
use super::bits::{Bit, Circuit, Word, definite};
use super::calls::{Program, arguments};
use super::ranges;
use super::symbolic::{Point, Step};

// This caps classical analysis work. If it is exceeded, no bounds from the
// partial analysis are used and the circuit is emitted at its original widths.
const MAX_INPUT_BITS: usize = 8;
const MAX_ATTEMPTS: usize = 1 << 20;

#[derive(Default)]
pub(super) struct Bounds {
    pub(super) widths: Vec<Option<usize>>,
    // The last attempt made by any input, including a potentially faulting attempt.
    pub(super) last_attempt: Option<u64>,
}

pub(super) fn analyze(program: &Program<'_>, input: &ApplyInput<'_>, count: usize) -> Option<Bounds> {
    let args = arguments(input);
    let wires = args
        .iter()
        .flatten()
        .filter_map(|bit| match *bit {
            Bit::Wire(wire, _) => Some(wire),
            Bit::Constant(_) => None,
        })
        .collect::<BTreeSet<_>>();
    if wires.len() > MAX_INPUT_BITS {
        return None;
    }
    let wires = wires.into_iter().collect::<Vec<_>>();
    let mut widths = vec![None::<usize>; count];
    let mut circuit = Circuit::new(input.scratch_start);
    let mut attempts = MAX_ATTEMPTS;
    let mut last_attempt = Some(0u64);
    for assignment in 0..1 << wires.len() {
        let args = args.iter().map(|word| {
            word.iter()
                .map(|bit| match *bit {
                    Bit::Constant(_) => *bit,
                    Bit::Wire(wire, inverted) => {
                        let index = wires.binary_search(&wire).expect("input wire");
                        Bit::Constant((assignment & (1 << index) != 0) ^ inverted)
                    }
                })
                .collect::<Word>()
        });
        let mut slots = program.root().initial(args);
        slots.resize_with(count, Vec::new);
        let mut point = Point::entry(BlockId(0));
        let mut stopped = None;
        for spent in 0..input.budget {
            attempts = attempts.checked_sub(1)?;
            point = match program.step(&mut circuit, &slots, point).ok()? {
                Step::Write(next, dst, word) => {
                    let bits = ranges::bits(definite(&word)?);
                    let width = &mut widths[dst.index()];
                    *width = Some(width.map_or(bits, |previous| previous.max(bits)));
                    slots[dst.index()] = word;
                    next
                }
                Step::Jump(target) => program.jump(&mut slots, point, target),
                Step::Transfer(next, writes) => {
                    for (dst, word) in writes {
                        slots[dst.index()] = word;
                    }
                    next
                }
                Step::Return(_) | Step::Fault(_) => {
                    stopped = Some(spent + 1);
                    break;
                }
                Step::Branch(..) | Step::CheckedWrite(..) => return None,
            };
        }
        last_attempt = last_attempt.zip(stopped).map(|(previous, stopped)| previous.max(stopped));
    }
    Some(Bounds { widths, last_attempt })
}
