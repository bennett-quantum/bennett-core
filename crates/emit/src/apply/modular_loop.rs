//! Prove a single shift loop whose residue can be updated without keeping its old value.

use std::collections::BTreeSet;

use bennett_exec::Wire;
use bennett_ir::{BinOp, BlockId, CastOp, Function, Inst, Operand, Predicate, Slot, Target, Terminator, Width};

use super::ApplyInput;
use super::bits::{Bit, Checkpoint, Circuit, ONE, Word, ZERO, definite};
use super::calls::Program;
use super::identity::same_word;
use super::memory::Memory;
use super::modular::{self, Factor, Multiplier};
use super::ranges::{bits, unsigned_max};
use super::symbolic::{Point, Step};

pub(super) struct Plan {
    initial: u128,
    modulus: u64,
    exponent: Word,
    factors: Vec<Factor>,
    condition: Condition,
}

#[derive(Clone, Copy)]
enum Condition {
    BitSet(Operand),
    BitClear(Operand),
    EveryPass,
}

struct Update<'a> {
    product: Product,
    latch: &'a Target,
    skipped: Option<Operand>,
    condition: Condition,
    steps: u64,
}

struct Product {
    left: Operand,
    right: Operand,
    modulus: Operand,
    width: Width,
    dividend: Slot,
    result: Slot,
    steps: u64,
}

impl Product {
    fn inline(instructions: &[Inst]) -> Option<Self> {
        let [
            Inst::Binary { op: BinOp::Mul, dst: product, lhs, rhs, width, .. },
            Inst::Binary { op: BinOp::Urem, dst: result, lhs: Operand::Slot(dividend), rhs: modulus, .. },
        ] = instructions
        else {
            return None;
        };
        (*product == *dividend).then_some(Self {
            left: *lhs,
            right: *rhs,
            modulus: *modulus,
            width: *width,
            dividend: *dividend,
            result: *result,
            steps: 2,
        })
    }

    // A direct helper can supply exactly the same update. Resolve its
    // arguments back to the caller's operands and charge call and return.
    fn recognize(program: &Program<'_>, point: Point, instructions: &[Inst]) -> Option<Self> {
        if let [Inst::Call { dst: Some(result), args, .. }] = instructions {
            let child = program.direct_child(point)?;
            let body = program.block(child);
            let product = Self::inline(&body.insts)?;
            if uses(program.function(child), product.dividend) != 1
                || !matches!(body.term, Terminator::Return(Some(Operand::Slot(slot))) if slot == product.result)
            {
                return None;
            }
            let params = &program.function(child).params;
            let substitute = |operand: Operand| match operand {
                Operand::Const(_) => Some(operand),
                Operand::Slot(slot) => {
                    params.iter().zip(args).find(|(param, _)| param.slot == slot).map(|(_, arg)| *arg)
                }
            };
            Some(Self {
                left: substitute(product.left)?,
                right: substitute(product.right)?,
                modulus: substitute(product.modulus)?,
                result: *result,
                steps: product.steps + 2,
                ..product
            })
        } else {
            let product = Self::inline(instructions)?;
            (uses(program.function(point), product.dividend) == 1).then_some(product)
        }
    }

    fn exact_product(&self, left: u128, right: u128) -> Option<u128> {
        left.checked_mul(right).filter(|&product| product <= self.width.mask())
    }
}

fn uses(function: &Function, slot: Slot) -> usize {
    let mut count = 0;
    let mut read = |source| count += usize::from(source == slot);
    for block in &function.blocks {
        for inst in &block.insts {
            inst.visit_uses(&mut read);
        }
        block.term.visit_uses(&mut read);
    }
    count
}

fn at_block(point: Point, block: BlockId) -> Point {
    Point { block, index: 0, ..point }
}

fn zero_test(inst: &Inst, condition: &Operand) -> Option<(Operand, bool)> {
    let Inst::Compare { pred, dst, lhs, rhs, .. } = inst else { return None };
    if *condition != Operand::Slot(*dst) || !matches!(pred, Predicate::Eq | Predicate::Ne) {
        return None;
    }
    let (value, zero) = if matches!(rhs, Operand::Const(c) if c.value() == 0) { (lhs, rhs) } else { (rhs, lhs) };
    matches!(zero, Operand::Const(c) if c.value() == 0).then_some((*value, *pred == Predicate::Eq))
}

fn parity_test(instructions: &[Inst], condition: &Operand) -> Option<(Operand, bool)> {
    match instructions {
        [Inst::Cast { op: CastOp::Trunc, dst, value, to, .. }]
            if to.bits() == 1 && *condition == Operand::Slot(*dst) =>
        {
            Some((*value, true))
        }
        [Inst::Binary { op: BinOp::And, dst, lhs, rhs, .. }, compare] => {
            let (masked, zero_when_true) = zero_test(compare, condition)?;
            let (value, mask) =
                if matches!(rhs, Operand::Const(c) if c.value() == 1) { (lhs, rhs) } else { (rhs, lhs) };
            (masked == Operand::Slot(*dst) && matches!(mask, Operand::Const(c) if c.value() == 1))
                .then_some((*value, !zero_when_true))
        }
        _ => None,
    }
}

impl<'a> Update<'a> {
    fn recognize(program: &Program<'a>, head: Point) -> Option<Self> {
        let body = program.block(head);
        match &body.term {
            Terminator::Branch { cond, if_true, if_false } => {
                let (exponent, set_when_true) = parity_test(&body.insts, cond)?;
                for (take, skip, set) in [(if_true, if_false, set_when_true), (if_false, if_true, !set_when_true)] {
                    let [old] = skip.args.as_slice() else { continue };
                    if !take.args.is_empty() || take.block == skip.block {
                        continue;
                    }
                    let update = at_block(head, take.block);
                    let arm = program.block(update);
                    let Terminator::Jump(join) = &arm.term else { continue };
                    if join.block != skip.block {
                        continue;
                    }
                    let Some(product) = Product::recognize(program, update, &arm.insts) else { continue };
                    if join.args != [Operand::Slot(product.result)] {
                        continue;
                    }
                    let condition = if set { Condition::BitSet(exponent) } else { Condition::BitClear(exponent) };
                    let steps = body.insts.len() as u64 + 2 + product.steps;
                    return Some(Self { product, latch: join, skipped: Some(*old), condition, steps });
                }
                None
            }
            Terminator::Jump(latch) => {
                let product = Product::recognize(program, head, &body.insts)?;
                if latch.args != [Operand::Slot(product.result)] {
                    return None;
                }
                let steps = product.steps + 1;
                Some(Self { product, latch, skipped: None, condition: Condition::EveryPass, steps })
            }
            Terminator::Switch { .. }
            | Terminator::Return(_)
            | Terminator::Exit(_)
            | Terminator::Longjmp { .. }
            | Terminator::Fault(_) => None,
        }
    }
}

fn clone_slots(slots: &[Word]) -> Vec<Word> {
    #[cfg(test)]
    tests::record_slot_words(slots.len());
    slots.to_vec()
}

fn next_definite<'a>(
    program: &Program<'a>,
    slots: &mut [Word],
    point: Point,
    step: Step<'a>,
) -> Result<Point, Step<'a>> {
    match step {
        Step::Write(next, dst, word) => {
            slots[dst.index()] = word;
            Ok(next)
        }
        Step::Jump(target) => Ok(program.jump(slots, point, target)),
        Step::Transfer(next, writes) => {
            for (dst, word) in writes {
                slots[dst.index()] = word;
            }
            Ok(next)
        }
        stop @ (Step::Return(_)
        | Step::Branch(..)
        | Step::Switch(..)
        | Step::Indirect(..)
        | Step::CheckedTransfer(..)
        | Step::Inactive
        | Step::Fault(_)) => Err(stop),
    }
}

// The loop has not executed on the probe state. In its frame, keep the
// existing restriction to copies, lossless conversions and unconditional
// transfers. Suspended callers have their exact pre-loop slots and memory.
fn replay_exit(
    program: &Program<'_>,
    slots: &mut [Word],
    mut point: Point,
    loop_copy: usize,
    memory: &Memory<'_>,
    scratch: Wire,
    budget: u64,
) -> Option<Word> {
    let mut seen = BTreeSet::new();
    let mut circuit = Circuit::new(scratch);
    let mut memory = memory.fork_for_exit();
    let mut steps = 0;
    while steps < budget && seen.insert(point) {
        if point.copy == loop_copy {
            let block = program.block(point);
            let safe = if let Some(inst) = block.insts.get(point.index) {
                let Inst::Cast { op: CastOp::Zext | CastOp::Trunc, value, to, .. } = inst else {
                    return None;
                };
                unsigned_max(&program.read(slots, point, value)) <= to.mask()
            } else {
                matches!(block.term, Terminator::Jump(_) | Terminator::Return(_))
            };
            if !safe {
                return None;
            }
        }
        steps += 1;
        #[cfg(test)]
        let gates = circuit.forward_len();
        let step = program.step_in(&mut circuit, slots, point, &mut memory, ONE).ok()?;
        memory.commit(&mut circuit);
        #[cfg(test)]
        tests::record_replay_gates(circuit.forward_len() - gates);
        if !circuit.fits() {
            return None;
        }
        let stop = match next_definite(program, slots, point, step) {
            Ok(next) => {
                point = next;
                continue;
            }
            Err(stop) => stop,
        };
        let Step::Return(word) = stop else { return None };
        return Some(word);
    }
    None
}

struct Shape<'a> {
    entry: Point,
    tested: Operand,
    entry_condition: usize,
    true_if_zero: bool,
    empty: &'a Target,
    exit: &'a Target,
    exit_condition: (usize, bool),
    latch: Point,
    back_to_test: Option<&'a Target>,
    joined: usize,
    initial: [Operand; 3],
    update: Product,
    square: Product,
    condition: Condition,
    pass_steps: u64,
}

impl<'a> Shape<'a> {
    fn recognize(program: &Program<'a>, entry: Point) -> Option<Self> {
        let entry_block = program.block(entry);
        if entry.index + 1 != entry_block.insts.len() {
            return None;
        }
        let Terminator::Branch { cond: cond @ Operand::Slot(entry_condition), if_true, if_false } = &entry_block.term
        else {
            return None;
        };
        let (tested, true_if_zero) = zero_test(&entry_block.insts[entry.index], cond)?;
        let (empty, begin) = if true_if_zero { (if_true, if_false) } else { (if_false, if_true) };
        let entry_condition = program.slot(entry, *entry_condition).index();
        let head = at_block(entry, begin.block);
        let body = program.block(head);
        let update = Update::recognize(program, head)?;
        let latch_point = at_block(head, update.latch.block);
        let latch = program.block(latch_point);
        let [joined] = latch.params.as_slice() else { return None };
        let [
            Inst::Binary { op: BinOp::Lshr, dst: shifted, lhs: old_exp, rhs: Operand::Const(amount), .. },
            after_shift @ ..,
        ] = latch.insts.as_slice()
        else {
            return None;
        };
        if amount.value() != 1 {
            return None;
        }
        let (carried, back, exit, exit_condition, square_insts, pretested) =
            if let Terminator::Branch { cond: cond @ Operand::Slot(condition), if_true, if_false } = &latch.term {
                let (compare, square_insts) = after_shift.split_last()?;
                let end_on_true = latch_test(compare, cond, *old_exp, *shifted)?;
                let (exit, back) = if end_on_true { (if_true, if_false) } else { (if_false, if_true) };
                if back.block != head.block {
                    return None;
                }
                let exit_condition = (program.slot(latch_point, *condition).index(), end_on_true);
                (body, back, exit, exit_condition, square_insts, false)
            } else if let Terminator::Jump(back) = &latch.term
                && back.block == entry.block
                && entry.index == 0
                && begin.args.is_empty()
                && tested == *old_exp
            {
                (entry_block, back, empty, (entry_condition, true_if_zero), after_shift, true)
            } else {
                return None;
            };
        if carried.params.len() != 3 {
            return None;
        }
        let square = Product::recognize(program, latch_point.next(), square_insts)?;
        let index_of = |source| back.args.iter().position(|&arg| arg == Operand::Slot(source));
        let residue = index_of(joined.slot)?;
        let exponent = index_of(*shifted)?;
        let base = index_of(square.result)?;
        if residue == exponent || residue == base || exponent == base {
            return None;
        }
        let old_residue = Operand::Slot(carried.params[residue].slot);
        let old_base = Operand::Slot(carried.params[base].slot);
        let residue_uses = 1 + usize::from(update.skipped.is_some()) + usize::from(pretested);
        if uses(program.function(entry), carried.params[residue].slot) != residue_uses {
            return None;
        }
        if *old_exp != Operand::Slot(carried.params[exponent].slot)
            || update.skipped.is_some_and(|old| old != old_residue)
            || square.left != old_base
            || square.right != old_base
            || !((update.product.left == old_residue && update.product.right == old_base)
                || (update.product.right == old_residue && update.product.left == old_base))
        {
            return None;
        }
        match update.condition {
            Condition::BitSet(value) | Condition::BitClear(value) if value != *old_exp => return None,
            Condition::BitSet(_) | Condition::BitClear(_) | Condition::EveryPass => {}
        }
        let captured = |operand| match operand {
            Operand::Const(_) => true,
            Operand::Slot(slot) => program.function(entry).params.iter().any(|param| param.slot == slot),
        };
        if !captured(update.product.modulus) || !captured(square.modulus) {
            return None;
        }
        let initial = [residue, base, exponent]
            .map(|index| if pretested { Operand::Slot(carried.params[index].slot) } else { begin.args[index] });
        let pass_steps = update.steps + square.steps + 3 + u64::from(pretested);
        Some(Self {
            entry,
            tested,
            entry_condition,
            true_if_zero,
            empty,
            exit,
            exit_condition,
            latch: latch_point,
            back_to_test: pretested.then_some(back),
            joined: program.slot(latch_point, joined.slot).index(),
            initial,
            update: update.product,
            square,
            condition: update.condition,
            pass_steps,
        })
    }
}

pub(super) struct Candidate<'a> {
    shape: Shape<'a>,
    slots: Vec<Word>,
    checkpoint: Checkpoint,
    prefix_steps: u64,
    scratch: Wire,
}

impl<'a> Candidate<'a> {
    pub(super) fn remember(
        program: &Program<'a>,
        slots: &[Word],
        entry: Point,
        prefix_steps: u64,
        circuit: &Circuit,
    ) -> Option<Self> {
        let shape = Shape::recognize(program, entry)?;
        Some(Self {
            shape,
            slots: clone_slots(slots),
            checkpoint: circuit.checkpoint(),
            prefix_steps,
            scratch: circuit.scratch_end()?,
        })
    }

    // A recognized loop can leave only through its two exits. Its direct
    // product helpers return immediately and never modify the byte map.
    pub(super) fn exited(&self, next: Point) -> bool {
        next.copy == self.shape.entry.copy
            && next.index == 0
            && [self.shape.empty.block, self.shape.exit.block].contains(&next.block)
    }

    pub(super) fn prove(
        self,
        input: &ApplyInput<'_>,
        program: &Program<'a>,
        memory: &Memory<'_>,
    ) -> Option<(Plan, Checkpoint)> {
        #[cfg(test)]
        tests::record_attempt();
        let Self { shape, mut slots, checkpoint, prefix_steps, scratch } = self;
        let loop_budget = input.budget.checked_sub(prefix_steps)?.checked_sub(2)?;
        let entry = shape.entry;
        slots[shape.entry_condition] = vec![Bit::Constant(!shape.true_if_zero)];
        let modulus = definite(&program.read(&slots, entry, &shape.update.modulus))?;
        if modulus < 2 || definite(&program.read(&slots, entry, &shape.square.modulus))? != modulus {
            return None;
        }
        let modulus64 = u64::try_from(modulus).ok()?;
        let [initial_word, base_word, exponent_word] = shape.initial.map(|source| program.read(&slots, entry, &source));
        let initial = definite(&initial_word)?;
        let mut factor = definite(&base_word)?;
        let mut exponent_bits = exponent_word;
        if initial >= modulus || !same_word(&exponent_bits, &program.read(&slots, entry, &shape.tested)) {
            return None;
        }
        let output_width = (input.out.end - input.out.start) as usize;
        let residue_width = modular::width(modulus64);
        if residue_width > output_width {
            return None;
        }
        exponent_bits.truncate(bits(unsigned_max(&exponent_bits)));
        if definite(&exponent_bits).is_some() {
            return None;
        }
        // Charge the longest body route before trying either return. The
        // shift bounds the number of passes independently of the budget.
        let loop_steps = shape.pass_steps * exponent_bits.len() as u64;
        let exit_budget = loop_budget.checked_sub(loop_steps)?;
        if exit_budget == 0 {
            return None;
        }
        let mut factors = Vec::new();
        for _ in &exponent_bits {
            shape.update.exact_product(modulus - 1, factor)?;
            factors.push(Factor::prove(factor, modulus64)?);
            factor = shape.square.exact_product(factor, factor)? % modulus;
        }

        // The only residue reads on the reached routes are the update and
        // the direct transfers. Check the two complete returns, including
        // any conversion in the wrapper, with distinct probe bits.
        let mut empty_slots = clone_slots(&slots);
        empty_slots[shape.entry_condition] = vec![Bit::Constant(shape.true_if_zero)];
        let probe_end = scratch.checked_add(residue_width as u64)?;
        let empty_point = program.jump(&mut empty_slots, entry, shape.empty);
        let zero_result =
            replay_exit(program, &mut empty_slots, empty_point, entry.copy, memory, probe_end, loop_budget)?;
        if definite(&zero_result) != Some(initial) || zero_result.len() != output_width {
            return None;
        }
        let probe = (scratch..probe_end).map(Bit::input).collect::<Word>();
        let joined = shape.joined;
        let mut joined_word = probe.clone();
        joined_word.resize(slots[joined].len(), ZERO);
        slots[joined] = joined_word;
        let exit_from = shape.back_to_test.map_or(shape.latch, |back| program.jump(&mut slots, shape.latch, back));
        let (condition, value) = shape.exit_condition;
        slots[condition] = vec![Bit::Constant(value)];
        let exit_point = program.jump(&mut slots, exit_from, shape.exit);
        let result = replay_exit(program, &mut slots, exit_point, entry.copy, memory, probe_end, exit_budget)?;
        let mut expected = probe;
        expected.resize(output_width, ZERO);
        if !same_word(&result, &expected) {
            return None;
        }

        Some((
            Plan { initial, modulus: modulus64, exponent: exponent_bits, factors, condition: shape.condition },
            checkpoint,
        ))
    }
}

impl Plan {
    pub(super) fn passes(&self) -> usize {
        self.factors.len()
    }

    pub(super) fn emit(&self, circuit: &mut Circuit) -> Word {
        let residue = (0..modular::width(self.modulus)).map(|_| circuit.fresh()).collect::<Vec<_>>();
        for (bit, &wire) in residue.iter().enumerate() {
            if self.initial >> bit & 1 != 0 {
                circuit.xor_into(wire, ONE, ONE);
            }
        }
        let multiplier = Multiplier::new(circuit, self.modulus);
        let controls = if matches!(self.condition, Condition::BitSet(_)) {
            self.exponent.clone()
        } else {
            // One backwards scan supplies the remaining-exponent test for
            // every pass. For an even exponent, only the higher bits matter.
            let mut higher = ZERO;
            let mut controls = Vec::new();
            for &bit in self.exponent.iter().rev() {
                let active = circuit.and(higher.not(), bit.not()).not();
                controls.push(if matches!(self.condition, Condition::EveryPass) {
                    active
                } else {
                    circuit.and(higher, bit.not())
                });
                higher = active;
            }
            controls.reverse();
            controls
        };
        for (enabled, &factor) in controls.into_iter().zip(&self.factors) {
            multiplier.multiply(circuit, &residue, factor, enabled);
        }
        residue.into_iter().map(|wire| circuit.value(wire)).collect()
    }
}

fn latch_test(compare: &Inst, cond: &Operand, old: Operand, shifted: Slot) -> Option<bool> {
    if let Some((word, zero_when_true)) = zero_test(compare, cond)
        && word == Operand::Slot(shifted)
    {
        return Some(zero_when_true);
    }
    let Inst::Compare { pred, dst, lhs, rhs: Operand::Const(limit), .. } = compare else { return None };
    if *cond != Operand::Slot(*dst) || *lhs != old {
        return None;
    }
    if *pred == Predicate::Ult && limit.value() == 2 {
        Some(true)
    } else if *pred == Predicate::Ugt && limit.value() == 1 {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "modular_loop_tests.rs"]
mod tests;
