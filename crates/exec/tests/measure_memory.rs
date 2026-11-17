//! `measure iN [ADDR]` (`docs/DESIGN.md`, "Qubits in memory"; the maintainer's decision M2 of 2026-11-16):
//! the `N` bits from bit 0 of the integer at the address, little-endian, read as a load reads them. The bits
//! of bytes that hold qubits are sampled jointly and the state collapses onto them, only those bits; the bits
//! of other bytes are the memory's. A byte whose eight bits were all measured is ordinary memory again,
//! holding the result. Each fixture's answer is written out by hand below.

use std::collections::{BTreeMap, BTreeSet};

use bennett_exec::{Executor, Options, Outcome, ReverseError};
use bennett_ir::ValidModule;
use num_complex::Complex64 as C;

const FIXTURES: &str = r#"
memory 256
data 17 = i8 171
data 24 = i32 305419896
; Bits 0 and 5 of byte 16 in superposition, byte 17 classical (0xAB): 0xAB00 + {0, 1, 32, 33}, a quarter each.
func @joint() -> i16 {
entry:
    h [16:0]
    h [16:5]
    %m = measure i16 [16]
    ret %m
}
; Only bit 0 is measured; bit 5 keeps its superposition.
func @partial() -> i1 {
entry:
    h [16:0]
    h [16:5]
    %m = measure i1 [16]
    ret %m
}
; Two bytes that hold qubits, correlated: byte 17 (promoted from 171) flips its bit 0 with bit 7 of byte 16.
; Sampled jointly: 0xAB00 or 0xAA80, never a mixture.
func @correlated() -> i16 {
entry:
    h [16:7]
    x [17:0] ctrl [16:7]
    %m = measure i16 [16]
    ret %m
}
; No qubits at the address: the memory's value, nothing sampled.
func @classical() -> i32 {
entry:
    %m = measure i32 [24]
    ret %m
}
; The widest integer: one cell (bit 0 of byte 16) and fifteen classical bytes, 0xAB at 17 and 0x12345678 at 24.
func @wide() -> i128 {
entry:
    h [16:0]
    %m = measure i128 [16]
    ret %m
}
; A narrow width reads only its low bits of a classical byte: 0xAB gives 0xB.
func @narrow() -> i4 {
entry:
    %m = measure i4 [17]
    ret %m
}
; The width ends inside a byte that holds qubits and is not the first: of byte 17 (0xAB, promoted) bits 0 to
; 3 are measured, 0xA00 or 0xB00, and bits 4 to 7 keep their superposition.
func @partial_high() -> i12 {
entry:
    h [17:0]
    h [17:4]
    %m = measure i12 [16]
    ret %m
}
func @bounds(i64 %p) -> i16 {
entry:
    %m = measure i16 [%p]
    ret %m
}
"#;

const SEEDS: u64 = 64;

fn executor<'m>(module: &'m ValidModule, entry: &str, args: &[u64], seed: u64) -> Executor<'m> {
    let options = Options { seed, reversible: true, budget: Some(1_000), ..Options::default() };
    Executor::new(module, module.function_named(entry).expect("entry"), args, options).expect("starts")
}

/// The outcomes over `SEEDS` seeds, each with the executor after it.
fn outcomes<'m>(module: &'m ValidModule, entry: &str) -> Vec<(u128, Executor<'m>)> {
    (0..SEEDS)
        .map(|seed| {
            let mut e = executor(module, entry, &[], seed);
            let Outcome::Returned(Some(value)) = e.run() else { panic!("@{entry} seed {seed}: {:?}", e.outcome()) };
            (value, e)
        })
        .collect()
}

/// The values the cell at `address` holds, with their amplitudes.
fn cell(e: &Executor<'_>, address: u64) -> BTreeMap<u64, C> {
    let snapshot = e.snapshot();
    let column = snapshot.quantum.cells.iter().find(|&&(a, _)| a == address).expect("a cell").1;
    let mut values = BTreeMap::new();
    for (config, amplitude) in &snapshot.quantum.amplitudes {
        *values.entry(config[column]).or_insert(C::new(0.0, 0.0)) += amplitude;
    }
    values
}

/// The byte at `address`, which must no longer hold qubits: a byte whose eight bits were all measured is
/// ordinary memory again, holding the result (the maintainer's decision of 2026-11-17).
fn ordinary(e: &Executor<'_>, address: u64) -> u8 {
    let snapshot = e.snapshot();
    assert!(snapshot.quantum.cells.iter().all(|&(a, _)| a != address), "byte {address} still holds qubits");
    snapshot.classical.memory[address as usize]
}

fn module() -> ValidModule {
    bennett_ir::load(FIXTURES).expect("valid")
}

#[test]
fn the_bits_of_cells_are_sampled_jointly_and_collapse_and_other_bytes_are_read() {
    let module = module();
    let mut seen = BTreeSet::new();
    for (value, e) in outcomes(&module, "joint") {
        assert!([0, 1, 32, 33].contains(&(value & 0xff)) && value >> 8 == 0xAB, "{value:#x}");
        // Measured whole: the byte is ordinary memory holding the result, and no qubits are left.
        assert_eq!(u128::from(ordinary(&e, 16)), value & 0xff);
        assert!(e.snapshot().quantum.amplitudes.iter().all(|(config, _)| config.is_empty()));
        seen.insert(value);
    }
    assert_eq!(seen.len(), 4, "every outcome is reached in {SEEDS} seeds: {seen:?}");
    let mut seen = BTreeSet::new();
    for (value, e) in outcomes(&module, "correlated") {
        assert!(value == 0xAB00 || value == 0xAA80, "{value:#x}");
        assert_eq!(u128::from(ordinary(&e, 17)), value >> 8);
        assert_eq!(u128::from(ordinary(&e, 16)), value & 0xff);
        seen.insert(value);
    }
    assert_eq!(seen.len(), 2);
}

#[test]
fn bits_not_measured_keep_their_superposition() {
    let module = module();
    let mut seen = BTreeSet::new();
    for (value, e) in outcomes(&module, "partial") {
        let cell = cell(&e, 16);
        let expected: Vec<u64> = vec![value as u64, value as u64 | 32];
        assert_eq!(cell.keys().copied().collect::<Vec<_>>(), expected);
        for amplitude in cell.values() {
            assert!((amplitude - C::new(0.5f64.sqrt(), 0.0)).norm() < 1e-12, "{cell:?}");
        }
        seen.insert(value);
    }
    assert_eq!(seen.len(), 2);
}

#[test]
fn bits_past_the_width_in_a_later_byte_keep_their_superposition() {
    let module = module();
    let mut seen = BTreeSet::new();
    for (value, e) in outcomes(&module, "partial_high") {
        assert!(value == 0xA00 || value == 0xB00, "{value:#x}");
        let measured = 0xA0 | (value >> 8) as u64;
        let cell = cell(&e, 17);
        assert_eq!(cell.keys().copied().collect::<Vec<_>>(), vec![measured, measured | 0x10], "{cell:?}");
        // Equal amplitudes of weight one half; bit 0 started at 1, so after a 1 is measured both carry -1.
        let amplitudes: Vec<C> = cell.values().copied().collect();
        assert!((amplitudes[0] - amplitudes[1]).norm() < 1e-12, "{cell:?}");
        assert!((amplitudes[0].norm() - 0.5f64.sqrt()).abs() < 1e-12, "{cell:?}");
        seen.insert(value);
    }
    assert_eq!(seen.len(), 2);
}

#[test]
fn memory_without_qubits_is_read_and_still_cannot_be_reversed_past() {
    let module = module();
    for (value, mut e) in outcomes(&module, "classical") {
        assert_eq!(value, 305_419_896);
        assert!(e.snapshot().quantum.cells.is_empty(), "nothing promoted");
        assert_eq!(e.reverse(), Err(ReverseError::PastMeasurement));
    }
    for (value, _) in outcomes(&module, "narrow") {
        assert_eq!(value, 0xB);
    }
    let mut seen = BTreeSet::new();
    for (value, e) in outcomes(&module, "wide") {
        assert_eq!(value & !1, 0x1234_5678_0000_0000_0000_AB00, "{value:#x}");
        assert_eq!(u128::from(ordinary(&e, 16)), value & 0xff);
        seen.insert(value);
    }
    assert_eq!(seen.len(), 2);
}

#[test]
fn a_mixed_width_measurement_releases_only_whole_bytes_and_allows_repromotion() {
    let module = bennett_ir::load(
        r#"
memory 256
data 32 = i16 9562
func @entry(i1 %finish) -> i16 {
entry:
    x [32:1]
    h [33:7]
    s [33:7]
    %partial = measure i12 [32]
    store i8 16, 32
    br %finish, finish, blocked
blocked:
    %early = load i8 33
    ret 0
finish:
    x [32:0]
    %low = measure i8 [32]
    %high = measure i8 [33]
    %both = load i16 32
    ret %both
}
"#,
    )
    .expect("valid program");
    let mut seen = BTreeSet::new();
    for seed in 0..SEEDS {
        for finish in [false, true] {
            let mut e = executor(&module, "entry", &[u64::from(finish)], seed);
            for _ in 0..4 {
                assert_eq!(e.step(), None);
            }
            // The initial word 0x255a becomes 0x?558, with bit 15 still in (|0> + i|1>)/sqrt(2).
            assert_eq!(ordinary(&e, 32), 0x58);
            let h = std::f64::consts::FRAC_1_SQRT_2;
            let state = cell(&e, 33);
            assert_eq!(state.keys().copied().collect::<Vec<_>>(), [0x25, 0xa5]);
            assert!((state[&0x25] - C::new(h, 0.0)).norm() < 1e-12, "{state:?}");
            assert!((state[&0xa5] - C::new(0.0, h)).norm() < 1e-12, "{state:?}");

            let outcome = e.run();
            if finish {
                let Outcome::Returned(Some(word)) = outcome else { panic!("{outcome:?}") };
                assert!([0x2511, 0xa511].contains(&word), "{word:#x}");
                assert_eq!(ordinary(&e, 32), 0x11);
                assert_eq!(ordinary(&e, 33), (word >> 8) as u8);
                seen.insert(word);
            } else {
                assert_eq!(outcome, Outcome::Faulted(bennett_ir::Fault::QubitMemoryAccess));
                assert_eq!(ordinary(&e, 32), 16, "the store to the fully measured byte succeeds");
                assert_eq!(cell(&e, 33), state);
            }
        }
    }
    assert_eq!(seen, BTreeSet::from([0x2511, 0xa511]));
}
