//! Grover search written in C over ordinary C predicates (`front/tests/c/grover.c`): the state before
//! measurement against sin²((2k + 1)θ) for several iteration counts, the measured frequency of solutions
//! over seeds, and the predicate in the device program as an applied ordinary function.

use std::process::Command;
use std::sync::OnceLock;

use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, MAX_CONFIGURATIONS, branches, c_dir, column, entry, lower, native_build, options, run,
};
use bennett_ir::{Inst, ValidModule};

/// The search space: 8-bit values.
const N: usize = 256;

/// How far the probability may be from the formula: rounding after a few hundred gates.
const TOLERANCE: f64 = 1e-9;

/// Runs per measured frequency.
const RUNS: u64 = 200;

/// How many standard deviations of the binomial count a measured frequency may be off.
const DEVIATIONS: f64 = 5.0;

/// How grover.c is lowered: from unoptimised C, with --taint-all, and from C clang has optimised (where
/// LLVM threads the predicate's loop into the branch around the marking gate).
const VARIANTS: [&str; 3] = ["O0", "taint-all", "O2"];

const CLANG_O2_FLAGS: &[&str] =
    &["-O2", "-fno-vectorize", "-fno-slp-vectorize", "-gline-tables-only", "-fno-discard-value-names"];

fn module(variant: &str) -> &'static ValidModule {
    static MODULES: OnceLock<Vec<ValidModule>> = OnceLock::new();
    let modules = MODULES.get_or_init(|| {
        let source = c_dir().join("grover.c");
        vec![
            lower(&source, "grover", CLANG_FLAGS, &[]),
            lower(&source, "grover.taint-all", CLANG_FLAGS, &["--taint-all"]),
            lower(&source, "grover.o2", CLANG_O2_FLAGS, &[]),
        ]
    });
    &modules[VARIANTS.iter().position(|v| *v == variant).unwrap_or_else(|| panic!("no variant {variant}"))]
}

/// The values of predicate `which` (0: one_solution, 1: digit_sum_12) for every x, from the native build.
fn predicate(which: usize) -> Vec<bool> {
    static TABLE: OnceLock<Vec<[bool; 2]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let executable = native_build(&c_dir().join("grover.c"), "grover", &["-DNATIVE"]);
        let output = Command::new(&executable).output().unwrap_or_else(|e| panic!("native grover: {e}"));
        assert!(output.status.success(), "native grover failed");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let mut fields = line.split(' ').map(|f| f == "1");
                [fields.next().unwrap_or(false), fields.next().unwrap_or(false)]
            })
            .collect()
    });
    assert_eq!(table.len(), N);
    table.iter().map(|row| row[which]).collect()
}

/// sin²((2k + 1)θ) with sin θ = √(M/N).
fn success(solutions: usize, iterations: u64) -> f64 {
    let theta = (solutions as f64 / N as f64).sqrt().asin();
    ((2 * iterations + 1) as f64 * theta).sin().powi(2)
}

/// Checks the state before measurement after `iterations` iterations: every solution has amplitude
/// sin((2k + 1)θ)/√M and every other value cos((2k + 1)θ)/√(N - M), and nothing else is left.
fn check_state(module: &ValidModule, search: &str, solutions: &[bool], iterations: u64) -> f64 {
    let m = solutions.iter().filter(|&&s| s).count();
    let angle = (2 * iterations + 1) as f64 * (m as f64 / N as f64).sqrt().asin();
    let (columns, state) = branches(module, search, &[iterations], MAX_CONFIGURATIONS);
    let (x, phase) = (column(&columns, "x"), column(&columns, "phase"));
    let mut probability = 0.0;
    for (values, amplitude) in &state {
        assert_eq!(values[phase], 0, "@{search}({iterations}): phase is left behind");
        let solution = solutions[values[x] as usize];
        let expected = if solution { angle.sin() / (m as f64).sqrt() } else { angle.cos() / ((N - m) as f64).sqrt() };
        assert!(
            (amplitude.re - expected).abs() <= TOLERANCE && amplitude.im.abs() <= TOLERANCE,
            "@{search}({iterations}): x = {} has {amplitude}, not {expected}",
            values[x]
        );
        if solution {
            probability += amplitude.norm_sqr();
        }
    }
    // A value whose amplitude is 0 is not in the state; the ones that are carry all the probability.
    let total: f64 = state.iter().map(|(_, a)| a.norm_sqr()).sum();
    assert!(state.len() <= N && (total - 1.0).abs() <= TOLERANCE, "@{search}({iterations}): {total}");
    assert!((probability - success(m, iterations)).abs() <= TOLERANCE, "@{search}({iterations}): {probability}");
    probability
}

fn check_search(search: &str, which: usize, iteration_counts: &[u64], measured: u64) {
    let solutions = predicate(which);
    let m = solutions.iter().filter(|&&s| s).count();
    for variant in VARIANTS {
        let module = module(variant);
        for &iterations in iteration_counts {
            check_state(module, search, &solutions, iterations);
        }
    }
    // Measured: the frequency of solutions over seeds, against the formula.
    let module = module("O0");
    let iterations = measured;
    let p = success(m, iterations);
    let found = (0..RUNS)
        .filter(|&seed| match run(module, search, &[iterations], seed) {
            Outcome::Returned(Some(x)) => solutions[x as usize],
            other => panic!("@{search}({iterations}), seed {seed}: {other:?}"),
        })
        .count();
    let allowed = DEVIATIONS * (p * (1.0 - p) * RUNS as f64).sqrt() + 1.0;
    assert!(
        (found as f64 - p * RUNS as f64).abs() <= allowed,
        "@{search}({iterations}): {found} of {RUNS} found a solution, expected {p} of them"
    );
}

#[test]
fn grover_finds_the_one_solution_of_an_ordinary_predicate() {
    assert_eq!(predicate(0).iter().filter(|&&s| s).count(), 1);
    // The optimum for M = 1 is about π/4 √N ≈ 12 iterations.
    // Six give about 0.53: the frequency then shows a wrong count of iterations or a wrong phase.
    check_search("search_one_solution", 0, &[0, 1, 2, 6, 12], 6);
}

#[test]
fn grover_finds_one_of_several_solutions_of_an_ordinary_predicate() {
    let m = predicate(1).iter().filter(|&&s| s).count();
    assert!(m > 4, "{m} solutions");
    check_search("search_digit_sum_12", 1, &[0, 1, 2, 3, 5], 1);
}

/// The predicate is not folded into gates: the device program applies an ordinary function that, run
/// as an ordinary function, computes the predicate on every x; and the search itself has no gate other
/// than the Hadamards, the X on `phase`, and Zs on `phase`.
#[test]
fn the_predicate_is_an_applied_ordinary_function() {
    let module = module("O0");
    for (search, which) in [("search_one_solution", 0), ("search_digit_sum_12", 1)] {
        let solutions = predicate(which);
        let function = module.function(entry(module, search));
        let applied: Vec<_> = function
            .blocks
            .iter()
            .flat_map(|block| &block.insts)
            .filter_map(|inst| match inst {
                Inst::Apply { callee, .. } => Some(*callee),
                _ => None,
            })
            .collect();
        let computes_predicate = applied.iter().any(|&callee| {
            module.function(callee).params.len() == 1
                && (0..N as u64).all(|x| {
                    let mut executor = Executor::new(module, callee, &[x], options(0)).expect("starts");
                    executor.run() == Outcome::Returned(Some(u64::from(solutions[x as usize])))
                })
        });
        assert!(computes_predicate, "@{search}: no applied function computes the predicate");
        for inst in function.blocks.iter().flat_map(|block| &block.insts) {
            if let Inst::Gate { gate, target, controls } = inst {
                let register = &function.slot_names[target.register.index()];
                let name = gate_name(gate);
                assert!(
                    (register == "x" && name == "h" && controls.is_empty())
                        || (register == "phase" && (name == "x" || name == "z")),
                    "@{search}: {name} on %{register}"
                );
            }
        }
    }
}

fn gate_name(gate: &bennett_ir::Gate) -> &'static str {
    match gate {
        bennett_ir::Gate::Fixed(fixed) => fixed.mnemonic(),
        bennett_ir::Gate::Rotate(axis, _) => axis.mnemonic(),
    }
}
