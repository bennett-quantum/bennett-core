//! `measure iN [ADDR]` (`docs/DESIGN.md`, "Qubits in memory"; the maintainer's decision M2 of 2026-11-16):
//! the `N` bits from bit 0 of the integer at the address, little-endian, read as a load reads them. The bits
//! of bytes that hold qubits are sampled jointly and the state collapses onto them, only those bits; the bits
//! of other bytes are the memory's. Each fixture's answer is written out by hand below, and every fixture is
//! also stepped against the independent whole-world reference (`oracle_support::worlds`), which follows a
//! sampled outcome only after checking its Born weight there.

#[allow(dead_code, reason = "this test uses the whole-world reference only")]
mod oracle_support;

use std::collections::{BTreeMap, BTreeSet};

use bennett_exec::{Executor, Options, Outcome, ReverseError};
use bennett_ir::{Fault, ValidModule};
use num_complex::Complex64 as C;
use oracle_support::worlds::{Model, basis};
use oracle_support::worlds_executor::{compare, observed_one_of, observed_reference};

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
func @bounds(i64 %p) -> i16 {
entry:
    %m = measure i16 [%p]
    ret %m
}
; A rewind across a measurement faults, as for a register.
func @across() -> i8 {
entry:
    h [16:0]
    %h = mark
    %m = measure i8 [16]
    rewind %h
    ret 0
}
; The thesis program in memory, measured at its address: 0 for every seed.
func @interfere() -> i8 {
entry:
    h [16:3]
    %h = mark
    %v = load i8 16
    %c = icmp eq i8 %v, 8
    br %c, one, done
one:
    store i8 9, 20
    br done
done:
    rewind %h
    h [16:3]
    %m = measure i8 [16]
    ret %m
}
; A load splits two worlds; the measurement observes one: 0 + 0 or 1 + 5, never 1 or 5.
func @observe() -> i8 {
entry:
    h [16:0]
    %v = load i8 16
    %t = trunc i8 %v to i1
    br %t, one, done
one:
    store i8 5, 20
    br done
done:
    %m = measure i8 [16]
    %s = load i8 20
    %r = add i8 %m, %s
    ret %r
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

fn module() -> ValidModule {
    bennett_ir::load(FIXTURES).expect("valid")
}

#[test]
fn the_bits_of_cells_are_sampled_jointly_and_collapse_and_other_bytes_are_read() {
    let module = module();
    let mut seen = BTreeSet::new();
    for (value, e) in outcomes(&module, "joint") {
        assert!([0, 1, 32, 33].contains(&(value & 0xff)) && value >> 8 == 0xAB, "{value:#x}");
        // Collapsed: the cell holds exactly the measured byte.
        let cell = cell(&e, 16);
        assert_eq!(cell.len(), 1, "{cell:?}");
        assert!((cell[&((value & 0xff) as u64)].norm() - 1.0).abs() < 1e-12);
        seen.insert(value);
    }
    assert_eq!(seen.len(), 4, "every outcome is reached in {SEEDS} seeds: {seen:?}");
    let mut seen = BTreeSet::new();
    for (value, e) in outcomes(&module, "correlated") {
        assert!(value == 0xAB00 || value == 0xAA80, "{value:#x}");
        assert_eq!(cell(&e, 17).len(), 1);
        assert_eq!(cell(&e, 16).len(), 1);
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
        assert_eq!(cell(&e, 16).len(), 1);
        seen.insert(value);
    }
    assert_eq!(seen.len(), 2);
}

#[test]
fn bounds_rewinds_and_worlds() {
    let module = module();
    let mut e = executor(&module, "bounds", &[255], 0);
    assert_eq!(e.run(), Outcome::Faulted(Fault::OutOfBounds));
    // The end of the address overflows: out of bounds, not a wrap to the start of memory.
    let mut e = executor(&module, "bounds", &[u64::MAX], 0);
    assert_eq!(e.run(), Outcome::Faulted(Fault::OutOfBounds));
    let mut e = executor(&module, "bounds", &[254], 0);
    assert_eq!(e.run(), Outcome::Returned(Some(0)));
    for seed in 0..SEEDS {
        let mut e = executor(&module, "across", &[], seed);
        assert_eq!(e.run(), Outcome::Faulted(Fault::BadRewind), "seed {seed}");
    }
    for (value, e) in outcomes(&module, "interfere") {
        assert_eq!(value, 0);
        assert_eq!(e.stats().observations, 0);
    }
    let mut seen = BTreeSet::new();
    for (value, e) in outcomes(&module, "observe") {
        assert!(value == 0 || value == 6, "{value}");
        let stats = e.stats();
        assert_eq!((stats.splits, stats.observations, stats.worlds), (1, 1, 1));
        seen.insert(value);
    }
    assert_eq!(seen.len(), 2);
}

/// Every fixture against the independent whole-world reference: step by step where the worlds stay in
/// step (the reference steps every world together; this executor parks a world that reaches a rewind or a
/// measurement first), and at the end for every fixture and seed, where the executor's final machine must be
/// one of the reference's observed outcomes with Born weight above zero.
#[test]
fn every_fixture_matches_the_independent_world_machine() {
    let module = module();
    let (mut stepped, mut ended) = (0, 0);
    for f in &module.functions {
        let cases: Vec<Vec<u64>> =
            if f.params.is_empty() { vec![vec![]] } else { vec![vec![254], vec![255], vec![u64::MAX]] };
        for args in cases {
            if !["interfere", "observe"].contains(&f.name.as_str()) {
                compare(&module, &f.name, &args, 256, true);
                stepped += 1;
            }
            let model = Model::new(&module, &f.name);
            let args128: Vec<u128> = args.iter().map(|&n| n.into()).collect();
            let reference = observed_reference(&model, basis(model.initial(&f.name, &args128)), 256);
            for seed in 0..8 {
                let mut e = executor(&module, &f.name, &args, seed);
                e.run();
                assert!(observed_one_of(&model, &reference, &e.snapshot()), "@{} {args:?} seed {seed}", f.name);
                ended += 1;
            }
        }
    }
    assert_eq!((stepped, ended), (10, 96));
}
