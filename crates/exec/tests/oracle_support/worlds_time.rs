//! Document-based time and guarded-call rules, separate from the snapshot interpreter.
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use super::{Action, Model, Record, Superposition, World, add};
use bennett_ir::Fault;

#[derive(Clone, Copy, Debug)]
pub enum MarkBudget {
    Unbudgeted,
    Unbounded,
    Deadline(u64),
}

#[derive(Clone, Debug, Default)]
pub struct Timing {
    pub clock: u64,
    pub rewind_visit: usize,
    pub marks: BTreeMap<usize, MarkBudget>,
    pub guards: BTreeMap<u64, u64>,
    pub leftovers: BTreeSet<usize>,
}

// Clocks and mark deadlines do not distinguish interfering states. A live call's
// remaining steps do: they determine whether the program receives a fault value.
impl PartialEq for Timing {
    fn eq(&self, other: &Self) -> bool {
        self.guards == other.guards
    }
}
impl Eq for Timing {}
impl PartialOrd for Timing {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Timing {
    fn cmp(&self, other: &Self) -> Ordering {
        self.guards.cmp(&other.guards)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diagnostic {
    World(Fault),
    Region(Fault),
}

impl Model<'_> {
    pub(super) fn charge_replay(&self) -> Result<(), Fault> {
        if self.unrunning.get() > 0 && self.checking.get() == 0 {
            if let Some(left) = self.replay_left.get() {
                if left == 0 {
                    self.replay_exhausted.set(true);
                    return Err(Fault::Budget);
                }
                self.replay_left.set(Some(left - 1));
            }
            self.replayed.set(self.replayed.get() + 1);
        }
        Ok(())
    }

    pub(super) fn spent_guard(&self, w: &World) -> Option<usize> {
        w.classical
            .frames
            .iter()
            .enumerate()
            .find(|(_, f)| f.fault.is_some() && w.classical.time.guards.get(&f.activation) == Some(&0))
            .map(|(i, _)| i)
    }

    pub(super) fn deadline_spent(&self, w: &World) -> bool {
        w.classical.marks.iter().any(|m| {
            m.valid
                && matches!(
            w.classical.time.marks.get(&m.handle), Some(MarkBudget::Deadline(d)) if w.classical.time.clock >= *d)
        })
    }

    pub(super) fn rewind_time(&self, w: &World, handle: usize, h: usize) -> Result<u64, Fault> {
        let read = w.history[h..]
            .iter()
            .any(|r| matches!(&r.action, Action::Read(_) | Action::Cells(_) | Action::MemoryGate { .. }));
        let budget = w.classical.time.marks.get(&handle).copied().unwrap_or(MarkBudget::Unbudgeted);
        if !self.region && read && matches!(budget, MarkBudget::Unbudgeted) {
            return Err(Fault::BadRewind);
        }
        Ok(match (read, budget) {
            (true, MarkBudget::Deadline(d)) => d,
            (false, _) | (true, MarkBudget::Unbudgeted | MarkBudget::Unbounded) => w.classical.time.clock,
        })
    }

    fn catch(&self, inputs: &Superposition, depth: usize, fault: Fault) -> Superposition {
        if self.replaying.get() == 0 && self.checking.get() == 0 && self.unrunning.get() == 0 {
            self.diagnostics.borrow_mut().push(Diagnostic::World(fault));
        }
        let mut result = Superposition::new();
        for (w, &a) in inputs {
            let mut out = w.clone();
            let frame = out.classical.frames[depth].clone();
            while out.classical.frames.len() > depth {
                let f = out.classical.frames.pop().expect("caught frame");
                self.leave(&mut out, f.activation);
                out.classical.time.guards.remove(&f.activation);
            }
            out.classical.sp = frame.entry_sp;
            out.frame_mut().instruction += 1;
            if let Some(dst) = frame.result {
                out.put(dst, super::Value::Int(0));
            }
            out.put(frame.fault.expect("guard has code slot"), super::Value::Int(fault.code().into()));
            out.classical.journal_records += 1;
            out.history.push(Record { before: w.classical.clone(), action: Action::Catch(fault) });
            add(&mut result, out, a);
        }
        result
    }

    pub(super) fn step_group(&self, inputs: &Superposition) -> Result<Superposition, Fault> {
        let first = inputs.keys().next().expect("group");
        if first.classical.finish.is_some() {
            return Ok(inputs.clone());
        }
        self.charge_replay()?;
        let time = &first.classical.time;
        let mark_spent = self.deadline_spent(first);
        let spent_guard = self.spent_guard(first);
        let attempt =
            if mark_spent && self.replaying.get() == 0 && self.unrunning.get() == 0 && self.checking.get() == 0 {
                Err(Fault::Budget)
            } else if let Some(depth) = spent_guard {
                Ok(self.catch(inputs, depth, Fault::Budget))
            } else {
                // The check sees all configurations; e6 forbids turning it into a program value.
                // Keep the M3 host fault until the per-branch jump rule is decided.
                if self.collective_jump_disagrees(inputs) {
                    return Err(Fault::BadLongjmp);
                }
                self.step_group_raw(inputs)
            };
        let outputs = match attempt {
            Ok(out) => out,
            Err(fault) => {
                let depth = first.classical.frames.iter().rposition(|f| f.fault.is_some());
                if self.replay_exhausted.get() {
                    return Err(Fault::Budget);
                }
                if let Some(depth) = depth {
                    self.catch(inputs, depth, fault)
                } else {
                    return Err(fault);
                }
            }
        };
        let mut result = Superposition::new();
        for (mut w, a) in outputs {
            // Replaying and inverse checks take no machine time. A catch is a step; the faulting
            // instruction has no separate step. Surviving guards pay for that step as well.
            if self.replaying.get() == 0 && self.unrunning.get() == 0 && self.checking.get() == 0 {
                w.classical.time.clock = w.classical.time.clock.max(time.clock.saturating_add(1));
            } else {
                w.classical.time.clock = time.clock;
            }
            for f in &w.classical.frames {
                if f.fault.is_some()
                    && let Some(&left) = time.guards.get(&f.activation)
                {
                    w.classical.time.guards.insert(f.activation, left.saturating_sub(1));
                }
            }
            add(&mut result, w, a);
        }
        Ok(result)
    }

    /// Bound replay as it executes, so even a path newly entering an endless loop terminates.
    /// This is run-resource exhaustion, outside every guarded call's fault-code interface.
    pub fn step_with_replay_budget(&self, state: &Superposition, left: u64) -> Superposition {
        let previous = self.replay_left.replace(Some(left));
        self.replay_exhausted.set(false);
        let replayed = self.replayed();
        let result = self.step(state);
        if self.replay_exhausted.get() {
            self.replayed.set(replayed);
        }
        self.replay_left.set(previous);
        result
    }

    /// Simulator-only diagnostics, never values delivered through a guarded call.
    /// Evaluate after `step`, with its input; the caller can retain the input for an atomic region stop.
    pub fn strict_stop(&self, input: &Superposition, output: &Superposition) -> Option<Diagnostic> {
        if self.machine_meaning {
            return None;
        }
        let groups: BTreeSet<_> =
            input.keys().filter(|w| w.classical.finish.is_none()).map(|w| (&w.classical, &w.history)).collect();
        let active: BTreeSet<_> =
            output.keys().filter(|w| w.classical.finish.is_none()).map(|w| (&w.classical, &w.history)).collect();
        let unhandled = output.keys().any(|w| matches!(w.classical.finish, Some(super::Finish::Faulted(_))));
        self.diagnostics
            .borrow()
            .iter()
            .find(|d| match d {
                Diagnostic::Region(_) => true,
                Diagnostic::World(_) => groups.len() > 1 && (active.len() > 1 || unhandled),
            })
            .cloned()
    }

    /// The post-step clock of the latest world is observable through Stats::clock; equality omits it.
    pub fn clock(&self, state: &Superposition) -> u64 {
        state.keys().map(|w| w.classical.time.clock).max().unwrap_or(0)
    }
}
