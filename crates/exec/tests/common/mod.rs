//! Helpers shared by the executor's tests. The harness takes and compares snapshots itself.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use bennett_exec::{Executor, Options, Outcome, Snapshot};
use bennett_ir::{FuncId, ValidModule};

/// How far apart amplitudes may be after numerical gate arithmetic and still count as equal. The
/// programs under test apply at most a few hundred gates, each adding rounding of order 1e-16.
pub const TOLERANCE: f64 = 1e-12;

/// A step budget far above what any test program needs (the longest, `sum` of 1000, takes about 4000
/// steps). It only turns a runaway program into a fault.
pub const STEPS: u64 = 1_000_000;

/// A limit on basis configurations far above what any test program reaches (Grover peaks at 32). It
/// only turns a runaway superposition into a fault.
pub const CONFIGURATIONS: usize = 1 << 12;

/// Options with both resource bounds set, so that no test run is unbounded.
pub fn bounded() -> Options {
    Options { budget: Some(STEPS), max_configurations: Some(CONFIGURATIONS), ..Options::default() }
}

/// [`bounded`], for hand-written programs that read their fault registers on purpose: an `apply` may
/// fault on some branches and the run goes on.
pub fn allowing_region_faults() -> Options {
    Options { allow_region_faults: true, ..bounded() }
}

pub fn load(text: &str) -> ValidModule {
    bennett_ir::load(text).unwrap_or_else(|error| panic!("{error}"))
}

pub fn entry(module: &ValidModule, name: &str) -> FuncId {
    module.function_named(name).unwrap_or_else(|| panic!("no function @{name}"))
}

pub fn executor<'m>(module: &'m ValidModule, name: &str, args: &[u64], options: Options) -> Executor<'m> {
    Executor::new(module, entry(module, name), args, options).unwrap_or_else(|error| panic!("{error}"))
}

/// Runs `name(args)` to the end.
pub fn run<'m>(module: &'m ValidModule, name: &str, args: &[u64], options: Options) -> (Outcome, Executor<'m>) {
    let mut executor = executor(module, name, args, options);
    (executor.run(), executor)
}

/// Asserts that two snapshots are the same state: classical parts exactly, amplitudes within tolerance.
pub fn assert_same_state(actual: &Snapshot, expected: &Snapshot, context: &str) {
    assert_eq!(actual.classical, expected.classical, "{context}: classical state differs");
    let distance = actual.quantum.distance(&expected.quantum);
    assert!(
        distance.is_some_and(|d| d <= TOLERANCE),
        "{context}: quantum state differs by {distance:?}\nactual: {:?}\nexpected: {:?}",
        actual.quantum,
        expected.quantum
    );
}

/// Runs `name(args)` reversibly with `options`, reverses it completely, and checks that the state the
/// harness recorded at entry is restored. Returns the forward outcome.
pub fn assert_reverses(module: &ValidModule, name: &str, args: &[u64], options: Options) -> Outcome {
    let mut executor = executor(module, name, args, Options { reversible: true, ..options });
    let at_entry = executor.snapshot();
    let outcome = executor.run();
    let steps = executor.stats().steps;
    let undone = executor.reverse().unwrap_or_else(|error| panic!("@{name}{args:?}: {error}"));
    assert_eq!(undone, steps, "@{name}{args:?}: every step is undone");
    assert_same_state(&executor.snapshot(), &at_entry, &format!("@{name}{args:?} after {outcome:?}"));
    outcome
}
