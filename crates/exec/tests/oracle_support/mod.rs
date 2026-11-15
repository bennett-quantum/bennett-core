//! Independent test oracles. Semantics: DESIGN.md “apply”, “Tests”, “The next extension”;
//! SYNTAX.md (the shared reader); MARKERS.md “Gates”. No executor implementation helpers.
//!
//! Journalled-machine integration:
//! - `permutation::check_bijection(inputs, outputs, step)` accepts any complete, ordered,
//!   cloneable state encoding. Include machine cells, PC/control state, every history word,
//!   history length and the entire nested mark stack. `step` must expose the actual single-step
//!   output, not a projected register result. Enumerate ALL legal clean and dirty encodings;
//!   reachable-state sampling or truncating/discarding overflowing history proves less.
//! - Source and target domains can differ for a push/pop but must have equal cardinality.
//!   Supply the machine's documented boundary/fault states too; do not silently wrap a stack
//!   unless that is its semantics. The independent history unit adapter now includes actual
//!   mark/nested-rewind states, allocation history and garbage-prefix words
//!   (`src/independent_history_tests.rs`).
//! - `dense::Oracle::new` + `prefix` give the independent expected vector from program text
//!   loaded by `bennett_ir::load`. Compare mark/call/rewind-keep snapshots with the existing
//!   apply vectors. The interpreter stops before measurement and caps total allocation at
//!   13 qubits; unsupported instructions panic rather than quietly receive guessed semantics.
//! - `dense::Dense::{gate, permute}` accept independent 2x2 matrices and basis maps, and the
//!   region-conjugation test explicitly computes A† G A. Map a future whole-machine basis
//!   (including history) to these dense indices for the same conjugation experiment.
//!
//! Mark/rewind integration uses a test-only private-state adapter; no production injection API.
//! Dense expectations are explicit U† X U matrices rather than a second journal interpreter.

pub mod dense;
pub mod permutation;
