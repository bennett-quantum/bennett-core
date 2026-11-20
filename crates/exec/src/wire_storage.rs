//! Bind device wire identities to bits of the sparse simulator's register columns.
//!
//! A binding is a run of consecutive wires stored 64 to a column in consecutive columns, so the bookkeeping
//! grows with the columns a register uses, not with its qubits, and growing it is fallible: a register the
//! host cannot hold is a stop of the simulator, not an abort (`docs/DESIGN.md`, "The simulator's own stops
//! and reports").

use std::collections::{BTreeMap, TryReserveError};
use std::ops::Range;

use crate::device::Wire;
use crate::quantum::{COLUMN_QUBITS, QubitRef};

/// Wires per column, as a wire count.
const COLUMN: u64 = COLUMN_QUBITS as u64;

/// Bound wires `start..end` (the map's key and `end`) map wire `w` to column `column + (w - origin) / 64`,
/// bit `(w - origin) % 64`. A run trimmed by a later binding keeps its origin.
#[derive(Clone, Copy, Debug)]
struct Run {
    origin: Wire,
    end: Wire,
    column: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WireStorage {
    /// The runs by their first wire; no two overlap.
    runs: BTreeMap<Wire, Run>,
    /// For each column: the wire of its bit 0 and which of its bits are bound (0: none).
    columns: Vec<(Wire, u64)>,
}

impl WireStorage {
    /// Binds `wires` to consecutive columns from `column`, 64 to a column, the last holding the rest.
    pub(crate) fn bind(&mut self, column: usize, wires: Range<Wire>) -> Result<(), TryReserveError> {
        let columns = usize::try_from((wires.end - wires.start).div_ceil(COLUMN)).unwrap_or(usize::MAX);
        let last = column.saturating_add(columns);
        if let Some(missing) = last.checked_sub(self.columns.len()).filter(|&missing| missing > 0) {
            self.columns.try_reserve(missing)?;
            self.columns.resize(last, (0, 0));
        }
        self.trim(wires.clone());
        for (k, entry) in self.columns[column..last].iter_mut().enumerate() {
            let first = wires.start + k as Wire * COLUMN;
            let count = (wires.end - first).min(COLUMN);
            *entry = (first, u64::MAX >> (COLUMN - count));
        }
        self.runs.insert(wires.start, Run { origin: wires.start, end: wires.end, column });
        Ok(())
    }

    /// Cuts `wires` out of the runs that overlap them; what remains of each keeps its mapping.
    fn trim(&mut self, wires: Range<Wire>) {
        let overlapping: Vec<(Wire, Run)> = self
            .runs
            .range(..wires.end)
            .rev()
            .take_while(|(_, run)| run.end > wires.start)
            .map(|(&start, &run)| (start, run))
            .collect();
        for (start, run) in overlapping {
            self.runs.remove(&start);
            if start < wires.start {
                self.runs.insert(start, Run { end: wires.start, ..run });
            }
            if wires.end < run.end {
                self.runs.insert(wires.end, run);
            }
        }
    }

    pub(crate) fn qubit(&self, wire: Wire) -> Option<QubitRef> {
        let (_, run) = self.runs.range(..=wire).next_back()?;
        if wire >= run.end {
            return None;
        }
        let offset = wire - run.origin;
        let column = run.column + usize::try_from(offset / COLUMN).ok()?;
        let bit = (offset % COLUMN) as u8;
        let &(first, live) = self.columns.get(column)?;
        (first == wire - Wire::from(bit) && live >> bit & 1 == 1).then_some(QubitRef { column, bit })
    }

    /// The bits of each column that `wires` name, if every one of them is bound.
    pub(crate) fn masks(&self, wires: Range<Wire>) -> Option<BTreeMap<usize, u64>> {
        let mut masks = BTreeMap::new();
        let mut wire = wires.start;
        while wire < wires.end {
            let qubit = self.qubit(wire)?;
            let count = (COLUMN - u64::from(qubit.bit)).min(wires.end - wire);
            let mask = (u64::MAX >> (COLUMN - count)) << qubit.bit;
            if self.mask(qubit.column) & mask != mask {
                return None;
            }
            *masks.entry(qubit.column).or_default() |= mask;
            wire += count;
        }
        Some(masks)
    }

    /// Whether any of `wires` is bound; the work grows with the columns of the runs that overlap them.
    pub(crate) fn any_bound(&self, wires: Range<Wire>) -> bool {
        let overlapping = self.runs.range(..wires.end).rev().take_while(|(_, run)| run.end > wires.start);
        overlapping.into_iter().any(|(&start, run)| {
            let (low, high) = (start.max(wires.start), run.end.min(wires.end));
            let (first, last) = ((low - run.origin) / COLUMN, (high - 1 - run.origin) / COLUMN);
            (first..=last).any(|k| {
                let base = run.origin + k * COLUMN;
                // A run can end at the top of the wire space; no wire is at or past it.
                let (from, to) = (low.max(base) - base, high.min(base.saturating_add(COLUMN)) - base);
                let mask = (u64::MAX >> (COLUMN - (to - from))) << from;
                let column = usize::try_from(k).ok().and_then(|k| self.columns.get(run.column + k));
                column.is_some_and(|&(first_wire, live)| first_wire == base && live & mask != 0)
            })
        })
    }

    pub(crate) fn mask(&self, column: usize) -> u64 {
        self.columns.get(column).map_or(0, |&(_, mask)| mask)
    }

    pub(crate) fn unbind(&mut self, column: usize, mask: u64) {
        if let Some((_, live)) = self.columns.get_mut(column) {
            *live &= !mask;
        }
    }
}
