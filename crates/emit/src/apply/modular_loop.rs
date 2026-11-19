//! Prove a single shift loop whose residue can be updated without keeping its old value.

use std::collections::BTreeSet;

use bennett_ir::{BinOp, BlockId, CastOp, Function, Inst, Operand, Predicate, Slot, Target, Terminator, Width};

use super::ApplyInput;
use super::bits::{Bit, Circuit, ONE, Word, ZERO, constant, definite};
use super::calls::Program;
use super::modular::{self, Factor, Multiplier};
use super::ranges::{bits, unsigned_max};
use super::symbolic::{Point, Step};

pub(super) struct Plan {
    initial: u128,
    modulus: u64,
    exponent: Word,
    factors: Vec<Factor>,
    condition: Condition,
    pub(super) definite_steps: u64,
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

enum End {
    Test(Point),
    Return(Word),
}

// Walk the acyclic wrapper code. The prefix can enter direct copies. Each
// exit must only move or losslessly convert its result on the way out.
fn straight(program: &Program<'_>, slots: &mut [Word], mut point: Point, prefix: bool) -> Option<(End, u64)> {
    let mut seen = BTreeSet::new();
    let mut circuit = Circuit::new(0);
    let mut steps = 0;
    while seen.insert(point) {
        let block = program.block(point);
        let inst = block.insts.get(point.index);
        if prefix
            && point.index + 1 == block.insts.len()
            && let (Some(inst), Terminator::Branch { cond, .. }) = (inst, &block.term)
            && zero_test(inst, cond).is_some()
        {
            return Some((End::Test(point), steps));
        }
        let allowed = if let Some(inst) = inst {
            if let Inst::Cast { op: CastOp::Zext | CastOp::Trunc, value, to, .. } = inst {
                unsigned_max(&program.read(slots, point, value)) <= to.mask()
            } else {
                prefix && program.direct_child(point).is_some()
            }
        } else {
            matches!(block.term, Terminator::Jump(_) | Terminator::Return(_))
        };
        if !allowed {
            return None;
        }
        steps += 1;
        point = match program.step(&mut circuit, slots, point).ok()? {
            Step::Write(next, dst, word) => {
                slots[dst.index()] = word;
                next
            }
            Step::Jump(target) => program.jump(slots, point, target),
            Step::Transfer(next, writes) => {
                for (dst, word) in writes {
                    slots[dst.index()] = word;
                }
                next
            }
            Step::Return(word) => return Some((End::Return(word), steps)),
            Step::Branch(..) | Step::CheckedWrite(..) | Step::Fault(_) => return None,
        };
    }
    None
}

impl Plan {
    pub(super) fn prove(input: &ApplyInput<'_>, program: &Program<'_>, slots: &[Word]) -> Option<Self> {
        if input.entry_fault.is_some() || input.budget == 0 {
            return None;
        }
        let mut slots = slots.to_vec();
        let (End::Test(entry), prefix_steps) = straight(program, &mut slots, Point::entry(BlockId(0)), true)? else {
            return None;
        };
        let entry_block = program.block(entry);
        let Terminator::Branch { cond, if_true, if_false } = &entry_block.term else { return None };
        let (tested, true_if_zero) = zero_test(&entry_block.insts[entry.index], cond)?;
        let (empty, begin) = if true_if_zero { (if_true, if_false) } else { (if_false, if_true) };
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
        let (carried, back, exit, square_insts, pretested) =
            if let Terminator::Branch { cond, if_true, if_false } = &latch.term {
                let (compare, square_insts) = after_shift.split_last()?;
                let end_on_true = latch_test(compare, cond, *old_exp, *shifted)?;
                let (exit, back) = if end_on_true { (if_true, if_false) } else { (if_false, if_true) };
                if back.block != head.block {
                    return None;
                }
                (body, back, exit, square_insts, false)
            } else if let Terminator::Jump(back) = &latch.term
                && back.block == entry.block
                && entry.index == 0
                && begin.args.is_empty()
                && tested == *old_exp
            {
                (entry_block, back, empty, after_shift, true)
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
        let modulus = definite(&program.read(&slots, entry, &update.product.modulus))?;
        if modulus < 2 || definite(&program.read(&slots, entry, &square.modulus))? != modulus {
            return None;
        }
        let modulus64 = u64::try_from(modulus).ok()?;
        let initial_word = |index: usize| {
            let source = if pretested { Operand::Slot(carried.params[index].slot) } else { begin.args[index] };
            program.read(&slots, entry, &source)
        };
        let initial = definite(&initial_word(residue))?;
        let mut factor = definite(&initial_word(base))?;
        let mut exponent_bits = initial_word(exponent);
        if initial >= modulus || exponent_bits != program.read(&slots, entry, &tested) {
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
        let mut factors = Vec::new();
        for _ in &exponent_bits {
            update.product.exact_product(modulus - 1, factor)?;
            factors.push(Factor::prove(factor, modulus64)?);
            factor = square.exact_product(factor, factor)? % modulus;
        }

        // The only residue reads on the reached routes are the update and
        // the direct transfers. Check the two complete returns, including
        // any conversion in the wrapper, with distinct probe bits.
        let mut empty_slots = slots.clone();
        let empty_point = program.jump(&mut empty_slots, entry, empty);
        let (End::Return(zero_result), empty_steps) = straight(program, &mut empty_slots, empty_point, false)? else {
            return None;
        };
        if zero_result != constant(initial, output_width) {
            return None;
        }
        let probe_end = input.scratch_start.checked_add(residue_width as u64)?;
        let probe = (input.scratch_start..probe_end).map(|wire| Bit::Wire(wire, false)).collect::<Word>();
        let joined = program.slot(latch_point, joined.slot).index();
        let mut joined_word = probe.clone();
        joined_word.resize(slots[joined].len(), ZERO);
        slots[joined] = joined_word;
        let exit_from = if pretested { program.jump(&mut slots, latch_point, back) } else { latch_point };
        let exit_point = program.jump(&mut slots, exit_from, exit);
        let (End::Return(result), exit_steps) = straight(program, &mut slots, exit_point, false)? else {
            return None;
        };
        let mut expected = probe;
        expected.resize(output_width, ZERO);
        if result != expected {
            return None;
        }

        // Count all calls and branches on the most expensive possible routes
        // with a single whole-function allowance. Additional padded rounds
        // are identities on inputs whose exponent has already shifted to zero.
        let entry_steps = prefix_steps + 2;
        // One shift and either a latch test and branch, or a jump back to
        // the header followed by its test and branch.
        let pass_steps = update.steps + square.steps + 3 + u64::from(pretested);
        if entry_steps + empty_steps > input.budget
            || entry_steps + pass_steps * exponent_bits.len() as u64 + exit_steps > input.budget
        {
            return None;
        }
        Some(Self {
            initial,
            modulus: modulus64,
            exponent: exponent_bits,
            factors,
            condition: update.condition,
            definite_steps: prefix_steps + 1,
        })
    }

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
        residue.into_iter().map(|wire| Bit::Wire(wire, false)).collect()
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
