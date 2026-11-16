//! Memory as an overlay of writes over a classical base (`docs/DESIGN.md`, "Memory as an overlay").
//!
//! Every world reads one classical base, shared and never written while it is shared, and keeps its own
//! overlay: the bytes it wrote that differ from the base. A split therefore copies the overlay, never the
//! memory, so what worlds hold grows with the bytes they wrote, not with the memory's size.
//!
//! The overlay is kept **normal**: it holds a byte exactly where the memory differs from the base. A
//! write of the base's own byte removes the entry. The overlay is then a function of the memory it
//! describes alone, `{(a, m[a]) : m[a] != base[a]}`, whatever order and number of writes made it, and
//! an ordered map compares and hashes its entries in address order. So two overlays over one base are
//! equal exactly when they describe the same memory, independently of the order of the writes: the
//! rule by which worlds that differ only in memory merge.
//!
//! A memory that is the only owner of its base (one world) writes the base itself, as a plain byte
//! array would; a world left alone folds its overlay back in (`fold`, and at its first write).

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

/// A world's memory: the shared classical base and this world's overlay over it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Memory {
    base: Arc<Vec<u8>>,
    /// Address to byte, where this world's memory differs from the base, and only there.
    overlay: BTreeMap<usize, u8>,
    /// A second owner of the base in a check, so that every write goes to the overlay, as when several
    /// worlds share the base.
    #[cfg(test)]
    pinned: Option<Arc<Vec<u8>>>,
}

impl Memory {
    /// A memory holding `bytes`, owning its base.
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Memory {
            base: Arc::new(bytes),
            overlay: BTreeMap::new(),
            #[cfg(test)]
            pinned: None,
        }
    }

    /// The size in bytes.
    pub(crate) fn len(&self) -> usize {
        self.base.len()
    }

    /// The bytes at `range`, which must be in bounds, into `out` (as long as the range).
    pub(crate) fn read_into(&self, range: Range<usize>, out: &mut [u8]) {
        out.copy_from_slice(&self.base[range.clone()]);
        if !self.overlay.is_empty() {
            for (&address, &byte) in self.overlay.range(range.clone()) {
                out[address - range.start] = byte;
            }
        }
    }

    /// Writes `bytes` at `start`; the range must be in bounds.
    pub(crate) fn write(&mut self, start: usize, bytes: &[u8]) {
        let range = start..start + bytes.len();
        if let Some(base) = Arc::get_mut(&mut self.base) {
            for (address, byte) in std::mem::take(&mut self.overlay) {
                base[address] = byte;
            }
            base[range].copy_from_slice(bytes);
            return;
        }
        for (address, &byte) in range.zip(bytes) {
            if byte == self.base[address] {
                self.overlay.remove(&address);
            } else {
                self.overlay.insert(address, byte);
            }
        }
    }

    /// Moves the overlay into the base if this memory is the base's only owner (one world is left).
    pub(crate) fn fold(&mut self) {
        if self.overlay.is_empty() {
            return;
        }
        if let Some(base) = Arc::get_mut(&mut self.base) {
            for (address, byte) in std::mem::take(&mut self.overlay) {
                base[address] = byte;
            }
        }
    }

    /// The number of bytes in the overlay: where this world's memory differs from the base.
    pub(crate) fn overlay_len(&self) -> usize {
        self.overlay.len()
    }

    /// A copy of every byte.
    pub(crate) fn to_vec(&self) -> Vec<u8> {
        let mut bytes = self.base.as_ref().clone();
        for (&address, &byte) in &self.overlay {
            bytes[address] = byte;
        }
        bytes
    }

    /// Whether the two memories hold the same bytes. Every world reads the same base (a base changes only
    /// while one world owns it alone), so this compares the normal overlays only.
    pub(crate) fn same(&self, other: &Memory) -> bool {
        assert!(Arc::ptr_eq(&self.base, &other.base), "worlds share one memory base");
        self.overlay == other.overlay
    }

    /// Hashes what `same` compares.
    pub(crate) fn hash_overlay<H: Hasher>(&self, state: &mut H) {
        self.overlay.hash(state);
    }

    /// The overlay's entries in address order, for a check that the representation is restored exactly.
    #[cfg(test)]
    pub(crate) fn overlay(&self) -> Vec<(usize, u8)> {
        self.overlay.iter().map(|(&address, &byte)| (address, byte)).collect()
    }

    /// Keeps a second owner of the base from now on, so that every write goes to the overlay, as when
    /// several worlds share the base.
    #[cfg(test)]
    pub(crate) fn pin(&mut self) {
        self.pinned = Some(Arc::clone(&self.base));
    }

    /// Whether the overlay is normal: no entry equals the base's byte.
    #[cfg(test)]
    pub(crate) fn is_normal(&self) -> bool {
        self.overlay.iter().all(|(&address, &byte)| self.base[address] != byte)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::seq::SliceRandom;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn hash(memory: &Memory) -> u64 {
        let mut state = std::hash::DefaultHasher::new();
        memory.hash_overlay(&mut state);
        state.finish()
    }

    /// Two worlds over one base, each given the same final bytes by writes in a different order, with
    /// writes of other values in between and writes of the base's own bytes, compare equal and hash
    /// equal, and their overlays hold exactly the bytes that differ from the base; worlds whose final
    /// bytes differ in one place compare unequal.
    #[test]
    fn overlays_are_equal_exactly_when_their_memories_are_whatever_the_order_of_writes() {
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let size = 64;
        let base_bytes: Vec<u8> = (0..size).map(|_| rng.random_range(0..4)).collect();
        let base = Memory::new(base_bytes.clone());
        let mut cases = 0;
        for _ in 0..500 {
            let target: Vec<u8> =
                base_bytes.iter().map(|&b| if rng.random_bool(0.3) { rng.random_range(0..4) } else { b }).collect();
            let mut worlds = [base.clone(), base.clone()];
            for world in &mut worlds {
                // Up to three writes of random values per byte, then its final value, each world in its own
                // shuffled order (the final writes come after every other, in a shuffled order too).
                let mut writes: Vec<(usize, u8, bool)> = Vec::new();
                for (address, &byte) in target.iter().enumerate() {
                    for _ in 0..rng.random_range(0..4u32) {
                        writes.push((address, rng.random_range(0..4), false));
                    }
                    writes.push((address, byte, true));
                }
                writes.shuffle(&mut rng);
                writes.sort_by_key(|&(_, _, last)| last);
                for (address, value, _) in writes {
                    world.write(address, &[value]);
                    assert!(world.is_normal());
                }
                assert_eq!(world.to_vec(), target);
            }
            assert!(worlds[0].same(&worlds[1]), "equal memories, different write orders");
            assert_eq!(hash(&worlds[0]), hash(&worlds[1]));
            let differing: Vec<(usize, u8)> =
                (0..size).filter(|&a| target[a] != base_bytes[a]).map(|a| (a, target[a])).collect();
            assert_eq!(worlds[0].overlay(), differing, "the overlay is exactly where memory differs");
            let address = rng.random_range(0..size);
            worlds[1].write(address, &[target[address] ^ 1]);
            assert!(!worlds[0].same(&worlds[1]), "one byte differs");
            cases += 1;
        }
        assert_eq!(cases, 500);
        // The base's only owner writes it directly and keeps no overlay.
        let mut alone = Memory::new(base_bytes.clone());
        alone.write(3, &[9, 9]);
        assert_eq!(alone.overlay_len(), 0);
        assert_eq!(&alone.to_vec()[3..5], &[9, 9]);
    }

    /// A shared memory folds back into its base when it becomes the only owner, and reads the same.
    #[test]
    fn a_world_left_alone_folds_its_overlay_into_the_base() {
        let mut memory = Memory::new(vec![1, 2, 3, 4]);
        let other = memory.clone();
        memory.write(1, &[7, 3]);
        assert_eq!(memory.overlay(), vec![(1, 7)]);
        memory.fold();
        assert_eq!(memory.overlay_len(), 1, "still shared: no fold");
        drop(other);
        memory.fold();
        assert_eq!(memory.overlay_len(), 0);
        assert_eq!(memory.to_vec(), vec![1, 7, 3, 4]);
        let mut out = [0; 2];
        memory.read_into(0..2, &mut out);
        assert_eq!(out, [1, 7]);
    }
}

#[cfg(test)]
#[path = "memory_independent_tests.rs"]
mod independent_tests;
