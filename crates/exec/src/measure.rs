//! The journal-content measurement: an opt-in observer of the journals of coherent regions.
//!
//! Off by default. [`Executor::measure_journal_content`] turns it on; nothing in this module ever writes
//! executor state (memory, slots, frames, tape, the quantum state, or the journal itself). While a
//! coherent region (`apply`) runs one branch, the observer copies each journal record the ordinary code
//! already pushed, tags each word with the witness that could make it recoverable, and forgets the copy
//! unless raw records were requested. The region then unwinds exactly as it would have without it.
//!
//! The classification ([`classify`]) is the experiment of `experiments/journal-content/README.md`: under
//! two alignments, is a journal word the same on every branch, recoverable from live state, or
//! irreducible? The definitions there are the ones implemented here.

use std::collections::HashMap;

use bennett_ir::{BlockId, FuncId, Inst, Operand, Slot, Terminator};

use crate::executor::{Executor, Frame, Outcome};
use crate::journal::Undo;

/// Where in a block a record was pushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SitePos {
    /// Entering the region callee: the `Call` record its frame push journals, before any step.
    Entry,
    /// Instruction `0` of the block: indexes count from 0.
    Inst(usize),
    /// The block's terminator.
    Term,
}

/// The static place that pushed a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Site {
    /// The function executing.
    pub func: FuncId,
    /// The block executing.
    pub block: BlockId,
    /// The instruction, terminator, or region entry.
    pub pos: SitePos,
}

/// One frame of a call path: the call that entered a frame, and which execution of that call site in
/// its caller's frame instance it was (from 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallPathStep {
    /// The call site, in the caller.
    pub site: Site,
    /// The index of this execution of the site within the caller frame instance.
    pub call_index: u64,
}

/// Which word of a record a journal word is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WordRole {
    /// The old slot value of a `Slot`, `Qalloc` or `Measure` record.
    Slot,
    /// The old bytes of a `Store` record.
    Store,
    /// The old slot value of an `Alloca` record.
    AllocaSlot,
    /// The old stack pointer of an `Alloca` record.
    AllocaStack,
    /// The old stack pointer of a `StackRestore` record.
    StackRestore,
    /// The old value of callee parameter `0` (by parameter position) in a `Call` record.
    CallParam(usize),
    /// The old value of target parameter `0` (by parameter position) in a `Jump` record.
    JumpParam(usize),
    /// The source block of a `Jump` record.
    JumpSource,
    /// The packed callee function and block of a `Return` record.
    ReturnPacked,
    /// The callee stack pointer of a `Return` record.
    ReturnStack,
    /// The old value of the caller's result slot in a `Return` record.
    ReturnResult,
}

/// What live thing holds a value, named in a [`WordWitness::LiveCopy`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceKind {
    /// A slot of a live frame.
    Slot,
    /// The stack pointer.
    StackPointer,
    /// A live frame's entry stack pointer.
    EntryStackPointer,
}

/// A call path of one `apply` execution, as an index into [`ApplyJournal::call_paths`]. Paths are
/// interned per `apply` execution, so two branches' records have equal ids exactly when their call
/// paths are equal. Path 0 is the empty path: the region's root frame.
pub type PathId = u32;

/// A live source that held a journal word's value just after the step that journalled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LiveSource {
    /// The call path of the frame holding it (0, the empty path, for the region root and the stack
    /// pointer).
    pub call_path: PathId,
    /// The function of that frame, when the source is in a frame.
    pub func: Option<FuncId>,
    /// The slot, when the source is a slot.
    pub slot: Option<Slot>,
    /// What kind of live thing it is.
    pub kind: SourceKind,
}

/// The witness recorded for one journal word instance: why it might not need keeping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WordWitness {
    /// No witness was found by the rules of this measurement.
    None,
    /// The first dynamic write to that slot in its frame activation: the old value is the executor's
    /// residue in a slot no code of this activation has defined, so there is no semantic value to keep.
    FirstWrite,
    /// The step wrote the value that was already there, so it destroyed nothing.
    Unchanged,
    /// A store to bytes no earlier store of this region branch wrote: the old bytes are the
    /// region-entry memory at the address, which is classical and still known.
    StoreBase,
    /// The value is still held by each of these live sources just after the step (sorted).
    LiveCopy(Vec<LiveSource>),
}

/// The output of a defining instruction that can be evaluated once, without replay or recursion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProducerOutput {
    /// A pure instruction result, or a store's value operand (the bytes it writes).
    Instruction,
    /// A block argument on the indexed target, in `Terminator::visit_targets` order.
    JumpArgument {
        /// The target index, distinguishing two edges to the same block.
        target: usize,
        /// The argument index within that target.
        argument: usize,
    },
}

/// One depth-one recovery circuit: re-evaluate this producer using live copies of its original
/// operands and current memory in the state just after the destructive step.
/// Equality to the old journal word is checked, not assumed. No operand's producer is followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecomputeWitness {
    /// The defining instruction or transfer.
    pub site: Site,
    /// Its frame's call path, shared across branches of this apply.
    pub call_path: PathId,
    /// Which output of that instruction defined the old value.
    pub output: ProducerOutput,
    /// For each slot operand, sorted live sources holding the operand's defining-time value.
    /// Constants remain constants of the instruction. A classifier intersects each list across branches.
    pub inputs: Vec<Vec<LiveSource>>,
}

impl RecomputeWitness {
    fn intersect(&mut self, other: &Self) -> bool {
        if (self.site, self.call_path, self.output) != (other.site, other.call_path, other.output)
            || self.inputs.len() != other.inputs.len()
        {
            return false;
        }
        for (common, sources) in self.inputs.iter_mut().zip(&other.inputs) {
            common.retain(|source| sources.binary_search(source).is_ok());
            if common.is_empty() {
                return false;
            }
        }
        true
    }
}

/// Observer-only provenance, never a source of constants in a recovery circuit. Historical operands
/// must each have a live copy after the destructive step before a witness is emitted.
#[derive(Clone)]
struct Producer {
    site: Site,
    call_path: PathId,
    output: ProducerOutput,
    inputs: Vec<(Slot, u128)>,
}

/// One journal word, as one branch journalled it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordInstance {
    /// Which word of its record it is.
    pub role: WordRole,
    /// The word's value.
    pub value: u128,
    /// Its witness, if any.
    pub witness: WordWitness,
    /// A checked depth-one producer, independently of the copy-only witness.
    pub recompute: Option<RecomputeWitness>,
}

/// One journal record, as one branch journalled it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordInstance {
    /// The place that pushed it.
    pub site: Site,
    /// Its kind.
    pub kind: Undo,
    /// Alignment A index: which execution of `site` this was in the whole region run (from 0).
    pub a_k: u64,
    /// Alignment B index: which execution of `site` this was in its frame instance (from 0).
    pub b_k: u64,
    /// The call path of the frame that executed it (0, the empty path, for the region root and region
    /// entry).
    pub call_path: PathId,
    /// Its words, in push order. Records with no words appear here with none.
    pub words: Vec<WordInstance>,
}

/// The journal of one branch of one `apply` execution.
#[derive(Clone, Debug, PartialEq)]
pub struct BranchJournal {
    /// The callee's argument values on this branch.
    pub args: Vec<u128>,
    /// How the callee run ended on this branch.
    pub outcome: Outcome,
    /// Steps the callee run took (the entry frame push is a record, not a step).
    pub steps: u64,
    /// Records in its journal, entry `Call` included.
    pub records: usize,
    /// Words in its journal.
    pub words: usize,
    /// Every record, when raw records were requested; empty otherwise.
    pub entries: Vec<RecordInstance>,
}

/// Every branch of one dynamic `apply` execution, in configuration order.
#[derive(Clone, Debug, PartialEq)]
pub struct ApplyJournal {
    /// The applied function.
    pub callee: FuncId,
    /// The call paths the branches' records name, by [`PathId`]; entry 0 is the empty path.
    pub call_paths: Vec<Vec<CallPathStep>>,
    /// One journal per basis configuration the callee ran on.
    pub branches: Vec<BranchJournal>,
}

/// Everything the measurement saw in a run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JournalMeasurement {
    /// Each dynamic `apply` execution, in order.
    pub applies: Vec<ApplyJournal>,
}

/// How branches are aligned before journal words are compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Alignment {
    /// A: (site, k), k counted over the whole region run.
    SiteCount,
    /// B: (call path, site, k), k counted within the frame instance.
    CallPath,
}

/// The class of an aligned journal-word position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WordClass {
    /// Present on every branch, with the same value.
    Same,
    /// Not the same on every branch, but every instance carries one common witness.
    Recoverable,
    /// A common depth-one producer recovers every instance (only with `classify_with_recompute`).
    Recomputed,
    /// No enabled witness covers the position.
    Irreducible,
}

/// Why a position counts as recoverable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecoverableReason {
    /// Every instance was a first write to its slot in its frame activation.
    FirstWrite,
    /// Every instance rewrote the value already there.
    Unchanged,
    /// Every instance was a first store to its bytes in the region branch.
    StoreBase,
    /// One live source held the value after the step on every branch that executed it.
    LiveCopy,
}

/// One aligned journal-word position of one `apply` execution, classified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassifiedPosition {
    /// The site of the record.
    pub site: Site,
    /// The record kind.
    pub kind: Undo,
    /// Which word of the record.
    pub role: WordRole,
    /// The alignment index k (A: region-wide; B: within the frame instance).
    pub k: u64,
    /// The call path, under alignment B (an index into [`ApplyJournal::call_paths`]); `None` under A.
    pub call_path: Option<PathId>,
    /// Branches (of `branches_total`) that executed this position.
    pub branches_present: usize,
    /// Branches the `apply` ran on.
    pub branches_total: usize,
    /// Word instances at this position (one per executing branch).
    pub instances: usize,
    /// Distinct values among them.
    pub distinct_values: usize,
    /// Whether every executing branch agrees on the value (regardless of presence elsewhere).
    pub same_where_present: bool,
    /// The position's class.
    pub class: WordClass,
    /// The witness behind `Recoverable`, when that is the class.
    pub reason: Option<RecoverableReason>,
    /// The common recovery circuit, only for the Recomputed class.
    pub recompute: Option<RecomputeWitness>,
}

/// The classification of one `apply` execution under both alignments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyClassification {
    /// The applied function.
    pub callee: FuncId,
    /// Branches the `apply` ran on.
    pub branches_total: usize,
    /// Positions under alignment A, in key order.
    pub site_count: Vec<ClassifiedPosition>,
    /// Positions under alignment B, in key order.
    pub call_path: Vec<ClassifiedPosition>,
}

/// Classifies every aligned word position of every measured `apply` execution.
///
/// A position is [`WordClass::Same`] when every branch executed it with one value. Otherwise it is
/// [`WordClass::Recoverable`] when one witness covers every instance: all first writes, all
/// unchanged, all first stores, or one common live source. Mixed witnesses do not count: a circuit
/// would need a different recovery per branch. Everything else is [`WordClass::Irreducible`] under
/// these rules, which deliberately exclude recomputing a value from its producer or replaying the
/// region from its preserved inputs, so the count is an upper bound for those stronger mechanisms.
pub fn classify(measurement: &JournalMeasurement) -> Vec<ApplyClassification> {
    classify_mode(measurement, false)
}

/// Like [`classify`], additionally crediting one common checked depth-one producer per position.
/// Same and copy recovery retain priority. This never follows an operand producer or replays a call.
pub fn classify_with_recompute(measurement: &JournalMeasurement) -> Vec<ApplyClassification> {
    classify_mode(measurement, true)
}

fn classify_mode(measurement: &JournalMeasurement, recompute: bool) -> Vec<ApplyClassification> {
    measurement
        .applies
        .iter()
        .map(|apply| ApplyClassification {
            callee: apply.callee,
            branches_total: apply.branches.len(),
            site_count: classify_alignment(apply, Alignment::SiteCount, recompute),
            call_path: classify_alignment(apply, Alignment::CallPath, recompute),
        })
        .collect()
}

/// The classification of one `apply` execution under one alignment.
fn classify_alignment(apply: &ApplyJournal, alignment: Alignment, recompute: bool) -> Vec<ClassifiedPosition> {
    type Key = (Option<PathId>, Site, u64, WordRole);
    struct Acc {
        kind: Undo,
        branches: usize,
        last_branch: usize,
        instances: usize,
        values: Vec<u128>,
        all_first_write: bool,
        all_unchanged: bool,
        all_store_base: bool,
        all_live_copy: bool,
        common_recompute: Option<RecomputeWitness>,
        all_recompute: bool,
        /// The sources common to every instance so far, sorted.
        live_intersection: Option<Vec<LiveSource>>,
    }
    let mut positions: HashMap<Key, Acc> = HashMap::new();
    for (branch, journal) in apply.branches.iter().enumerate() {
        for record in &journal.entries {
            for word in &record.words {
                let key: Key = match alignment {
                    Alignment::SiteCount => (None, record.site, record.a_k, word.role),
                    Alignment::CallPath => (Some(record.call_path), record.site, record.b_k, word.role),
                };
                let acc = positions.entry(key).or_insert_with(|| Acc {
                    kind: record.kind,
                    branches: 0,
                    last_branch: usize::MAX,
                    instances: 0,
                    values: Vec::new(),
                    all_first_write: true,
                    all_unchanged: true,
                    all_store_base: true,
                    all_live_copy: true,
                    common_recompute: word.recompute.clone(),
                    all_recompute: recompute && word.recompute.is_some(),
                    live_intersection: None,
                });
                if acc.last_branch != branch {
                    acc.branches += 1;
                    acc.last_branch = branch;
                }
                acc.all_recompute &= match (&mut acc.common_recompute, &word.recompute) {
                    (Some(common), Some(witness)) => common.intersect(witness),
                    _ => false,
                };
                acc.instances += 1;
                acc.values.push(word.value);
                let (first_write, unchanged, store_base) = match &word.witness {
                    WordWitness::FirstWrite => (true, false, false),
                    WordWitness::Unchanged => (false, true, false),
                    WordWitness::StoreBase => (false, false, true),
                    WordWitness::LiveCopy(sources) => {
                        acc.live_intersection = Some(match acc.live_intersection.take() {
                            Some(mut common) => {
                                common.retain(|source| sources.binary_search(source).is_ok());
                                common
                            }
                            None => sources.clone(),
                        });
                        (false, false, false)
                    }
                    WordWitness::None => (false, false, false),
                };
                acc.all_first_write &= first_write;
                acc.all_unchanged &= unchanged;
                acc.all_store_base &= store_base;
                acc.all_live_copy &= matches!(word.witness, WordWitness::LiveCopy(_));
            }
        }
    }
    let total = apply.branches.len();
    let mut positions: Vec<(Key, Acc)> = positions.into_iter().collect();
    positions.sort_unstable_by_key(|(key, _)| *key);
    positions
        .into_iter()
        .map(|((call_path, site, k, role), mut acc)| {
            acc.values.sort_unstable();
            acc.values.dedup();
            let same_where_present = acc.values.len() == 1;
            let reason = if acc.all_first_write {
                Some(RecoverableReason::FirstWrite)
            } else if acc.all_unchanged {
                Some(RecoverableReason::Unchanged)
            } else if acc.all_store_base {
                Some(RecoverableReason::StoreBase)
            } else if acc.all_live_copy && acc.live_intersection.as_ref().is_some_and(|common| !common.is_empty()) {
                Some(RecoverableReason::LiveCopy)
            } else {
                None
            };
            let class = if acc.branches == total && same_where_present {
                WordClass::Same
            } else if reason.is_some() {
                WordClass::Recoverable
            } else if acc.all_recompute {
                WordClass::Recomputed
            } else {
                WordClass::Irreducible
            };
            ClassifiedPosition {
                site,
                kind: acc.kind,
                role,
                k,
                call_path,
                branches_present: acc.branches,
                branches_total: total,
                instances: acc.instances,
                distinct_values: acc.values.len(),
                same_where_present,
                class,
                reason: (class == WordClass::Recoverable).then_some(reason).flatten(),
                recompute: (class == WordClass::Recomputed).then_some(acc.common_recompute).flatten(),
            }
        })
        .collect()
}

/// One frame of the observer's shadow of a region branch. It mirrors an executor frame so site
/// counts, call paths, and first writes can be named; it is observer state, never executor state.
struct ShadowFrame {
    call_path: PathId,
    site_counts: HashMap<Site, u64>,
    slot_writes: Vec<u32>,
    producers: Vec<Option<Producer>>,
}

/// The observer. Created by [`Executor::measure_journal_content`]; dropped or taken by the caller.
pub(crate) struct Collector {
    keep_raw: bool,
    measurement: JournalMeasurement,
    a_counts: HashMap<Site, u64>,
    shadow: Vec<ShadowFrame>,
    written: Vec<bool>,
    written_list: Vec<usize>,
    /// Exact, unfragmented ranges last written by a store; overlaps invalidate the old producer.
    stores: HashMap<(usize, usize), Producer>,
    entries: Vec<RecordInstance>,
    branch_args: Vec<u128>,
    captured_records: usize,
    captured_words: usize,
    /// The interning table of the current `apply` execution's call paths.
    path_ids: HashMap<Vec<CallPathStep>, PathId>,
    /// Scratch for the live sources of one step, reused so that a step allocates nothing for them.
    live: Vec<(LiveSource, u128)>,
}

impl Collector {
    fn new(keep_raw: bool) -> Self {
        Collector {
            keep_raw,
            measurement: JournalMeasurement::default(),
            a_counts: HashMap::new(),
            shadow: Vec::new(),
            written: Vec::new(),
            written_list: Vec::new(),
            stores: HashMap::new(),
            entries: Vec::new(),
            branch_args: Vec::new(),
            captured_records: 0,
            captured_words: 0,
            path_ids: HashMap::new(),
            live: Vec::new(),
        }
    }

    /// The id of the call path `parent` extended by `step`, interned in the current `apply`.
    fn intern(&mut self, parent: PathId, step: CallPathStep) -> PathId {
        let apply = self.measurement.applies.last_mut().expect("a region branch belongs to an apply measurement");
        let mut path = apply.call_paths[parent as usize].clone();
        path.push(step);
        let next = PathId::try_from(apply.call_paths.len()).expect("call paths fit in u32");
        *self.path_ids.entry(path).or_insert_with_key(|path| {
            apply.call_paths.push(path.clone());
            next
        })
    }

    fn next_a(&mut self, site: Site) -> u64 {
        let count = self.a_counts.entry(site).or_insert(0);
        let k = *count;
        *count += 1;
        k
    }

    /// Records a write to `slot` of shadow frame `frame`; returns whether it was the first in the
    /// activation.
    fn bump(&mut self, frame: usize, slot: Slot) -> bool {
        let writes = &mut self.shadow[frame].slot_writes;
        if slot.index() >= writes.len() {
            writes.resize(slot.index() + 1, 0);
        }
        let first = writes[slot.index()] == 0;
        writes[slot.index()] += 1;
        first
    }

    fn push_record(&mut self, record: RecordInstance) {
        self.captured_records += 1;
        self.captured_words += record.words.len();
        if self.keep_raw {
            self.entries.push(record);
        }
    }

    fn reset_branch(&mut self) {
        for &byte in &self.written_list {
            self.written[byte] = false;
        }
        self.written_list.clear();
        self.stores.clear();
        self.a_counts.clear();
        self.shadow.clear();
        self.entries.clear();
        self.branch_args.clear();
        self.captured_records = 0;
        self.captured_words = 0;
    }
}

/// Where a step about to run in a region sits, captured before it runs.
pub(crate) struct Pending {
    site: Site,
    frame: Frame,
    records_before: usize,
    words_before: usize,
    producers: Vec<Producer>,
}

/// Executor facts about a just-run step, gathered before the observer is borrowed again.
struct StepInfo {
    kind: Undo,
    words: Vec<u128>,
    dst: Option<Slot>,
    dst_new: Option<u128>,
    store_range: Option<std::ops::Range<usize>>,
    store_new: Option<u128>,
    callee_slot_count: usize,
    callee_param_slots: Vec<Slot>,
    jump_params: Vec<(Slot, u128)>,
    return_dst: Option<Slot>,
    return_dst_new: Option<u128>,
}

fn live_copy(live: &[(LiveSource, u128)], value: u128) -> WordWitness {
    let mut sources: Vec<LiveSource> =
        live.iter().filter(|(_, held)| *held == value).map(|(source, _)| *source).collect();
    // Sorted and unique: `classify_alignment` intersects these lists with a binary search.
    sources.sort_unstable();
    sources.dedup();
    if sources.is_empty() { WordWitness::None } else { WordWitness::LiveCopy(sources) }
}

fn slot_witness(first: bool, new: Option<u128>, value: u128, live: &[(LiveSource, u128)]) -> WordWitness {
    if first {
        WordWitness::FirstWrite
    } else if new == Some(value) {
        WordWitness::Unchanged
    } else {
        live_copy(live, value)
    }
}

impl Executor<'_> {
    /// Turns the journal-content measurement on for coherent regions run from now on. With
    /// `keep_raw`, every journal record of every branch is kept (what [`classify`] needs); without
    /// it, only per-branch counts, arguments and outcomes are. Off by default, and a pure observer
    /// when on: it copies journal words the run already made and writes no executor state, so a run
    /// with it on has the outcome, state and journal of the same run with it off.
    pub fn measure_journal_content(&mut self, keep_raw: bool) {
        self.journal_measurement = Some(Collector::new(keep_raw));
    }

    /// The measurement so far, if it is on.
    pub fn journal_measurement(&self) -> Option<&JournalMeasurement> {
        self.journal_measurement.as_ref().map(|collector| &collector.measurement)
    }

    /// Turns the measurement off and returns what it saw.
    pub fn take_journal_measurement(&mut self) -> Option<JournalMeasurement> {
        self.journal_measurement.take().map(|collector| collector.measurement)
    }

    pub(crate) fn measure_begin_apply(&mut self, callee: FuncId) {
        if let Some(collector) = &mut self.journal_measurement {
            collector.measurement.applies.push(ApplyJournal {
                callee,
                call_paths: vec![Vec::new()],
                branches: Vec::new(),
            });
            collector.path_ids.clear();
            collector.path_ids.insert(Vec::new(), 0);
        }
    }

    pub(crate) fn measure_begin_branch(&mut self, args: &[u128]) {
        let memory_len = self.memory.len();
        if let Some(collector) = &mut self.journal_measurement {
            collector.reset_branch();
            if collector.written.len() != memory_len {
                collector.written = vec![false; memory_len];
            }
            collector.branch_args = args.to_vec();
        }
    }

    /// Records the `Call` a region entry's frame push journalled, and opens the root shadow frame.
    pub(crate) fn measure_entry_call(&mut self, callee: FuncId) {
        if self.journal_measurement.is_none() {
            return;
        }
        let function = self.function(callee);
        let slot_count = function.slot_count();
        let param_slots: Vec<Slot> = function.params.iter().map(|param| param.slot).collect();
        let journal = self.journal.as_ref().expect("a region runs with a journal");
        let words = journal.words().to_vec();
        let site = Site { func: callee, block: BlockId(0), pos: SitePos::Entry };
        let collector = self.journal_measurement.as_mut().expect("checked above");
        let a_k = collector.next_a(site);
        let mut slot_writes = vec![0; slot_count];
        for slot in &param_slots {
            if slot.index() < slot_writes.len() {
                slot_writes[slot.index()] = 1;
            }
        }
        collector.shadow.push(ShadowFrame {
            call_path: 0,
            site_counts: HashMap::new(),
            producers: vec![None; slot_count],
            slot_writes,
        });
        let record = RecordInstance {
            site,
            kind: Undo::Call,
            a_k,
            b_k: a_k,
            call_path: 0,
            words: words
                .into_iter()
                .enumerate()
                .map(|(index, value)| WordInstance {
                    role: WordRole::CallParam(index),
                    value,
                    witness: WordWitness::FirstWrite,
                    recompute: None,
                })
                .collect(),
        };
        collector.push_record(record);
    }

    pub(crate) fn measure_end_branch(
        &mut self,
        outcome: Outcome,
        steps: u64,
        journal_records: usize,
        journal_words: usize,
    ) {
        let Some(collector) = &mut self.journal_measurement else { return };
        assert_eq!(
            (collector.captured_records, collector.captured_words),
            (journal_records, journal_words),
            "the observer copies the whole region journal"
        );
        let branch = BranchJournal {
            args: std::mem::take(&mut collector.branch_args),
            outcome,
            steps,
            records: journal_records,
            words: journal_words,
            entries: std::mem::take(&mut collector.entries),
        };
        collector.reset_branch();
        collector
            .measurement
            .applies
            .last_mut()
            .expect("a region branch belongs to an apply measurement")
            .branches
            .push(branch);
    }

    pub(crate) fn measure_before_step(&self) -> Option<Pending> {
        if self.journal_measurement.is_none() || !self.in_region {
            return None;
        }
        let frame = *self.top();
        let block = self.function(frame.func).block(frame.block);
        let pos = if frame.inst < block.insts.len() { SitePos::Inst(frame.inst) } else { SitePos::Term };
        let journal = self.journal.as_ref().expect("a region runs with a journal");
        let site = Site { func: frame.func, block: frame.block, pos };
        let call_path = self.journal_measurement.as_ref()?.shadow.last()?.call_path;
        let mut outputs = Vec::new();
        if pos == SitePos::Term {
            let target = match &block.term {
                Terminator::Jump(_) => Some(0),
                Terminator::Branch { cond, .. } => Some(usize::from(self.read(cond) != 1)),
                Terminator::Switch { value, cases, .. } => {
                    Some(cases.iter().position(|case| case.value == self.read(value)).unwrap_or(cases.len()))
                }
                _ => None,
            };
            let mut index = 0;
            block.term.visit_targets(|edge| {
                if Some(index) == target {
                    outputs.extend(
                        (0..edge.args.len()).map(|argument| ProducerOutput::JumpArgument { target: index, argument }),
                    );
                }
                index += 1;
            });
        } else {
            outputs.push(ProducerOutput::Instruction);
        }
        let producers = outputs
            .into_iter()
            .filter_map(|output| {
                let operands = self.producer_operands(site, output)?;
                let inputs = operands
                    .into_iter()
                    .filter_map(|operand| match operand {
                        Operand::Slot(slot) => Some((slot, self.slot(slot))),
                        Operand::Const(_) => None,
                    })
                    .collect();
                Some(Producer { site, call_path, output, inputs })
            })
            .collect();
        Some(Pending { site, frame, records_before: journal.records(), words_before: journal.words().len(), producers })
    }

    /// Copies the one record the step just pushed, with witnesses from the state after the step.
    pub(crate) fn measure_after_step(&mut self, pending: Pending) {
        let Some(mut collector) = self.journal_measurement.take() else { return };
        let info = self.step_info(&pending, &mut collector);
        self.measure_record(pending, info, &mut collector);
        self.journal_measurement = Some(collector);
    }

    /// Classifies the words of the record a step pushed and updates the shadow frames.
    fn measure_record(&self, pending: Pending, info: StepInfo, collector: &mut Collector) {
        let executing = collector.shadow.len() - 1;
        let a_k = collector.next_a(pending.site);
        let b_k = {
            let count = collector.shadow[executing].site_counts.entry(pending.site).or_insert(0);
            let k = *count;
            *count += 1;
            k
        };
        let call_path = collector.shadow[executing].call_path;
        let live_scratch = std::mem::take(&mut collector.live);
        let live = &live_scratch[..];
        let mut words: Vec<WordInstance> = match info.kind {
            Undo::Slot | Undo::Qalloc | Undo::Measure => {
                let dst = info.dst.expect("a slot record defines a result");
                let first = collector.bump(executing, dst);
                vec![WordInstance {
                    role: WordRole::Slot,
                    value: info.words[0],
                    witness: slot_witness(first, info.dst_new, info.words[0], live),
                    recompute: None,
                }]
            }
            Undo::Store => {
                let range = info.store_range.clone().expect("a store record has a range");
                let first_store = !range.clone().any(|byte| collector.written[byte]);
                let witness = if first_store {
                    WordWitness::StoreBase
                } else if info.store_new == Some(info.words[0]) {
                    WordWitness::Unchanged
                } else {
                    live_copy(live, info.words[0])
                };
                for byte in range {
                    if !collector.written[byte] {
                        collector.written[byte] = true;
                        collector.written_list.push(byte);
                    }
                }
                vec![WordInstance { role: WordRole::Store, value: info.words[0], witness, recompute: None }]
            }
            Undo::Alloca => {
                let dst = info.dst.expect("an alloca defines its address");
                let first = collector.bump(executing, dst);
                vec![
                    WordInstance {
                        role: WordRole::AllocaSlot,
                        value: info.words[0],
                        witness: slot_witness(first, info.dst_new, info.words[0], live),
                        recompute: None,
                    },
                    WordInstance {
                        role: WordRole::AllocaStack,
                        value: info.words[1],
                        witness: live_copy(live, info.words[1]),
                        recompute: None,
                    },
                ]
            }
            Undo::StackRestore => vec![WordInstance {
                role: WordRole::StackRestore,
                value: info.words[0],
                witness: live_copy(live, info.words[0]),
                recompute: None,
            }],
            Undo::Call => info
                .words
                .iter()
                .enumerate()
                .map(|(index, &value)| WordInstance {
                    role: WordRole::CallParam(index),
                    value,
                    witness: WordWitness::FirstWrite,
                    recompute: None,
                })
                .collect(),
            Undo::Jump => {
                let mut words = Vec::with_capacity(info.words.len());
                for (index, &(slot, new)) in info.jump_params.iter().enumerate() {
                    let first = collector.bump(executing, slot);
                    words.push(WordInstance {
                        role: WordRole::JumpParam(index),
                        value: info.words[index],
                        witness: slot_witness(first, Some(new), info.words[index], live),
                        recompute: None,
                    });
                }
                // The predecessor block is control history: a slot that happens to hold the same
                // small integer is not a copy of it, so no live-copy witness is attempted.
                words.push(WordInstance {
                    role: WordRole::JumpSource,
                    value: *info.words.last().expect("a jump record ends with its source block"),
                    witness: WordWitness::None,
                    recompute: None,
                });
                words
            }
            Undo::Return => {
                let mut words = vec![
                    WordInstance {
                        role: WordRole::ReturnPacked,
                        value: info.words[0],
                        witness: WordWitness::None,
                        recompute: None,
                    },
                    WordInstance {
                        role: WordRole::ReturnStack,
                        value: info.words[1],
                        witness: live_copy(live, info.words[1]),
                        recompute: None,
                    },
                ];
                if info.words.len() > 2 {
                    let dst = info.return_dst.expect("a return with a third word wrote a result slot");
                    let first = collector.bump(executing - 1, dst);
                    words.push(WordInstance {
                        role: WordRole::ReturnResult,
                        value: info.words[2],
                        witness: slot_witness(first, info.return_dst_new, info.words[2], live),
                        recompute: None,
                    });
                }
                words
            }
            Undo::Effect | Undo::Finish | Undo::Gate | Undo::Qfree | Undo::Apply => Vec::new(),
        };
        // Check original operand values against live state; captured values alone never suffice.
        for word in &mut words {
            let producer = match word.role {
                WordRole::Slot | WordRole::AllocaSlot => {
                    info.dst.and_then(|dst| collector.shadow[executing].producers[dst.index()].as_ref())
                }
                WordRole::JumpParam(index) => {
                    collector.shadow[executing].producers[info.jump_params[index].0.index()].as_ref()
                }
                WordRole::ReturnResult => {
                    info.return_dst.and_then(|dst| collector.shadow[executing - 1].producers[dst.index()].as_ref())
                }
                WordRole::Store => info.store_range.as_ref().and_then(|r| collector.stores.get(&(r.start, r.end))),
                _ => None,
            };
            word.recompute = producer.and_then(|p| self.recompute_witness(p, word.value, live));
        }
        match info.kind {
            Undo::Slot | Undo::Alloca => {
                let dst = info.dst.expect("a result slot");
                collector.shadow[executing].producers[dst.index()] = pending.producers.first().cloned();
            }
            Undo::Jump => {
                for (argument, &(slot, _)) in info.jump_params.iter().enumerate() {
                    collector.shadow[executing].producers[slot.index()] = pending.producers.get(argument).cloned();
                }
            }
            Undo::Return => {
                if let Some(dst) = info.return_dst {
                    // Replaying a callee is not depth-one recomputation.
                    collector.shadow[executing - 1].producers[dst.index()] = None;
                }
            }
            Undo::Store => {
                let range = info.store_range.as_ref().expect("store range");
                collector.stores.retain(|&(start, end), _| end <= range.start || start >= range.end);
                if let Some(producer) = pending.producers.first() {
                    collector.stores.insert((range.start, range.end), producer.clone());
                }
            }
            _ => {}
        }
        assert_eq!(words.len(), info.words.len(), "every journal word gets a role");
        collector.live = live_scratch;
        collector.push_record(RecordInstance { site: pending.site, kind: info.kind, a_k, b_k, call_path, words });
        match info.kind {
            Undo::Call => {
                let callee_path = collector.intern(call_path, CallPathStep { site: pending.site, call_index: b_k });
                let mut slot_writes = vec![0; info.callee_slot_count];
                for slot in &info.callee_param_slots {
                    if slot.index() < slot_writes.len() {
                        slot_writes[slot.index()] = 1;
                    }
                }
                collector.shadow.push(ShadowFrame {
                    call_path: callee_path,
                    site_counts: HashMap::new(),
                    producers: vec![None; info.callee_slot_count],
                    slot_writes,
                });
            }
            Undo::Return => {
                collector.shadow.pop().expect("a return pops a shadow frame");
            }
            _ => {}
        }
    }

    /// The semantic operands of the supported producer, in fixed order. Stores and transfers
    /// produce their value operands, not an arithmetic chain that led to those operands.
    fn producer_operands(&self, site: Site, output: ProducerOutput) -> Option<Vec<Operand>> {
        let block = self.function(site.func).block(site.block);
        match (site.pos, output) {
            (SitePos::Inst(index), ProducerOutput::Instruction) => Some(match &block.insts[index] {
                Inst::Binary { lhs, rhs, .. } | Inst::Compare { lhs, rhs, .. } => vec![*lhs, *rhs],
                Inst::Select { cond, if_true, if_false, .. } => vec![*cond, *if_true, *if_false],
                Inst::Cast { value, .. } | Inst::Store { value, .. } => vec![*value],
                Inst::Load { address, .. } => vec![*address],
                Inst::FuncAddr { .. } | Inst::StackSave { .. } => vec![],
                _ => return None,
            }),
            (SitePos::Term, ProducerOutput::JumpArgument { target, argument }) => {
                let mut operand = None;
                let mut index = 0;
                block.term.visit_targets(|edge| {
                    if index == target {
                        operand = edge.args.get(argument).copied();
                    }
                    index += 1;
                });
                operand.map(|operand| vec![operand])
            }
            _ => None,
        }
    }

    fn recompute_witness(
        &self,
        producer: &Producer,
        old: u128,
        live: &[(LiveSource, u128)],
    ) -> Option<RecomputeWitness> {
        let mut inputs = Vec::with_capacity(producer.inputs.len());
        for &(_, value) in &producer.inputs {
            let WordWitness::LiveCopy(sources) = live_copy(live, value) else { return None };
            inputs.push(sources);
        }
        // Evaluate from actual live sources, not from historical snapshots in the observer.
        let read = |operand: &Operand| match operand {
            Operand::Const(value) => value.value(),
            Operand::Slot(slot) => {
                let index = producer.inputs.iter().position(|(input, _)| input == slot).expect("captured operand");
                live.iter().find(|(source, _)| *source == inputs[index][0]).expect("live source").1
            }
        };
        let block = self.function(producer.site.func).block(producer.site.block);
        let value = match (producer.site.pos, producer.output) {
            (SitePos::Inst(index), ProducerOutput::Instruction) => match &block.insts[index] {
                Inst::Binary { op, width, lhs, rhs, .. } => {
                    crate::arith::binary(*op, *width, read(lhs), read(rhs)).ok()
                }
                Inst::Compare { pred, width, lhs, rhs, .. } => {
                    Some(u128::from(crate::arith::compare(*pred, *width, read(lhs), read(rhs))))
                }
                Inst::Select { cond, if_true, if_false, .. } => {
                    Some(read(if read(cond) == 1 { if_true } else { if_false }))
                }
                Inst::Cast { op, from, to, value, .. } => Some(crate::arith::cast(*op, *from, *to, read(value))),
                Inst::Load { width, address, .. } => self
                    .memory_range(read(address) as u64, *width)
                    .ok()
                    .map(|range| self.read_bytes(range) & width.mask()),
                Inst::FuncAddr { func, .. } => Some(u128::from(func.address())),
                Inst::StackSave { .. } => Some(u128::from(self.stack_pointer)),
                Inst::Store { value, .. } => Some(read(value)),
                _ => None,
            },
            (SitePos::Term, ProducerOutput::JumpArgument { .. }) => {
                self.producer_operands(producer.site, producer.output).map(|operands| read(&operands[0]))
            }
            _ => None,
        };
        (value == Some(old)).then_some(RecomputeWitness {
            site: producer.site,
            call_path: producer.call_path,
            output: producer.output,
            inputs,
        })
    }

    fn step_info(&self, pending: &Pending, collector: &mut Collector) -> StepInfo {
        let journal = self.journal.as_ref().expect("a region runs with a journal");
        assert_eq!(journal.records(), pending.records_before + 1, "a step journals one record");
        let kind = journal.kind_at(pending.records_before);
        let words = journal.words()[pending.words_before..].to_vec();
        let site = pending.site;
        let inst = match site.pos {
            SitePos::Inst(index) => Some(&self.function(site.func).block(site.block).insts[index]),
            SitePos::Entry | SitePos::Term => None,
        };
        let dst = inst.and_then(Inst::result);
        let dst_new = dst.map(|slot| self.slots[pending.frame.base + slot.index()]);
        let (store_range, store_new) = match inst {
            Some(Inst::Store { width, address, .. }) => {
                let range = self.memory_range(self.read_word(address), *width).expect("a store that ran is in bounds");
                let new = self.read_bytes(range.clone());
                (Some(range), Some(new))
            }
            _ => (None, None),
        };
        let (callee_slot_count, callee_param_slots) = if kind == Undo::Call {
            let function = self.function(self.top().func);
            (function.slot_count(), function.params.iter().map(|param| param.slot).collect())
        } else {
            (0, Vec::new())
        };
        let jump_params = if kind == Undo::Jump {
            let frame = *self.top();
            self.function(frame.func)
                .block(frame.block)
                .params
                .iter()
                .map(|param| (param.slot, self.slots[frame.base + param.slot.index()]))
                .collect()
        } else {
            Vec::new()
        };
        let (return_dst, return_dst_new) = if kind == Undo::Return && words.len() > 2 {
            let caller = *self.top();
            let call = &self.function(caller.func).block(caller.block).insts[caller.inst - 1];
            let dst = call.result();
            (dst, dst.map(|slot| self.slots[caller.base + slot.index()]))
        } else {
            (None, None)
        };
        let mut live = std::mem::take(&mut collector.live);
        self.live_sources(kind, &collector.shadow, &mut live);
        collector.live = live;
        StepInfo {
            kind,
            words,
            dst,
            dst_new,
            store_range,
            store_new,
            callee_slot_count,
            callee_param_slots,
            jump_params,
            return_dst,
            return_dst_new,
        }
    }

    /// Every live slot of the region's frames, their entry stack pointers, and the stack pointer,
    /// with values, named by call path so instances can be compared across branches; into `live`,
    /// which is cleared first. For a `Call`, the new frame is already pushed but has no shadow yet
    /// (its words are all first writes, which need no scan); for a `Return`, the callee frame is
    /// already popped and is left out.
    fn live_sources(&self, kind: Undo, shadow: &[ShadowFrame], live: &mut Vec<(LiveSource, u128)>) {
        live.clear();
        let paired = match kind {
            Undo::Return => shadow.len() - 1,
            _ => shadow.len(),
        };
        let start = self.frames.len() - paired - usize::from(kind == Undo::Call);
        for (frame, shadow) in self.frames[start..start + paired].iter().zip(shadow) {
            let path = shadow.call_path;
            for slot in 0..self.function(frame.func).slot_count() {
                if let Some(&value) = self.slots.get(frame.base + slot) {
                    let slot = Slot(u32::try_from(slot).expect("a slot index fits in u32"));
                    let source = LiveSource {
                        call_path: path,
                        func: Some(frame.func),
                        slot: Some(slot),
                        kind: SourceKind::Slot,
                    };
                    live.push((source, value));
                }
            }
            let source =
                LiveSource { call_path: path, func: Some(frame.func), slot: None, kind: SourceKind::EntryStackPointer };
            live.push((source, u128::from(frame.entry_stack_pointer)));
        }
        let source = LiveSource { call_path: 0, func: None, slot: None, kind: SourceKind::StackPointer };
        live.push((source, u128::from(self.stack_pointer)));
    }
}
