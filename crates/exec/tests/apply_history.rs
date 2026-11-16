//! `apply` as mark, call, rewind-keep (`docs/DESIGN.md`, "`apply` as mark, call, rewind-keep"): equal to
//! branch enumeration on the examples, and what each way costs. In compare mode the executor computes
//! every `apply` both ways from the same state and stops at any difference (result, the world's state and
//! journal, amplitudes, the promised counts); the whole suites are also run in that mode (`HANDOFF.md`).

mod common;

use bennett_exec::{ApplyMode, Options, Outcome, RegionCost, Stats};
use common::{allowing_region_faults, assert_same_state, bounded, executor, load};

const THESIS: &str = include_str!("../../../examples/thesis.bn");
const FAULT: &str = include_str!("../../../examples/fault.bn");
const GROVER: &str = include_str!("../../../examples/grover.bn");

/// Runs `name(args)` in `mode`, reversibly, and reverses it (unless it measured); returns the outcome,
/// the counts after the forward run, and the state then.
fn run_in(
    module: &bennett_ir::ValidModule,
    name: &str,
    args: &[u64],
    options: Options,
    mode: ApplyMode,
) -> (Outcome, Stats, bennett_exec::Snapshot) {
    let mut executor = executor(module, name, args, Options { reversible: true, ..options });
    executor.set_apply_mode(mode);
    let at_entry = executor.snapshot();
    let outcome = executor.run();
    let (stats, state) = (executor.stats(), executor.snapshot());
    match executor.reverse() {
        Ok(_) => assert_same_state(&executor.snapshot(), &at_entry, &format!("@{name}{args:?} reversed")),
        Err(bennett_exec::ReverseError::PastMeasurement) => {}
        Err(error) => panic!("@{name}{args:?}: {error}"),
    }
    (outcome, stats, state)
}

/// Every entry of the three examples that use `apply`, over 16 seeds, strict and allowing region faults:
/// compare mode passes, and history and branches give the same outcome, state (measured outcomes
/// included, so the same random draws), promised counts and reversal.
#[test]
fn apply_by_history_equals_branch_enumeration_on_the_examples() {
    let mut runs = 0;
    for (text, entries) in [
        (THESIS, &[("entangled", None), ("main", None)][..]),
        (GROVER, &[("searched", Some(0)), ("searched", Some(1)), ("searched", Some(2)), ("main", None)][..]),
        (FAULT, &(0..16).map(|n| ("main", Some(n))).collect::<Vec<_>>()[..]),
    ] {
        let module = load(text);
        for &(name, arg) in entries {
            let args: Vec<u64> = arg.into_iter().collect();
            for seed in 0..16 {
                for options in [bounded(), allowing_region_faults()] {
                    let options = Options { seed, ..options };
                    let (outcome, _, _) = run_in(&module, name, &args, options, ApplyMode::Compare);
                    let (h_outcome, h, h_state) = run_in(&module, name, &args, options, ApplyMode::History);
                    let (b_outcome, b, b_state) = run_in(&module, name, &args, options, ApplyMode::Branches);
                    let context = format!("@{name}{args:?} seed {seed}");
                    assert_eq!((h_outcome, b_outcome), (outcome, outcome), "{context}");
                    assert_same_state(&h_state, &b_state, &context);
                    assert_eq!(
                        (h.steps, h.region_steps, h.region_faults, h.region_fault_weight, h.journal_bytes),
                        (b.steps, b.region_steps, b.region_faults, b.region_fault_weight, b.journal_bytes),
                        "{context}"
                    );
                    runs += 1;
                }
            }
        }
    }
    assert_eq!(runs, (2 + 4 + 16) * 16 * 2);
}

/// The known exponential inside a region, and the region over spectators: a callee that cleans up at
/// every level (marks, calls the next level, rewinds) or leaves its callees' history open, applied
/// over a 2-qubit argument in superposition with one spectator qubit (8 configurations, 4 argument
/// values). Both ways do the same work per run, which doubles per level when cleaning up (un-running a
/// rewind runs its region again, whether by the keep's un-run or by the enumeration's undo) and grows by
/// a constant when the history is open; the idiom runs once per argument value (4 worlds) where the
/// enumeration runs once per configuration (8).
const LEVELS: &str = r#"
memory 4096

func @work(i8 %d) -> i8 {
entry:
    %a = add i8 %d, 1
    %b = mul i8 %a, 3
    ret %b
}

; Cleans up after every level: marks, calls the next level, rewinds (a rewind in a region keeps nothing).
func @clean(i8 %d) -> i8 {
entry:
    %w = call @work(%d)
    %h = mark
    %leaf = icmp eq i8 %d, 0
    br %leaf, done, deeper
deeper:
    %d1 = sub i8 %d, 1
    %ignored = call @clean(%d1)
    br done
done:
    rewind %h
    ret %w
}

; The same calls with the callees' history left open until the region's own un-run.
func @open(i8 %d) -> i8 {
entry:
    %w = call @work(%d)
    %leaf = icmp eq i8 %d, 0
    br %leaf, done, deeper
deeper:
    %d1 = sub i8 %d, 1
    %ignored = call @open(%d1)
    br done
done:
    ret %w
}

func @region_clean(i2 %x, i8 %d) -> i8 {
entry:
    %w = call @clean(%d)
    %wide = zext i2 %x to i8
    %r = add i8 %w, %wide
    ret %r
}

func @region_open(i2 %x, i8 %d) -> i8 {
entry:
    %w = call @open(%d)
    %wide = zext i2 %x to i8
    %r = add i8 %w, %wide
    ret %r
}

func @top_clean(i8 %d) {
entry:
    %x = qalloc 2
    %s = qalloc 1
    h %x[0]
    h %x[1]
    h %s[0]
    %out = qalloc 8
    %e = qalloc 4
    apply @region_clean(%x, %d) -> %out, %e budget 100000000
    ret
}

func @top_open(i8 %d) {
entry:
    %x = qalloc 2
    %s = qalloc 1
    h %x[0]
    h %x[1]
    h %s[0]
    %out = qalloc 8
    %e = qalloc 4
    apply @region_open(%x, %d) -> %out, %e budget 100000000
    ret
}
"#;

#[test]
fn what_each_way_of_computing_apply_costs_on_the_known_exponential() {
    let module = load(LEVELS);
    let cost = |name: &str, d: u64, mode: ApplyMode| -> (RegionCost, u64) {
        let (outcome, stats, _) = run_in(&module, name, &[d], Options { budget: None, ..bounded() }, mode);
        assert_eq!(outcome, Outcome::Returned(None), "@{name}({d})");
        (stats.region_cost, stats.region_steps)
    };
    println!("level\tway\thistory runs\twork\tjournal bytes\tcopied bytes\tbranches runs\twork\tjournal bytes");
    let mut rows = Vec::new();
    for name in ["top_clean", "top_open"] {
        for d in 0..=8 {
            let (h, h_steps) = cost(name, d, ApplyMode::History);
            assert_eq!(h.copied_bytes, 0, "argument parts run in place; this region never forks on memory");
            let (b, b_steps) = cost(name, d, ApplyMode::Branches);
            cost(name, d, ApplyMode::Compare);
            assert_eq!(h_steps, b_steps, "@{name}({d}): region steps per configuration");
            assert_eq!((h.runs, b.runs), (4, 8), "@{name}({d}): a world per argument value, a run per configuration");
            assert_eq!(2 * h.work, b.work, "@{name}({d}): the same work per run");
            println!(
                "{d}\t{name}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                h.runs, h.work, h.journal_bytes, h.copied_bytes, b.runs, b.work, b.journal_bytes
            );
            rows.push((name, d, h.work / h.runs));
        }
    }
    let per_run = |name: &str, d: u64| rows.iter().find(|row| row.0 == name && row.1 == d).expect("measured").2;
    for d in 1..=8 {
        let (c0, c1) = (per_run("top_clean", d - 1), per_run("top_clean", d));
        assert!(c1 >= 2 * c0 && c1 <= 3 * c0, "cleaning up: level {d} costs {c1} per run, level {} {c0}", d - 1);
        let (o0, o1) = (per_run("top_open", d - 1), per_run("top_open", d));
        assert_eq!(o1 - o0, per_run("top_open", 1) - per_run("top_open", 0), "open history: constant per level");
    }
    assert_eq!((per_run("top_clean", 8), per_run("top_open", 8)), CLEAN_AND_OPEN_AT_8, "recorded in docs/DESIGN.md");
}

/// Work per run (forward, rewound, run again and un-run steps) at depth 8, the same both ways.
const CLEAN_AND_OPEN_AT_8: (u64, u64) = (14_311, 183);

#[test]
fn environment_selects_each_apply_mode_and_compare_appends_its_tally() {
    const CHILD: &str = "BENNETT_TEST_APPLY_MODE_CHILD";
    const NAME: &str = "environment_selects_each_apply_mode_and_compare_appends_its_tally";
    if let Ok(mode) = std::env::var(CHILD) {
        let expected = match mode.as_str() {
            "unset" => None,
            "history" => Some(ApplyMode::History),
            "branches" => Some(ApplyMode::Branches),
            "compare" => Some(ApplyMode::Compare),
            _ => unreachable!("parent's cases"),
        };
        assert_eq!(ApplyMode::from_env(), expected);
        if mode == "compare" {
            let module = load(
                "memory 128\nfunc @f() -> i1 {\nentry:\nret 1\n}\nfunc @main() {\nentry:\n%o = qalloc 1\n%e = qalloc 4\napply @f() -> %o, %e budget 4\nret\n}",
            );
            assert_eq!(run_in(&module, "main", &[], bounded(), ApplyMode::Compare).0, Outcome::Returned(None));
        }
        return;
    }
    let tally = std::env::temp_dir().join(format!("bennett-apply-tally-{}", std::process::id()));
    std::fs::write(&tally, "existing\n").expect("initial tally");
    for mode in ["unset", "history", "branches", "compare"] {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child.args(["--exact", NAME]).env(CHILD, mode).env_remove("BENNETT_APPLY").env_remove("BENNETT_APPLY_TALLY");
        if mode != "unset" {
            child.env("BENNETT_APPLY", mode);
        }
        if mode == "compare" {
            child.env("BENNETT_APPLY_TALLY", &tally);
        }
        let output = child.output().expect("isolated environment test");
        assert!(output.status.success(), "{mode}: {}", String::from_utf8_lossy(&output.stderr));
    }
    let written = std::fs::read_to_string(&tally).expect("tally");
    std::fs::remove_file(tally).expect("remove tally");
    assert!(
        written.starts_with("existing\n") && written.len() > "existing\n".len(),
        "each comparison appends its costs"
    );
}
