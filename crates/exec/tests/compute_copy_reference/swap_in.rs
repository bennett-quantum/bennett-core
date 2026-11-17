//! Store by computing into a spare register, exchanging it and clearing or releasing it.

use std::collections::BTreeMap;

use bennett_ir::Module;
use num_complex::Complex;

use super::{Reference, Vector, permute};

// Each basis configuration lists the argument, target, spare and fault registers.
pub type Registers = [u64; 4];
pub type StoreVector = Vector<Registers>;
pub type Kept = [u64; 2];
pub type Density = BTreeMap<(Kept, Kept), Complex<i64>>;

pub fn apply_to_spare<const N: usize>(
    module: &Module,
    function: &str,
    budget: usize,
    state: StoreVector,
    arguments: impl Fn(&Registers) -> [u64; N],
) -> StoreVector {
    Reference::new(module, function)
        .apply(state, budget, arguments, |[_, _, spare, fault], (value, code)| {
            *spare ^= value;
            *fault ^= code;
        })
        .0
}

pub fn exchange(mut state: StoreVector, width: u32) -> StoreVector {
    for bit in 0..width {
        let mask = 1 << bit;
        state = permute(state, |[_, target, spare, _]| *spare ^= *target & mask);
        state = permute(state, |[_, target, spare, _]| *target ^= *spare & mask);
        state = permute(state, |[_, target, spare, _]| *spare ^= *target & mask);
    }
    state
}

pub fn clear_known(mut state: StoreVector, value: u64) -> StoreVector {
    for bit in 0..u64::BITS {
        let mask = 1 << bit;
        if value & mask != 0 {
            state = permute(state, |[_, _, spare, _]| *spare ^= mask);
        }
    }
    state
}

// A partial trace sums the outer products of the kept vectors at each definite
// value of the released registers. The common denominator is squared.
pub fn release(state: &StoreVector) -> Density {
    let mut by_released_pair = BTreeMap::<_, Vector<Kept>>::new();
    for (&[argument, target, spare, fault], &amplitude) in state {
        by_released_pair.entry([spare, fault]).or_default().insert([argument, target], amplitude);
    }
    let mut reduced = Density::new();
    for sector in by_released_pair.values() {
        for (&row, &alpha) in sector {
            for (&column, &beta) in sector {
                *reduced.entry((row, column)).or_default() += alpha * beta.conj();
            }
        }
    }
    reduced.retain(|_, amplitude| *amplitude != Complex::default());
    reduced
}
