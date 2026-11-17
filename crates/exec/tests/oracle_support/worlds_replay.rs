//! Inversion of snapshot history and dynamic-visit replay, independently of production journals.
use super::{Action, Model, Superposition, World, add, basis, clean};
use bennett_ir::{Fault, Inst, Operand};
use std::collections::BTreeMap;

impl Model<'_> {
    /// Undo a complete superposition, combining inverse-gate paths BEFORE testing clean discard.
    /// A per-input check would incorrectly reject paths whose temporary amplitudes cancel.
    pub fn undo_state(&self, inputs: &Superposition) -> Result<Superposition, Fault> {
        self.replaying.set(self.replaying.get() + 1);
        let result = self.undo_state_raw(inputs);
        self.replaying.set(self.replaying.get() - 1);
        result.map(|state| {
            let mut result = BTreeMap::new();
            let clock = self.clock(inputs);
            for (mut w, a) in state {
                w.classical.time.clock = if w.history.is_empty() { 0 } else { clock };
                add(&mut result, w, a);
            }
            result
        })
    }

    fn undo_state_raw(&self, inputs: &Superposition) -> Result<Superposition, Fault> {
        let mut groups = BTreeMap::<_, Superposition>::new();
        for (w, &a) in inputs {
            let record = w.history.last().ok_or(Fault::BadRewind)?;
            add(groups.entry(record.clone()).or_default(), w.clone(), a);
        }
        let mut result = BTreeMap::new();
        for (record, group) in groups {
            let mut state = BTreeMap::new();
            for (mut w, a) in group {
                w.history.drop_last_mut();
                add(&mut state, w, a);
            }
            if let Action::JoinedRewind { start, visit } = &record.action {
                let site = record.before.frames.last().expect("rewind site");
                let mut reset = BTreeMap::new();
                for (mut w, a) in state {
                    w.classical = (**start).clone();
                    add(&mut reset, w, a);
                }
                let mut completed = self.joined_until(reset, 4096, Some(site));
                for _ in 1..*visit {
                    let w = completed.keys().next().expect("replay");
                    let Some(Inst::Rewind { mark, .. }) = self.instruction(&w.classical) else {
                        return Err(Fault::BadRewind);
                    };
                    let h = w.integer(Operand::Slot(*mark))? as usize;
                    completed = self.joined_rewind(&completed, h)?;
                    completed = self.joined_until(completed, 4096, Some(site));
                }
                if completed.keys().any(|w| w.classical.finish.is_some()) {
                    return Err(Fault::BadRewind);
                }
                let Some(Inst::Rewind { keep, .. }) = self.instruction(&record.before) else { unreachable!() };
                for (mut w, a) in completed {
                    for (r, n) in self.keeps(&w, keep, None)? {
                        w.quantum[r] ^= n;
                    }
                    add(&mut result, w, a);
                }
                continue;
            }
            if let Action::Rewind { start, .. } = &record.action {
                let mut reset = BTreeMap::new();
                for (mut w, a) in state {
                    w.classical = (**start).clone();
                    add(&mut reset, w, a);
                }
                state = reset;
                let site = record.before.frames.last().expect("rewind site");
                for visit in 0..record.before.time.rewind_visit.max(1) {
                    if visit > 0 {
                        state = self.step_group(&state)?;
                    }
                    while state.keys().any(|w| {
                        let f = w.frame();
                        (f.activation, f.block, f.instruction) != (site.activation, site.block, site.instruction)
                    }) {
                        state = self.step_group(&state)?;
                        if state.keys().any(|w| w.classical.finish.is_some()) {
                            return Err(Fault::BadRewind);
                        }
                    }
                }
                for (mut w, a) in state {
                    if w.classical.finish.is_some() {
                        return Err(Fault::BadRewind);
                    }
                    let Some(Inst::Rewind { keep, .. }) = self.instruction(&record.before) else { unreachable!() };
                    for (r, n) in self.keeps(&w, keep, None)? {
                        w.quantum[r] ^= n;
                    }
                    w.classical.time.guards = record.before.time.guards.clone();
                    add(&mut result, w, a);
                }
                continue;
            }
            let abandoned = if let Action::Allocate(id) = record.action {
                if state.keys().any(|w| w.quantum[id] != 0) {
                    if !self.machine_meaning {
                        return Err(Fault::BadRewind);
                    }
                    let mut r = state.keys().next().expect("allocation group").classical.registers[id].clone();
                    r.generation += 1;
                    Some((id, r))
                } else {
                    None
                }
            } else {
                None
            };
            let mut undone = BTreeMap::new();
            for (mut w, a) in state {
                match &record.action {
                    Action::Measurement | Action::LeftBehind => return Err(Fault::BadRewind),
                    Action::Allocate(_) => {
                        if abandoned.is_none() {
                            w.quantum.truncate(record.before.registers.len());
                        }
                    }
                    Action::Free(id) => w.quantum[*id] = 0,
                    Action::LeftoverFree(_) => {}
                    Action::Read(reads) => {
                        if reads.iter().any(|&(id, value)| w.quantum[id] != value) {
                            return Err(Fault::BadRewind);
                        }
                    }
                    Action::Cells(cells) => {
                        for &(id, old, new) in cells {
                            if w.quantum[id] != u64::from(new) {
                                return Err(Fault::BadRewind);
                            }
                            w.quantum[id] = u64::from(old);
                        }
                    }
                    _ => {}
                }
                for (out, b) in self.quantum_action(w, &record, true)? {
                    add(&mut undone, out, a * b);
                }
            }
            clean(&mut undone);
            for (mut out, a) in undone {
                if let Action::MemoryGate { promoted, .. } = &record.action {
                    for &(_, id, old) in promoted {
                        if out.quantum[id] != u64::from(old) {
                            return Err(Fault::BadRewind);
                        }
                        out.quantum[id] = 0;
                    }
                    out.quantum.truncate(record.before.registers.len());
                }
                let clock = out.classical.time.clock;
                let guards = out.classical.time.guards.clone();
                out.classical = record.before.clone();
                out.classical.time.clock = clock;
                // An inverse taken by a forward rewind gives no budget back to live calls.
                // Restoration/reversal still restores the recorded counts exactly.
                if self.unrunning.get() > 0 {
                    for (activation, left) in guards {
                        if let Some(remaining) = out.classical.time.guards.get_mut(&activation) {
                            *remaining = left;
                        }
                    }
                }
                if let Some((id, r)) = &abandoned {
                    if *id == out.classical.registers.len() {
                        out.classical.registers.push(r.clone());
                    } else {
                        out.classical.registers[*id] = r.clone();
                    }
                    out.classical.time.leftovers.insert(*id);
                }
                add(&mut result, out, a);
            }
        }
        clean(&mut result);
        Ok(result)
    }

    pub fn undo(&self, w: World) -> Result<Superposition, Fault> {
        self.undo_state(&basis(w))
    }

    /// Invert unequal paths longest-history first; their shares meet before a common discard.
    pub fn reverse_joined(&self, mut state: Superposition) -> Result<Superposition, Fault> {
        loop {
            let longest = state.keys().map(|w| w.history.len()).max().unwrap_or(0);
            if longest == 0 {
                return Ok(state);
            }
            let (mut active, mut waiting) = (Superposition::new(), Superposition::new());
            for (w, a) in state {
                if w.history.len() == longest {
                    add(&mut active, w, a);
                } else {
                    add(&mut waiting, w, a);
                }
            }
            for (w, a) in self.undo_state(&active)? {
                add(&mut waiting, w, a);
            }
            clean(&mut waiting);
            state = waiting;
        }
    }
}
