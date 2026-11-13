//! Ordinary scalar C functions (`front/tests/c/corpus.c`), lowered by bennett-front and checked on every
//! 8-bit input: against the natively compiled function, and applied coherently to all inputs at once.
//! This is evidence, not proof: a native build is a second implementation of the same C, not a
//! specification.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use bennett_exec::{Outcome, RunError};
use bennett_front_tests::{
    CLANG_FLAGS, c_dir, column, corpus_names, lower, native_results, registers, run, run_with_stats,
    state_before_first_measurement, state_without_fault_registers,
};
use bennett_ir::{Fault, ValidModule};

/// Every 8-bit input.
const INPUTS: u64 = 256;

/// How far an amplitude may be from 1/16 after eight Hadamards.
const TOLERANCE: f64 = 1e-12;

/// Seeds for whole coherent runs, which end by measuring one branch.
const SEEDS: u64 = 4;

/// The ordinary arguments the coherent wrappers are run with; each XORs x with a different mask.
const KEYS: [u64; 2] = [0, 5];

/// The mask `coherent.c` computes from `key`.
fn mask(key: u64) -> u64 {
    (key * 37 + 11) & 0xff
}

fn corpus(front_flags: &'static [&'static str]) -> &'static ValidModule {
    static PRECISE: OnceLock<ValidModule> = OnceLock::new();
    static COARSE: OnceLock<ValidModule> = OnceLock::new();
    let (cell, name) = if front_flags.is_empty() { (&PRECISE, "coherent") } else { (&COARSE, "coherent.taint-all") };
    cell.get_or_init(|| lower(&c_dir().join("coherent.c"), name, CLANG_FLAGS, front_flags))
}

/// Each function's device result on every input: `Ok(value)` as an unsigned 32-bit value, or the fault.
fn device_results() -> &'static BTreeMap<String, Vec<Result<u64, Fault>>> {
    static RESULTS: OnceLock<BTreeMap<String, Vec<Result<u64, Fault>>>> = OnceLock::new();
    RESULTS.get_or_init(|| {
        let module = corpus(&[]);
        corpus_names()
            .into_iter()
            .map(|name| {
                let results = (0..INPUTS)
                    .map(|x| match run(module, &name, &[x], 0) {
                        Outcome::Returned(Some(value)) => Ok(value),
                        Outcome::Faulted(fault) => Err(fault),
                        other => panic!("@{name}({x}): {other:?}"),
                    })
                    .collect();
                (name, results)
            })
            .collect()
    })
}

/// The only faults the corpus may have: division by zero, at 0, in the two dividing functions.
fn expected_fault(name: &str, x: u64) -> Option<Fault> {
    (name.ends_with("divide") && x == 0).then_some(Fault::DivideByZero)
}

#[test]
fn every_function_agrees_with_the_native_build_on_every_input() {
    let names = corpus_names();
    assert!(names.len() >= 15, "the corpus is listed: {names:?}");
    let mut compared = 0;
    for (name, results) in device_results() {
        for (x, result) in results.iter().enumerate() {
            assert_eq!(result.err(), expected_fault(name, x as u64), "@{name}({x})");
        }
        let inputs: Vec<u64> = (0..INPUTS).filter(|&x| results[x as usize].is_ok()).collect();
        let native = native_results(name, &inputs);
        assert_eq!(native.len(), inputs.len(), "{name}: native results");
        for (&x, expected) in inputs.iter().zip(native) {
            assert_eq!(results[x as usize], Ok(expected), "@{name}({x}) against the native build");
            compared += 1;
        }
    }
    assert_eq!(compared, names.len() as u64 * INPUTS - 2, "every input of every function but the two faults");
}

/// How far a fault's weight may be from its exact value after eight Hadamards.
const WEIGHT_TOLERANCE: f64 = 1e-12;

/// The inputs on which `name` faults.
fn faulting_inputs(name: &str) -> usize {
    device_results()[name].iter().filter(|result| result.is_err()).count()
}

/// For a function that faults on some inputs, applying it to all of them at once is a region fault on
/// those branches. Runs are strict, so the run stops there with the fault, the number of faulting branches
/// and their weight. Returns the outcome, for comparison.
fn assert_region_fault(module: &ValidModule, name: &str, key: u64) -> Outcome {
    let (outcome, stats) = run_with_stats(module, &format!("coherent_{name}"), &[key], 0);
    let Outcome::Stopped(RunError::RegionFault { fault, branches, configurations, .. }) = outcome else {
        panic!("{name}({key}): {outcome:?}")
    };
    let expected = device_results()[name].iter().find_map(|result| result.err());
    assert_eq!((Some(fault), branches, configurations), (expected, faulting_inputs(name), INPUTS as usize), "{name}");
    let weight = faulting_inputs(name) as f64 / INPUTS as f64;
    assert_eq!(stats.region_faults, 1, "{name}");
    assert!((stats.region_fault_weight - weight).abs() <= WEIGHT_TOLERANCE, "{name}: {}", stats.region_fault_weight);
    outcome
}

/// Checks that just before measuring, `coherent_<name>` holds every input x in `q` (one qubit per byte)
/// with amplitude 1/16, f(x) in `out`, and a clean fault register.
fn assert_every_branch_holds_x_and_f_of_x(module: &ValidModule, name: &str, key: u64) -> Vec<(Vec<u64>, String)> {
    let entry = format!("coherent_{name}");
    let columns = registers(module, &entry);
    let (q, out, fault) = (column(&columns, "q"), column(&columns, "out"), column(&columns, "out.fault"));
    let state = state_before_first_measurement(module, &entry, &[key]).quantum;
    assert_eq!(state.amplitudes.len(), INPUTS as usize, "{name}: one branch per input");
    let expected = &device_results()[name];
    let mut seen = vec![false; INPUTS as usize];
    for (config, amplitude) in &state.amplitudes {
        assert!((amplitude.norm() - 1.0 / 16.0).abs() <= TOLERANCE, "{name}: amplitude {amplitude}");
        let x = (0..8).map(|i| (config[q] >> (8 * i) & 1) << i).sum::<u64>() ^ mask(key);
        assert_eq!(config[q] & !0x0101_0101_0101_0101, 0, "{name}: only bit 0 of each byte of q is set");
        seen[x as usize] = true;
        assert_eq!((Ok(config[out]), config[fault]), (expected[x as usize], 0), "{name}: branch x = {x}");
    }
    assert!(seen.iter().all(|&s| s), "{name}: every input appears");
    state_without_fault_registers(module, &entry, &[key])
        .into_iter()
        .map(|(config, amplitude)| (config, format!("{amplitude:.15}")))
        .collect()
}

#[test]
fn every_function_applied_coherently_holds_x_and_f_of_x_on_every_branch() {
    let module = corpus(&[]);
    for (name, key) in corpus_names().into_iter().flat_map(|name| KEYS.map(|key| (name.clone(), key))) {
        if faulting_inputs(&name) > 0 {
            assert_region_fault(module, &name, key);
            continue;
        }
        assert_every_branch_holds_x_and_f_of_x(module, &name, key);
        // Whole runs measure one branch: the measured output is f of the measured input.
        for seed in 0..SEEDS {
            match run(module, &format!("coherent_{name}"), &[key], seed) {
                Outcome::Returned(Some(packed)) => {
                    let (x, y) = (packed >> 32, packed & 0xffff_ffff);
                    assert_eq!(Ok(y), device_results()[&name][x as usize], "{name}: seed {seed} measured x = {x}");
                }
                other => panic!("{name}: seed {seed}: {other:?}"),
            }
        }
    }
}

/// Coarser taint costs more, never correctness: with everything that can run coherently treated as
/// tainted, every state before measurement is the same.
#[test]
fn treating_everything_as_tainted_changes_no_state() {
    let (precise, coarse) = (corpus(&[]), corpus(&["--taint-all"]));
    assert_ne!(bennett_ir::write(precise), bennett_ir::write(coarse), "--taint-all changes the program");
    for (name, key) in corpus_names().into_iter().flat_map(|name| KEYS.map(|key| (name.clone(), key))) {
        if faulting_inputs(&name) > 0 {
            assert_eq!(assert_region_fault(precise, &name, key), assert_region_fault(coarse, &name, key), "{name}");
            continue;
        }
        let a = assert_every_branch_holds_x_and_f_of_x(precise, &name, key);
        let b = assert_every_branch_holds_x_and_f_of_x(coarse, &name, key);
        assert_eq!(a, b, "{name}({key})");
    }
}
