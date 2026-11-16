//! An independent check of the journal-content recomputation measurements
//! (`experiments/journal-content/README.md`: depth-one recomputation, live chains, journal-assisted tape).
//!
//! It shares no code path with the observer (`measure.rs`) or the classifiers (`measure/chains.rs`).
//! Each branch of each `apply` is replayed by a fresh executor that runs the wrapper to the `apply` and
//! then enters the callee the way `region::evaluate` does, with the observer off. This module keeps its
//! own call paths, alignment counts, slot and store provenance and memory-write log, and snapshots the
//! region's frames, slots and stack pointer after every step. A recipe is evaluated from that replayed
//! post-step state (and, for the tape, from retained journal words of earlier records), with its own
//! evaluator; the integer operations are the executor's (`arith`), as the README states.
//!
//! - Soundness: every position a classifier credits as Recomputed must have a common recipe of the
//!   stated kind whose evaluation equals the journal word on every executing branch. For depth one the
//!   classifier's own recipe ([`RecomputeWitness`]) is re-evaluated from the named live sources.
//! - Completeness (spot check): positions left Irreducible for which this module's search finds a recipe
//!   of the stated kind (same producer site, call path and output on every branch; live leaves or literal
//!   operands; for the tape, Same/retained words of earlier records processed earlier) are counted, and
//!   the greedy tape is re-run with that search to give the Irreducible counts the stated rules imply.
//!   No misses are allowed on these fixtures. `COPY_WITNESS_GAP` pins the former gap where instances
//!   with a copy witness (first write, unchanged, store base) carried no chain context.
//! - The observer on and off: the same state and outer journal after every step of every random program.
//!
//! The C lookups run here too (`c_lookups_recompute_soundly`, ignored: minutes and the lowered program),
//! and the global-write workloads (`c_globals_check_independently`, ignored): soundness as above, and the
//! `apply` callee journal (the classified proxy) against each actual world's journal from its `mark` to
//! its `rewind`, with `Stats` against counts taken from the worlds.

use std::collections::HashMap;
use std::ops::Range;

use bennett_ir::{BlockId, FuncId, Inst, Operand, Slot, Terminator, ValidModule};

use crate::executor::{Executor, Frame, Options, Outcome};
use crate::journal::{Journal, Undo};
use crate::measure::{
    Alignment, ApplyJournal, CallPathStep, ClassifiedPosition, JournalMeasurement, LiveSource, ProducerOutput, Site,
    SitePos, SourceKind, WordClass, WordRole, classify_with_chains, classify_with_recompute,
};

/// A value as this module's provenance knows it: the dynamic node that produced it, if supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Val {
    node: Option<usize>,
    value: u128,
}

/// One dynamic execution of a supported producer, as this module tracked it.
#[derive(Clone, Debug)]
struct Node {
    site: Site,
    path: usize,
    output: ProducerOutput,
    /// One per slot operand, in operand order: the operand's value and producer when the producer ran.
    inputs: Vec<Val>,
    value: u128,
}

/// The region's state just after one record was journalled.
#[derive(Clone, Debug)]
struct State {
    /// Region frames, bottom up, with this module's call-path id.
    frames: Vec<(usize, Frame)>,
    /// The region frames' slots, from the root frame's base: `slots[frame.base - offset + slot]`.
    slots: Vec<u128>,
    offset: usize,
    sp: u64,
}

impl State {
    fn slot(&self, frame: &Frame, slot: usize) -> Option<u128> {
        self.slots.get(frame.base - self.offset + slot).copied()
    }
}

#[derive(Clone, Debug)]
struct Rec {
    site: Site,
    kind: Undo,
    a_k: u64,
    b_k: u64,
    path: usize,
    words: Vec<u128>,
    /// This module's producer of each word's old value, where it has one.
    roots: Vec<Option<usize>>,
}

struct Replay {
    recs: Vec<Rec>,
    states: Vec<State>,
    nodes: Vec<Node>,
    entry_memory: Vec<u8>,
    /// (record, start, new bytes): every memory change, by record.
    writes: Vec<(usize, usize, Vec<u8>)>,
}

impl Replay {
    fn byte(&self, record: usize, address: usize) -> Option<u8> {
        for (r, start, bytes) in self.writes.iter().rev() {
            if *r <= record && (*start..*start + bytes.len()).contains(&address) {
                return Some(bytes[address - start]);
            }
        }
        self.entry_memory.get(address).copied()
    }
}

/// Call paths of one `apply`, interned across its branches.
#[derive(Default)]
struct Paths {
    ids: HashMap<Vec<CallPathStep>, usize>,
    paths: Vec<Vec<CallPathStep>>,
}

impl Paths {
    fn intern(&mut self, path: Vec<CallPathStep>) -> usize {
        if let Some(&id) = self.ids.get(&path) {
            return id;
        }
        self.paths.push(path.clone());
        self.ids.insert(path, self.paths.len() - 1);
        self.paths.len() - 1
    }
}

struct Shadow {
    path: usize,
    counts: HashMap<Site, u64>,
    defs: HashMap<Slot, usize>,
}

/// The slot operands of a supported producer instruction, in operand order, or `None` if unsupported.
fn producer_operands(inst: &Inst) -> Option<Vec<Operand>> {
    Some(match inst {
        Inst::Binary { lhs, rhs, .. } | Inst::Compare { lhs, rhs, .. } => vec![*lhs, *rhs],
        Inst::Select { cond, if_true, if_false, .. } => vec![*cond, *if_true, *if_false],
        Inst::Cast { value, .. } | Inst::Store { value, .. } => vec![*value],
        Inst::Load { address, .. } => vec![*address],
        Inst::FuncAddr { .. } | Inst::StackSave { .. } => vec![],
        _ => return None,
    })
}

fn val_of(e: &Executor<'_>, defs: &HashMap<Slot, usize>, operand: &Operand) -> Option<Val> {
    match operand {
        Operand::Slot(slot) => Some(Val { node: defs.get(slot).copied(), value: e.slot(*slot) }),
        Operand::Const(_) => None,
    }
}

fn snapshot(e: &Executor<'_>, base: usize, shadow: &[Shadow]) -> State {
    let frames = &e.frames[base..];
    assert_eq!(frames.len(), shadow.len(), "shadow mirrors the region frames");
    State {
        frames: shadow.iter().zip(frames).map(|(s, f)| (s.path, *f)).collect(),
        slots: e.slots[frames[0].base..e.live_top()].to_vec(),
        offset: frames[0].base,
        sp: e.stack_pointer,
    }
}

/// Replays branch `args` of the `apply_index`-th dynamic `apply` of `main(main_args)`.
fn replay(
    module: &ValidModule,
    main: FuncId,
    main_args: &[u64],
    apply_index: usize,
    args: &[u128],
    paths: &mut Paths,
) -> Replay {
    let options = Options { budget: Some(10_000_000), allow_region_faults: true, ..Options::default() };
    let mut e = Executor::new(module, main, main_args, options).expect("starts");
    let mut seen = 0;
    let (callee, budget) = loop {
        let top = *e.top();
        if let Some(Inst::Apply { callee, budget, .. }) = e.function(top.func).block(top.block).insts.get(top.inst) {
            if seen == apply_index {
                break (*callee, e.read_word(budget));
            }
            seen += 1;
        }
        assert!(e.step().is_none(), "the run reaches apply {apply_index}");
    };
    // Exactly what `region::evaluate` sets up, with the observer off.
    e.journal = Some(Journal::default());
    e.outcome = None;
    e.steps = 0;
    e.budget = Some(budget);
    e.base_depth = e.frames.len();
    e.in_region = true;
    let base = e.frames.len();
    let entry_memory = e.memory.to_vec();
    let small = entry_memory.len() <= 1 << 17;
    e.push_frame(callee, args).expect("the region callee enters");
    let mut a_counts: HashMap<Site, u64> = HashMap::new();
    let root_path = paths.intern(Vec::new());
    let mut shadow = vec![Shadow { path: root_path, counts: HashMap::new(), defs: HashMap::new() }];
    let entry_site = Site { func: callee, block: BlockId(0), pos: SitePos::Entry };
    a_counts.insert(entry_site, 1);
    let journal = e.journal.as_ref().expect("journal");
    assert_eq!(journal.records(), 1);
    let mut replay = Replay {
        recs: vec![Rec {
            site: entry_site,
            kind: journal.kind_at(0),
            a_k: 0,
            b_k: 0,
            path: root_path,
            words: journal.words().to_vec(),
            roots: vec![None; journal.words().len()],
        }],
        states: vec![snapshot(&e, base, &shadow)],
        nodes: Vec::new(),
        entry_memory,
        writes: Vec::new(),
    };
    let mut stores: Vec<(Range<usize>, usize)> = Vec::new();
    loop {
        let top = *e.top();
        let function = e.function(top.func);
        let block = function.block(top.block);
        let inst = block.insts.get(top.inst);
        let site =
            Site { func: top.func, block: top.block, pos: inst.map_or(SitePos::Term, |_| SitePos::Inst(top.inst)) };
        let executing = shadow.len() - 1;
        let defs = &shadow[executing].defs;
        // Pre-step: producers about to run (output, operands' Vals), and old-value roots.
        let mut producers: Vec<(ProducerOutput, Vec<Val>)> = Vec::new();
        let mut store_range = None;
        let mut slot_root = None;
        let mut jump_roots = Vec::new();
        let mut target_params: Vec<Slot> = Vec::new();
        match inst {
            Some(inst) => {
                if let Some(operands) = producer_operands(inst) {
                    let inputs = operands.iter().filter_map(|o| val_of(&e, defs, o)).collect();
                    producers.push((ProducerOutput::Instruction, inputs));
                }
                if let Inst::Store { width, address, .. } = inst {
                    let start = e.read_word(address) as usize;
                    let range = start..start + width.bytes() as usize;
                    slot_root = stores.iter().find(|(r, _)| *r == range).map(|(_, n)| *n);
                    store_range = Some(range);
                } else if let Some(dst) = inst.result() {
                    slot_root = defs.get(&dst).copied();
                }
            }
            None => {
                let mut taken = None;
                let target = match &block.term {
                    Terminator::Jump(_) => Some(0),
                    Terminator::Branch { cond, .. } => Some(if e.read(cond) == 1 { 0 } else { 1 }),
                    Terminator::Switch { value, cases, .. } => {
                        let v = e.read(value);
                        Some(cases.iter().position(|c| c.value == v).unwrap_or(cases.len()))
                    }
                    _ => None,
                };
                let mut index = 0;
                block.term.visit_targets(|edge| {
                    if Some(index) == target {
                        taken = Some(edge);
                    }
                    index += 1;
                });
                if let Some(edge) = taken {
                    for (argument, operand) in edge.args.iter().enumerate() {
                        let inputs = val_of(&e, defs, operand).into_iter().collect();
                        producers
                            .push((ProducerOutput::JumpArgument { target: target.expect("taken"), argument }, inputs));
                    }
                    target_params = function.block(edge.block).params.iter().map(|p| p.slot).collect();
                    jump_roots = target_params.iter().map(|s| defs.get(s).copied()).collect();
                }
            }
        }
        let journal = e.journal.as_ref().expect("journal");
        let (records_before, words_before) = (journal.records(), journal.words().len());
        let memory_before = small.then(|| e.memory.to_vec());
        let outcome = e.step();
        let journal = e.journal.as_ref().expect("journal");
        if journal.records() == records_before {
            assert!(matches!(outcome, Some(Outcome::Faulted(_))), "only a fault journals nothing");
            break;
        }
        assert_eq!(journal.records(), records_before + 1, "one record per step");
        let record = records_before;
        let kind = journal.kind_at(record);
        let words = journal.words()[words_before..].to_vec();
        let a_k = {
            let c = a_counts.entry(site).or_insert(0);
            *c += 1;
            *c - 1
        };
        let b_k = {
            let c = shadow[executing].counts.entry(site).or_insert(0);
            *c += 1;
            *c - 1
        };
        let path = shadow[executing].path;
        let mut roots = vec![None; words.len()];
        // Memory changes, found by comparison when memory is small.
        match memory_before {
            Some(before) => {
                let now = e.memory.to_vec();
                let mut address = 0;
                while address < before.len() {
                    if before[address] != now[address] {
                        let start = address;
                        while address < before.len() && before[address] != now[address] {
                            address += 1;
                        }
                        replay.writes.push((record, start, now[start..address].to_vec()));
                    } else {
                        address += 1;
                    }
                }
            }
            None => {
                let range = match (kind, inst) {
                    (Undo::Store, _) => store_range.clone(),
                    (Undo::Setjmp, Some(Inst::Setjmp { buffer, .. })) => {
                        let start = e.read_word(buffer) as usize;
                        Some(start..start + 32)
                    }
                    _ => None,
                };
                if let Some(range) = range {
                    let mut bytes = vec![0; range.len()];
                    e.memory.read_into(range.clone(), &mut bytes);
                    replay.writes.push((record, range.start, bytes));
                }
            }
        }
        let new_node = |replay: &mut Replay, output: ProducerOutput, inputs: Vec<Val>, value: u128| {
            replay.nodes.push(Node { site, path, output, inputs, value });
            replay.nodes.len() - 1
        };
        match kind {
            Undo::Slot | Undo::Qalloc | Undo::Measure | Undo::Mark | Undo::Alloca => {
                let dst = inst.and_then(Inst::result).expect("a slot record has a result");
                roots[0] = if kind == Undo::Slot { slot_root } else { None };
                let value = e.slots[e.top().base + dst.index()];
                match producers.pop() {
                    Some((output, inputs)) if kind == Undo::Slot => {
                        let node = new_node(&mut replay, output, inputs, value);
                        shadow[executing].defs.insert(dst, node);
                    }
                    _ => {
                        shadow[executing].defs.remove(&dst);
                    }
                }
            }
            Undo::Store => {
                let range = store_range.expect("a store has a range");
                roots[0] = slot_root;
                let mut bytes = [0u8; 16];
                e.memory.read_into(range.clone(), &mut bytes[..range.len()]);
                let value = u128::from_le_bytes(bytes);
                stores.retain(|(r, _)| r.end <= range.start || r.start >= range.end);
                let (output, inputs) = producers.pop().expect("a store produces its value operand");
                let node = new_node(&mut replay, output, inputs, value);
                stores.push((range, node));
            }
            Undo::Jump => {
                let frame = *e.top();
                for (index, slot) in target_params.iter().enumerate() {
                    roots[index] = jump_roots[index];
                    let value = e.slots[frame.base + slot.index()];
                    let (output, inputs) = producers[index].clone();
                    let node = new_node(&mut replay, output, inputs, value);
                    shadow[executing].defs.insert(*slot, node);
                }
            }
            Undo::Call => {
                let mut p = paths.paths[path].clone();
                p.push(CallPathStep { site, call_index: b_k });
                let id = paths.intern(p);
                shadow.push(Shadow { path: id, counts: HashMap::new(), defs: HashMap::new() });
            }
            Undo::Return => {
                shadow.pop();
                let caller = *e.top();
                if let Some(dst) = e.function(caller.func).block(caller.block).insts[caller.inst - 1].result() {
                    shadow.last_mut().expect("caller").defs.remove(&dst);
                }
            }
            Undo::Setjmp => {
                let Some(Inst::Setjmp { buffer, dst }) = inst else { panic!("a setjmp record comes from a setjmp") };
                // The buffer operand is not the result, so it still holds the address after the step.
                let start = e.read_word(buffer) as usize;
                let range = start..start + 32;
                stores.retain(|(r, _)| r.end <= range.start || r.start >= range.end);
                shadow[executing].defs.remove(dst);
            }
            Undo::Longjmp => {
                let keep = e.frames.len() - base;
                shadow.truncate(keep);
                let top = *e.top();
                if let Some(dst) = e.function(top.func).block(top.block).insts[top.inst - 1].result() {
                    shadow.last_mut().expect("target").defs.remove(&dst);
                }
            }
            _ => {}
        }
        replay.recs.push(Rec { site, kind, a_k, b_k, path, words, roots });
        replay.states.push(snapshot(&e, base, &shadow));
        if outcome.is_some() {
            break;
        }
    }
    replay
}

/// One aligned instance: (branch, record, word index).
type Inst3 = (usize, usize, usize);

/// A retained (Same or still-Irreducible) word position usable as a tape leaf: per branch, its record
/// and value.
struct Seed {
    per_branch: HashMap<usize, (usize, u128)>,
}

struct Search<'a> {
    module: &'a ValidModule,
    replays: &'a [Replay],
    /// (branch, record) of each instance of the target position.
    at: Vec<(usize, usize)>,
    seeds: &'a [&'a Seed],
    /// Require the same producer call path across branches (the stated rule); otherwise site and output.
    strict: bool,
    depth_one: bool,
    memo: HashMap<Vec<Val>, Option<Vec<u128>>>,
}

impl Search<'_> {
    fn live_leaf(&self, vals: &[Val]) -> bool {
        let (b0, r0) = self.at[0];
        let s0 = &self.replays[b0].states[r0];
        let holds = |j: usize, path: usize, slot: Option<usize>| -> bool {
            let (b, r) = self.at[j];
            let s = &self.replays[b].states[r];
            match path {
                usize::MAX => u128::from(s.sp) == vals[j].value,
                _ => s.frames.iter().find(|(p, _)| *p == path).is_some_and(|(_, f)| match slot {
                    Some(slot) => s.slot(f, slot) == Some(vals[j].value),
                    None => u128::from(f.entry_stack_pointer) == vals[j].value,
                }),
            }
        };
        let all = |path: usize, slot: Option<usize>| (0..self.at.len()).all(|j| holds(j, path, slot));
        if u128::from(s0.sp) == vals[0].value && all(usize::MAX, None) {
            return true;
        }
        for (path, frame) in &s0.frames {
            for slot in 0..self.module.function(frame.func).slot_count() {
                if s0.slot(frame, slot) == Some(vals[0].value) && all(*path, Some(slot)) {
                    return true;
                }
            }
            if u128::from(frame.entry_stack_pointer) == vals[0].value && all(*path, None) {
                return true;
            }
        }
        false
    }

    /// How the root was recovered: the common live source, or the producer site.
    fn explain(&self, vals: &[Val]) -> String {
        let (b0, r0) = self.at[0];
        let s0 = &self.replays[b0].states[r0];
        for (path, frame) in &s0.frames {
            for slot in 0..self.module.function(frame.func).slot_count() {
                if s0.slot(frame, slot) == Some(vals[0].value)
                    && (0..self.at.len()).all(|j| {
                        let (b, r) = self.at[j];
                        let s = &self.replays[b].states[r];
                        s.frames
                            .iter()
                            .find(|(p, _)| p == path)
                            .is_some_and(|(_, f)| s.slot(f, slot) == Some(vals[j].value))
                    })
                {
                    return format!("live copy: path {path} f{} slot {slot}", frame.func.0);
                }
            }
        }
        match vals[0].node {
            Some(n) => format!("producer {:?}", self.replays[b0].nodes[n].site),
            None => "other leaf".to_owned(),
        }
    }

    fn tape_leaf(&self, vals: &[Val]) -> bool {
        self.seeds.iter().any(|seed| {
            self.at
                .iter()
                .zip(vals)
                .all(|((b, r), v)| seed.per_branch.get(b).is_some_and(|(sr, sv)| sr < r && *sv == v.value))
        })
    }

    /// The values a common recipe computes on each branch, if this module finds one.
    fn recover(&mut self, vals: Vec<Val>, root: bool) -> Option<Vec<u128>> {
        if let Some(done) = self.memo.get(&vals) {
            return done.clone();
        }
        let result = self.recover_uncached(&vals, root);
        self.memo.insert(vals, result.clone());
        result
    }

    fn recover_uncached(&mut self, vals: &[Val], root: bool) -> Option<Vec<u128>> {
        let leaf_allowed = !(self.depth_one && root);
        if leaf_allowed && (self.live_leaf(vals) || (!self.depth_one && self.tape_leaf(vals))) {
            return Some(vals.iter().map(|v| v.value).collect());
        }
        if self.depth_one && !root {
            return None;
        }
        let nodes: Vec<&Node> = vals
            .iter()
            .zip(&self.at)
            .map(|(v, (b, _))| v.node.map(|n| &self.replays[*b].nodes[n]))
            .collect::<Option<_>>()?;
        let first = nodes[0];
        if nodes
            .iter()
            .any(|n| n.site != first.site || n.output != first.output || (self.strict && n.path != first.path))
        {
            return None;
        }
        let operands = self.operands(first.site, first.output)?;
        let mut children = Vec::new();
        let mut slot_index = 0;
        for operand in &operands {
            match operand {
                Operand::Const(c) => children.push(vec![c.value(); vals.len()]),
                Operand::Slot(_) => {
                    let child: Vec<Val> = nodes.iter().map(|n| n.inputs[slot_index]).collect();
                    slot_index += 1;
                    children.push(self.recover(child, false)?);
                }
            }
        }
        let mut out = Vec::with_capacity(vals.len());
        for (j, node) in nodes.iter().enumerate() {
            let (b, r) = self.at[j];
            let args: Vec<u128> = children.iter().map(|c| c[j]).collect();
            let value = evaluate(self.module, &self.replays[b], r, first.site, first.output, &args)?;
            if value != node.value {
                return None;
            }
            out.push(value);
        }
        Some(out)
    }

    fn operands(&self, site: Site, output: ProducerOutput) -> Option<Vec<Operand>> {
        let block = self.module.function(site.func).block(site.block);
        match (site.pos, output) {
            (SitePos::Inst(i), ProducerOutput::Instruction) => producer_operands(&block.insts[i]),
            (SitePos::Term, ProducerOutput::JumpArgument { target, argument }) => {
                let mut found = None;
                let mut index = 0;
                block.term.visit_targets(|edge| {
                    if index == target {
                        found = edge.args.get(argument).copied();
                    }
                    index += 1;
                });
                found.map(|o| vec![o])
            }
            _ => None,
        }
    }
}

/// Evaluates the producer at `site` on operand values `args` (all operands, constants included) in the
/// replayed state just after record `record`: current memory, current stack pointer.
fn evaluate(
    module: &ValidModule,
    replay: &Replay,
    record: usize,
    site: Site,
    output: ProducerOutput,
    args: &[u128],
) -> Option<u128> {
    let block = module.function(site.func).block(site.block);
    match (site.pos, output) {
        (SitePos::Term, ProducerOutput::JumpArgument { .. }) => args.first().copied(),
        (SitePos::Inst(i), ProducerOutput::Instruction) => match &block.insts[i] {
            Inst::Binary { op, width, .. } => crate::arith::binary(*op, *width, args[0], args[1]).ok(),
            Inst::Compare { pred, width, .. } => {
                Some(u128::from(crate::arith::compare(*pred, *width, args[0], args[1])))
            }
            Inst::Select { .. } => Some(if args[0] == 1 { args[1] } else { args[2] }),
            Inst::Cast { op, from, to, .. } => Some(crate::arith::cast(*op, *from, *to, args[0])),
            Inst::Load { width, .. } => {
                let start = usize::try_from(args[0]).ok()?;
                let mut value = 0u128;
                for k in 0..width.bytes() as usize {
                    value |= u128::from(replay.byte(record, start.checked_add(k)?)?) << (8 * k);
                }
                Some(value & width.mask())
            }
            Inst::FuncAddr { func, .. } => Some(u128::from(func.address())),
            Inst::StackSave { .. } => Some(u128::from(replay.states[record].sp)),
            Inst::Store { .. } => args.first().copied(),
            _ => None,
        },
        _ => None,
    }
}

type Key = (Option<u32>, Site, u64, WordRole);

fn key_of(position: &ClassifiedPosition) -> Key {
    (position.call_path, position.site, position.k, position.role)
}

/// Counts for one measured program, summed over applies and both alignments.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    /// Records replayed and matched with the observer's copy.
    pub records: usize,
    /// Roots of this module's provenance whose value differs from the journal word (expected 0).
    pub root_mismatches: usize,
    /// Depth-one Recomputed positions; of these, positions whose classifier recipe re-evaluated wrong.
    pub depth_one_credited: usize,
    pub depth_one_witness_wrong: Vec<String>,
    /// Same, with this module's own search (no recipe found / found but evaluation wrong).
    pub depth_one_unconfirmed: Vec<String>,
    pub live_credited: usize,
    pub live_unconfirmed: Vec<String>,
    pub tape_credited: usize,
    pub tape_unconfirmed: Vec<String>,
    /// Irreducible positions (non-control) this module's stated-kind search recovers.
    pub depth_one_missed: usize,
    pub live_missed: usize,
    pub tape_missed: usize,
    pub irreducible_scalar: [usize; 3],
    /// Up to a few missed positions per mode, with the classifier's blockers.
    pub missed_examples: Vec<String>,
    /// Of the live and tape misses: positions where some instance carries a copy witness (first write,
    /// unchanged, store base), so the observer kept no chain context for it.
    pub missed_with_copy_witness: [usize; 2],
    /// Irreducible positions (control included) as classified: live, tape.
    pub irreducible_total: [usize; 2],
    /// Irreducible positions if this module's stated-kind search is applied to every non-Same,
    /// non-Recoverable position: live, and a greedy tape in the classifier's order whose seeds are the
    /// Same and still-Irreducible positions of this re-run.
    pub irreducible_rerun: [usize; 2],
    /// `irreducible_total` and `irreducible_rerun` of alignment A, then B: [[total live, total tape,
    /// rerun live, rerun tape]; 2].
    pub by_alignment: [[usize; 4]; 2],
    /// Tape-credited positions by record kind and role, to show what the check covered.
    pub credited_roles: std::collections::BTreeMap<String, usize>,
}

fn control(role: WordRole) -> bool {
    matches!(role, WordRole::JumpSource | WordRole::ReturnPacked | WordRole::LongjmpControl(_))
}

/// Runs `main(main_args)` with the measurement on and checks every apply.
pub(crate) fn check(module: &ValidModule, main: &str, main_args: &[u64]) -> Report {
    let main = module.function_named(main).expect("main");
    let options = Options { budget: Some(10_000_000), allow_region_faults: true, ..Options::default() };
    let mut executor = Executor::new(module, main, main_args, options).expect("starts");
    executor.measure_journal_content(true);
    executor.run();
    let measurement = executor.take_journal_measurement().expect("on");
    check_measurement(module, main, main_args, &measurement)
}

fn check_measurement(module: &ValidModule, main: FuncId, main_args: &[u64], m: &JournalMeasurement) -> Report {
    let recompute = classify_with_recompute(m);
    let live = classify_with_chains(m, false);
    let tape = classify_with_chains(m, true);
    let mut report = Report::default();
    for (index, apply) in m.applies.iter().enumerate() {
        let mut paths = Paths::default();
        let replays: Vec<Replay> =
            apply.branches.iter().map(|b| replay(module, main, main_args, index, &b.args, &mut paths)).collect();
        validate_replays(apply, &replays, &paths, &mut report);
        for alignment in [Alignment::SiteCount, Alignment::CallPath] {
            let pick = |c: &crate::measure::ApplyClassification| -> Vec<ClassifiedPosition> {
                match alignment {
                    Alignment::SiteCount => c.site_count.clone(),
                    Alignment::CallPath => c.call_path.clone(),
                }
            };
            let (rec, liv, tap) = (pick(&recompute[index]), pick(&live[index]), pick(&tape[index]));
            let mut instances: HashMap<Key, Vec<Inst3>> = HashMap::new();
            for (b, branch) in apply.branches.iter().enumerate() {
                for (r, record) in branch.entries.iter().enumerate() {
                    for (w, word) in record.words.iter().enumerate() {
                        let key = match alignment {
                            Alignment::SiteCount => (None, record.site, record.a_k, word.role),
                            Alignment::CallPath => (Some(record.call_path), record.site, record.b_k, word.role),
                        };
                        instances.entry(key).or_default().push((b, r, w));
                    }
                }
            }
            let before = (report.irreducible_total, report.irreducible_rerun);
            check_alignment(module, apply, &replays, &paths, &instances, [&rec, &liv, &tap], &mut report);
            let row = &mut report.by_alignment[usize::from(alignment == Alignment::CallPath)];
            row[0] += report.irreducible_total[0] - before.0[0];
            row[1] += report.irreducible_total[1] - before.0[1];
            row[2] += report.irreducible_rerun[0] - before.1[0];
            row[3] += report.irreducible_rerun[1] - before.1[1];
        }
    }
    report
}

fn validate_replays(apply: &ApplyJournal, replays: &[Replay], paths: &Paths, report: &mut Report) {
    for (b, (branch, replay)) in apply.branches.iter().zip(replays).enumerate() {
        assert_eq!(branch.entries.len(), replay.recs.len(), "branch {b}: records");
        for (r, (theirs, mine)) in branch.entries.iter().zip(&replay.recs).enumerate() {
            assert_eq!(
                (theirs.site, theirs.kind, theirs.a_k, theirs.b_k),
                (mine.site, mine.kind, mine.a_k, mine.b_k),
                "branch {b} record {r}"
            );
            assert_eq!(apply.call_paths[theirs.call_path as usize], paths.paths[mine.path], "branch {b} record {r}");
            let values: Vec<u128> = theirs.words.iter().map(|w| w.value).collect();
            assert_eq!(values, mine.words, "branch {b} record {r}: words");
            for (w, root) in mine.roots.iter().enumerate() {
                if let Some(node) = root
                    && replay.nodes[*node].value != mine.words[w]
                {
                    report.root_mismatches += 1;
                }
            }
            report.records += 1;
        }
    }
}

fn source_value(apply: &ApplyJournal, paths: &Paths, state: &State, source: &LiveSource) -> Option<u128> {
    if source.kind == SourceKind::StackPointer {
        return Some(u128::from(state.sp));
    }
    let path = paths.ids.get(&apply.call_paths[source.call_path as usize])?;
    let (_, frame) = state.frames.iter().find(|(p, _)| p == path)?;
    match source.kind {
        SourceKind::EntryStackPointer => Some(u128::from(frame.entry_stack_pointer)),
        _ => state.slot(frame, source.slot?.index()),
    }
}

fn check_alignment(
    module: &ValidModule,
    apply: &ApplyJournal,
    replays: &[Replay],
    paths: &Paths,
    instances: &HashMap<Key, Vec<Inst3>>,
    [rec, liv, tap]: [&Vec<ClassifiedPosition>; 3],
    report: &mut Report,
) {
    let word = |(b, r, w): Inst3| replays[b].recs[r].words[w];
    let root = |(b, r, w): Inst3| Val { node: replays[b].recs[r].roots[w], value: replays[b].recs[r].words[w] };
    let name = |p: &ClassifiedPosition| format!("{:?} k={} {:?} path={:?}", p.site, p.k, p.role, p.call_path);
    let copy_witness = |p: &ClassifiedPosition| {
        instances[&key_of(p)].iter().any(|&(b, r, w)| {
            matches!(
                apply.branches[b].entries[r].words[w].witness,
                crate::measure::WordWitness::FirstWrite
                    | crate::measure::WordWitness::Unchanged
                    | crate::measure::WordWitness::StoreBase
            )
        })
    };
    let search = |at: &[Inst3], seeds: &[&Seed], strict: bool, depth_one: bool| -> Option<bool> {
        let mut s = Search {
            module,
            replays,
            at: at.iter().map(|(b, r, _)| (*b, *r)).collect(),
            seeds,
            strict,
            depth_one,
            memo: HashMap::new(),
        };
        let values = s.recover(at.iter().map(|i| root(*i)).collect(), true)?;
        Some(at.iter().zip(values).all(|(i, v)| word(*i) == v))
    };
    // Depth one: re-evaluate the classifier's own recipe, then search independently.
    for p in rec.iter().filter(|p| p.class == WordClass::Recomputed) {
        report.depth_one_credited += 1;
        let at = &instances[&key_of(p)];
        let witness = p.recompute.as_ref().expect("a Recomputed depth-one position names its recipe");
        let block = module.function(witness.site.func).block(witness.site.block);
        let operands = match (witness.site.pos, witness.output) {
            (SitePos::Inst(i), _) => producer_operands(&block.insts[i]),
            (SitePos::Term, ProducerOutput::JumpArgument { target, argument }) => {
                let mut found = None;
                let mut index = 0;
                block.term.visit_targets(|edge| {
                    if index == target {
                        found = edge.args.get(argument).copied();
                    }
                    index += 1;
                });
                found.map(|o| vec![o])
            }
            _ => None,
        };
        let ok = operands.is_some_and(|operands| {
            at.iter().all(|&(b, r, w)| {
                let state = &replays[b].states[r];
                let mut slot_index = 0;
                let mut args = Vec::new();
                for operand in &operands {
                    match operand {
                        Operand::Const(c) => args.push(Some(c.value())),
                        Operand::Slot(_) => {
                            let sources = &witness.inputs[slot_index];
                            slot_index += 1;
                            let values: Vec<_> = sources.iter().map(|s| source_value(apply, paths, state, s)).collect();
                            // Every listed common source must hold the same value.
                            args.push(values[0].filter(|v| values.iter().all(|x| *x == Some(*v))));
                        }
                    }
                }
                let args: Option<Vec<u128>> = args.into_iter().collect();
                args.and_then(|args| evaluate(module, &replays[b], r, witness.site, witness.output, &args))
                    == Some(replays[b].recs[r].words[w])
            })
        });
        if !ok {
            report.depth_one_witness_wrong.push(name(p));
        }
        if search(at, &[], false, true) != Some(true) {
            report.depth_one_unconfirmed.push(name(p));
        }
    }
    // Chains: this module's own search must find a sound common recipe for every credited position.
    let seed_of = |p: &ClassifiedPosition| Seed {
        per_branch: instances[&key_of(p)].iter().map(|&(b, r, w)| (b, (r, word((b, r, w))))).collect(),
    };
    let retained: Vec<Seed> =
        tap.iter().filter(|p| matches!(p.class, WordClass::Same | WordClass::Irreducible)).map(seed_of).collect();
    let retained_refs: Vec<&Seed> = retained.iter().collect();
    for (positions, tape) in [(liv, false), (tap, true)] {
        for p in positions.iter().filter(|p| p.class == WordClass::Recomputed) {
            let at = &instances[&key_of(p)];
            let seeds: &[&Seed] = if tape { &retained_refs } else { &[] };
            let ok = search(at, seeds, false, false) == Some(true);
            if tape {
                *report.credited_roles.entry(format!("{:?}/{:?}", p.kind, p.role)).or_default() += 1;
                report.tape_credited += 1;
                if !ok {
                    report.tape_unconfirmed.push(name(p));
                }
            } else {
                report.live_credited += 1;
                if !ok {
                    report.live_unconfirmed.push(name(p));
                }
            }
        }
    }
    // Completeness: Irreducible scalar positions a stated-kind recipe recovers.
    for p in rec.iter().filter(|p| p.class == WordClass::Irreducible && !control(p.role)) {
        report.irreducible_scalar[0] += 1;
        if search(&instances[&key_of(p)], &[], true, true) == Some(true) {
            report.depth_one_missed += 1;
        }
    }
    for p in liv.iter().filter(|p| p.class == WordClass::Irreducible && !control(p.role)) {
        report.irreducible_scalar[1] += 1;
        if search(&instances[&key_of(p)], &[], true, false) == Some(true) {
            report.live_missed += 1;
            report.missed_with_copy_witness[0] += usize::from(copy_witness(p));
            if report.missed_examples.len() < 3 {
                let at = &instances[&key_of(p)];
                let s = Search {
                    module,
                    replays,
                    at: at.iter().map(|(b, r, _)| (*b, *r)).collect(),
                    seeds: &[],
                    strict: true,
                    depth_one: false,
                    memo: HashMap::new(),
                };
                let vals: Vec<Val> = at.iter().map(|i| root(*i)).collect();
                let witnesses: Vec<String> = at
                    .iter()
                    .take(6)
                    .map(|&(b, r, w)| {
                        format!("b{b}:{}={:?}", word((b, r, w)), apply.branches[b].entries[r].words[w].witness)
                    })
                    .collect();
                report.missed_examples.push(format!("{} -> {}; {:?}", name(p), s.explain(&vals), witnesses));
            }
        }
    }
    // The tape's processing order: (largest record index over instances, key); earlier retained only.
    let max_record = |p: &ClassifiedPosition| instances[&key_of(p)].iter().map(|i| i.1).max().unwrap_or(0);
    let mut order: Vec<&ClassifiedPosition> = tap.iter().collect();
    order.sort_by_key(|p| (max_record(p), key_of(p)));
    report.irreducible_total[0] += liv.iter().filter(|p| p.class == WordClass::Irreducible).count();
    report.irreducible_total[1] += tap.iter().filter(|p| p.class == WordClass::Irreducible).count();
    report.irreducible_rerun[0] += liv
        .iter()
        .filter(|p| {
            matches!(p.class, WordClass::Irreducible | WordClass::Recomputed)
                && (control(p.role) || search(&instances[&key_of(p)], &[], true, false) != Some(true))
        })
        .count();
    let mut rerun_seeds: Vec<Seed> = Vec::new();
    for p in &order {
        let retained = match p.class {
            WordClass::Same => true,
            WordClass::Recoverable => false,
            _ => {
                let refs: Vec<&Seed> = rerun_seeds.iter().collect();
                control(p.role) || search(&instances[&key_of(p)], &refs, true, false) != Some(true)
            }
        };
        if retained {
            if p.class != WordClass::Same {
                report.irreducible_rerun[1] += 1;
            }
            rerun_seeds.push(seed_of(p));
        }
    }
    let mut earlier: Vec<Seed> = Vec::new();
    for p in order {
        if p.class == WordClass::Irreducible && !control(p.role) {
            report.irreducible_scalar[2] += 1;
            let refs: Vec<&Seed> = earlier.iter().collect();
            if search(&instances[&key_of(p)], &refs, true, false) == Some(true) {
                report.tape_missed += 1;
                report.missed_with_copy_witness[1] += usize::from(copy_witness(p));
                if report.missed_examples.len() < 6 {
                    report.missed_examples.push(format!("tape {} {:?}", name(p), p.chain));
                }
            }
        }
        if matches!(p.class, WordClass::Same | WordClass::Irreducible) {
            earlier.push(seed_of(p));
        }
    }
}

const HAND: &str = include_str!("../tests/journal_content.rs");

/// The `const NAME: &str = "...";` program of the hand-check test file.
fn program(name: &str) -> String {
    let start = HAND.find(&format!("const {name}: &str = \"")).expect("program") + name.len() + 16;
    let end = start + HAND[start..].find("\";\n").expect("end");
    HAND[start..end].to_owned()
}

fn load(text: &str) -> ValidModule {
    bennett_ir::load(text).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

fn assert_sound(what: &str, report: &Report) {
    assert_eq!(report.root_mismatches, 0, "{what}: provenance roots {report:#?}");
    assert!(report.depth_one_witness_wrong.is_empty(), "{what}: {report:#?}");
    assert!(report.depth_one_unconfirmed.is_empty(), "{what}: {report:#?}");
    assert!(report.live_unconfirmed.is_empty(), "{what}: {report:#?}");
    assert!(report.tape_unconfirmed.is_empty(), "{what}: {report:#?}");
}

#[test]
fn hand_cases_recompute_soundly_and_completely() {
    let hand = program("HAND");
    let alias = program("ALIAS");
    let no_alias = alias.replace(
        "    %v = mul i8 %n, %seed\n    %saved = or i8 %n, 0",
        "    %saved = or i8 %n, 0\n    %v = mul i8 %n, %seed",
    );
    let deep = no_alias.replace("br loop(i8 2)", "br loop(i8 40)").replace("budget 100", "budget 1000");
    for (what, text, main) in [
        ("HAND/@f", hand.clone(), "main_f"),
        ("HAND/@g", hand, "main_g"),
        ("ALIAS", alias, "main"),
        ("ALIAS saved first", no_alias, "main"),
        ("ALIAS depth 40", deep, "main"),
    ] {
        let module = load(&text);
        let report = check(&module, main, &[]);
        assert_sound(what, &report);
        assert_eq!((report.depth_one_missed, report.live_missed, report.tape_missed), (0, 0, 0), "{what}: {report:#?}");
        eprintln!("{what}: {report:?}");
    }
}

/// A random coherent callee: a loop whose trip count depends on the 3-bit input, carrying two values,
/// with a diamond inside, stores and loads at fixed and data-dependent offsets of one stack cell,
/// selects, comparisons and calls to a helper whose result is opaque to the measurement.
fn random_program(seed: u64) -> String {
    use rand::{Rng, SeedableRng};
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
    let mut next = 0;
    let mut fresh = |prefix: &str| {
        next += 1;
        format!("%{prefix}{next}")
    };
    // Emits `count` random i8 operations into `out`, drawing operands from `pool` and adding results.
    let section = |rng: &mut rand_chacha::ChaCha8Rng,
                   fresh: &mut dyn FnMut(&str) -> String,
                   pool: &mut Vec<String>,
                   out: &mut String,
                   count: usize| {
        let pick = |rng: &mut rand_chacha::ChaCha8Rng, pool: &[String]| -> String {
            if rng.random_bool(0.2) {
                format!("{}", rng.random_range(0..8u8))
            } else {
                pool[rng.random_range(0..pool.len())].clone()
            }
        };
        for _ in 0..count {
            let dst = fresh("v");
            match rng.random_range(0..10) {
                0..=3 => {
                    let op = ["add", "sub", "mul", "xor", "and", "or", "shl", "lshr"][rng.random_range(0..8)];
                    let (a, b) = (pick(rng, pool), pick(rng, pool));
                    out.push_str(&format!("    {dst} = {op} i8 {a}, {b}\n"));
                }
                4 => {
                    let d = fresh("d");
                    let (a, b) = (pick(rng, pool), pick(rng, pool));
                    out.push_str(&format!("    {d} = or i8 {b}, 1\n    {dst} = udiv i8 {a}, {d}\n"));
                }
                5 => {
                    let c = fresh("c");
                    let pred = ["eq", "ult", "slt", "ne"][rng.random_range(0..4)];
                    let (a, b, t, f) = (pick(rng, pool), pick(rng, pool), pick(rng, pool), pick(rng, pool));
                    out.push_str(&format!("    {c} = icmp {pred} i8 {a}, {b}\n    {dst} = select i8 {c}, {t}, {f}\n"));
                }
                6 | 7 => {
                    // A load of 1, 2 or 4 bytes at a fixed or data-dependent, possibly overlapping offset.
                    let address = cell_address(rng, fresh, pool, out);
                    let width = [8, 16, 32][rng.random_range(0..3)];
                    if width == 8 {
                        out.push_str(&format!("    {dst} = load i8 {address}\n"));
                    } else {
                        let wide = fresh("l");
                        out.push_str(&format!(
                            "    {wide} = load i{width} {address}\n    {dst} = trunc i{width} {wide} to i8\n"
                        ));
                    }
                }
                8 => {
                    let address = cell_address(rng, fresh, pool, out);
                    let v = pick(rng, pool);
                    let width = [8, 16, 32][rng.random_range(0..3)];
                    if width == 8 {
                        out.push_str(&format!("    store i8 {v}, {address}\n"));
                    } else {
                        let wide = fresh("z");
                        let v = if v.starts_with('%') { v.clone() } else { format!("i8 {v}") };
                        let v = v.trim_start_matches("i8 ");
                        out.push_str(&format!(
                            "    {wide} = zext i8 {v} to i{width}\n    store i{width} {wide}, {address}\n"
                        ));
                    }
                    out.push_str(&format!("    {dst} = add i8 {v}, 0\n"));
                }
                _ => {
                    let typed = |s: String| if s.starts_with('%') { s } else { format!("i8 {s}") };
                    let (a, b) = (typed(pick(rng, pool)), typed(pick(rng, pool)));
                    out.push_str(&format!("    {dst} = call @h({a}, {b})\n"));
                }
            }
            pool.push(dst);
        }
    };
    fn cell_address(
        rng: &mut rand_chacha::ChaCha8Rng,
        fresh: &mut dyn FnMut(&str) -> String,
        pool: &[String],
        out: &mut String,
    ) -> String {
        let a = fresh("a");
        if rng.random_bool(0.5) {
            out.push_str(&format!("    {a} = add i64 %cell, {}\n", rng.random_range(0..32)));
        } else {
            let (m, w) = (fresh("m"), fresh("w"));
            let v = &pool[rng.random_range(0..pool.len())];
            let mask = [24, 31, 7][rng.random_range(0..3)];
            out.push_str(&format!(
                "    {m} = and i8 {v}, {mask}\n    {w} = zext i8 {m} to i64\n    {a} = add i64 %cell, {w}\n"
            ));
        }
        a
    }
    let mut text = String::from(
        "memory 1024\nfunc @h(i8 %a, i8 %b) -> i8 {\nentry:\n    %t = mul i8 %a, 3\n    %u = xor i8 %t, %b\n    ret %u\n}\n",
    );
    let mut body = String::new();
    body.push_str("func @f(i3 %x0) -> i8 {\nentry:\n    %x = zext i3 %x0 to i8\n    %cell = alloca 48\n");
    for offset in [0, 8, 16, 24] {
        let v = if rng.random_bool(0.5) { "%x".to_owned() } else { format!("{}", rng.random_range(0..8u8)) };
        body.push_str(&format!("    %s{offset} = add i64 %cell, {offset}\n    store i8 {v}, %s{offset}\n"));
    }
    let mut pool = vec!["%x".to_owned()];
    section(&mut rng, &mut fresh, &mut pool, &mut body, 2);
    let trips = fresh("t");
    body.push_str(&format!("    {trips}0 = and i8 %x, 3\n    {trips} = add i8 {trips}0, 1\n"));
    let (i0, i1) = (pool[rng.random_range(0..pool.len())].clone(), pool[rng.random_range(0..pool.len())].clone());
    body.push_str(&format!("    br loop({i0}, {i1}, {trips})\nloop(i8 %p0, i8 %p1, i8 %n):\n"));
    let mut header = vec!["%x".to_owned(), "%p0".to_owned(), "%p1".to_owned(), "%n".to_owned()];
    let count = rng.random_range(1..5);
    section(&mut rng, &mut fresh, &mut header, &mut body, count);
    let c = fresh("c");
    let (a, b) = (header[rng.random_range(0..header.len())].clone(), header[rng.random_range(0..header.len())].clone());
    if rng.random_bool(0.3) {
        // A switch with two edges to `left` (distinct edges to one block) and a default.
        let k = fresh("k");
        body.push_str(&format!(
            "    {c} = icmp ult i8 {a}, {b}\n    {k} = and i8 {a}, 3\n    switch i8 {k}, 0: left, 1: right, 2: left, default: right\n"
        ));
    } else {
        body.push_str(&format!("    {c} = icmp ult i8 {a}, {b}\n    br {c}, left, right\n"));
    }
    for side in ["left", "right"] {
        body.push_str(&format!("{side}:\n"));
        let mut pool = header.clone();
        let count = rng.random_range(0..4);
        section(&mut rng, &mut fresh, &mut pool, &mut body, count);
        let (j0, j1) = (pool[rng.random_range(0..pool.len())].clone(), pool[rng.random_range(0..pool.len())].clone());
        body.push_str(&format!("    br join({j0}, {j1})\n"));
    }
    body.push_str("join(i8 %j0, i8 %j1):\n");
    let mut tail = header.clone();
    tail.extend(["%j0".to_owned(), "%j1".to_owned()]);
    let count = rng.random_range(0..4);
    section(&mut rng, &mut fresh, &mut tail, &mut body, count);
    let (n0, n1, r) = (
        tail[rng.random_range(0..tail.len())].clone(),
        tail[rng.random_range(0..tail.len())].clone(),
        tail[rng.random_range(0..tail.len())].clone(),
    );
    body.push_str(&format!(
        "    %n.next = sub i8 %n, 1\n    %more = icmp ult i8 0, %n.next\n    br %more, loop({n0}, {n1}, %n.next), done({r})\ndone(i8 %out):\n    ret %out\n}}\n"
    ));
    text.push_str(&body);
    text.push_str(
        "func @main() -> i8 {\nentry:\n    %q = qalloc 3\n    h %q[0]\n    h %q[1]\n    h %q[2]\n    %y = qalloc 8\n    %e = qalloc 5\n    apply @f(%q) -> %y, %e budget 5000\n    ret 0\n}\n",
    );
    text
}

/// A reversible run with the observer on and off: the same outcome, statistics, exact state and outer
/// journal (kinds and words) after every step, and the same state again after reversal.
fn assert_observer_changes_nothing(module: &ValidModule, what: &str) {
    let main = module.function_named("main").expect("main");
    let options =
        Options { budget: Some(10_000_000), reversible: true, allow_region_faults: true, ..Options::default() };
    let mut off = Executor::new(module, main, &[], options).expect("starts");
    let mut on = Executor::new(module, main, &[], options).expect("starts");
    // The observer classifies the enumeration's branches, so it computes `apply` that way.
    off.set_apply_mode(crate::ApplyMode::Branches);
    on.measure_journal_content(true);
    let journal = |e: &Executor<'_>| {
        let j = e.journal.as_ref().expect("reversible");
        ((0..j.records()).map(|i| j.kind_at(i)).collect::<Vec<_>>(), j.words().to_vec())
    };
    loop {
        let (a, b) = (off.step(), on.step());
        assert_eq!(a, b, "{what}: outcome");
        assert_eq!(off.stats(), on.stats(), "{what}: stats");
        assert_eq!(off.snapshot(), on.snapshot(), "{what}: state");
        assert_eq!(
            (off.memory.to_vec(), &off.slots, off.stack_pointer),
            (on.memory.to_vec(), &on.slots, on.stack_pointer),
            "{what}"
        );
        assert_eq!(journal(&off), journal(&on), "{what}: journal");
        if a.is_some() {
            break;
        }
    }
    assert_eq!(off.reverse().expect("reverses"), on.reverse().expect("reverses"), "{what}: steps undone");
    assert_eq!(off.snapshot(), on.snapshot(), "{what}: reversed state");
}

/// Seeded random programs: every credited word recovers on every branch; completeness counted.
#[test]
fn random_programs_recompute_soundly() {
    let mut total = Report::default();
    for seed in 0..RANDOM_SEEDS {
        let text = random_program(seed);
        let module = load(&text);
        let report = check(&module, "main", &[]);
        assert_sound(&format!("seed {seed}\n{text}"), &report);
        assert_observer_changes_nothing(&module, &format!("seed {seed}"));
        total.records += report.records;
        for (role, count) in &report.credited_roles {
            *total.credited_roles.entry(role.clone()).or_default() += count;
        }
        total.depth_one_credited += report.depth_one_credited;
        total.live_credited += report.live_credited;
        total.tape_credited += report.tape_credited;
        total.depth_one_missed += report.depth_one_missed;
        total.live_missed += report.live_missed;
        total.tape_missed += report.tape_missed;
        for k in 0..2 {
            total.missed_with_copy_witness[k] += report.missed_with_copy_witness[k];
            total.irreducible_total[k] += report.irreducible_total[k];
            total.irreducible_rerun[k] += report.irreducible_rerun[k];
        }
        assert_eq!((report.depth_one_missed, report.live_missed, report.tape_missed), (0, 0, 0), "seed {seed}");
        assert_eq!(report.irreducible_rerun, report.irreducible_total, "seed {seed}: the greedy tape");
        for k in 0..3 {
            total.irreducible_scalar[k] += report.irreducible_scalar[k];
        }
    }
    eprintln!("random programs: {total:?}");
    assert!(total.live_credited > 0 && total.tape_credited > total.live_credited, "{total:?}");
}

const RANDOM_SEEDS: u64 = 200;

/// The C lookups (`front/tests/c/journal_content.c`) as the front lowered them: set
/// `BENNETT_JC_BN` to the `.bn` file (`target/front-tests/journal_content.bn` after the front tests)
/// and `BENNETT_JC_RUNS` to `lookup:reps,...` (e.g. `jc_hash_lookup:2`).
#[test]
#[ignore = "needs the lowered lookups; minutes"]
fn c_lookups_recompute_soundly() {
    let path = std::env::var("BENNETT_JC_BN").expect("BENNETT_JC_BN");
    let module = load(&std::fs::read_to_string(path).expect("lowered lookups"));
    let runs = std::env::var("BENNETT_JC_RUNS").unwrap_or_else(|_| "jc_hash_lookup:2".to_owned());
    for run in runs.split(',') {
        let (lookup, reps) = run.split_once(':').expect("lookup:reps");
        let report = check(&module, &format!("coherent_{lookup}"), &[0, reps.parse().expect("reps")]);
        // Both alignments are summed; every count is over positions.
        eprintln!(
            "jc\t{run}\tirreducible live/tape={:?}\trerun={:?}\tA,B [live, tape, rerun live, rerun tape]={:?}\trecords={}\tcredited d1/live/tape={}/{}/{}\tunconfirmed={}/{}/{}/{}\tscalar irreducible d1/live/tape={:?}\tmissed d1/live/tape={}/{}/{}\tmissed with copy witness live/tape={:?}",
            report.irreducible_total,
            report.irreducible_rerun,
            report.by_alignment,
            report.records,
            report.depth_one_credited,
            report.live_credited,
            report.tape_credited,
            report.depth_one_witness_wrong.len(),
            report.depth_one_unconfirmed.len(),
            report.live_unconfirmed.len(),
            report.tape_unconfirmed.len(),
            report.irreducible_scalar,
            report.depth_one_missed,
            report.live_missed,
            report.tape_missed,
            report.missed_with_copy_witness,
        );
        for example in &report.missed_examples {
            eprintln!("jc-example\t{run}\t{example}");
        }
        assert_sound(run, &report);
        assert_eq!((report.depth_one_missed, report.live_missed, report.tape_missed), (0, 0, 0), "{run}");
        assert_eq!(report.irreducible_rerun, report.irreducible_total, "{run}: the greedy tape");
    }
}

/// The smallest program of the completeness gap: the second `%v = call @id(%a)` overwrites 7 with 7
/// on branch 0 (Unchanged) and 8 with 9 on branch 1, and `%w` holds the old value on both branches,
/// so one common copy from `%w` recovers the position. Originally an instance with a cheap copy
/// witness carried no chain context, hiding that common copy from both chain modes. Copy-only
/// and depth-one still refuse the mixed witnesses; chains must recover the position.
const COPY_WITNESS_GAP: &str = "memory 1024
func @id(i8 %x) -> i8 {
entry:
    ret %x
}
func @f(i1 %b) -> i8 {
entry:
    %c = zext i1 %b to i8
    %a0 = add i8 %c, 7
    br loop(%a0, i8 2)
loop(i8 %a, i8 %n):
    %v = call @id(%a)
    %w = add i8 %v, 0
    %n.next = sub i8 %n, 1
    %more = icmp ult i8 0, %n.next
    %a.next = add i8 %a, %c
    br %more, loop(%a.next, %n.next), done(%v)
done(i8 %out):
    ret %out
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 5
    apply @f(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}
";

/// The witness itself, checked independently: the second call's old result is 7 / 8, `%w` holds it on
/// both branches. The independent search and both chain classifiers must agree on that recovery.
#[test]
fn a_common_live_copy_recovers_mixed_copy_witnesses_independently() {
    let module = load(COPY_WITNESS_GAP);
    let report = check(&module, "main", &[]);
    assert_sound("copy-witness gap", &report);
    assert_eq!((report.live_missed, report.tape_missed), (0, 0), "{report:#?}");
    assert_eq!(report.missed_with_copy_witness, [0, 0], "{report:#?}");
    assert_eq!(report.irreducible_rerun, report.irreducible_total, "{report:#?}");
}

/// What the stated chain rule implies for the witness: a common live leaf is a zero-instruction
/// recovery DAG, so the position should be Recomputed by `classify_with_chains`.
#[test]
fn chains_credit_a_common_live_copy_across_mixed_copy_witnesses() {
    let module = load(COPY_WITNESS_GAP);
    let main = module.function_named("main").expect("main");
    let mut executor = Executor::new(&module, main, &[], Options::default()).expect("starts");
    executor.measure_journal_content(true);
    executor.run();
    let m = executor.take_journal_measurement().expect("on");
    let record = m.applies[0].branches[0]
        .entries
        .iter()
        .filter(|r| r.words.iter().any(|w| w.role == WordRole::ReturnResult))
        .nth(1)
        .expect("the second call's return record");
    for tape in [false, true] {
        let classes = classify_with_chains(&m, tape);
        for (positions, path, k) in
            [(&classes[0].site_count, None, record.a_k), (&classes[0].call_path, Some(record.call_path), record.b_k)]
        {
            let second = positions
                .iter()
                .find(|p| p.role == WordRole::ReturnResult && p.site == record.site && p.call_path == path && p.k == k)
                .expect("the second call's result");
            assert_eq!(second.class, WordClass::Recomputed, "tape={tape}: {second:?}");
            assert_eq!(second.chain.as_ref().expect("common leaf").instructions, 0);
        }
    }
}

/// The global-write workloads of `front/tests/tests/journal_global.rs` with the published drivers,
/// over the initialized image `front/tests/tests/journal_global_check.rs` writes.
fn global_drivers(base: &str, work: &str) -> (ValidModule, ValidModule) {
    let h: String = (0..8).map(|i| format!("    h %q[{i}]\n")).collect();
    let world = format!(
        "{base}
func @world(i32 %reps) {{
entry:
    %q = qalloc 8
{h}    %out = qalloc 32
    %r = qalloc 1
    %m = mark
    %v = call @{work}(%q, %reps)
    rewind %m keep (%v -> %out)
    ret
}}"
    );
    let oracle = format!(
        "{base}
func @oracle(i32 %reps) {{
entry:
    %q = qalloc 8
{h}    %out = qalloc 32
    %r = qalloc 1
    %fault = qalloc 5
    apply @{work}(%q, %reps) -> %out, %fault budget 100000
    ret
}}"
    );
    (load(&world), load(&oracle))
}

/// One world's history of the call, just before its `rewind`: records from the mark on.
#[derive(Debug)]
struct WorldCall {
    kinds: Vec<Undo>,
    words: Vec<u128>,
    /// The journal's metadata entries from the mark on: register reads, cells.
    reads: usize,
    cells: usize,
    bytes: usize,
}

/// What the actual worlds driver did, measured from the executor's worlds, not from `Stats`.
#[derive(Debug, Default)]
struct WorldRun {
    /// Per input x (the world's `%q`), its call history.
    calls: HashMap<u128, WorldCall>,
    /// Journal bytes before any world split (the shared prefix, mark included).
    prefix_bytes: usize,
    /// The most journal bytes and overlay bytes summed over worlds after any tick, computed here:
    /// overlay as the bytes where a world's memory differs from the memory at the split.
    peak_journal: usize,
    peak_overlay: usize,
    stats: Option<crate::executor::Stats>,
}

fn run_worlds(module: &ValidModule, reps: u64) -> WorldRun {
    let main = module.function_named("world").expect("world");
    let options = Options { reversible: true, budget: Some(100_000_000), ..Options::default() };
    let mut e = Executor::new(module, main, &[reps], options).expect("starts");
    let mut run = WorldRun::default();
    let mut mark: Option<(usize, usize)> = None;
    let mut split_memory: Option<Vec<u8>> = None;
    loop {
        // Before each tick: a world whose next instruction is the rewind is captured.
        let next_is_rewind = |frames: &[Frame]| {
            frames.last().is_some_and(|f| {
                matches!(e.function(f.func).block(f.block).insts.get(f.inst), Some(Inst::Rewind { .. }))
            })
        };
        if let Some((records, words)) = mark {
            let states = e.world_states();
            let journals = e.journals();
            for (state, journal) in states.iter().zip(&journals) {
                if !next_is_rewind(state.frames) {
                    continue;
                }
                let journal = journal.expect("reversible");
                let column = journal.reads_from(records).first().expect("the call read %q").column;
                let configs: Vec<u64> = state.quantum.amplitudes().keys().map(|c| c[column]).collect();
                assert!(configs.windows(2).all(|w| w[0] == w[1]), "one x per world");
                let x = u128::from(configs[0]);
                let call = WorldCall {
                    kinds: (records..journal.records()).map(|i| journal.kind_at(i)).collect(),
                    words: journal.words()[words..].to_vec(),
                    reads: journal.reads_from(records).len(),
                    cells: journal.cells_from(records).len(),
                    bytes: journal.bytes(),
                };
                if let Some(old) = run.calls.insert(x, call) {
                    panic!("x={x} captured twice: {old:?}");
                }
            }
        }
        let was_single = !e.has_worlds();
        let done = e.step();
        if mark.is_none()
            && let Some(journal) = e.journal.as_ref()
            && journal.marks().len() == 1
        {
            mark = Some((journal.records(), journal.words().len()));
            run.prefix_bytes = journal.bytes();
        }
        if was_single && e.has_worlds() && split_memory.is_none() {
            // The split read happened in this tick, before any write of the call.
            split_memory = Some(e.world_states()[0].memory.to_vec());
        }
        let journal: usize = e.journals().into_iter().flatten().map(Journal::bytes).sum();
        run.peak_journal = run.peak_journal.max(journal);
        if let Some(base) = &split_memory
            && e.has_worlds()
        {
            let overlay: usize = e
                .world_states()
                .iter()
                .map(|w| w.memory.to_vec().iter().zip(base).filter(|(a, b)| a != b).count())
                .sum();
            run.peak_overlay = run.peak_overlay.max(overlay);
        }
        if let Some(outcome) = done {
            assert_eq!(outcome, Outcome::Returned(None));
            break;
        }
    }
    run.stats = Some(e.stats());
    run
}

/// The global-write measurement checked independently (`experiments/journal-content/README.md`,
/// "Global-memory writes on the worlds machine"). Set `BENNETT_JG_BN` to the image
/// (`target/front-tests/journal_global_base.bn` after `front/test.sh --test journal_global_check --
/// --ignored`) and `BENNETT_JG_RUNS` to `work:reps,...`. Per run: soundness of every credited word by
/// this module's replay; then the actual worlds driver's per-world call history against the `apply`
/// callee journal for the same input, and `Stats` against counts taken here from the worlds.
#[test]
#[ignore = "needs the lowered global workloads; minutes"]
fn c_globals_check_independently() {
    let base = std::fs::read_to_string(std::env::var("BENNETT_JG_BN").expect("BENNETT_JG_BN")).expect("image");
    let runs = std::env::var("BENNETT_JG_RUNS").unwrap_or_else(|_| "jg_log:1".to_owned());
    let soundness = std::env::var("BENNETT_JG_SOUNDNESS").map_or(true, |v| v != "0");
    for run in runs.split(',') {
        let (work, reps) = run.split_once(':').expect("work:reps");
        let reps: u64 = reps.parse().expect("reps");
        let (world, oracle) = global_drivers(&base, work);
        if soundness {
            let report = check(&oracle, "oracle", &[reps]);
            eprintln!(
                "jg-sound\t{run}\tA,B [live, tape, rerun live, rerun tape]={:?}\trecords={}\tcredited d1/live/tape={}/{}/{}\tunconfirmed={}/{}/{}/{}\troot mismatches={}\tmissed d1/live/tape={}/{}/{}\tmissed with copy witness={:?}",
                report.by_alignment,
                report.records,
                report.depth_one_credited,
                report.live_credited,
                report.tape_credited,
                report.depth_one_witness_wrong.len(),
                report.depth_one_unconfirmed.len(),
                report.live_unconfirmed.len(),
                report.tape_unconfirmed.len(),
                report.root_mismatches,
                report.depth_one_missed,
                report.live_missed,
                report.tape_missed,
                report.missed_with_copy_witness,
            );
            for example in report.missed_examples.iter().take(6) {
                eprintln!("jg-example\t{run}\t{example}");
            }
            assert_sound(run, &report);
        }
        // The proxy: the apply callee journal for each x, against the world's own call history.
        let main = oracle.function_named("oracle").expect("oracle");
        let options = Options { reversible: true, budget: Some(100_000_000), ..Options::default() };
        let mut e = Executor::new(&oracle, main, &[reps], options).expect("starts");
        e.measure_journal_content(true);
        assert_eq!(e.run(), Outcome::Returned(None));
        let measured = e.take_journal_measurement().expect("on");
        let apply = &measured.applies[0];
        // What the global writes destroyed: Store (old bytes) positions by class, per mode, alignment A.
        let modes = [("copy", crate::measure::classify(&measured)), ("tape", classify_with_chains(&measured, true))];
        for (mode, classes) in &modes {
            let mut by_class = [0usize; 4];
            let mut instances = [0usize; 4];
            for p in classes[0].site_count.iter().filter(|p| p.role == WordRole::Store) {
                let i = [WordClass::Same, WordClass::Recoverable, WordClass::Recomputed, WordClass::Irreducible]
                    .iter()
                    .position(|c| *c == p.class)
                    .expect("class");
                by_class[i] += 1;
                instances[i] += p.instances;
            }
            eprintln!(
                "jg-store\t{run}\t{mode}\tStore positions same/copy/recomputed/irreducible={by_class:?}\tinstances={instances:?}"
            );
        }
        let worlds = run_worlds(&world, reps);
        let stats = worlds.stats.expect("stats");
        assert_eq!(worlds.calls.len(), apply.branches.len(), "{run}: one captured world per branch");
        let mut differences: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        let (mut proxy_records, mut proxy_words, mut world_records, mut world_words) = (0, 0, 0, 0);
        let (mut equal_body, mut reads, mut cells) = (0, 0, 0);
        let mut world_bytes_sum = 0;
        let mut tails: Vec<Vec<u128>> = Vec::new();
        // Per position (both alignments): whether its instances differ between proxy and world, as
        // bits (1: equal, 2: differs). A position with both cannot keep its classification for free.
        let mut shifted_a: HashMap<(Site, u64, WordRole), u8> = HashMap::new();
        let mut shifted_b: HashMap<(crate::measure::PathId, Site, u64, WordRole), u8> = HashMap::new();
        for branch in &apply.branches {
            let call = &worlds.calls[&branch.args[0]];
            let kinds: Vec<Undo> = branch.entries.iter().map(|r| r.kind).collect();
            let words: Vec<u128> = branch.entries.iter().flat_map(|r| r.words.iter().map(|w| w.value)).collect();
            proxy_records += kinds.len();
            proxy_words += words.len();
            world_records += call.kinds.len();
            world_words += call.words.len();
            reads += call.reads;
            cells += call.cells;
            world_bytes_sum += call.bytes;
            // Record kinds: what differs, as (proxy kind, world kind) at each end.
            let first = (kinds.first().copied(), call.kinds.first().copied());
            let last = (kinds.last().copied(), call.kinds.last().copied());
            *differences
                .entry(format!("first {first:?} last {last:?} len {}", call.kinds.len() as i64 - kinds.len() as i64))
                .or_default() += 1;
            // Record by record, words split by the proxy's word count; the last record is compared
            // by kind only (Finish has no words, the world's Return has three or four).
            let mut at = 0;
            let mut same_branch = kinds.len() == call.kinds.len();
            for (r, record) in branch.entries.iter().enumerate() {
                if r + 1 == branch.entries.len() {
                    break;
                }
                if call.kinds.get(r) != Some(&record.kind) {
                    same_branch = false;
                    *differences.entry(format!("kind differs at record {r}")).or_default() += 1;
                    break;
                }
                for word in &record.words {
                    let theirs = call.words.get(at).copied();
                    let differs = theirs != Some(word.value);
                    let flag = 1u8 << u8::from(differs);
                    *shifted_a.entry((record.site, record.a_k, word.role)).or_default() |= flag;
                    *shifted_b.entry((record.call_path, record.site, record.b_k, word.role)).or_default() |= flag;
                    if theirs != Some(word.value) {
                        same_branch = false;
                        let offset = theirs.map(|t| word.value.wrapping_sub(t) as i128);
                        let key = format!("word differs: {:?}/{:?} by proxy-world {offset:?}", record.kind, word.role);
                        let entry = differences.entry(key).or_default();
                        *entry += 1;
                        if *entry == 1 {
                            eprintln!(
                                "jg-proxy-example\t{run}\tx={}\trecord {r} {:?} at {:?} path {}\tproxy {:#x} world {:?}",
                                branch.args[0], record.kind, record.site, record.call_path, word.value, theirs
                            );
                        }
                    }
                    at += 1;
                }
            }
            *differences
                .entry(format!("world words after the last proxy record: {}", call.words.len() - at))
                .or_default() += 1;
            tails.push(call.words[at..].to_vec());
            if same_branch {
                equal_body += 1;
            }
        }
        // The words the proxy does not have (the root Return's): how many distinct values per word.
        let width = tails.iter().map(Vec::len).max().unwrap_or(0);
        let distinct: Vec<usize> = (0..width)
            .map(|i| tails.iter().filter_map(|t| t.get(i)).collect::<std::collections::BTreeSet<_>>().len())
            .collect();
        let mixed = |flags: &mut dyn Iterator<Item = u8>| {
            let flags: Vec<u8> = flags.collect();
            (flags.iter().filter(|f| **f == 2).count(), flags.iter().filter(|f| **f == 3).count())
        };
        eprintln!(
            "jg-proxy-positions\t{run}\tpositions all-shifted/mixed A={:?} B={:?}",
            mixed(&mut shifted_a.values().copied()),
            mixed(&mut shifted_b.values().copied())
        );
        eprintln!("jg-proxy-tail\t{run}\tdistinct values of the world's root Return words={distinct:?}");
        eprintln!(
            "jg-proxy\t{run}\tbranches={}\tbody equal={}\tproxy records/words={proxy_records}/{proxy_words}\tworld records/words={world_records}/{world_words}\tworld reads/cells={reads}/{cells}\tdifferences={differences:?}",
            apply.branches.len(),
            equal_body
        );
        eprintln!(
            "jg-stats\t{run}\tStats steps/world_steps/rewound/rerun={}/{}/{}/{}\tworld records from mark summed={world_records}\tpeak journal here/sampled-sum-at-capture={}/{world_bytes_sum}\tprefix={}\tpeak overlay here/Stats={}/{}\tsplits/merges={}/{}\tcopied journal/overlay={}/{}\tend journal={:?}",
            stats.steps,
            stats.world_steps,
            stats.rewound_steps,
            stats.rerun_steps,
            worlds.peak_journal,
            worlds.prefix_bytes,
            worlds.peak_overlay,
            stats.peak_overlay_bytes,
            stats.splits,
            stats.merges,
            stats.split_journal_bytes,
            stats.split_memory_bytes,
            stats.journal_bytes,
        );
        // The measured relation of proxy and world, asserted: same records and kinds on every branch;
        // words equal except stack addresses shifted by one constant per word role, never mixed within a
        // position (so every Same/Irreducible relation, hence every class, is the world's too); the
        // world's extra root `Return` words take one value on all worlds; `Stats` counts what the worlds did.
        assert!(
            !differences.keys().any(|k| k.starts_with("kind differs") || k.contains("None")),
            "{run}: {differences:?}"
        );
        assert_eq!(proxy_records, world_records, "{run}");
        let roles =
            differences.keys().filter(|k| k.starts_with("word differs")).map(|k| k.split(" by ").next().expect("key"));
        let roles: Vec<&str> = roles.collect();
        assert!(roles.windows(2).all(|w| w[0] != w[1]), "{run}: one offset per role: {differences:?}");
        assert_eq!(mixed(&mut shifted_a.values().copied()).1 + mixed(&mut shifted_b.values().copied()).1, 0, "{run}");
        assert!(distinct.iter().all(|d| *d == 1), "{run}: {distinct:?}");
        assert_eq!((stats.rewound_steps, stats.rerun_steps), (world_records as u64, 0), "{run}");
        assert_eq!(worlds.peak_overlay, stats.peak_overlay_bytes, "{run}");
    }
}
