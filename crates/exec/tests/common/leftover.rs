//! Helpers for the tests of running regardless outside strict mode (`leftover_wires.rs`,
//! `joint_leftovers.rs`): each program's body runs on three preparations of its control qubit, |0>, |+> and
//! |1>, and its final state is compared with one worked by hand.

use super::{assert_same_state, bounded, executor, load};
use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use num_complex::Complex64 as C;

/// 1/sqrt 2.
pub const HALF: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// The machine's meaning, reversible, bounded.
pub fn machine() -> Options {
    Options { machine_meaning: true, reversible: true, ..bounded() }
}

/// Strict mode, reversible, bounded.
pub fn strict() -> Options {
    Options { reversible: true, ..bounded() }
}

/// The three entries: `@zero`, `@plus` and `@one` allocate the control qubit, prepare it and call `@body`.
pub fn entries(body: &str) -> String {
    let entry = |name: &str, prepare: &str| {
        format!(
            "func @{name}() -> i8 {{\nentry:\n    %c = qalloc 1\n{prepare}    %v = call @body(%c)\n    ret %v\n}}\n"
        )
    };
    format!(
        "memory 256\n\nfunc @id(i1 %x) -> i1 {{\nentry:\n    ret %x\n}}\n\n{body}\n{}{}{}",
        entry("zero", ""),
        entry("plus", "    h %c\n"),
        entry("one", "    x %c\n")
    )
}

/// The final quantum state of a machine run of `entry`, checked against `expected` (configuration, real
/// amplitude) in configuration order, and the leftover qubits; then the run reverses to its entry.
pub fn check(text: &str, entry: &str, expected: &[(&[u64], f64)], leftover: u32) {
    let module = load(text);
    let mut e = executor(&module, entry, &[], machine());
    let start = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(Some(7)), "@{entry}");
    let quantum = e.snapshot().quantum;
    assert_eq!(quantum.amplitudes.len(), expected.len(), "@{entry}: {quantum:?}");
    for ((config, amplitude), (want, re)) in quantum.amplitudes.iter().zip(expected) {
        assert_eq!(config.as_slice(), *want, "@{entry}: {quantum:?}");
        assert!((amplitude - C::new(*re, 0.0)).norm() < 1e-12, "@{entry}: {quantum:?}");
    }
    assert_eq!(e.stats().leftover_qubits, leftover, "@{entry}");
    e.reverse().unwrap_or_else(|error| panic!("@{entry}: reverses: {error:?}"));
    assert_same_state(&e.snapshot(), &start, &format!("@{entry}: back to the entry"));
}

/// Strict mode keeps each of these a diagnostic: the rewind faults `bad_rewind` where the un-run is not
/// faithful (`plus`, `one`) and returns where it is (`zero`).
pub fn strict_unchanged(text: &str) {
    let module = load(text);
    assert_eq!(super::run(&module, "zero", &[], strict()).0, Outcome::Returned(Some(7)));
    for entry in ["plus", "one"] {
        assert_eq!(super::run(&module, entry, &[], strict()).0, Outcome::Faulted(Fault::BadRewind), "@{entry}");
    }
}
