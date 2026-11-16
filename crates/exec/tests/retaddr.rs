//! `retaddr`: the code address a frame returns to, read from the live frames; the same for every call from
//! one site, different between sites, the entry and beyond defined; un-run like any value; inside a coherent
//! region the region's frames sit on the run's own (`docs/DESIGN.md`, "Return addresses").
mod common;

use bennett_exec::{Options, Outcome};
use bennett_ir::{ENTRY_RETURN_ADDRESS, FuncId, code_address};
use common::{assert_reverses, assert_same_state, bounded, executor, load, run};

const EXAMPLE: &str = include_str!("../../../examples/retaddr.bn");

#[test]
fn the_example_holds_every_property_and_reverses() {
    let module = load(EXAMPLE);
    let (outcome, _) = run(&module, "main", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(127)));
    assert_eq!(assert_reverses(&module, "main", &[], bounded()), outcome);
}

#[test]
fn each_step_back_restores_the_state_before_that_step() {
    let module = load(EXAMPLE);
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let mut before = vec![executor.snapshot()];
    while executor.step().is_none() {
        before.push(executor.snapshot());
    }
    assert_eq!(executor.outcome(), Some(Outcome::Returned(Some(127))));
    while let Some(expected) = before.pop() {
        assert!(executor.step_back().expect("reversible"));
        assert_same_state(&executor.snapshot(), &expected, &format!("after undoing step {}", before.len()));
    }
    assert!(!executor.step_back().expect("reversible"));
}

/// Positions count every instruction and terminator in block order, so a call in a later block is offset by
/// the earlier blocks' lengths plus their terminators.
const POSITIONS: &str = r#"
memory 512
func @here() -> i64 {
entry:
    %r = retaddr 0
    ret %r
}
func @deep() -> i64 {
entry:
    %r = retaddr 2
    ret %r
}
func @middle() -> i64 {
entry:
    %r = call @deep()
    ret %r
}
func @first() -> i64 {
entry:
    %x = add i64 1, 2
    br next
next:
    %y = add i64 %x, 3
    %r = call @here()
    ret %r
}
func @second() -> i64 {
entry:
    %pad = add i64 0, 0
    %r = call @middle()
    ret %r
}
func @outer() -> i64 {
entry:
    %r = call @second()
    ret %r
}
func @own() -> i64 {
entry:
    %r = retaddr 0
    ret %r
}
func @far() -> i64 {
entry:
    %r = retaddr 4000000000
    ret %r
}
"#;

#[test]
fn addresses_follow_the_documented_encoding() {
    let module = load(POSITIONS);
    let id = |name: &str| common::entry(&module, name);
    let returned = |name: &str| match run(&module, name, &[], bounded()).0 {
        Outcome::Returned(Some(value)) => value as u64,
        other => panic!("@{name}: {other:?}"),
    };
    // @first: `entry` has 2 positions (the add and the branch), then `next`'s add, then the call at 3.
    assert_eq!(returned("first"), code_address(id("first"), 4));
    assert_eq!(returned("first"), (u64::from(id("first").0) + 1) << 32 | 4);
    // Two levels below @deep: @outer's call of @second, at position 0.
    assert_eq!(returned("outer"), code_address(id("outer"), 1));
    // @second is the outermost frame here: its own return address, two levels below @deep.
    assert_eq!(returned("second"), ENTRY_RETURN_ADDRESS);
    assert_eq!(returned("own"), ENTRY_RETURN_ADDRESS);
    assert_eq!(returned("far"), 0);
    assert!(code_address(FuncId(0), 1) > u64::from(u32::MAX), "never a function's address");
}

/// A coherent region reads its own frames and, below them, the run's: the region callee returns to the
/// `apply` (position 4 of @main), on every branch.
const REGION: &str = r#"
memory 512
func @where(i1 %b) -> i1 {
entry:
    %r = retaddr 0
    %u = retaddr 1
    %v = retaddr 2
    %at_apply = icmp eq i64 %r, 8589934597
    %entry = icmp eq i64 %u, -4294967296
    %beyond = icmp eq i64 %v, 0
    %both = and i1 %at_apply, %entry
    %all = and i1 %both, %beyond
    ret %all
}
func @main() -> i1 {
entry:
    %q = qalloc 1
    h %q
    %out = qalloc 1
    %fault = qalloc 5
    apply @where(%q) -> %out, %fault budget 100
    %m = measure %out
    ret %m
}
"#;

#[test]
fn a_region_reads_the_frames_it_runs_on() {
    let module = load(REGION);
    assert_eq!(code_address(common::entry(&module, "main"), 5), 8_589_934_597);
    for seed in 0..4 {
        let (outcome, executor) = run(&module, "main", &[], Options { seed, ..bounded() });
        assert_eq!(outcome, Outcome::Returned(Some(1)), "seed {seed}");
        assert_eq!(executor.stats().region_faults, 0);
    }
    let unmeasured = REGION.replace("    %m = measure %out\n    ret %m", "    ret 0");
    assert_reverses(&load(&unmeasured), "main", &[], bounded());
}

#[test]
fn the_text_form_round_trips() {
    let module = load(POSITIONS);
    let text = bennett_ir::write(&module);
    assert!(text.contains("%r = retaddr 4000000000"), "{text}");
    assert_eq!(load(&text).functions, module.functions);
}
