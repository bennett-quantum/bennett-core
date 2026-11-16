//! The check that no value a rewind restored is used after it (`docs/SYNTAX.md`, "History").
//!
//! A `rewind` un-runs every step since its mark, so a slot defined between the mark and the rewind holds,
//! after it, its value at the mark (or none). Such a use is rejected. The check is a forward dataflow over
//! every path of the function, so a use is rejected when some path reaches it from such a rewind without
//! defining the slot again; a path is any path of the control-flow graph, and a resumption after a `setjmp`
//! may follow any call or `longjmp` of the function, the ways a `longjmp` can return to it.

use fixedbitset::FixedBitSet;

use crate::{Function, Inst, Slot, Terminator};

/// The facts at one point of the function, each true on some path to it.
#[derive(Clone, PartialEq, Eq)]
struct State {
    /// For each `mark` of the function, the slots defined since it last ran.
    since: Vec<FixedBitSet>,
    /// The slots holding a value a rewind restored: defined between a mark and a rewind of it, and not
    /// defined again since the rewind.
    stale: FixedBitSet,
}

impl State {
    fn union_with(&mut self, other: &State) {
        for (mine, theirs) in self.since.iter_mut().zip(&other.since) {
            mine.union_with(theirs);
        }
        self.stale.union_with(&other.stale);
    }

    fn define(&mut self, slot: Slot) {
        self.stale.set(slot.index(), false);
        for since in &mut self.since {
            since.insert(slot.index());
        }
    }
}

struct Analysis<'a> {
    function: &'a Function,
    /// For each slot, the index of the mark that defines it, if one does.
    mark_of: Vec<Option<usize>>,
    marks: usize,
}

/// Every use of a slot holding a value a rewind restored, in block order, each slot once: its first use.
/// A rewind whose handle no mark of the function defines is skipped (reported elsewhere).
pub(super) fn rewound_uses(function: &Function) -> Vec<(usize, Option<usize>, Slot)> {
    let has_rewind = function.blocks.iter().any(|block| block.insts.iter().any(|i| matches!(i, Inst::Rewind { .. })));
    if !has_rewind {
        return Vec::new();
    }
    let mut mark_of = vec![None; function.slot_count()];
    let mut marks = 0;
    for block in &function.blocks {
        for inst in &block.insts {
            if let Inst::Mark { dst } = inst {
                mark_of[dst.index()] = Some(marks);
                marks += 1;
            }
        }
    }
    let analysis = Analysis { function, mark_of, marks };
    let (entries, jumps) = analysis.fixpoint();
    let mut found = Vec::new();
    let mut reported = FixedBitSet::with_capacity(function.slot_count());
    for (b, entry) in entries.iter().enumerate() {
        let Some(entry) = entry else { continue };
        analysis.walk(b, entry.clone(), &jumps, &mut |position, state, slot| {
            if state.stale.contains(slot.index()) && !reported.put(slot.index()) {
                found.push((b, position, slot));
            }
        });
    }
    found
}

impl Analysis<'_> {
    fn empty(&self) -> State {
        let slots = self.function.slot_count();
        State { since: vec![FixedBitSet::with_capacity(slots); self.marks], stale: FixedBitSet::with_capacity(slots) }
    }

    /// The state at the entry of every reachable block (`None`: unreachable), and the union of the states at
    /// every call and `longjmp`, where a `setjmp` of the function may resume.
    fn fixpoint(&self) -> (Vec<Option<State>>, State) {
        let blocks = &self.function.blocks;
        let mut entries: Vec<Option<State>> = vec![None; blocks.len()];
        let mut start = self.empty();
        for param in &self.function.params {
            start.define(param.slot);
        }
        entries[0] = Some(start);
        let mut jumps = self.empty();
        let mut work: Vec<usize> = vec![0];
        loop {
            let mut seen_jumps = jumps.clone();
            while let Some(b) = work.pop() {
                let entry = entries[b].clone().expect("queued blocks are reached");
                let exit = self.walk(b, entry, &jumps, &mut |_, _, _| {});
                seen_jumps.union_with(&exit.jumps);
                blocks[b].term.visit_targets(|target| {
                    let t = target.block.index();
                    let changed = match &mut entries[t] {
                        Some(state) => {
                            let before = state.clone();
                            state.union_with(&exit.state);
                            *state != before
                        }
                        slot @ None => {
                            *slot = Some(exit.state.clone());
                            true
                        }
                    };
                    if changed && !work.contains(&t) {
                        work.push(t);
                    }
                });
            }
            if seen_jumps == jumps {
                return (entries, jumps);
            }
            jumps = seen_jumps;
            for (b, block) in blocks.iter().enumerate() {
                if entries[b].is_some() && block.insts.iter().any(|inst| matches!(inst, Inst::Setjmp { .. })) {
                    work.push(b);
                }
            }
        }
    }

    /// Runs block `b` from `state`, calling `uses` with each slot read (instruction position, or `None` for
    /// the terminator) and the state it is read in.
    fn walk(
        &self,
        b: usize,
        mut state: State,
        jumps: &State,
        uses: &mut impl FnMut(Option<usize>, &State, Slot),
    ) -> Exit {
        let block = &self.function.blocks[b];
        let mut sources = self.empty();
        for param in &block.params {
            state.define(param.slot);
        }
        for (i, inst) in block.insts.iter().enumerate() {
            if let Inst::Setjmp { .. } = inst {
                state.union_with(jumps);
            }
            inst.visit_uses(|slot| uses(Some(i), &state, slot));
            match inst {
                Inst::Call { .. } | Inst::CallIndirect { .. } => sources.union_with(&state),
                Inst::Rewind { mark, .. } => {
                    if let Some(k) = self.mark_of[mark.index()] {
                        let restored = state.since[k].clone();
                        state.stale.union_with(&restored);
                    }
                }
                // Nothing else changes which slots are stale or where a `setjmp` may resume.
                Inst::Binary { .. }
                | Inst::Compare { .. }
                | Inst::Select { .. }
                | Inst::Cast { .. }
                | Inst::Load { .. }
                | Inst::Store { .. }
                | Inst::Alloca { .. }
                | Inst::StackSave { .. }
                | Inst::StackRestore { .. }
                | Inst::Setjmp { .. }
                | Inst::ReturnAddress { .. }
                | Inst::FuncAddr { .. }
                | Inst::Effect { .. }
                | Inst::Qalloc { .. }
                | Inst::Qfree { .. }
                | Inst::Gate { .. }
                | Inst::GateAt { .. }
                | Inst::Measure { .. }
                | Inst::Apply { .. }
                | Inst::Mark { .. } => {}
            }
            if let Some(dst) = inst.result() {
                state.define(dst);
            }
            if let Inst::Mark { dst } = inst
                && let Some(k) = self.mark_of[dst.index()]
            {
                state.since[k].clear();
            }
        }
        block.term.visit_uses(|slot| uses(None, &state, slot));
        if let Terminator::Longjmp { .. } = block.term {
            sources.union_with(&state);
        }
        Exit { state, jumps: sources }
    }
}

struct Exit {
    state: State,
    jumps: State,
}
