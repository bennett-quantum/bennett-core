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
// `worlds::Model::new(program, entry)` owns the immutable classical base;
// `initial(entry,args)` constructs one complete `World`. Use `worlds::basis` or build a
// `worlds::Superposition` (BTreeMap<World, Complex64>) with arbitrary initial complex amplitudes.
// `run(initial, forward_rounds)` returns the complete final superposition, with `Finish` on EVERY
// world. `step` and `undo_state` expose the unprojected state for the same finite-domain checker.
// Measurement is an explicit boundary; `measurement_branches` gives Born weights and normalized
// states, and `collapse` follows a chosen outcome without assuming identical seeded RNG streams.
// The current executor adapter checks public memory, live slots (including encoded handles), every
// frame and PC, stack, tape, outcome, widths/generations and amplitudes after every instruction.
// It cannot compare private executor history encodings; model whole-state bijections and existing
// private-state executor bijection tests are separate checks. When worlds lands, replace only this
// adapter with a conversion that retains each world's private journal/marks, not a marginal over
// registers. The mathematical model deliberately uses classical snapshots, not production records.
//
// Open semantics must be resolved before asserting outcomes for mismatched history shapes,
// divergent fault/measurement sites or other unspecified changed-read rewinds. One round steps every
// active world, finished worlds wait; the model's fault groups are equal classical state AND history
// (today's common quantum shape). This is an explicit extension convention, not a scheduler/fault
// scope requirement for the next implementation. The model detects failed clean inverse/replay as
// bad_rewind per the documented interim rule. The progress handover lists the remaining questions.
