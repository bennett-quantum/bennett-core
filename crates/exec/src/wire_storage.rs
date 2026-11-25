//! The wire table: which device wire names which bit of the sparse simulator's register columns.
//!
//! A qubit of the simulator is a **storage bit**: bit `b` of column `c` is storage bit `64 * c + b`. The
//! table is a bijection between the live wires and the bound storage bits (`docs/DESIGN.md`, "Running a
//! program"), stored as **segments**: wires `w..w + n` are the storage bits `s..s + n`, in order. A segment
//! is what one allocation bound, less what releases took out of it; so the table grows with allocations,
//! not with qubits. Beside the segments, each column's mask of bound bits, their image, changed only with
//! them. Every query works on the segments that overlap the wires it names, split at segment and column
//! boundaries; an empty range overlaps none.

use std::collections::{BTreeMap, TryReserveError};
use std::ops::Range;

use crate::device::Wire;
use crate::quantum::{COLUMN_QUBITS, QubitRef};

/// Bits per column, as a storage bit count.
const COLUMN: u64 = COLUMN_QUBITS as u64;

/// The storage bit of bit 0 of `column`.
pub(crate) fn column_bit(column: usize) -> u64 {
    column as u64 * COLUMN
}

/// Wires from the key of the map to `end` are the storage bits from `bit` on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Segment {
    end: Wire,
    bit: u64,
}

/// Consecutive wires that are consecutive bits of one column, from bit `low`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Piece {
    pub(crate) wires: Range<Wire>,
    pub(crate) column: usize,
    pub(crate) low: u8,
}

impl Piece {
    /// The piece's bits of its column.
    pub(crate) fn mask(&self) -> u64 {
        (u64::MAX >> (COLUMN - (self.wires.end - self.wires.start))) << self.low
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WireStorage {
    /// By first wire; no two overlap, and none continues another (both its wires and its bits): the form is
    /// canonical, so two tables are equal exactly when they bind the same wires to the same bits.
    segments: BTreeMap<Wire, Segment>,
    /// For each column, its bits bound to a wire.
    bound: Vec<u64>,
}

impl WireStorage {
    /// Makes room for the masks of `columns` columns, or fails with the table unchanged.
    pub(crate) fn grow(&mut self, columns: usize) -> Result<(), TryReserveError> {
        if let Some(missing) = columns.checked_sub(self.bound.len()).filter(|&missing| missing > 0) {
            self.bound.try_reserve_exact(missing)?;
            self.bound.resize(columns, 0);
        }
        Ok(())
    }

    /// Binds `wires`, none of them bound, to the storage bits from `bit`, none of them bound, in columns
    /// the table has room for (`grow`).
    pub(crate) fn bind(&mut self, wires: Range<Wire>, bit: u64) {
        if wires.is_empty() {
            return;
        }
        debug_assert!(!self.any_bound(wires.clone()), "binding a bound wire");
        let mut segment = Segment { end: wires.end, bit };
        let mut start = wires.start;
        // Merge with the segment before and the one after where both the wires and the bits continue.
        if let Some((&before, &previous)) = self.segments.range(..start).next_back()
            && previous.end == start
            && previous.bit + (start - before) == bit
        {
            self.segments.remove(&before);
            (start, segment.bit) = (before, previous.bit);
        }
        if let Some(&next) = self.segments.get(&wires.end)
            && next.bit == bit + (wires.end - wires.start)
        {
            self.segments.remove(&wires.end);
            segment.end = next.end;
        }
        self.segments.insert(start, segment);
        for piece in split(wires.start, Segment { end: wires.end, bit }, wires.clone()) {
            debug_assert_eq!(self.bound[piece.column] & piece.mask(), 0, "binding a bound bit");
            self.bound[piece.column] |= piece.mask();
        }
    }

    /// Unbinds every bound wire of `wires`; what remains of a segment keeps its bits.
    pub(crate) fn unbind(&mut self, wires: Range<Wire>) {
        let overlapping: Vec<(Wire, Segment)> = self.overlapping(wires.clone()).collect();
        for (start, segment) in overlapping {
            for piece in split(start, segment, wires.clone()) {
                self.bound[piece.column] &= !piece.mask();
            }
            self.segments.remove(&start);
            if start < wires.start {
                self.segments.insert(start, Segment { end: wires.start, ..segment });
            }
            if wires.end < segment.end {
                let bit = segment.bit + (wires.end - start);
                self.segments.insert(wires.end, Segment { bit, ..segment });
            }
        }
    }

    /// The segments that hold a wire of `wires`, by first wire.
    fn overlapping(&self, wires: Range<Wire>) -> impl Iterator<Item = (Wire, Segment)> + '_ {
        let containing = self.segments.range(..=wires.start).next_back();
        let from = containing.filter(|(_, segment)| segment.end > wires.start).map_or(wires.start, |(&start, _)| start);
        let to = if wires.is_empty() { from } else { wires.end };
        self.segments.range(from..to).map(|(&start, &segment)| (start, segment))
    }

    /// Whether every wire of `wires` is bound.
    pub(crate) fn covers(&self, wires: Range<Wire>) -> bool {
        let mut next = wires.start;
        for (start, segment) in self.overlapping(wires.clone()) {
            if start > next {
                return false;
            }
            next = segment.end;
        }
        next >= wires.end
    }

    /// The pieces of `wires`, in wire order: of the bound wires only, so a caller checks `covers` first.
    pub(crate) fn pieces(&self, wires: Range<Wire>) -> impl Iterator<Item = Piece> + '_ {
        self.overlapping(wires.clone()).flat_map(move |(start, segment)| split(start, segment, wires.clone()))
    }

    /// Whether any wire of `wires` is bound.
    pub(crate) fn any_bound(&self, wires: Range<Wire>) -> bool {
        self.overlapping(wires).next().is_some()
    }

    pub(crate) fn qubit(&self, wire: Wire) -> Option<QubitRef> {
        let (&start, segment) = self.segments.range(..=wire).next_back()?;
        (wire < segment.end).then(|| {
            let bit = segment.bit + (wire - start);
            QubitRef { column: (bit / COLUMN) as usize, bit: (bit % COLUMN) as u8 }
        })
    }

    /// The bound bits of `column`.
    pub(crate) fn mask(&self, column: usize) -> u64 {
        self.bound.get(column).copied().unwrap_or(0)
    }

    /// Every segment: its wires and the storage bit of its first wire, in wire order.
    pub(crate) fn segments(&self) -> impl Iterator<Item = (Range<Wire>, u64)> + '_ {
        self.segments.iter().map(|(&start, segment)| (start..segment.end, segment.bit))
    }

    /// Forgets the masks of columns from `columns` on, which have no bound bit.
    pub(crate) fn truncate(&mut self, columns: usize) {
        debug_assert!(self.bound.iter().skip(columns).all(|&mask| mask == 0));
        self.bound.truncate(columns);
        // Give back the room of a wide register once it is gone, not on every release.
        if self.bound.capacity() / 2 > columns {
            self.bound.shrink_to_fit();
        }
    }
}

/// The part of `segment` (from wire `start`) inside `wires`, as pieces in wire order.
fn split(start: Wire, segment: Segment, wires: Range<Wire>) -> impl Iterator<Item = Piece> {
    let (from, to) = (start.max(wires.start), segment.end.min(wires.end));
    let first = segment.bit + (from - start);
    let mut wire = from;
    std::iter::from_fn(move || {
        (wire < to).then(|| {
            let bit = first + (wire - from);
            let low = bit % COLUMN;
            let count = (COLUMN - low).min(to - wire);
            let piece = Piece { wires: wire..wire + count, column: (bit / COLUMN) as usize, low: low as u8 };
            wire += count;
            piece
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(columns: usize) -> WireStorage {
        let mut table = WireStorage::default();
        table.grow(columns).expect("a few columns");
        table
    }

    /// A release across two allocations next to each other names bits of two columns: each piece stays in
    /// its own allocation.
    #[test]
    fn pieces_stop_at_the_end_of_an_allocation() {
        let mut table = table(2);
        table.bind(0..2, column_bit(0));
        table.bind(2..3, column_bit(1));
        assert!(table.covers(0..3));
        let pieces: Vec<Piece> = table.pieces(0..3).collect();
        assert_eq!(pieces, [Piece { wires: 0..2, column: 0, low: 0 }, Piece { wires: 2..3, column: 1, low: 0 }]);
    }

    /// An empty range inside a segment overlaps nothing.
    #[test]
    fn an_empty_range_names_no_wire() {
        let mut table = table(1);
        table.bind(0..2, column_bit(0));
        assert!(!table.any_bound(1..1));
        assert!(table.covers(1..1));
        assert_eq!(table.pieces(1..1).count(), 0);
    }

    /// Unbinding the middle of a segment leaves two that keep their bits; binding it back merges them.
    #[test]
    fn unbinding_splits_and_binding_back_merges() {
        let mut table = table(3);
        table.bind(10..140, column_bit(0));
        table.unbind(60..70);
        assert_eq!(table.qubit(70), Some(QubitRef { column: 0, bit: 60 }));
        assert_eq!(table.qubit(65), None);
        assert_eq!(table.mask(0), !(0x3ff << 50));
        assert!(!table.covers(50..80));
        table.bind(60..70, 50);
        assert_eq!(table.segments().collect::<Vec<_>>(), [(10..140, 0)]);
        assert_eq!(table.mask(2), 0b11);
    }

    /// Wires up to the top of the numbering are ordinary.
    #[test]
    fn the_top_wires_are_ordinary() {
        let mut table = table(1);
        table.bind(Wire::MAX - 3..Wire::MAX, column_bit(0));
        assert!(table.covers(Wire::MAX - 3..Wire::MAX));
        assert_eq!(table.qubit(Wire::MAX - 1), Some(QubitRef { column: 0, bit: 2 }));
        table.unbind(Wire::MAX - 1..Wire::MAX);
        assert_eq!(table.mask(0), 0b11);
    }
}
