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

/// The simulator's stops never reach a program as a value. Inside an applied function the quantum steps that
/// a stop concerns (allocation, gates, release, a nested `apply`) fault `quantum_in_region` first, so with
/// the configuration limit at the two configurations `%q` gives, the applied `qalloc` and `h` (which would
/// double them) and the nested `apply` give the program code 8 for each value of `%q`, never a stop; the
/// program measures 8. The run's report of nonzero releases stays 0: the release inside faulted first.
#[test]
fn no_stop_or_report_of_the_simulator_becomes_a_value_of_the_program() {
    let module = load(
        "memory 64
func @grow(i1 %x) -> i1 {
entry:
    %r = qalloc 1
    h %r
    qfree %r
    ret 0
}
func @nested(i1 %x) -> i1 {
entry:
    call @inner()
    ret 0
}
func @inner() {
entry:
    %o = qalloc 1
    %e = qalloc 5
    apply @grow(i1 0) -> %o, %e budget 10
    ret
}
func @main(i1 %which) -> i5 {
entry:
    %q = qalloc 1
    h %q
    %o = qalloc 1
    %e = qalloc 5
    br %which, nested, grow
grow:
    apply @grow(%q) -> %o, %e budget 10
    br done
nested:
    apply @nested(%q) -> %o, %e budget 10
    br done
done:
    %f = measure %e
    ret %f
}",
    );
    for which in [0, 1] {
        let options = Options { max_configurations: Some(2), ..common::allowing_region_faults() };
        let (outcome, e) = common::run(&module, "main", &[which], options);
        assert_eq!(outcome, Outcome::Returned(Some(Fault::QuantumInRegion.code().into())), "which {which}");
        assert_eq!((e.stats().region_faults, e.stats().nonzero_releases), (1, 0), "which {which}");
    }
}
