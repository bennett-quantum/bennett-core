//! A simulator stop ends execution without writing any value into the device's state.

mod common;

use bennett_exec::{Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};

#[test]
fn configuration_stops_restore_registers_and_all_address_promotions() {
    for (prepare, gate) in [
        ("%q = qalloc 1", "h %q"),
        ("", "h [64:0]"),
        ("", "h [64:0] ctrl [65:0]"),
        ("z [64:7]", "h [64:0] ctrl [65:0]"),
    ] {
        let module = load(&format!(
            "memory 512\ndata 64 = i8 2\ndata 65 = i8 1\nfunc @main() -> i8 {{\nentry:\n{prepare}\n{gate}\neffect 88\nstore i8 99, 8\nret 77\n}}"
        ));
        let mut e =
            executor(&module, "main", &[], Options { max_configurations: Some(1), reversible: true, ..bounded() });
        let entry = e.snapshot();
        if !prepare.is_empty() {
            assert_eq!(e.step(), None);
        }
        let before = e.snapshot();
        let journal = e.stats().journal_bytes;
        let stopped = Outcome::Stopped(RunError::ConfigurationLimit { limit: 1 });
        for _ in 0..3 {
            assert_eq!(e.run(), stopped);
            let mut actual = e.snapshot();
            actual.classical.outcome = None;
            assert_same_state(&actual, &before, gate);
            assert_eq!(e.stats().journal_bytes, journal);
            assert!(e.tape().is_empty());
            assert_eq!(e.stats().nonzero_releases, 0);
        }
        e.reverse().expect("reverse successful prefix of stopped run");
        assert_same_state(&e.snapshot(), &entry, "stopped run entry");
    }
}

#[test]
fn strict_partial_fault_stops_without_copying_any_result_or_fault_into_targets() {
    let module = load(
        "memory 512
func @probe(i1 %x) -> i8 {
entry:
 store i8 99, 8
 %wide = zext i1 %x to i64
 %address = add i64 %wide, 63
 %v = load i8 %address
 ret 29
}
func @main() -> i8 {
entry:
 %q = qalloc 1
 %out = qalloc 8
 %fault = qalloc 5
 h %q
 x %out[2]
 x %fault[1]
 z [64:7]
 apply @probe(%q) -> %out, %fault budget 10
 effect 88
 ret 77
}",
    );
    let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let entry = e.snapshot();
    for _ in 0..7 {
        assert_eq!(e.step(), None);
    }
    let before = e.snapshot();
    let stopped = Outcome::Stopped(RunError::RegionFault {
        callee: module.function_named("probe").expect("probe"),
        fault: Fault::QubitMemoryAccess,
        faulting: 1,
        configurations: 2,
    });
    assert_eq!(e.run(), stopped);
    assert_eq!(e.step(), Some(stopped));
    let mut actual = e.snapshot();
    actual.classical.outcome = None;
    assert_same_state(&actual, &before, "strict stop before XOR");
    assert_eq!(e.stats().region_faults, 1);
    assert!((e.stats().region_fault_weight - 0.5).abs() < 1e-12);
    assert!(e.tape().is_empty());
    e.reverse().expect("strict stop reverses");
    assert_same_state(&e.snapshot(), &entry, "strict entry");
}

#[test]
fn applied_address_gates_and_measurements_fault_before_touching_state_or_simulator_limits() {
    for operation in ["h [64:0]", "%v = measure i8 [64]", "h [64:0] ctrl [65:0]"] {
        let module = load(&format!(
            "memory 512
data 65 = i8 1
func @probe(i1 %x) -> i1 {{
entry:
 store i8 99, 8
 {operation}
 ret %x
}}
func @main() -> i5 {{
entry:
 %q = qalloc 1
 %out = qalloc 1
 %fault = qalloc 5
 h %q
 x %fault[1]
 apply @probe(%q) -> %out, %fault budget 10
 %m = measure %fault
 ret %m
}}"
        ));
        for seed in 0..8 {
            let options = Options { max_configurations: Some(2), allow_region_faults: true, seed, ..bounded() };
            let mut e = executor(&module, "main", &[], options);
            assert_eq!(e.run(), Outcome::Returned(Some(2 ^ 8)), "{operation}, seed {seed}");
            assert!(e.snapshot().quantum.cells.is_empty());
            assert_eq!(e.snapshot().classical.memory[8], 0);
            assert_eq!(e.stats().nonzero_releases, 0);
        }
    }
}
