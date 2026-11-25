//! Address and switch values allowed by proved bits and shared qubits.

use std::collections::{BTreeMap, btree_map::Entry};

use super::bits::Bit;

pub(super) struct Selector {
    fixed: u128,
    mask: u128,
    aliases: Vec<(usize, usize, bool)>,
    distinct_wires: usize,
}

impl Selector {
    pub(super) fn new(selector: &[Bit]) -> Self {
        let mut fixed = 0;
        let mut mask = 0;
        let mut first_use = BTreeMap::new();
        let mut aliases = Vec::new();
        for (index, &bit) in selector.iter().enumerate() {
            match bit {
                Bit::Constant(value) => {
                    mask |= 1u128 << index;
                    fixed |= u128::from(value) << index;
                }
                Bit::Wire(wire, inverted) => match first_use.entry(wire) {
                    Entry::Vacant(entry) => {
                        entry.insert((index, inverted));
                    }
                    Entry::Occupied(entry) => {
                        let &(first, first_inverted) = entry.get();
                        aliases.push((index, first, inverted != first_inverted));
                    }
                },
            }
        }
        Self { fixed, mask, aliases, distinct_wires: first_use.len() }
    }

    pub(super) fn allows(&self, value: u128) -> bool {
        value & self.mask == self.fixed
            && self.aliases.iter().all(|&(index, first, inverted)| {
                let different = (value >> index ^ value >> first) & 1 != 0;
                different == inverted
            })
    }

    // Each compatible value describes one assignment to the distinct wires.
    // The caller supplies the count of distinct compatible values.
    pub(super) fn covered_by(&self, count: usize) -> bool {
        1usize.checked_shl(self.distinct_wires as u32) == Some(count)
    }
}
