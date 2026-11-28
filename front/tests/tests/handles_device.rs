//! The milestone programs of the handle interface on the device (`examples/handles/`: the thesis program, Grover
//! over a C predicate, Shor for 15), each linked with musl by `rt/musl/link.py` through `bennett-cc` and
//! `bennett-front` and run by the simulator, at `-O0` and `-O2`. As natively (`handles_native.rs`), a measuring build
//! gives every budget name its own unbounded value, the device's step report (`docs/HANDLES.md`, section 7, D16)
//! gives the most steps each `apply` used, and the result build gives each name that count: the result runs print
//! what the measuring runs print, seed by seed; one step less ends an input at the budget. Both levels print the same, seed by seed, and what they print is in the native stand-in's
//! comparison class (`docs/HANDLES.md`, section 7): the same result lines where the stand-in's are certain, and
//! outcomes the stand-in gives a nonzero probability where they are random. Every `apply` of these programs runs on
//! the simulator's direct evaluation; the counter refuses `apply` to a block by name until the gate emitter takes
//! it (D17), so the Toffoli and qubit counts are not measured here.
use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, clang, out_dir, root};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

const LEVELS: [(&str, &[&str]); 2] = [("O0", CLANG_FLAGS), ("O2", CLANG_O2_FLAGS)];
const SEEDS: [u64; 3] = [1, 2, 3];
/// A bound on the steps of a whole run, far above the programs' (Shor's eight attempts take about 10^7).
const RUN_BUDGET: u64 = 2_000_000_000;
/// Basis configurations. The simulator keeps a released register's qubits as unnamed columns (the partial trace),
/// so Shor's residue register, released entangled with the exponent register in each attempt, multiplies the
/// configurations by its 4 values per attempt: at most 256 in the first attempt, 4^(k-1) 256 in attempt k
/// (measured: 4,096 in the third). This allows five attempts; the seeds below need at most three.
const MAX_CONFIGURATIONS: usize = 1 << 16;

fn examples() -> PathBuf {
    root().join("examples/handles")
}

fn musl_built() {
    static BUILD: Once = Once::new();
    BUILD.call_once(|| {
        let build = Command::new("python3").arg(root().join("rt/musl/build.py")).output().expect("musl build");
        assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    });
}

/// `source` linked with musl at `flags` with the budget definitions `budgets`: the device program.
fn link(source: &Path, name: &str, flags: &[&str], budgets: &[(&str, String)]) -> bennett_ir::ValidModule {
    musl_built();
    let file = out_dir().join(format!("{name}.bn"));
    let mut command = Command::new("python3");
    command.arg(root().join("rt/musl/link.py")).arg(source).arg("-o").arg(&file);
    let include = format!("-I{}", examples().display());
    let defines: Vec<String> = budgets.iter().map(|(n, v)| format!("-DBQ_BUDGET_{n}={v}")).collect();
    for flag in flags.iter().copied().chain([include.as_str()]).chain(defines.iter().map(String::as_str)) {
        command.arg(format!("--clang-flag={flag}"));
    }
    let linked = command.output().expect("musl link");
    assert!(linked.status.success(), "{name}: {}", String::from_utf8_lossy(&linked.stderr));
    bennett_ir::load(&std::fs::read_to_string(&file).expect("device text")).expect("valid device program")
}

/// One run: the outcome, the console, and per `apply` (budget, inputs, largest count, inputs at the budget).
#[derive(Debug, Clone, PartialEq)]
struct Run {
    outcome: Outcome,
    console: String,
    applies: Vec<(u64, u64, Option<u64>, u64)>,
}

fn run(module: &bennett_ir::ValidModule, seed: u64) -> Run {
    let entry = module.function_named("__bennett_entry").expect("entry");
    let options =
        Options { budget: Some(RUN_BUDGET), max_configurations: Some(MAX_CONFIGURATIONS), seed, ..Options::default() };
    let mut executor = Executor::new(module, entry, &[], options).expect("starts");
    executor.keep_apply_reports();
    let outcome = executor.run();
    let applies = executor.apply_reports().iter().map(|r| (r.budget, r.inputs, r.largest, r.at_budget)).collect();
    Run { outcome, console: String::from_utf8_lossy(executor.tape()).into_owned(), applies }
}

/// The program at one level: measured, built with the counts, run with every seed; the result runs (asserted equal
/// to the measuring runs) and the largest count per budget name. Checks that one step less ends an input at the
/// budget.
fn measured_runs(source: &Path, budgets: &[&str], level: &str, flags: &[&str]) -> (Vec<Run>, Vec<u64>) {
    let label = source.file_stem().expect("name").to_string_lossy().into_owned();
    let mark = |i: usize| u64::MAX - i as u64;
    let measuring: Vec<(&str, String)> =
        budgets.iter().enumerate().map(|(i, name)| (*name, format!("(BQ_UNBOUNDED-{i})"))).collect();
    let module = link(source, &format!("handles-device-{label}-{level}-measure"), flags, &measuring);
    let runs: Vec<Run> = SEEDS.iter().map(|&seed| run(&module, seed)).collect();
    let mut most = vec![0u64; budgets.len()];
    for apply in runs.iter().flat_map(|r| &r.applies) {
        let (budget, inputs, largest, at_budget) = *apply;
        let i = (0..budgets.len()).find(|&i| budget == mark(i)).unwrap_or_else(|| panic!("{label}: {apply:?}"));
        assert_eq!(at_budget, 0, "{label} {level}: an unbounded apply reached its budget: {apply:?}");
        assert!(inputs >= 1, "{label} {level}: {apply:?}");
        most[i] = most[i].max(largest.expect("a count"));
    }
    let defined = |counts: &[u64]| -> Vec<(&str, String)> {
        budgets.iter().zip(counts).map(|(name, steps)| (*name, steps.to_string())).collect()
    };
    let result = link(source, &format!("handles-device-{label}-{level}-result"), flags, &defined(&most));
    let results: Vec<Run> = SEEDS.iter().map(|&seed| run(&result, seed)).collect();
    for (seed, (got, measuring)) in SEEDS.iter().zip(results.iter().zip(&runs)) {
        assert_eq!(
            (&got.outcome, &got.console),
            (&measuring.outcome, &measuring.console),
            "{label} {level} seed {seed}: the measured budgets change the result"
        );
        let inputs = |r: &Run| r.applies.iter().map(|a| (a.1, a.3)).collect::<Vec<_>>();
        assert_eq!(inputs(got), inputs(measuring), "{label} {level} seed {seed}");
    }
    // One step less: some input of an apply with that budget ends at the budget (the program reads the fault only
    // with the probability of those inputs, so its output is not asserted).
    for i in 0..budgets.len() {
        let mut short = most.clone();
        short[i] -= 1;
        let module = link(source, &format!("handles-device-{label}-{level}-short{i}"), flags, &defined(&short));
        let got = run(&module, SEEDS[0]);
        let first = got.applies.iter().find(|a| a.0 == short[i]).unwrap_or_else(|| panic!("{label} {level}: {got:?}"));
        assert!(first.3 > 0, "{label} {level}: one step less ends no input at the budget: {got:?}");
    }
    (results, most)
}

/// The program at both levels; the two print the same with every seed. Returns the `-O0` runs.
fn on_the_device(file: &str, budgets: &[&str]) -> Vec<Run> {
    let source = examples().join(file);
    let levels: Vec<(Vec<Run>, Vec<u64>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = LEVELS
            .iter()
            .map(|(level, flags)| {
                let source = &source;
                scope.spawn(move || measured_runs(source, budgets, level, flags))
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("level")).collect()
    });
    for (seed, (a, b)) in SEEDS.iter().zip(levels[0].0.iter().zip(&levels[1].0)) {
        assert_eq!((&a.outcome, &a.console), (&b.outcome, &b.console), "{file} seed {seed}: -O0 and -O2 differ");
    }
    levels[0].0.clone()
}

/// The native stand-in's run of `file` with `seed` (every budget unbounded): standard output, probability tables
/// included (`BQ_PROBS`), and status.
fn standin(file: &str, budgets: &[&str], seed: u64) -> (String, i32) {
    let label = file.trim_end_matches(".c");
    let executable = out_dir().join(format!("handles-device-standin-{label}"));
    let include = root().join("front/include");
    let mut command = Command::new(clang());
    command
        .args(["-std=gnu11", "-O2", "-fsanitize-coverage=trace-pc-guard,pc-table,no-prune"])
        .arg(format!("-I{}", include.display()))
        .arg(format!("-I{}", examples().display()))
        .args(budgets.iter().map(|n| format!("-DBQ_BUDGET_{n}=BQ_UNBOUNDED")))
        .arg(examples().join(file))
        .arg(root().join("front/tests/handles/standin.c"))
        .args(["-lm", "-o"])
        .arg(&executable);
    let built = command.output().expect("clang");
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = Command::new(&executable).env("BQ_SEED", seed.to_string()).env("BQ_PROBS", "1").output().expect("run");
    (String::from_utf8_lossy(&ran.stdout).into_owned(), ran.status.code().expect("status"))
}

/// The probability tables `P[measure k]: y:p ...` of the stand-in's output, in order.
fn tables(stdout: &str) -> Vec<BTreeMap<u64, f64>> {
    let parse = |entry: &str| -> (u64, f64) {
        let (y, p) = entry.split_once(':').unwrap_or_else(|| panic!("{entry}"));
        (y.parse().expect("outcome"), p.parse().expect("probability"))
    };
    let rows = stdout.lines().filter_map(|l| l.strip_prefix("P[measure ")?.split_once("]:"));
    rows.map(|(_, entries)| entries.split_whitespace().map(parse).collect()).collect()
}

fn results(stdout: &str) -> Vec<&str> {
    stdout.lines().filter(|l| !l.starts_with("P[")).collect()
}

#[test]
fn the_thesis_program_runs_on_the_device() {
    let budgets = ["negate"];
    let (native, status) = standin("thesis.c", &budgets, SEEDS[0]);
    assert_eq!(status, 0);
    let correlated = |line: &str| line.ends_with("q = 0, r = 1") || line.ends_with("q = 1, r = 0");
    assert!(results(&native).iter().all(|l| correlated(l)), "{native}");
    for run in on_the_device("thesis.c", &budgets) {
        assert_eq!(run.outcome, Outcome::Returned(Some(0)), "{run:?}");
        let lines = results(&run.console);
        assert_eq!(lines.len(), 8, "{run:?}");
        for (k, line) in lines.iter().enumerate() {
            assert!(line.starts_with(&format!("shot {k}: ")) && correlated(line), "{run:?}");
        }
        assert_eq!(run.applies.len(), 8, "{run:?}");
        assert!(run.applies.iter().all(|a| a.1 == 2), "{run:?}");
    }
}

#[test]
fn grover_over_a_c_predicate_runs_on_the_device() {
    let budgets = ["is_marked"];
    let (native, status) = standin("grover_predicate.c", &budgets, SEEDS[0]);
    assert_eq!(status, 0);
    let expected = results(&native);
    assert_eq!(expected, ["iterations 12, measured 235, is_marked = 1"]);
    for run in on_the_device("grover_predicate.c", &budgets) {
        assert_eq!(run.outcome, Outcome::Returned(Some(0)), "{run:?}");
        assert_eq!(results(&run.console), expected, "{run:?}");
        assert_eq!(run.applies.len(), 12, "{run:?}");
        assert!(run.applies.iter().all(|a| a.1 == 256), "{run:?}");
    }
}

#[test]
fn shor_for_15_runs_on_the_device() {
    let budgets = ["modexp"];
    let (native, status) = standin("shor15.c", &budgets, SEEDS[0]);
    assert_eq!(status, 0);
    // Every attempt measures the fault register, then the exponent register: the second table of each pair.
    let found = tables(&native);
    assert!(found.len() >= 2, "{native}");
    let exponent = &found[1];
    for run in on_the_device("shor15.c", &budgets) {
        assert_eq!(run.outcome, Outcome::Returned(Some(0)), "{run:?}");
        let lines = results(&run.console);
        let last = lines.last().expect("a line");
        assert!(last.ends_with(" - 15 = 3 x 5") || last.ends_with(" - 15 = 5 x 3"), "{run:?}");
        for line in &lines {
            let y: u64 = line
                .split_once("y = ")
                .and_then(|(_, rest)| rest.split_once(',').map(|(y, _)| y.parse().expect("y")))
                .unwrap_or_else(|| panic!("{line}"));
            assert!(exponent.get(&y).is_some_and(|&p| p > 0.0), "{line}: the stand-in gives y = {y} no weight");
        }
        assert_eq!(run.applies.len(), lines.len(), "{run:?}");
        assert!(run.applies.iter().all(|a| a.1 == 256), "{run:?}");
    }
}
