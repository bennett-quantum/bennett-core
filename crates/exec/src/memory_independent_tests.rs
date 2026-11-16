//! Dense byte-array checks independent of the ordered overlay representation.
use super::Memory;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

fn verify(memory: &Memory, dense: &[u8], base: &[u8]) {
    assert_eq!(memory.to_vec(), dense);
    assert_eq!(memory.base.as_slice(), base, "no shared-base write");
    let expected: Vec<_> = dense.iter().enumerate().filter(|&(i, b)| *b != base[i]).map(|(i, &b)| (i, b)).collect();
    assert_eq!(memory.overlay(), expected);
    // Exercise read_into independently of to_vec, across every interval (including empty and ends).
    for start in 0..=dense.len() {
        for end in start..=dense.len() {
            let mut found = vec![0xcc; end - start];
            memory.read_into(start..end, &mut found);
            assert_eq!(found, dense[start..end]);
        }
    }
}

#[test]
fn overlapping_writes_and_their_undo_match_dense_bytes_and_preserve_the_base() {
    let mut rng = ChaCha8Rng::seed_from_u64(0x1965);
    let mut operations = 0;
    for width in 1_u32..=128 {
        let len = width.div_ceil(8) as usize;
        let base: Vec<_> = (0..32).map(|_| rng.random::<u8>()).collect();
        let anchor = Memory::new(base.clone());
        let mut memory = anchor.clone();
        let mut dense = base.clone();
        let mut undo = Vec::new();
        for address in [0, 32 - len, 1, 15] {
            let mut bytes: Vec<_> = (0..len).map(|_| rng.random::<u8>()).collect();
            // Partial base restoration within the same write; not just all-zero bases.
            for i in (0..len).step_by(2) {
                bytes[i] = base[address + i];
            }
            undo.push((address, dense[address..address + len].to_vec(), memory.overlay()));
            memory.write(address, &bytes);
            dense[address..address + len].copy_from_slice(&bytes);
            verify(&memory, &dense, &base);
            operations += 1;
        }
        while let Some((address, old, overlay)) = undo.pop() {
            memory.write(address, &old);
            dense[address..address + old.len()].copy_from_slice(&old);
            verify(&memory, &dense, &base);
            assert_eq!(memory.overlay(), overlay, "undo restores exact representation");
            operations += 1;
        }
        assert!(memory.overlay().is_empty());
        assert!(Arc::ptr_eq(&memory.base, &anchor.base));
    }
    println!("{operations} writes/undos; all 128 widths; every read interval and exact overlay restored");
}

#[test]
fn clones_copy_only_written_bytes_and_repeated_folds_start_a_new_shared_base() {
    let mut rng = ChaCha8Rng::seed_from_u64(0x5eed);
    let mut memory = Memory::new((0..96).map(|_| rng.random::<u8>()).collect());
    for round in 0..64 {
        let base = memory.to_vec();
        let mut other = memory.clone();
        let old = Arc::clone(&memory.base);
        let mut dense = base.clone();
        for start in [0, 1, 79, 80] {
            let bytes: Vec<_> = (0..16).map(|_| rng.random::<u8>()).collect();
            memory.write(start, &bytes);
            dense[start..start + 16].copy_from_slice(&bytes);
        }
        verify(&memory, &dense, &base);
        let mut copy = memory.clone();
        assert!(Arc::ptr_eq(&memory.base, &copy.base));
        assert_eq!(memory.overlay(), copy.overlay());
        // Restore the clone in reverse address order; equality/hash must depend only on bytes.
        for address in (0..96).rev() {
            copy.write(address, &base[address..address + 1]);
        }
        assert!(copy.same(&other));
        let hash = |m: &Memory| {
            let mut h = std::hash::DefaultHasher::new();
            m.hash_overlay(&mut h);
            h.finish()
        };
        assert_eq!(hash(&copy), hash(&other));
        other.write(0, &[base[0] ^ 1]);
        assert!(!copy.same(&other));
        drop((copy, other, old));
        if round % 2 == 0 {
            memory.fold();
        } else {
            memory.write(96, &[]);
        }
        assert_eq!(memory.overlay_len(), 0);
        assert_eq!(memory.base.as_slice(), dense);
        // Hashes of the expected ordered set are another representation check.
        let mut h = std::hash::DefaultHasher::new();
        std::collections::BTreeMap::<usize, u8>::new().hash(&mut h);
        assert_eq!(hash(&memory), h.finish());
    }
    println!("64 clone/restore/discard/fold/new-split cycles");
}
