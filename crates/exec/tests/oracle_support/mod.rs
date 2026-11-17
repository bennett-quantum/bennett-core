//! Independent test oracles. Semantics: DESIGN.md "`apply`: a coherent region", "Tests"; SYNTAX.md (the
//! shared reader); MARKERS.md "Gates". No executor implementation helpers.
//!
//! - `permutation::check_bijection(inputs, outputs, step)` accepts any complete, ordered, cloneable state
//!   encoding. Source and target domains can differ for a push/pop but must have equal cardinality.
//! - `dense::Oracle::new` + `prefix` give the independent expected vector from program text loaded by
//!   `bennett_ir::load`. The interpreter stops before measurement and caps total allocation at 13 qubits;
//!   unsupported instructions panic rather than quietly receive guessed semantics.
//! - `dense::Dense::{gate, permute}` accept independent 2x2 matrices and basis maps, and the
//!   region-conjugation test explicitly computes A† G A.

pub mod dense;
pub mod permutation;
