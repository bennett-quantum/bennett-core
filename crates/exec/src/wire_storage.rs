//! Bind device wire identities to bits of the sparse simulator's register columns.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::device::Wire;
use crate::quantum::QubitRef;

#[derive(Clone, Debug, Default)]
pub(crate) struct WireStorage {
    wires: BTreeMap<Wire, QubitRef>,
    columns: BTreeMap<usize, (Wire, u64)>,
}

impl WireStorage {
    pub(crate) fn bind(&mut self, column: usize, wires: Range<Wire>) {
        let mask = u64::MAX >> (64 - (wires.end - wires.start));
        self.columns.insert(column, (wires.start, mask));
        self.wires.extend(wires.enumerate().map(|(bit, wire)| (wire, QubitRef { column, bit: bit as u8 })));
    }

    pub(crate) fn qubit(&self, wire: Wire) -> Option<QubitRef> {
        self.wires.get(&wire).copied()
    }

    pub(crate) fn mask(&self, column: usize) -> u64 {
        self.columns.get(&column).map_or(0, |&(_, mask)| mask)
    }

    pub(crate) fn unbind(&mut self, column: usize, mask: u64) {
        let Some((first, live)) = self.columns.get_mut(&column) else { return };
        let mut removed = *live & mask;
        *live &= !mask;
        while removed != 0 {
            self.wires.remove(&(*first + Wire::from(removed.trailing_zeros())));
            removed &= removed - 1;
        }
        if *live == 0 {
            self.columns.remove(&column);
        }
    }
}
