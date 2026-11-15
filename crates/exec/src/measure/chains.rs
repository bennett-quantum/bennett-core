//! Unbounded, local producer DAG recovery. Stored history is observer metadata, never free input.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use bennett_ir::{FuncId, Inst, Operand, Slot};

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChainContext {
    pub node: Option<usize>,
    pub live: Arc<Vec<(LiveSource, u128)>>,
    pub record: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MemoryWrite {
    pub record: usize,
    pub range: Range<usize>,
    pub value: u128,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct HistoryNode {
    value: u128,
    producer: Option<(Site, PathId, ProducerOutput)>,
    parents: Vec<usize>,
    operands: Vec<Operand>,
    instruction: Option<Inst>,
    load: Option<(usize, usize)>,
    record: usize,
    missing: Option<(FuncId, Slot)>,
}

impl HistoryNode {
    pub fn missing(value: u128, func: FuncId, slot: Slot) -> Self {
        Self {
            value,
            producer: None,
            parents: vec![],
            operands: vec![],
            instruction: None,
            load: None,
            record: 0,
            missing: Some((func, slot)),
        }
    }

    pub fn produced(
        value: u128,
        producer: &Producer,
        parents: Vec<usize>,
        operands: Vec<Operand>,
        load: Option<(usize, usize)>,
        record: usize,
    ) -> Self {
        Self {
            value,
            producer: Some((producer.site, producer.call_path, producer.output)),
            parents,
            operands,
            instruction: None,
            load,
            record,
            missing: None,
        }
    }

    pub fn set_instruction(&mut self, inst: Inst) {
        self.instruction = Some(inst);
    }

    fn verify(&self, branch: &BranchJournal, context: &ChainContext) -> bool {
        let mut index = 0;
        let values: Vec<_> = self
            .operands
            .iter()
            .map(|operand| match operand {
                Operand::Const(v) => v.value(),
                Operand::Slot(_) => {
                    let value = branch.history[self.parents[index]].value;
                    index += 1;
                    value
                }
            })
            .collect();
        let result = match &self.instruction {
            Some(Inst::Binary { op, width, .. }) => crate::arith::binary(*op, *width, values[0], values[1]).ok(),
            Some(Inst::Compare { pred, width, .. }) => {
                Some(u128::from(crate::arith::compare(*pred, *width, values[0], values[1])))
            }
            Some(Inst::Cast { op, from, to, .. }) => Some(crate::arith::cast(*op, *from, *to, values[0])),
            Some(Inst::Select { .. }) => Some(if values[0] == 1 { values[1] } else { values[2] }),
            Some(Inst::FuncAddr { func, .. }) => Some(u128::from(func.address())),
            Some(Inst::StackSave { .. }) => {
                context.live.iter().find(|(s, _)| s.kind == SourceKind::StackPointer).map(|(_, v)| *v)
            }
            Some(Inst::Load { width, .. }) => {
                let (start, bytes) = self.load.expect("load range");
                let mut value = self.value;
                // Reconstruct the CURRENT bytes for verification only. They are read by the recipe,
                // not supplied as historical constants. Writes before the defining load are irrelevant.
                for write in &branch.memory_writes {
                    if write.record > self.record && write.record <= context.record {
                        for byte in start.max(write.range.start)..(start + bytes).min(write.range.end) {
                            let shift = (byte - start) * 8;
                            value = (value & !(255u128 << shift))
                                | (((write.value >> ((byte - write.range.start) * 8)) & 255) << shift);
                        }
                    }
                }
                Some(value & width.mask())
            }
            Some(Inst::Store { .. }) | None => values.first().copied(),
            _ => None,
        };
        result == Some(self.value)
    }
}

/// Cost of the checked common DAG, or measured reasons recovery failed. A fresh DAG is evaluated
/// for each recovered word; common subexpressions within that word are evaluated once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChainRecovery {
    /// Re-evaluated device instructions per executing branch (zero for a leaf copy).
    pub instructions: u64,
    /// Longest producer path; there is no depth limit.
    pub depth: u64,
    /// Whether a common circuit was found, even for a position already credited by older classes.
    pub recovered: bool,
    /// Whether each branch separately has a recipe (does not certify a common circuit).
    pub individually_recovered: bool,
    /// Terminal obstacles measured in the DAG. Names include missing function/slot origins.
    pub blockers: Vec<String>,
}

type Key = (Option<PathId>, Site, u64, WordRole);
fn key(record: &RecordInstance, role: WordRole, alignment: Alignment) -> Key {
    match alignment {
        Alignment::SiteCount => (None, record.site, record.a_k, role),
        Alignment::CallPath => (Some(record.call_path), record.site, record.b_k, role),
    }
}
struct Instance<'a> {
    branch: &'a BranchJournal,
    word: &'a WordInstance,
    record: usize,
}
struct Seed<'a> {
    instances: BTreeMap<usize, (usize, &'a WordInstance)>,
}

/// Adds unlimited producer-chain recovery after the existing copy and depth-one classes.
/// With `journal_inputs`, already counted Same/retained-Irreducible positions can be leaves, but
/// only if their actual record precedes this word on EVERY executing branch. Positions are visited
/// by (maximum dynamic record index, alignment key); omitted words are never free journal inputs.
/// This gives an acyclic, greedy local tape, not an optimal pebbling schedule.
pub fn classify_with_chains(measurement: &JournalMeasurement, journal_inputs: bool) -> Vec<ApplyClassification> {
    let mut classes = classify_with_recompute(measurement);
    for (apply, class) in measurement.applies.iter().zip(&mut classes) {
        for (alignment, positions) in
            [(Alignment::SiteCount, &mut class.site_count), (Alignment::CallPath, &mut class.call_path)]
        {
            let mut instances: BTreeMap<Key, BTreeMap<usize, Instance<'_>>> = BTreeMap::new();
            for (b, branch) in apply.branches.iter().enumerate() {
                for (r, record) in branch.entries.iter().enumerate() {
                    for word in &record.words {
                        instances
                            .entry(key(record, word.role, alignment))
                            .or_default()
                            .insert(b, Instance { branch, word, record: r });
                    }
                }
            }
            let mut order: Vec<_> = (0..positions.len()).collect();
            order.sort_unstable_by_key(|&i| {
                let p = &positions[i];
                let k = (p.call_path, p.site, p.k, p.role);
                (instances[&k].values().map(|i| i.record).max().unwrap_or(0), k)
            });
            let mut seeds = Vec::new();
            for i in order {
                let p = &mut positions[i];
                let k = (p.call_path, p.site, p.k, p.role);
                let refs: Vec<_> = instances[&k].iter().map(|(&b, i)| (b, i)).collect();
                if matches!(p.class, WordClass::Irreducible | WordClass::Recomputed) {
                    let mut recovery = solve(&refs, if journal_inputs { &seeds } else { &[] });
                    if !recovery.recovered {
                        let individual: Vec<_> =
                            refs.iter().map(|r| solve(&[*r], if journal_inputs { &seeds } else { &[] })).collect();
                        recovery.individually_recovered = individual.iter().all(|r| r.recovered);
                        let failures: BTreeSet<_> =
                            individual.iter().flat_map(|r| r.blockers.iter().map(|s| format!("branch: {s}"))).collect();
                        recovery.blockers.extend(failures);
                    }
                    // Preserve the previous witness even if a different common DAG was not found.
                    if p.class == WordClass::Recomputed && !recovery.recovered {
                        recovery.instructions = 1;
                        recovery.depth = 1;
                        recovery.recovered = true;
                        recovery.individually_recovered = true;
                        recovery.blockers.clear();
                    }
                    if recovery.recovered {
                        p.class = WordClass::Recomputed;
                    }
                    p.chain = Some(recovery);
                }
                if journal_inputs && matches!(p.class, WordClass::Same | WordClass::Irreducible) {
                    seeds.push(Seed {
                        instances: instances[&k].iter().map(|(&b, i)| (b, (i.record, i.word))).collect(),
                    });
                }
            }
        }
    }
    classes
}

#[derive(Clone)]
struct Proof {
    children: Vec<Vec<Option<usize>>>,
    evaluated: bool,
    depth: u64,
    success: bool,
}

fn solve(refs: &[(usize, &Instance<'_>)], seeds: &[Seed<'_>]) -> ChainRecovery {
    // These words encode reachability, not ordinary scalar data. A numeric coincidence
    // in a slot/tape word is not a witness for the missing branch-presence predicate.
    if matches!(refs[0].1.word.role, WordRole::JumpSource | WordRole::ReturnPacked) {
        return ChainRecovery {
            blockers: vec!["control presence needs replay".to_owned()],
            ..ChainRecovery::default()
        };
    }
    if refs.len() == 1 && !matches!(refs[0].1.word.witness, WordWitness::None) {
        return ChainRecovery { recovered: true, individually_recovered: true, ..ChainRecovery::default() };
    }
    let root: Vec<_> = refs.iter().map(|(_, i)| i.word.chain.as_ref().and_then(|c| c.node)).collect();
    let mut pending = vec![(root.clone(), false)];
    let mut proofs: HashMap<Vec<Option<usize>>, Proof> = HashMap::new();
    let mut blockers = BTreeSet::new();
    while let Some((ids, finishing)) = pending.pop() {
        if proofs.contains_key(&ids) {
            continue;
        }
        let values: Vec<_> =
            ids.iter().zip(refs).map(|(id, (_, i))| id.map_or(i.word.value, |id| i.branch.history[id].value)).collect();
        let live = refs
            .iter()
            .enumerate()
            .all(|(j, (_, i))| i.word.chain.as_ref().is_some_and(|c| c.live.iter().any(|(_, v)| *v == values[j])));
        let common_live = live
            && refs[0].1.word.chain.as_ref().is_some_and(|c| {
                c.live.iter().any(|(s, v)| {
                    *v == values[0]
                        && refs.iter().enumerate().skip(1).all(|(j, (_, i))| {
                            i.word
                                .chain
                                .as_ref()
                                .is_some_and(|c| c.live.iter().any(|(other, v)| other == s && *v == values[j]))
                        })
                })
            });
        let journal = seeds.iter().any(|seed| {
            // Reading one aligned, already counted word is the same circuit on every branch.
            refs.iter()
                .enumerate()
                .all(|(j, (b, i))| seed.instances.get(b).is_some_and(|(r, w)| *r < i.record && w.value == values[j]))
        });
        if common_live || journal {
            proofs.insert(ids, Proof { children: vec![], evaluated: false, depth: 0, success: true });
            continue;
        }
        let nodes: Vec<_> = ids.iter().zip(refs).map(|(id, (_, i))| id.map(|id| &i.branch.history[id])).collect();
        let label = nodes[0].and_then(|n| n.producer);
        let valid = label.is_some() && nodes.iter().all(|n| n.is_some_and(|n| n.producer == label));
        if !valid {
            for node in &nodes {
                blockers.insert(match node {
                    Some(n) if n.missing.is_some() => {
                        let (f, s) = n.missing.expect("missing origin");
                        format!("missing-origin f{} slot{}", f.0, s.0)
                    }
                    None => "no-producer for journal role".to_owned(),
                    _ => "different-producer across branches".to_owned(),
                });
            }
            proofs.insert(ids, Proof { children: vec![], evaluated: false, depth: 0, success: false });
            continue;
        }
        if nodes.iter().zip(refs).any(|(n, (_, i))| {
            !n.expect("common producer").verify(i.branch, i.word.chain.as_ref().expect("node context"))
        }) {
            blockers.insert("current-memory-or-stack differs".to_owned());
            proofs.insert(ids, Proof { children: vec![], evaluated: false, depth: 0, success: false });
            continue;
        }
        let children: Vec<Vec<Option<usize>>> = (0..nodes[0].expect("common producer").parents.len())
            .map(|index| nodes.iter().map(|n| Some(n.expect("common producer").parents[index])).collect())
            .collect();
        if !finishing {
            pending.push((ids, true));
            for child in children.iter().rev() {
                if !proofs.contains_key(child) {
                    pending.push((child.clone(), false));
                }
            }
        } else {
            let success = children.iter().all(|c| proofs[c].success);
            let depth = 1 + children.iter().map(|c| proofs[c].depth).max().unwrap_or(0);
            proofs.insert(ids, Proof { children, evaluated: true, depth, success });
        }
    }
    let success = proofs[&root].success;
    let mut instructions = 0;
    let mut visited = BTreeSet::new();
    let mut todo = vec![root.clone()];
    while let Some(id) = todo.pop() {
        if visited.insert(id.clone()) {
            let proof = &proofs[&id];
            instructions += u64::from(proof.evaluated);
            todo.extend(proof.children.iter().cloned());
        }
    }
    ChainRecovery {
        instructions: if success { instructions } else { 0 },
        depth: if success { proofs[&root].depth } else { 0 },
        recovered: success,
        individually_recovered: success,
        blockers: if success { vec![] } else { blockers.into_iter().collect() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn individual_chains_need_one_common_leaf_source() {
        let module = bennett_ir::load(
            "memory 1024
func @f(i1 %b) -> i8 {
entry:
    %seed = select i8 %b, 5, 3
    br loop(i8 2)
loop(i8 %n):
    %v = mul i8 %n, %seed
    %next = sub i8 %n, 1
    %more = icmp ult i8 0, %next
    br %more, loop(%next), done(%v)
done(i8 %out):
    ret %out
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 4
    apply @f(%q) -> %y, %e budget 100
    %m = measure %y
    ret %m
}",
        )
        .expect("valid");
        let mut executor = Executor::new(&module, FuncId(1), &[], crate::Options::default()).expect("starts");
        executor.measure_journal_content(true);
        executor.run();
        let mut m = executor.take_journal_measurement().expect("on");
        let find = |p: &&ClassifiedPosition| p.site.block == BlockId(1) && p.site.pos == SitePos::Inst(0) && p.k == 1;
        assert_eq!(
            classify_with_chains(&m, false)[0].site_count.iter().find(find).expect("multiply").class,
            WordClass::Recomputed
        );
        let word = &mut m.applies[0].branches[1]
            .entries
            .iter_mut()
            .find(|r| r.site.block == BlockId(1) && r.site.pos == SitePos::Inst(0) && r.a_k == 1)
            .expect("multiply")
            .words[0];
        let context = word.chain.as_mut().expect("context");
        for (source, _) in Arc::make_mut(&mut context.live) {
            source.call_path = PathId::MAX;
        }
        let classes = classify_with_chains(&m, false);
        for positions in [&classes[0].site_count, &classes[0].call_path] {
            let p = positions.iter().find(find).expect("multiply");
            assert_eq!(p.class, WordClass::Irreducible);
            assert!(p.chain.as_ref().expect("diagnostic").individually_recovered);
        }
    }
}
