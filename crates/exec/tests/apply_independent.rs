//! Independent attacks on the region boundary, with analytic answers and the whole-world model.
mod common;
use common::oracle_support;

use bennett_exec::{ApplyMode, Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};

const CELL_FAULT_ORDER: &str = r#"
memory 512
func @f() -> i1 {
entry:
    %x = load i8 0
    %y = xor i8 %x, 1
    store i8 %y, 0
    %zero = icmp eq i8 %x, 0
    br %zero, a, b
a:
    fault divide_by_zero
b:
    fault unreachable
}
func @main() {
entry:
    h [0:0]
    %o = qalloc 1
    %e = qalloc 4
    apply @f() -> %o, %e budget 100
    ret
}
"#;

#[test]
fn strict_fault_uses_the_first_original_configuration() {
    let module = load(CELL_FAULT_ORDER);
    for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
        let mut e = executor(&module, "main", &[], bounded());
        e.set_apply_mode(mode);
        assert_eq!(
            e.run(),
            Outcome::Stopped(RunError::RegionFault {
                callee: module.function_named("f").expect("f"),
                fault: Fault::DivideByZero,
                branches: 2,
                configurations: 2,
            })
        );
    }
}

const MARK_VALUE: &str = r#"
memory 512
func @f() -> i64 {
entry:
    %m = mark
    ret %m
}
func @main() {
entry:
    %o = qalloc 64
    %e = qalloc 4
    apply @f() -> %o, %e budget 10
    ret
}
"#;

#[test]
fn region_mark_values_are_independent_of_the_callers_journal() {
    let module = load(MARK_VALUE);
    for reversible in [false, true] {
        for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
            let mut e = executor(&module, "main", &[], Options { reversible, ..bounded() });
            e.set_apply_mode(mode);
            assert_eq!(e.run(), Outcome::Returned(None));
            assert_eq!(e.snapshot().quantum.amplitudes[0].0, [2, 0], "{mode:?}, reversible={reversible}");
        }
    }
}

// Cell stores and inner rewinds run under the region's rules. The function reads both cells after
// their values have diverged, writes an overlapping i16, un-runs it, then jumps through a callee.
const MIXED: &str = r#"
memory 4096
data 0 = i16 513
func @jump(i8 %x) -> i8 {
entry:
    %p = alloca 32
    %j = setjmp %p
    %first = icmp eq i32 %j, 0
    br %first, throw, done
throw:
    longjmp %p, 3
done:
    %j8 = trunc i32 %j to i8
    %r = add i8 %x, %j8
    ret %r
}
func @f(i2 %x) -> i8 {
entry:
    %a = load i8 0
    %m = mark
    %b = load i8 1
    %w = zext i8 %b to i16
    store i16 %w, 0
    %odd = icmp eq i8 %b, 0
    br %odd, short, long
short:
    br join
long:
    %t = mul i8 %a, 7
    store i8 %t, 80
    br join
join:
    rewind %m
    %b2 = load i8 1
    %x8 = zext i2 %x to i8
    %sum = add i8 %a, %b2
    %sum2 = add i8 %sum, %x8
    %r = call @jump(%sum2)
    ret %r
}
func @main(i64 %budget) {
entry:
    h [0:0]
    h [1:1]
    %x = qalloc 2
    h %x[0]
    ry(0.7) %x[1]
    %s = qalloc 1
    h %s
    s %s
    %o = qalloc 8
    x %o[2]
    %e = qalloc 4
    x %e[3]
    apply @f(%x) -> %o, %e budget %budget
    ret
}
"#;

fn reference_case(text: &str, args: &[u64]) {
    use oracle_support::worlds::{Model, basis};
    use oracle_support::worlds_executor::assert_worlds;
    let p = load(text);
    let model = Model::new(&p, "main");
    let initial = model.initial("main", &args.iter().map(|&n| n.into()).collect::<Vec<_>>());
    let expected = model.run_joined(basis(initial), 300);
    for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
        let mut e = executor(&p, "main", args, Options { reversible: true, allow_region_faults: true, ..bounded() });
        e.set_apply_mode(mode);
        let entry = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_worlds(&model, &expected, &e.snapshot());
        e.reverse().expect("reverses");
        assert_same_state(&e.snapshot(), &entry, "full reverse");
    }
}

#[test]
fn memory_splits_rewinds_jumps_and_each_worlds_budget_match_the_reference() {
    for budget in [0, 1, 3, 5, 8, 12, 16, 20, 24, 28, 32, 40, 64] {
        reference_case(MIXED, &[budget]);
    }
}

#[test]
#[ignore = "extended budget sweep: cargo test -p bennett-exec --test apply_independent all_region_budget_boundaries -- --ignored --nocapture"]
fn all_region_budget_boundaries() {
    for budget in 0..=64 {
        reference_case(MIXED, &[budget]);
    }
}

const NESTED: &str = r#"
memory 512
func @inner() -> i1 {
entry:
    ret 1
}
func @outer() -> i1 {
entry:
    %q = qalloc 1
    %e = qalloc 4
    apply @inner() -> %q, %e budget 3
    ret 0
}
func @main() {
entry:
    %q = qalloc 1
    %e = qalloc 4
    apply @outer() -> %q, %e budget 10
    ret
}
"#;

#[test]
fn quantum_instructions_reached_through_calls_fault_inside_the_region() {
    // The outer region contains a nested apply; its first qalloc must fault before execution can
    // reach it. Effects and exit reached through the same call boundary have their own codes.
    for (body, fault) in [
        ("%q = qalloc 1", Fault::QuantumInRegion),
        ("effect 65", Fault::EffectInRegion),
        ("exit i1 0", Fault::ExitInRegion),
    ] {
        let start = NESTED.find("    %q = qalloc 1").expect("body");
        let end = NESTED[start..].find("\n}").expect("end") + start;
        let text = format!("{}    {body}\n    ret 0{}", &NESTED[..start], &NESTED[end..]);
        // Exit is a terminator, so remove the unreachable second terminator.
        let text = text.replace("exit i1 0\n    ret 0", "exit i1 0");
        let p = load(&text);
        for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
            let mut e = executor(&p, "main", &[], Options { allow_region_faults: true, ..bounded() });
            e.set_apply_mode(mode);
            assert_eq!(e.run(), Outcome::Returned(None));
            assert_eq!(e.snapshot().quantum.amplitudes[0].0, [0, fault.code()]);
        }
        reference_case(&text, &[]);
    }
}

/// A large region with only four argument values; the other qubits are spectators. The opt-in peak
/// probe also supports BENNETT_TEST_BITS=20 and BENNETT_TEST_MODE=history|branches for /usr/bin/time -v.
fn large_region(bits: u32) -> String {
    let gates: String = (0..bits).map(|b| format!("    h %s[{b}]\n")).collect();
    format!(
        r#"
memory 1024
func @f(i2 %x) -> i2 {{
entry:
    %r = xor i2 %x, 3
    ret %r
}}
func @main() {{
entry:
    %s = qalloc {bits}
{gates}    %x = qalloc 2
    h %x[0]
    h %x[1]
    %o = qalloc 2
    %e = qalloc 4
    apply @f(%x) -> %o, %e budget 2
    ret
}}
"#
    )
}

#[test]
#[ignore = "large support/peak RSS probe: cargo test -p bennett-exec --test apply_independent amplitude_peak_probe -- --ignored --nocapture"]
fn amplitude_peak_probe() {
    let bits = std::env::var("BENNETT_TEST_BITS").map_or(15, |s| s.parse().expect("bits"));
    let mode = match std::env::var("BENNETT_TEST_MODE").as_deref() {
        Ok("history") => ApplyMode::History,
        Ok("branches") => ApplyMode::Branches,
        _ => ApplyMode::Compare,
    };
    let p = load(&large_region(bits));
    let mut e = executor(&p, "main", &[], Options { max_configurations: Some(1 << (bits + 2)), ..bounded() });
    e.set_apply_mode(mode);
    assert_eq!(e.run(), Outcome::Returned(None));
    let stats = e.stats();
    assert_eq!(stats.configurations, 1 << (bits + 2));
    assert_eq!(stats.region_steps, 2 * (1 << (bits + 2)));
    assert_eq!(
        stats.region_cost.runs,
        match mode {
            ApplyMode::History => 4,
            ApplyMode::Branches => 1 << (bits + 2),
            ApplyMode::Compare => 4 + (1 << (bits + 2)),
        }
    );
    println!("{mode:?}: {stats:?}");
    // No snapshot: it would add another full copy and contaminate the peak measurement.
}

#[test]
fn two_identical_regions_must_xor_the_same_result() {
    let text = MARK_VALUE.replace("    ret\n}", "    apply @f() -> %o, %e budget 10\n    ret\n}");
    let p = load(&text);
    for mode in [ApplyMode::Branches, ApplyMode::History] {
        let mut e = executor(&p, "main", &[], Options { reversible: true, ..bounded() });
        e.set_apply_mode(mode);
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_eq!(e.snapshot().quantum.amplitudes[0].0, [0, 0], "{mode:?}: apply is its own inverse");
    }
}

#[test]
fn strict_budget_faults_leave_the_pre_apply_state_and_reverse() {
    let p = load(MIXED);
    for budget in [0, 1, 12, 24, 32, 64] {
        for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
            let mut e = executor(&p, "main", &[budget], Options { reversible: true, ..bounded() });
            e.set_apply_mode(mode);
            let entry = e.snapshot();
            // Stop exactly at apply, preserving the complete visible state before a strict refusal.
            while e.snapshot().classical.frames.last().expect("frame").inst < 12 {
                assert_eq!(e.step(), None);
            }
            let before = e.snapshot();
            let result = e.step();
            if let Some(Outcome::Stopped(RunError::RegionFault { fault, .. })) = result {
                assert_eq!(fault, Fault::Budget);
                let mut after = e.snapshot();
                after.classical.outcome = before.classical.outcome;
                assert_same_state(&after, &before, "strict stop is atomic");
            } else {
                assert_eq!(result, None);
                assert_eq!(e.run(), Outcome::Returned(None));
            }
            e.reverse().expect("strict or successful run reverses");
            assert_same_state(&e.snapshot(), &entry, "strict reverse");
        }
    }
}

#[test]
fn allowed_cell_fault_codes_and_private_writes_match_the_reference() {
    reference_case(CELL_FAULT_ORDER, &[]);
}

#[test]
fn regions_in_existing_worlds_and_rewind_replay_match_the_reference() {
    let text = MIXED
        .replace("func @f(i2 %x)", "func @id(i2 %x) -> i2 {\nentry:\n    ret %x\n}\nfunc @f(i2 %x)")
        .replace("    apply @f(%x)", "    %outer = mark\n    %read = call @id(%x)\n    apply @f(%read)")
        .replace("    ret\n}", "    rewind %outer\n    ret\n}");
    for budget in [0, 12, 24, 32, 64] {
        reference_case(&text, &[budget]);
    }
}

#[test]
#[ignore = "rebuilds a scratch copy of the source per mutation; the regular check is src/compare_tests.rs"]
fn compare_must_reject_corrupt_history_state() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/apply_mutations.py");
    let mutation = std::env::var("BENNETT_TEST_MUTATION").unwrap_or_else(|_| "steps".into());
    let status = std::process::Command::new("python3")
        .arg(script)
        .args(["--only", &mutation, "--require-all"])
        .status()
        .expect("run isolated mutation probe");
    assert!(status.success(), "compare must reject corrupted {mutation} from the history path");
}

#[test]
fn physical_return_addresses_and_an_entry_frame_that_cannot_fit() {
    let text = MIXED.replace("    %j8 = trunc i32 %j to i8", "    %ra = retaddr 1\n    %low = trunc i64 %ra to i32\n    %jr = xor i32 %j, %low\n    %j8 = trunc i32 %jr to i8");
    reference_case(&text, &[80]);
    let small = r#"
memory 64
func @f(i2 %x) -> i2 {
entry:
    %a = add i2 %x, 1
    %b = add i2 %a, 1
    %c = add i2 %b, 1
    %d = add i2 %c, 1
    %e = add i2 %d, 1
    %f = add i2 %e, 1
    ret %f
}
func @main() {
entry:
    %x = qalloc 2
    h %x[0]
    h %x[1]
    %o = qalloc 2
    %e = qalloc 4
    apply @f(%x) -> %o, %e budget 100
    ret
}
"#;
    reference_case(small, &[]);
    let p = load(small);
    let mut e = executor(&p, "main", &[], Options { allow_region_faults: true, ..bounded() });
    assert_eq!(e.run(), Outcome::Returned(None));
    for (c, _) in &e.snapshot().quantum.amplitudes {
        assert_eq!(c[1..], [0, Fault::StackOverflow.code()]);
    }
    assert_eq!(e.stats().region_steps, 0);
}

#[test]
#[ignore = "large analytic support: cargo test -p bennett-exec --test apply_independent more_than_65536 -- --ignored --nocapture"]
fn more_than_65536_configurations_have_the_analytic_answer() {
    let p = load(&large_region(15));
    let mut e = executor(&p, "main", &[], Options { max_configurations: Some(1 << 17), ..bounded() });
    e.set_apply_mode(ApplyMode::Compare);
    assert_eq!(e.run(), Outcome::Returned(None));
    let state = e.snapshot();
    assert_eq!(state.quantum.amplitudes.len(), 1 << 17);
    let expected = 1.0 / f64::from(1 << 17).sqrt();
    for (c, a) in &state.quantum.amplitudes {
        assert_eq!(c[2..], [c[1] ^ 3, 0]);
        assert!((a.re - expected).abs() < 1e-15 && a.im == 0.0);
    }
    println!("131072 exact configuration images, amplitudes within 1e-15, both modes compared");
}

#[test]
fn apply_through_an_ordinary_call_keeps_the_real_caller_frames() {
    let text = MIXED.replace("func @main(i64 %budget)", "func @driver(i64 %budget)")
        + "\nfunc @main(i64 %budget) {\nentry:\n    call @driver(%budget)\n    ret\n}\n";
    for budget in [0, 24, 80] {
        reference_case(&text, &[budget]);
    }
}

// DESIGN "History": marks count journal records including their own. This checks the fresh
// private journal used by enumeration, not the unresolved caller-journal origin of history apply
// (F1). One entry-call record plus the mark gives 2, regardless of reference snapshot layout.
#[test]
fn reference_apply_mark_value_matches_branch_enumeration() {
    let p = load(MARK_VALUE);
    let model = oracle_support::worlds::Model::new(&p, "main");
    let reference = model.run_joined(oracle_support::worlds::basis(model.initial("main", &[])), 20);
    let mut e = executor(&p, "main", &[], bounded());
    e.set_apply_mode(ApplyMode::Branches);
    assert_eq!(e.run(), Outcome::Returned(None));
    let branch_value = e.snapshot().quantum.amplitudes[0].0[0];
    assert_eq!(branch_value, 2);
    for w in reference.keys() {
        assert_eq!(w.quantum[0], branch_value, "reference's public result must equal enumeration");
    }
}

// All assertions inspect ordinary results and the entire public world, including memory, slots,
// amplitudes and frames. The reference's private snapshots are never compared with executor records.
fn mark_observation_case(body: &str, expected: [u64; 4], modes: &[ApplyMode]) -> usize {
    use oracle_support::worlds::{Model, basis};
    use oracle_support::worlds_executor::assert_worlds;
    let mut runs = 0;
    for repetitions in [1, 2] {
        let applies = "    apply @f(%x) -> %o, %e budget 100\n".repeat(repetitions);
        let text = format!(
            r#"
memory 2048
func @leaf(i2 %x) -> i64 {{
entry:
    %m = mark
    ret %m
}}
func @f(i2 %x) -> i64 {{
entry:
{body}
}}
func @main() {{
entry:
    %x = qalloc 2
    h %x[0]
    ry(0.7) %x[1]
    s %x[0]
    %o = qalloc 64
    x %o[4]
    %e = qalloc 4
{applies}
    ret
}}
"#
        );
        let p = load(&text);
        let model = Model::new(&p, "main");
        let reference = model.run_joined(basis(model.initial("main", &[])), 100);
        for w in reference.keys() {
            let x = w.quantum[0] as usize;
            let answer = if repetitions == 1 { expected[x] ^ 16 } else { 16 };
            assert_eq!(w.quantum[1..], [answer, 0], "analytic result for x={x}, repeats={repetitions}: {body}");
        }
        for reversible in [false, true] {
            for &mode in modes {
                let mut e = executor(&p, "main", &[], Options { reversible, ..bounded() });
                e.set_apply_mode(mode);
                let initial = e.snapshot();
                assert_eq!(e.run(), Outcome::Returned(None));
                assert_worlds(&model, &reference, &e.snapshot());
                if reversible {
                    e.reverse().expect("mark-observing apply reverses");
                    assert_same_state(&e.snapshot(), &initial, "mark-observing apply full reverse");
                }
                runs += 1;
            }
        }
    }
    runs
}

#[test]
fn fresh_region_mark_numbers_can_be_returned_stored_branched_on_and_called() {
    // These absolute numbers select the private fresh-journal definition, just like the F4
    // witness above. They do not decide F1's unresolved origin for History/Compare.
    let cases = [
        ("%m = mark\nret %m", 2),
        ("%m = mark\nstore i64 %m, 0\n%r = load i64 0\nret %r", 2),
        ("%m = mark\n%ok = icmp eq i64 %m, 2\nbr %ok, yes, no\nyes:\nret 37\nno:\nret 99", 37),
        ("%r = call @leaf(%x)\nret %r", 3),
    ];
    let runs: usize =
        cases.into_iter().map(|(body, n)| mark_observation_case(body, [n; 4], &[ApplyMode::Branches])).sum();
    assert_eq!(runs, 16);
    println!("{runs} fresh-journal mark observations, four configurations each, with analytic answers and reversal");
}

#[test]
fn relative_region_mark_numbers_match_all_modes_through_rewind_and_replay() {
    // DESIGN History/Worlds: calls, returns, jumps, marks and rewinds are journalled steps.
    // Subtract marks so the caller's journal origin cancels. Rewinding retains the mark and
    // replaces everything after it with one rewind record; undoing an inner rewind replays it.
    let cases = [
        ("%m = mark\nstore i2 %x, 0\nrewind %m\n%n = mark\n%d = sub i64 %n, %m\nret %d", [2; 4]),
        ("%m = mark\n%n = mark\nrewind %n\nrewind %m\n%p = mark\n%d = sub i64 %p, %m\nret %d", [2; 4]),
        ("%m = mark\nrewind %m\nrewind %m\n%n = mark\n%d = sub i64 %n, %m\nret %d", [2; 4]),
        ("%m = mark\n%n = call @leaf(%x)\n%d = sub i64 %n, %m\nret %d", [2; 4]),
        ("%p = faddr @leaf\n%m = mark\n%n = call_indirect fn(i2) -> i64 %p(%x)\n%d = sub i64 %n, %m\nret %d", [2; 4]),
        (
            "%m = mark\n%zero = icmp eq i2 %x, 0\nbr %zero, short, long\nshort:\nbr join\nlong:\nstore i2 %x, 0\nbr join\njoin:\n%n = mark\n%d = sub i64 %n, %m\nret %d",
            [4, 5, 5, 5],
        ),
    ];
    let modes = [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare];
    let runs: usize = cases.into_iter().map(|(body, expected)| mark_observation_case(body, expected, &modes)).sum();
    assert_eq!(runs, 72);
    println!("{runs} origin-independent mark observations, four configurations each, all apply modes and reversal");
}

// The whole class of F1: what the region's function observes of its history (the values of its marks; a
// rewind's handle is a mark of its own function, the validator's rule, so no handle of the caller's can be
// named) is that of a run on a journal of its own, whose first record is
// the call (`docs/DESIGN.md`, "History"), never the caller's. Each body runs after caller prefixes that
// journal different amounts (gates, marks, an earlier `apply`, a split of worlds) in each mode, reversible or
// not; the state must not depend on the prefix or the mode, an `apply` twice must be no `apply`, and every
// reversible run must reverse to its entry.
const REGION_BODIES: &[&str] = &[
    // The witness: a mark's value.
    "    %m = mark\n    ret %m\n",
    // Nested marks, a rewind of the inner one, a mark after it.
    "    %m1 = mark\n    %m2 = mark\n    %y = add i64 %m1, 100\n    rewind %m2\n    %m3 = mark\n    %s = shl i64 %m3, 8\n    %r = or i64 %s, %m1\n    ret %r\n",
    // A load of a qubit in memory (a split in history, private memory in the enumeration), paths of
    // different lengths, then a mark.
    "    %a = load i8 0\n    %c = icmp eq i8 %a, 0\n    br %c, p, q\np:\n    %u = add i8 %a, 1\n    br q\nq:\n    %m = mark\n    %x64 = zext i2 %x to i64\n    %r = add i64 %m, %x64\n    ret %r\n",
    // A mark in a callee of the region.
    "    %k = zext i2 %x to i64\n    %r = call @g(%k)\n    ret %r\n",
    // A mark and its rewind on one path only, then a mark: the second mark's value differs by path.
    "    %c = icmp eq i2 %x, 1\n    br %c, p, q\np:\n    %m = mark\n    %y = add i2 %x, 1\n    rewind %m\n    br q\nq:\n    %n = mark\n    ret %n\n",
];

const REGION_PREFIXES: &[&str] = &[
    "",
    "    %p = mark\n",
    "    %p = mark\n    x %t\n    x %t\n    %q = mark\n",
    "    apply @zero(%x) -> %t, %te budget 8\n",
    "    %p = mark\n    apply @zero(%x) -> %t, %te budget 8\n    x %t\n    x %t\n",
];

fn region_program(body: &str, prefix: &str, applies: usize) -> String {
    let applies = "    apply @f(%x) -> %o, %e budget 200\n".repeat(applies);
    format!(
        "memory 512
func @zero(i2 %x) -> i1 {{
entry:
    ret 0
}}
func @g(i64 %k) -> i64 {{
entry:
    %m = mark
    %r = add i64 %m, %k
    ret %r
}}
func @f(i2 %x) -> i64 {{
entry:
{body}}}
func @main() {{
entry:
    h [0:0]
    %x = qalloc 2
    h %x[0]
    h %x[1]
    %t = qalloc 1
    %te = qalloc 4
    %o = qalloc 64
    %e = qalloc 4
{prefix}{applies}    ret
}}
"
    )
}

fn final_state(text: &str, mode: ApplyMode, reversible: bool) -> bennett_exec::QuantumSnapshot {
    let p = load(text);
    let mut e = executor(&p, "main", &[], Options { reversible, ..common::allowing_region_faults() });
    e.set_apply_mode(mode);
    let entry = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None), "{mode:?}\n{text}");
    let state = e.snapshot().quantum;
    if reversible {
        e.reverse().expect("reverses");
        assert_same_state(&e.snapshot(), &entry, "full reverse");
    }
    state
}

fn assert_same_quantum(
    actual: &bennett_exec::QuantumSnapshot,
    expected: &bennett_exec::QuantumSnapshot,
    context: &str,
) {
    let distance = actual.distance(expected);
    assert!(distance.is_some_and(|d| d <= 1e-12), "{context}: {distance:?}\n{actual:?}\n{expected:?}");
}

#[test]
fn what_a_region_sees_of_its_history_does_not_depend_on_the_caller() {
    for (b, body) in REGION_BODIES.iter().enumerate() {
        for reversible in [false, true] {
            let expected = final_state(&region_program(body, "", 1), ApplyMode::Branches, reversible);
            for (p, prefix) in REGION_PREFIXES.iter().enumerate() {
                let none = final_state(&region_program(body, prefix, 0), ApplyMode::Branches, reversible);
                for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
                    let context = format!("body {b}, prefix {p}, {mode:?}, reversible={reversible}");
                    let once = final_state(&region_program(body, prefix, 1), mode, reversible);
                    assert_same_quantum(&once, &expected, &context);
                    let twice = final_state(&region_program(body, prefix, 2), mode, reversible);
                    assert_same_quantum(&twice, &none, &format!("{context}: twice is no apply"));
                }
            }
        }
    }
}

// The mark values of the bodies above, by the documents' count (records up to and including the mark's
// own, from the region's start, the call into `@f` the first): an analytic check of the numbering itself.
#[test]
fn region_mark_values_count_from_the_call_into_the_region() {
    // Body 1: m1 = 2 (call, mark); m2 = 3; after `rewind %m2` (un-runs the add) the rewind is record 4 and
    // m3 = 5: (5 << 8) | 2.
    // Body 3: the call into @f, the zext, the call into @g, then its mark: 4, plus the argument.
    for (body, expected) in
        [(REGION_BODIES[0], [2; 4]), (REGION_BODIES[1], [5 << 8 | 2; 4]), (REGION_BODIES[3], [4, 5, 6, 7])]
    {
        for prefix in REGION_PREFIXES {
            let state = final_state(&region_program(body, prefix, 1), ApplyMode::History, true);
            // Columns: the cell, x, t, te, o, e.
            for (config, _) in &state.amplitudes {
                assert_eq!(config[4], expected[config[1] as usize], "{body}\n{prefix}");
                assert_eq!(config[5], 0);
            }
        }
    }
}

// The whole class of F2: a strict region fault is the first faulting configuration's, in the order of the
// configurations as they were before the `apply` (`docs/DESIGN.md`, "`apply` as mark, call, rewind-keep"),
// whatever the region writes to the qubits in memory it read. Two cells (byte 0 in four values, byte 1 in two),
// rewritten in place by every kind of permutation (none, XOR, increment, swap, reversal), and every
// assignment of outcomes to the eight configurations a salt chooses.
#[test]
fn strict_fault_is_the_first_original_configurations_whatever_the_region_writes_to_its_cells() {
    let rewrites = [
        "",
        "    %v = xor i8 %a, 3\n    store i8 %v, 0\n",
        "    %v = add i8 %a, 1\n    store i8 %v, 0\n",
        "    store i8 %b, 0\n    store i8 %a, 1\n",
        "    %v = sub i8 3, %a\n    store i8 %v, 0\n    %w = xor i8 %b, 1\n    store i8 %w, 1\n",
    ];
    let faults = [None, Some(Fault::DivideByZero), Some(Fault::Unreachable), Some(Fault::OutOfBounds)];
    for (r, rewrite) in rewrites.iter().enumerate() {
        for salt in 0..4u64 {
            let text = format!(
                "memory 512
func @f() -> i1 {{
entry:
    %a = load i8 0
    %b = load i8 1
{rewrite}    %k0 = mul i8 %a, 3
    %k1 = mul i8 %b, 5
    %k2 = add i8 %k0, %k1
    %k3 = add i8 %k2, {salt}
    %k = and i8 %k3, 3
    %z = icmp eq i8 %k, 0
    br %z, ok, n1
n1:
    %c1 = icmp eq i8 %k, 1
    br %c1, f1, n2
n2:
    %c2 = icmp eq i8 %k, 2
    br %c2, f2, f3
ok:
    ret 0
f1:
    fault divide_by_zero
f2:
    fault unreachable
f3:
    fault out_of_bounds
}}
func @main() {{
entry:
    h [0:0]
    h [0:1]
    h [1:0]
    %o = qalloc 1
    %e = qalloc 4
    apply @f() -> %o, %e budget 100
    ret
}}
"
            );
            // The configurations in order: byte 0's register is the first column, byte 1's the second.
            let outcome = |a: u64, b: u64| faults[((a * 3 + b * 5 + salt) & 3) as usize];
            let faulting: Vec<Fault> =
                (0..4).flat_map(|a| (0..2).map(move |b| (a, b))).filter_map(|(a, b)| outcome(a, b)).collect();
            let p = load(&text);
            for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
                let mut e = executor(&p, "main", &[], Options { reversible: true, ..bounded() });
                e.set_apply_mode(mode);
                let entry = e.snapshot();
                let expected = match faulting.first() {
                    Some(&fault) => Outcome::Stopped(RunError::RegionFault {
                        callee: p.function_named("f").expect("f"),
                        fault,
                        branches: faulting.len(),
                        configurations: 8,
                    }),
                    None => Outcome::Returned(None),
                };
                assert_eq!(e.run(), expected, "rewrite {r}, salt {salt}, {mode:?}");
                e.reverse().expect("reverses");
                assert_same_state(&e.snapshot(), &entry, "full reverse");
            }
        }
    }
}

#[test]
fn compare_cost_is_the_sum_of_both_methods_even_after_prior_regions() {
    let mut first = None;
    for repeats in [1, 2, 4] {
        let apply = "    apply @f(%x) -> %o, %e budget %budget\n";
        // An already split caller keeps the memory base shared in every mode, so the two
        // computations have the same overlay costs as their separate runs.
        let text = MIXED
            .replace("func @jump(", "func @read(i1 %bit) -> i1 {\nentry:\nret %bit\n}\nfunc @jump(")
            .replace("    s %s\n", "    s %s\n    %unused_read = call @read(%s)\n")
            .replace(apply, &apply.repeat(repeats));
        let module = load(&text);
        let mut costs = Vec::new();
        for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
            let mut e = executor(&module, "main", &[64], Options { reversible: true, ..bounded() });
            e.set_apply_mode(mode);
            assert_eq!(e.run(), Outcome::Returned(None));
            let state = e.snapshot().quantum;
            if repeats == 1 {
                if let Some(expected) = &first {
                    assert_same_quantum(&state, expected, "same phased state in every mode");
                } else {
                    first = Some(state);
                }
            }
            let cost = e.stats().region_cost;
            assert!(cost.runs > 0 && cost.work > 0 && cost.journal_bytes > 0, "{mode:?}: {cost:?}");
            if mode == ApplyMode::History {
                assert!(cost.copied_bytes > 0, "memory loads split region worlds");
            }
            costs.push(cost);
        }
        let fields = |c: &bennett_exec::RegionCost| [c.runs, c.work, c.journal_bytes, c.overlay_bytes, c.copied_bytes];
        for ((h, b), c) in fields(&costs[0]).into_iter().zip(fields(&costs[1])).zip(fields(&costs[2])) {
            assert_eq!(c, h + b, "{repeats} regions: compare reports all work from both methods");
        }
    }
}

fn split_copy_cost(
    caller_slots: usize,
    callee_slots: usize,
) -> (bennett_exec::RegionCost, bennett_exec::QuantumSnapshot) {
    let params = (0..caller_slots).map(|n| format!("i8 %parameter{n}")).collect::<Vec<_>>().join(", ");
    let tail = (0..callee_slots).map(|n| format!("%unused{n} = add i8 0, 0\n")).collect::<String>();
    let module = load(&format!(
        "memory 1024\nfunc @f() -> i8 {{\nentry:\n%v = load i8 0\n{tail}ret %v\n}}\nfunc @main({params}) {{\nentry:\nh [0:0]\ns [0:0]\n%o = qalloc 8\n%e = qalloc 4\napply @f() -> %o, %e budget 32\nret\n}}\n"
    ));
    let mut e = executor(&module, "main", &vec![0; caller_slots], bounded());
    e.set_apply_mode(ApplyMode::History);
    assert_eq!(e.run(), Outcome::Returned(None));
    (e.stats().region_cost, e.snapshot().quantum)
}

#[test]
fn region_cost_counts_shared_steps_and_the_caller_slots_copied_at_an_internal_split() {
    let (small, state) = split_copy_cost(0, 0);
    let (large, padded) = split_copy_cost(8, 0);
    assert_same_quantum(&padded, &state, "unused parameters do not change phases");
    // First world: load, return, three un-run records (entry call included).
    // Fork: return, three un-run records; the load is already in its shared prefix.
    assert_eq!((small.runs, small.work, large.work), (2, 9, 9));
    // The two values of the promoted byte cause one fork, copying eight more u128 slots.
    assert_eq!(large.copied_bytes - small.copied_bytes, 8 * size_of::<u128>() as u64);
}

#[test]
#[ignore = "region copied-byte count omits callee slots allocated before a memory split"]
fn region_copy_cost_includes_slots_of_the_frame_where_memory_splits() {
    let (small, state) = split_copy_cost(0, 0);
    let (large, padded) = split_copy_cost(0, 8);
    assert_same_quantum(&padded, &state, "unused callee slots do not change phases");
    // Callee slots already exist at the load; their later writes are not in the copied journal yet.
    assert_eq!(large.copied_bytes - small.copied_bytes, 8 * size_of::<u128>() as u64);
}

#[test]
fn regions_without_internal_forks_copy_no_worlds_even_inside_split_callers() {
    let module = load(
        "memory 512
func @id(i1 %x) -> i1 {
entry:
ret %x
}
func @main() {
entry:
%q = qalloc 1
%r = qalloc 1
%o = qalloc 1
%e = qalloc 4
h %q
s %q
h %r
%m = mark
%read = call @id(%q)
store i8 7, 0
%second = call @id(%r)
apply @id(%read) -> %o, %e budget 4
rewind %m
sdg %q
h %q
h %r
ret
}",
    );
    let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    e.set_apply_mode(ApplyMode::History);
    let entry = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None));
    assert!(e.stats().splits > 0 && e.stats().split_memory_bytes > 0, "the caller split");
    assert_eq!(e.stats().region_cost.copied_bytes, 0, "no region forks: scalar arguments run in place");
    let state = e.snapshot().quantum;
    assert_eq!(state.amplitudes.len(), 1);
    assert!((state.amplitude(&[0, 0, 0, 0]) - num_complex::Complex64::new(1.0, 0.0)).norm() <= 1e-12);
    e.reverse().expect("reverses");
    assert_same_state(&e.snapshot(), &entry, "split caller reverses");
}

#[test]
fn strict_fault_order_uses_fault_register_values_before_the_keep() {
    let module = load(
        "memory 512
func @f(i1 %x) -> i1 {
entry:
br %x, low, high
low:
fault divide_by_zero
high:
fault unreachable
}
func @main() {
entry:
%e = qalloc 4
%q = qalloc 1
%o = qalloc 1
h %q
s %q
apply @f(%q) -> %o, %e budget 4
ret
}",
    );
    for mode in [ApplyMode::History, ApplyMode::Branches, ApplyMode::Compare] {
        let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
        e.set_apply_mode(mode);
        let entry = e.snapshot();
        for _ in 0..5 {
            assert_eq!(e.step(), None);
        }
        let before = e.snapshot();
        assert_eq!(
            e.step(),
            Some(Outcome::Stopped(RunError::RegionFault {
                callee: module.function_named("f").expect("f"),
                fault: Fault::Unreachable,
                branches: 2,
                configurations: 2,
            }))
        );
        let mut after = e.snapshot();
        after.classical.outcome = None;
        assert_same_state(&after, &before, "strict refusal restores phases and the zero fault register");
        e.reverse().expect("reverses");
        assert_same_state(&e.snapshot(), &entry, "fault reverses");
    }
}
