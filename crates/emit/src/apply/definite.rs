//! Prove constants for the operations that need them, independently of the step budget.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use bennett_exec::Wire;
use bennett_ir::{BinOp, BlockId, FuncId, Inst, Operand, Slot, Terminator, Width};

use super::bits::{Bit, Circuit, Word, ZERO, constant, definite};
use super::identity::Identity;
use super::successors::{self, Edges};
use super::symbolic::Frame;
use super::work::{self, Work};
use super::{arithmetic, division, ranges};

mod results;
pub(super) use results::CallProofs;
use results::ReturnWord;

fn fold_inst(
    inst: &Inst,
    mut read: impl FnMut(&Operand) -> Option<Word>,
    circuit: &mut Circuit,
    unknown: impl FnOnce() -> Word,
) -> Option<Word> {
    match inst {
        // Missing inputs can arrive on a later visit. An operation with known
        // operands that cannot be folded contributes an unknown value now.
        Inst::Binary { op, lhs, rhs, .. } => {
            let left = read(lhs)?;
            let right = read(rhs)?;
            Some(arithmetic::fold_binary(circuit, *op, &left, &right).unwrap_or_else(|| {
                let mut word = unknown();
                if *op == BinOp::Mul {
                    let width =
                        ranges::product_bits(ranges::unsigned_max(&left), ranges::unsigned_max(&right), word.len());
                    word[width..].fill(ZERO);
                } else if matches!(op, BinOp::Urem | BinOp::Srem) {
                    division::apply_remainder_facts(*op, &left, &right, &mut word);
                }
                word
            }))
        }
        Inst::Compare { pred, lhs, rhs, .. } => {
            Some(vec![arithmetic::compare(circuit, *pred, &read(lhs)?, &read(rhs)?)])
        }
        Inst::Cast { op, to, value, .. } => Some(arithmetic::cast(*op, read(value)?, to.bits() as usize)),
        Inst::Select { cond, if_true, if_false, .. } => {
            let condition = read(cond)?[0];
            if let Some(choice) = condition.literal() {
                read(if choice { if_true } else { if_false })
            } else {
                Some(
                    read(if_true)?
                        .into_iter()
                        .zip(read(if_false)?)
                        .map(|(yes, no)| circuit.select(condition, yes, no))
                        .collect(),
                )
            }
        }
        Inst::FuncAddr { func, .. } => Some(constant(u128::from(func.address()), u64::BITS as usize)),
        Inst::Call { .. } | Inst::CallIndirect { .. } => unreachable!("calls use the return proof"),
        Inst::Load { .. }
        | Inst::Alloca { .. }
        | Inst::StackSave { .. }
        | Inst::Setjmp { .. }
        | Inst::ReturnAddress { .. } => Some(unknown()),
        Inst::Store { .. } | Inst::StackRestore { .. } => None,
        Inst::Effect { .. }
        | Inst::Qalloc { .. }
        | Inst::QallocBy { .. }
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::RotateBy { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => unreachable!("checked the supported instructions"),
    }
}

pub(super) struct CallSite<'a> {
    pub(super) target: Option<&'a Operand>,
    pub(super) args: &'a [Operand],
    pub(super) argument_facts: Rc<[Option<Word>]>,
    pub(super) dst: Option<Slot>,
    pub(super) candidates: BTreeSet<FuncId>,
    pub(super) invalid_possible: bool,
}

type Calls<'a> = BTreeMap<(BlockId, usize), CallSite<'a>>;

fn record_call<'a, 'sites>(
    calls: &'sites mut Calls<'a>,
    at: (BlockId, usize),
    call: CallSite<'a>,
) -> &'sites CallSite<'a> {
    let site = calls.entry(at).or_insert_with(|| CallSite {
        target: call.target,
        args: call.args,
        argument_facts: Rc::default(),
        dst: call.dst,
        candidates: BTreeSet::new(),
        invalid_possible: false,
    });
    // Retain the possibilities from earlier visits as input facts change at joins.
    site.candidates.extend(call.candidates);
    site.invalid_possible |= call.invalid_possible;
    site.argument_facts = call.argument_facts;
    site
}

fn intersect_return(joined: &mut Option<ReturnWord>, word: ReturnWord) {
    if let Some(joined) = joined {
        for (old, bit) in joined.iter_mut().zip(word) {
            work::add(Work::FactJoins, 1);
            if !old.zip(bit).is_some_and(|(old, bit)| old.same_value(bit)) {
                *old = None;
            }
        }
    } else {
        work::add(Work::FactStores, word.len());
        *joined = Some(word);
    }
}

type ArgumentIdentities = Vec<Vec<Identity>>;

// Number the argument identities by their first occurrence. The reverse map
// translates the callee's argument identities back into the caller's word.
fn normalize_arguments(
    frame: &Frame<'_>,
    inputs: impl Iterator<Item = Option<Word>>,
) -> (ArgumentIdentities, Vec<Option<Bit>>) {
    let mut identities = BTreeMap::new();
    let mut sources = Vec::new();
    let words = frame
        .module
        .function(frame.callee)
        .params
        .iter()
        .zip(inputs)
        .map(|(parameter, input)| match input {
            Some(word) => word
                .into_iter()
                .map(|bit| match bit.identity() {
                    literal @ Identity::Literal(_) => literal,
                    Identity::Source(source, inverted) => {
                        let (number, first_inverted) = *identities.entry(source).or_insert_with(|| {
                            let number = sources.len() as Wire;
                            sources.push(Some(bit));
                            (number, inverted)
                        });
                        Identity::input(number, inverted ^ first_inverted)
                    }
                })
                .collect(),
            None => (0..frame.slot_width(parameter.slot))
                .map(|_| {
                    let number = sources.len() as Wire;
                    sources.push(None);
                    Identity::input(number, false)
                })
                .collect(),
        })
        .collect();
    (words, sources)
}

#[derive(Clone, Copy)]
struct ArgumentBit {
    slot: Slot,
    index: usize,
    inverted: bool,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Accesses {
    pub(super) memory: bool,
    pub(super) stack: bool,
}

impl Accesses {
    pub(super) fn observe(&mut self, inst: &Inst) {
        self.memory |= matches!(inst, Inst::Load { .. } | Inst::Store { .. });
        self.stack |= matches!(inst, Inst::Alloca { .. } | Inst::StackSave { .. } | Inst::StackRestore { .. });
    }
}

#[derive(Default)]
pub(super) struct Constants<'a> {
    slots: Vec<Option<Word>>,
    argument_bits: Vec<ArgumentBit>,
    returned: Option<ReturnWord>,
    switches: Vec<Edges<'a>>,
    pub(super) calls: Calls<'a>,
    may_return: bool,
    reached: BTreeMap<BlockId, usize>,
    outgoing: BTreeMap<BlockId, Vec<&'a bennett_ir::Target>>,
    accesses: Option<Accesses>,
    definite_stores: bool,
}

impl<'a> Constants<'a> {
    pub(super) fn project(&self, slots: &[Word], operand: &Operand, mut word: Word) -> Word {
        let Operand::Slot(slot) = operand else { return word };
        if let Some(proved) = self.slots.get(slot.index()).and_then(Option::as_ref) {
            for (bit, fact) in word.iter_mut().zip(proved) {
                work::add(Work::FactReads, 1);
                match fact.identity() {
                    Identity::Literal(value) => *bit = Bit::Constant(value),
                    Identity::Source(source, inverted) => {
                        // A current frame's immutable argument slots carry
                        // the physical bits for its abstract identities.
                        if let Some(argument) = source.argument_index().and_then(|index| self.argument_bits.get(index))
                        {
                            let original = slots[argument.slot.index()][argument.index];
                            *bit = original.with_flip(inverted ^ argument.inverted);
                        }
                    }
                }
            }
        }
        word
    }

    pub(super) fn switch_edges(&self, block: BlockId) -> &Edges<'a> {
        &self.switches[block.index()]
    }

    pub(super) fn for_emission(
        frame: &Frame<'a>,
        inputs: impl Iterator<Item = Option<Word>>,
        proofs: &mut CallProofs<'a>,
        needs_analysis: bool,
    ) -> Rc<Self> {
        let (inputs, _) = normalize_arguments(frame, inputs);
        proofs.plan(frame.callee, inputs, needs_analysis)
    }

    pub(super) fn accesses(&self) -> Option<Accesses> {
        self.accesses
    }

    pub(super) fn outgoing(&self, block: BlockId) -> Option<&[&'a bennett_ir::Target]> {
        self.accesses.map(|_| self.outgoing.get(&block).map(Vec::as_slice).unwrap_or_default())
    }

    // Count the reached instruction prefix plus the terminator, when reached.
    // A missing proof leaves the caller to use its conservative fallback.
    pub(super) fn reached_points(&self, block: BlockId) -> Option<usize> {
        self.accesses.map(|_| self.reached.get(&block).copied().unwrap_or(0))
    }

    pub(super) fn definite_stores(&self) -> bool {
        self.definite_stores
    }

    fn with_arguments(frame: &Frame<'a>, inputs: &[Vec<Identity>]) -> Self {
        let function = frame.module.function(frame.callee);
        let mut slots = vec![None; function.slot_count()];
        let mut argument_bits = Vec::new();
        for (parameter, input) in function.params.iter().zip(inputs) {
            work::add(Work::FactReads, input.len());
            for (index, bit) in input.iter().enumerate() {
                if let Identity::Source(source, inverted) = *bit
                    && source.argument_index() == Some(argument_bits.len())
                {
                    argument_bits.push(ArgumentBit { slot: parameter.slot, index, inverted });
                }
            }
            slots[parameter.slot.index()] = Some(input.iter().map(|&identity| identity.argument_bit()).collect());
        }

        Self {
            slots,
            argument_bits,
            switches: vec![Edges::default(); function.blocks.len()],
            reached: BTreeMap::from([(BlockId(0), 0)]),
            definite_stores: true,
            ..Self::default()
        }
    }
}

struct Visit {
    block: BlockId,
    at: usize,
    local: BTreeMap<Slot, Word>,
    circuit: Circuit,
    call: Option<CallTransfer>,
}

struct CallTransfer {
    dst: Option<Slot>,
    arguments: Rc<[Option<Word>]>,
    targets: VecDeque<FuncId>,
    returning: bool,
    returned: Option<ReturnWord>,
}

struct Solver<'a> {
    frame: Frame<'a>,
    plan: Constants<'a>,
    worklist: VecDeque<BlockId>,
    queued: BTreeSet<BlockId>,
    readers: BTreeMap<Slot, BTreeSet<BlockId>>,
    current: Option<Visit>,
}

impl<'a> Solver<'a> {
    fn new(frame: Frame<'a>, inputs: &[Vec<Identity>]) -> Self {
        let entry = BlockId(0);
        let mut plan = Constants::with_arguments(&frame, inputs);
        plan.accesses = Some(Accesses::default());
        Self {
            frame,
            plan,
            worklist: VecDeque::from([entry]),
            queued: BTreeSet::from([entry]),
            readers: BTreeMap::new(),
            current: None,
        }
    }

    // A stopping proof that read no parameter holds for every argument word.
    // Its other facts do not reference the original argument identities.
    fn generalize_stop(&mut self) -> bool {
        let params = &self.frame.module.function(self.frame.callee).params;
        if self.plan.may_return
            || params.iter().any(|param| {
                work::add(Work::TableEntries, 1);
                self.readers.contains_key(&param.slot)
            })
        {
            return false;
        }
        for param in params {
            self.plan.slots[param.slot.index()] = None;
        }
        self.plan.argument_bits.clear();
        true
    }

    fn enqueue(&mut self, block: BlockId) {
        if self.queued.insert(block) {
            self.worklist.push_back(block);
            work::add(Work::BlockSchedules, 1);
        }
    }

    fn read(&mut self, block: BlockId, local: &BTreeMap<Slot, Word>, operand: &Operand) -> Option<Word> {
        let word = match operand {
            Operand::Const(value) => Some(constant(value.value(), value.width().bits() as usize)),
            Operand::Slot(slot) => {
                work::add(Work::TableEntries, 1);
                if let Some(word) = local.get(slot) {
                    Some(word.clone())
                } else {
                    self.readers.entry(*slot).or_default().insert(block);
                    self.plan.slots[slot.index()].clone()
                }
            }
        };
        work::add(Work::OperandBits, word.as_ref().map_or(0, Vec::len));
        word
    }

    fn unknown_bit(&self, slot: Slot, index: usize) -> Bit {
        let arguments = self.plan.argument_bits.len() as Wire;
        let stride = Wire::from(Width::MAX_BITS);
        Bit::input(arguments + stride * Wire::from(slot.0) + index as Wire)
    }

    fn unknown_word(&self, slot: Slot) -> Word {
        (0..self.frame.slot_width(slot)).map(|index| self.unknown_bit(slot, index)).collect()
    }

    fn join(&mut self, slot: Slot, mut word: Word) {
        let old = &self.plan.slots[slot.index()];
        let mut changed = old.is_none();
        for (index, bit) in word.iter_mut().enumerate() {
            work::add(Work::FactJoins, 1);
            let identity = bit.identity();
            let crosses_block = match identity {
                Identity::Literal(_) => true,
                Identity::Source(source, _) => {
                    source.argument_index().is_some_and(|i| i < self.plan.argument_bits.len())
                }
            };
            *bit = if crosses_block && old.as_ref().is_none_or(|old| old[index].same_value(*bit)) {
                identity.argument_bit()
            } else {
                self.unknown_bit(slot, index)
            };
            changed |= old.as_ref().is_some_and(|old| !old[index].same_value(*bit));
        }
        if changed {
            work::add(Work::FactStores, 1 + word.len());
            self.plan.slots[slot.index()] = Some(word);
            if let Some(readers) = self.readers.get(&slot) {
                for &block in readers {
                    if self.queued.insert(block) {
                        self.worklist.push_back(block);
                        work::add(Work::BlockSchedules, 1);
                    }
                }
            }
        }
    }

    // Run to the next unresolved call. Keep the current transfer and its local
    // facts so a return continues at the call instead of replaying the prefix.
    fn advance(&mut self, mut answer: impl FnMut(FuncId, &[Option<Word>]) -> Option<results::CallResult>) -> bool {
        let function = self.frame.module.function(self.frame.callee);
        let _temporary = work::Temporary::start();
        let declared_memory = !self.frame.module.data.is_empty();
        loop {
            let mut visit = if let Some(visit) = self.current.take() {
                visit
            } else if let Some(block) = self.worklist.pop_front() {
                self.queued.remove(&block);
                work::add(Work::BlockVisits, 1);
                work::add(Work::Transfers, 1);
                let scratch = self.plan.argument_bits.len() + Width::MAX_BITS as usize * self.plan.slots.len();
                Visit { block, at: 0, local: BTreeMap::new(), circuit: Circuit::new(scratch as Wire), call: None }
            } else {
                return true;
            };
            let block = function.block(visit.block);
            let mut stopped = false;
            while visit.at < block.insts.len() {
                if let Some(call) = &mut visit.call {
                    while let Some(&target) = call.targets.front() {
                        let Some(result) = answer(target, &call.arguments) else {
                            self.current = Some(visit);
                            return false;
                        };
                        call.targets.pop_front();
                        if let results::CallResult::Returns(mut word) = result {
                            call.returning = true;
                            if let Some(dst) = call.dst {
                                word.resize(self.frame.slot_width(dst), None);
                                intersect_return(&mut call.returned, word);
                            }
                        }
                    }
                    let call = visit.call.take().expect("finished call transfer");
                    if !call.returning {
                        stopped = true;
                        break;
                    }
                    if let Some((slot, word)) = call.dst.zip(call.returned) {
                        let unknown = self.unknown_word(slot);
                        let word = word.into_iter().zip(unknown).map(|(bit, unknown)| bit.unwrap_or(unknown)).collect();
                        visit.local.insert(slot, word);
                    }
                    visit.at += 1;
                    continue;
                }

                let inst = &block.insts[visit.at];
                work::add(Work::Operations, 1);
                work::add(Work::FaultScans, 1);
                if successors::instruction_fault(inst).is_some() {
                    stopped = true;
                    break;
                }
                self.plan.accesses.as_mut().expect("active proof").observe(inst);
                let call = match inst {
                    Inst::Call { callee, args, dst } => Some((None, *dst, args, BTreeSet::from([*callee]), false)),
                    Inst::CallIndirect { signature, target, args, dst } => {
                        let Some(address) = self.read(visit.block, &visit.local, target) else {
                            stopped = true;
                            break;
                        };
                        let (targets, invalid) = successors::indirect_targets(self.frame.module, signature, &address);
                        Some((Some(target), *dst, args, targets, invalid))
                    }
                    Inst::Binary { .. }
                    | Inst::Compare { .. }
                    | Inst::Select { .. }
                    | Inst::Cast { .. }
                    | Inst::Load { .. }
                    | Inst::Store { .. }
                    | Inst::StackSave { .. }
                    | Inst::StackRestore { .. }
                    | Inst::Alloca { .. }
                    | Inst::Setjmp { .. }
                    | Inst::ReturnAddress { .. }
                    | Inst::Effect { .. }
                    | Inst::Qalloc { .. }
                    | Inst::QallocBy { .. }
                    | Inst::Qfree { .. }
                    | Inst::Gate { .. }
                    | Inst::GateAt { .. }
                    | Inst::RotateBy { .. }
                    | Inst::Measure { .. }
                    | Inst::Apply { .. }
                    | Inst::FuncAddr { .. } => None,
                };
                if let Some((target, dst, args, candidates, invalid_possible)) = call {
                    let argument_facts: Rc<[_]> =
                        args.iter().map(|arg| self.read(visit.block, &visit.local, arg)).collect();
                    let site = record_call(
                        &mut self.plan.calls,
                        (visit.block, visit.at),
                        CallSite { target, dst, args, candidates, invalid_possible, argument_facts },
                    );
                    visit.call = Some(CallTransfer {
                        dst,
                        arguments: Rc::clone(&site.argument_facts),
                        targets: site.candidates.iter().copied().collect(),
                        returning: false,
                        returned: None,
                    });
                    work::add(Work::Successors, site.candidates.len());
                    continue;
                }
                if declared_memory && let Inst::Store { address, .. } = inst {
                    self.plan.definite_stores &=
                        self.read(visit.block, &visit.local, address).as_deref().and_then(definite).is_some();
                }
                if let Some(result) = inst.result() {
                    let width = self.frame.slot_width(result);
                    let first = self.unknown_bit(result, 0).wire().expect("abstract slot bit").0;
                    let word = fold_inst(
                        inst,
                        |op| self.read(visit.block, &visit.local, op),
                        &mut visit.circuit,
                        || (0..width).map(|index| Bit::input(first + index as Wire)).collect(),
                    );
                    if let Some(word) = word {
                        visit.local.insert(result, word);
                    }
                }
                visit.at += 1;
            }
            let reached = self.plan.reached.get_mut(&visit.block).expect("scheduled block");
            *reached = (*reached).max(visit.at + 1);
            if !stopped {
                self.terminator(&visit, &block.term);
            }
            for (slot, word) in visit.local {
                self.join(slot, word);
            }
        }
    }

    fn terminator(&mut self, visit: &Visit, term: &'a Terminator) {
        work::add(Work::Operations, 1);
        let mut targets = Vec::new();
        let facts = successors::terminator(
            term,
            |operand| self.read(visit.block, &visit.local, operand),
            |target| targets.push(target),
        );
        if let Some(edges) = facts {
            self.plan.switches[visit.block.index()] = edges;
        }
        // Non-local control is unsupported by emission. Keep its return
        // possibility when summarizing potential callees.
        self.plan.may_return |= matches!(term, Terminator::Return(_) | Terminator::Longjmp { .. });
        if let Terminator::Return(Some(value)) = term
            && let Some(word) = self.read(visit.block, &visit.local, value)
        {
            let proof = word.into_iter().map(|bit| {
                let identity = bit.identity();
                match identity {
                    Identity::Literal(_) => Some(identity.argument_bit()),
                    Identity::Source(source, _) => source
                        .argument_index()
                        .filter(|&index| index < self.plan.argument_bits.len())
                        .map(|_| identity.argument_bit()),
                }
            });
            intersect_return(&mut self.plan.returned, proof.collect());
        }
        let function = self.frame.module.function(self.frame.callee);
        // Evaluate all incoming operands against the old facts before joining
        // any updates, including on a back edge to the same block.
        let mut incoming = Vec::new();
        for &target in &targets {
            if self.reach(target.block) {
                self.enqueue(target.block);
            }
            for (param, operand) in function.block(target.block).params.iter().zip(&target.args) {
                if let Some(word) = self.read(visit.block, &visit.local, operand) {
                    incoming.push((param.slot, word));
                }
            }
        }
        for (slot, word) in incoming {
            self.join(slot, word);
        }
        work::add(Work::FlowEdgeStores, targets.len());
        self.plan.outgoing.insert(visit.block, targets);
    }

    fn reach(&mut self, block: BlockId) -> bool {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.plan.reached.entry(block) {
            entry.insert(0);
            true
        } else {
            false
        }
    }
}
