//! Stack state carried by each copied frame.

use bennett_exec::STACK_ALIGNMENT;
use bennett_ir::{BinOp, Fault, Predicate, Slot};

use super::arithmetic;
use super::bits::{Bit, Circuit, Word, ZERO, constant};
use super::symbolic::{Point, Step};

#[derive(Clone, Copy)]
pub(super) struct StackSlots {
    pub(super) current: Slot,
    pub(super) top: Slot,
}

impl StackSlots {
    pub(super) fn append(slots: &mut Vec<Word>, address: u64) -> Self {
        let current = Slot(slots.len() as u32);
        let top = Slot(current.0 + 1);
        slots.extend([word(address), word(address)]);
        Self { current, top }
    }

    pub(super) fn allocate<'a>(
        self,
        circuit: &mut Circuit,
        slots: &[Word],
        next: Point,
        dst: Slot,
        size: &[Bit],
        floor: u64,
    ) -> Step<'a> {
        let (current, failed) = reserve(circuit, &slots[self.current.index()], size, floor, true);
        Step::checked(next, vec![(self.current, current.clone()), (dst, current)], failed, Fault::StackOverflow)
    }

    pub(super) fn restore<'a>(self, circuit: &mut Circuit, slots: &[Word], next: Point, pointer: Word) -> Step<'a> {
        let below = arithmetic::compare(circuit, Predicate::Ult, &pointer, &slots[self.current.index()]);
        let above = arithmetic::compare(circuit, Predicate::Ugt, &pointer, &slots[self.top.index()]);
        let failed = circuit.and(below.not(), above.not()).not();
        Step::checked(next, vec![(self.current, pointer)], failed, Fault::BadStackRestore)
    }
}

pub(super) fn reserve(circuit: &mut Circuit, current: &[Bit], size: &[Bit], floor: u64, align: bool) -> (Word, Bit) {
    let underflow = arithmetic::compare(circuit, Predicate::Ult, current, size);
    let mut lowered = arithmetic::binary(circuit, BinOp::Sub, current, size).expect("stack subtraction");
    if align {
        lowered[..STACK_ALIGNMENT.trailing_zeros() as usize].fill(ZERO);
    }
    let below = arithmetic::compare(circuit, Predicate::Ult, &lowered, &word(floor));
    (lowered, circuit.and(underflow.not(), below.not()).not())
}

pub(super) fn word(value: u64) -> Word {
    constant(u128::from(value), u64::BITS as usize)
}
