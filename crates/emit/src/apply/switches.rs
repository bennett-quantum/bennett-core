//! Switch edges permitted by the selector's known bits and wire identities.

use std::collections::{BTreeMap, btree_map::Entry};

use bennett_ir::{SwitchCase, Target};

use super::bits::Bit;

#[derive(Clone, Default)]
pub(super) struct Edges<'a> {
    pub(super) cases: Vec<&'a SwitchCase>,
    pub(super) default: Option<&'a Target>,
}

impl<'a> Edges<'a> {
    pub(super) fn new(cases: &'a [SwitchCase], default: &'a Target) -> Self {
        Self { cases: cases.iter().collect(), default: Some(default) }
    }

    pub(super) fn retain_possible(&mut self, selector: &[Bit]) {
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
        self.cases.retain(|case| {
            case.value & mask == fixed
                && aliases.iter().all(|&(index, first, inverted)| {
                    let different = (case.value >> index ^ case.value >> first) & 1 != 0;
                    different == inverted
                })
        });
        // Each compatible case describes one assignment to the distinct wires,
        // and validation has checked that the case values are distinct.
        if 1usize.checked_shl(first_use.len() as u32) == Some(self.cases.len()) {
            self.default = None;
        }
    }
}
