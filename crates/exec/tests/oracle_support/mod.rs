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

#[allow(dead_code, reason = "test binaries use different oracle entry points")]
pub mod worlds;
#[allow(dead_code, reason = "test binaries use different adapters")]
pub mod worlds_executor;

// Whole-world reference integration (test-only):
// `worlds::Model::new(program, entry)` owns immutable base memory. `World` retains
// every classical coordinate and generous snapshot history; it is NOT production journal encoding.
// `step`/`run` compare one-world prefixes; `run_joined` independently executes coherent
// marked regions, waits at their rewind sites, and sums inverse paths before shared-history
// gates/allocations. Joined rewind records replay to dynamic visits, so unequal path lengths
// share history again. These are the two choices now decided by DESIGN.md “Worlds”.
// `observed_reference` enumerates Born instruments through measurement and host return;
// `assert_worlds` matches unordered complete classical/history groups, `assert_observed`
// checks the normalized sampled world without assuming identical RNG draw consumption.
// Public snapshots omit private journal/dead slots. `src/independent_history_tests.rs`
// independently checks private split/merge state, dirty history, allocations, read metadata,
// signed superpositions, injectivity and exact inverses. Model history uses snapshots so
// it does not assert equality of private encodings between the two implementations.
// Joined evaluation compares completed coherent regions, not tick budgets or strict-stop
// policy. Multiple rewind sites/time remain explicitly undecided; Unaligned and WorldFault
// are tested by direct policy probes. The optional suite adapter uses args capped at three
// for long cost workloads; original tests still run their original arguments and limits.
