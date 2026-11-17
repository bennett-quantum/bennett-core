//! The simulator's strict mode against the machine's meaning (`docs/DESIGN.md`, "Strict mode"; the
//! maintainer's decisions of the afternoon of 2026-11-16). Strict mode, the default, adds one diagnostic no
//! device can make: an `apply` faulting for some argument values stops the run. Under
//! `Options::machine_meaning` the fault code is the program's to measure. (`qfree` is the partial trace in
//! both: `release.rs`.)

mod common;

use bennett_exec::{Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, load};

fn machine() -> Options {
    Options { machine_meaning: true, reversible: true, ..bounded() }
}

fn strict() -> Options {
    Options { reversible: true, ..bounded() }
}

/// An `apply` whose callee faults (`examples/fault.bn`: `@triangle(n)` runs out of its budget for n > 3):
/// strict mode stops the run with a region fault; the machine goes on, the fault code in the program's
/// register, exactly as `allow_region_faults` gives.
#[test]
fn a_region_fault_is_a_fault_code_on_the_machine() {
    let module = load(include_str!("../../../examples/fault.bn"));
    for n in 0..16u64 {
        let (outcome, e) = common::run(&module, "main", &[n], machine());
        let allowed = Options { allow_region_faults: true, reversible: true, ..bounded() };
        let (expected, a) = common::run(&module, "main", &[n], allowed);
        assert_eq!(outcome, expected, "n = {n}");
        assert_same_state(&e.snapshot(), &a.snapshot(), &format!("n = {n}"));
        assert_eq!(e.stats().region_faults, u64::from(n > 3));
        let stopped =
            matches!(common::run(&module, "main", &[n], strict()).0, Outcome::Stopped(RunError::RegionFault { .. }));
        assert_eq!(stopped, n > 3, "n = {n}");
        // `out | fault << 8`: the budget's code where it faulted.
        let Outcome::Returned(Some(packed)) = outcome else { panic!("n = {n}: {outcome:?}") };
        assert_eq!(packed >> 8, if n > 3 { Fault::Budget.code().into() } else { 0 }, "n = {n}");
    }
}
