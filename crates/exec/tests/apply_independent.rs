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
#[ignore = "wrong strict fault: cell writes reorder faulting configurations before un-run"]
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
#[ignore = "history apply exposes the caller journal position, unlike branch enumeration"]
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
#[ignore = "returning an inner mark makes two history applies fail the promised self-inverse law"]
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
#[ignore = "diagnostic: compare catches F1/F2 while their ignored wrong-answer witnesses remain unfixed"]
fn compare_detects_the_executed_semantic_disagreements() {
    for (text, reversible) in [(CELL_FAULT_ORDER, false), (MARK_VALUE, true)] {
        let p = load(text);
        let mut e = executor(&p, "main", &[], Options { reversible, ..bounded() });
        e.set_apply_mode(ApplyMode::Compare);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| e.run()));
        assert!(panic.is_err(), "the reduced wrong-answer witness must be caught");
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
#[ignore = "compare omits the promised outer steps count; mutates and compiles only a scratch copy"]
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
