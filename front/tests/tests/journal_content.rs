//! The journal-content measurement (`experiments/journal-content/README.md`) on C: memory-heavy lookups
//! over 1,024 keys (`front/tests/c/journal_content.c`) and everything else that runs coherently in these
//! tests. Each coherent region's journal words are classified per branch as the same on every branch,
//! recoverable from live state, recomputed in one operation, or irreducible, under two alignments.
//!
//! The tables of the README come from one command, which prints them as tab-separated lines starting
//! with `table`: `front/test.sh --test journal_content -- --include-ignored --nocapture --test-threads=1`.

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::OnceLock;

use bennett_exec::{
    ApplyClassification, ClassifiedPosition, Executor, JournalMeasurement, Options, Outcome, Undo, WordClass, WordRole,
    classify, classify_with_chains, classify_with_recompute,
};
use bennett_front_tests::{CLANG_FLAGS, c_dir, entry, lower, native_build, options, root, run};
use bennett_ir::{Inst, Type, ValidModule};

/// The lookups of `journal_content.c`, each applied to all 256 values of x.
const LOOKUPS: [&str; 4] = ["jc_binary_search", "jc_hash_lookup", "jc_btree_lookup", "jc_write_lookup"];

/// The repetition counts the native build prints and the slopes are fitted over.
const REPS: [u64; 4] = [1, 2, 4, 8];

/// Every 8-bit input.
const INPUTS: usize = 256;

/// Basis configurations allowed: the in-place updates of `inplace.c` reach 2^17.
const MAX_CONFIGURATIONS: usize = 1 << 17;

/// The key the wrappers XOR x with; any key gives all 256 inputs.
const KEY: u64 = 0;

/// The mask `journal_content.c` computes from the key.
fn mask(key: u64) -> u64 {
    (key * 37 + 11) & 0xff
}

/// The C programs of these tests whose functions apply ordinary code coherently, besides the lookups.
const COHERENT_PROGRAMS: [&str; 11] = [
    "thesis",
    "thesis_kept",
    "kickback",
    "fault_cancel",
    "controlled",
    "grover",
    "inplace",
    "coarse",
    "float_controlled",
    "float_long_double_controlled",
    "coherent",
];

fn lookups_module() -> &'static ValidModule {
    static MODULE: OnceLock<ValidModule> = OnceLock::new();
    MODULE.get_or_init(|| lower(&c_dir().join("journal_content.c"), "journal_content", CLANG_FLAGS, &[]))
}

/// The native results: `native[(x, reps)]` = [binary search, hash, B-tree, write].
fn native() -> &'static BTreeMap<(u64, u64), [u64; 4]> {
    static NATIVE: OnceLock<BTreeMap<(u64, u64), [u64; 4]>> = OnceLock::new();
    NATIVE.get_or_init(|| {
        let executable = native_build(&c_dir().join("journal_content.c"), "journal_content", &["-DNATIVE"]);
        let output = Command::new(&executable).output().unwrap_or_else(|e| panic!("cannot run {executable:?}: {e}"));
        assert!(output.status.success(), "the native lookups failed");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let fields: Vec<u64> =
                    line.split(' ').map(|f| f.parse().unwrap_or_else(|e| panic!("{line:?}: {e}"))).collect();
                ((fields[0], fields[1]), [fields[2], fields[3], fields[4], fields[5]])
            })
            .collect()
    })
}

/// Runs `name(args)` to the end with the measurement on and raw records kept. Region faults are allowed:
/// a program that reads its fault register runs on, and the faulting branches are measured too.
fn measure(module: &ValidModule, name: &str, args: &[u64]) -> JournalMeasurement {
    let options = Options { allow_region_faults: true, max_configurations: Some(MAX_CONFIGURATIONS), ..options(0) };
    let mut executor =
        Executor::new(module, entry(module, name), args, options).unwrap_or_else(|e| panic!("@{name}: {e}"));
    executor.measure_journal_content(true);
    let outcome = executor.run();
    assert!(matches!(outcome, Outcome::Returned(_)), "@{name}{args:?}: {outcome:?}");
    executor.take_journal_measurement().unwrap_or_else(|| panic!("@{name}: the measurement is on"))
}

/// What the tests keep of one measured lookup run: the lookup's own `apply` (the front adds others,
/// each on one branch), as each branch's arguments and outcome, and its classification summary.
struct LookupRun {
    branches: Vec<(Vec<u128>, Outcome)>,
    summaries: [Summary; 4],
}

fn lookup_run(lookup: &str, reps: u64, chains: bool) -> LookupRun {
    let module = lookups_module();
    let mut measurement = measure(module, &format!("coherent_{lookup}"), &[KEY, reps]);
    let callee = entry(module, &format!("coherent_{lookup}.apply"));
    assert!(
        measurement.applies.iter().all(|a| a.callee == callee || a.branches.len() == 1),
        "{lookup}: every other apply is on one branch"
    );
    measurement.applies.retain(|apply| apply.callee == callee);
    assert_eq!(measurement.applies.len(), 1, "{lookup}: one apply of the lookup");
    let mut summaries = summaries(&measurement, chains);
    name_blockers(module, &mut summaries);
    assert_refinement(&summaries);
    let branches = measurement.applies[0].branches.iter().map(|b| (b.args.clone(), b.outcome)).collect();
    LookupRun { branches, summaries }
}

/// The measured runs of every lookup at every repetition count, made once per test process.
fn lookup_runs() -> &'static BTreeMap<(&'static str, u64), LookupRun> {
    static RUNS: OnceLock<BTreeMap<(&'static str, u64), LookupRun>> = OnceLock::new();
    RUNS.get_or_init(|| {
        LOOKUPS
            .into_iter()
            .flat_map(|lookup| REPS.map(|reps| (lookup, reps)))
            .map(|(lookup, reps)| ((lookup, reps), lookup_run(lookup, reps, false)))
            .collect()
    })
}

/// Each branch of each measured region computed the native result for its x: the applied function's
/// arguments are (key mask, reps, q) with qubit i of x in bit 8i of q, and it returns the lookup. Whole
/// runs, which measure one branch, return it too.
#[test]
fn every_lookup_agrees_with_the_native_build_on_every_branch() {
    for ((lookup, reps), measured) in lookup_runs() {
        let which = LOOKUPS.iter().position(|l| l == lookup).unwrap_or_else(|| panic!("{lookup}"));
        let mut seen = vec![false; INPUTS];
        for (args, outcome) in &measured.branches {
            let [add, branch_reps, q] = args[..] else { panic!("{lookup}: arguments {args:?}") };
            assert_eq!((add, branch_reps), (u128::from(mask(KEY)), u128::from(*reps)), "{lookup}");
            let x = (0..8).map(|i| (q >> (8 * i) & 1) << i).sum::<u128>() ^ add;
            let x = u64::try_from(x).unwrap_or_else(|e| panic!("{lookup}: {e}"));
            seen[x as usize] = true;
            assert_eq!(
                *outcome,
                Outcome::Returned(Some(u128::from(native()[&(x, *reps)][which]))),
                "{lookup}: x = {x}, reps = {reps}"
            );
        }
        assert!(seen.iter().all(|&s| s), "{lookup}({reps}): every input is a branch");
        for seed in 0..2 {
            let name = format!("coherent_{lookup}");
            let Outcome::Returned(Some(packed)) = run(lookups_module(), &name, &[KEY, *reps], seed) else {
                panic!("{name}({reps}) seed {seed}")
            };
            let (x, y) = ((packed >> 32) as u64, (packed & 0xffff_ffff) as u64);
            assert_eq!(y, native()[&(x, *reps)][which], "{name}({reps}) seed {seed}: measured x = {x}");
        }
    }
}

/// Counts by class (same, recoverable, irreducible, recomputed): of positions, and of word instances (one per
/// branch that journalled the position).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Tally {
    positions: [u64; 4],
    instances: [u64; 4],
    /// Positions present on only some branches.
    partial: u64,
    /// Irreducible positions whose value is the same on every branch that has them: all they hold is
    /// which branches reached them (control history).
    control: u64,
    instructions: u64,
    recovered_instances: u64,
    max_instructions: u64,
    max_depth: u64,
    individually_recovered: u64,
}

impl Tally {
    fn add(&mut self, position: &ClassifiedPosition) {
        let class = match position.class {
            WordClass::Same => 0,
            WordClass::Recoverable => 1,
            WordClass::Irreducible => 2,
            WordClass::Recomputed => 3,
        };
        if let Some(chain) = &position.chain {
            if chain.recovered {
                self.instructions += chain.instructions * position.instances as u64;
                self.recovered_instances += position.instances as u64;
                self.max_instructions = self.max_instructions.max(chain.instructions);
                self.max_depth = self.max_depth.max(chain.depth);
            } else if chain.individually_recovered {
                self.individually_recovered += 1;
            }
        }
        self.positions[class] += 1;
        self.instances[class] += position.instances as u64;
        self.partial += u64::from(position.branches_present < position.branches_total);
        self.control += u64::from(position.class == WordClass::Irreducible && position.same_where_present);
    }

    fn total_positions(&self) -> u64 {
        self.positions.iter().sum()
    }

    fn total_instances(&self) -> u64 {
        self.instances.iter().sum()
    }
}

/// What one run journalled in its coherent regions, and how it classifies.
#[derive(Debug, Default)]
struct Summary {
    applies: u64,
    branches: u64,
    /// Steps over every branch of every apply.
    steps: u64,
    /// Journal words over every branch of every apply.
    words: u64,
    /// The longest branch journal of each apply, summed over applies: what a machine keeping each
    /// branch's whole journal in superposition holds, in words.
    max_words: u64,
    /// By alignment (0 = A, 1 = B): everything.
    all: [Tally; 2],
    /// By alignment and record kind/word role.
    by_role: [BTreeMap<String, Tally>; 2],
    blockers: [BTreeMap<String, u64>; 2],
}

fn role_name(kind: Undo, role: WordRole) -> String {
    let role = match role {
        WordRole::CallParam(_) => "param".to_owned(),
        WordRole::JumpParam(_) => "param".to_owned(),
        other => format!("{other:?}"),
    };
    format!("{kind:?}/{role}")
}

fn summarize(measurement: &JournalMeasurement, classes: &[ApplyClassification]) -> Summary {
    let mut summary = Summary::default();
    for (apply, classified) in measurement.applies.iter().zip(classes) {
        summary.applies += 1;
        summary.branches += apply.branches.len() as u64;
        summary.steps += apply.branches.iter().map(|b| b.steps).sum::<u64>();
        summary.words += apply.branches.iter().map(|b| b.words as u64).sum::<u64>();
        summary.max_words += apply.branches.iter().map(|b| b.words as u64).max().unwrap_or(0);
        for (alignment, positions) in [&classified.site_count, &classified.call_path].into_iter().enumerate() {
            // Every word instance of every branch journal is in exactly one position.
            let instances: usize = positions.iter().map(|p| p.instances).sum();
            assert_eq!(instances, apply.branches.iter().map(|b| b.words).sum::<usize>(), "alignment {alignment}");
            for position in positions {
                if position.class == WordClass::Irreducible {
                    if let Some(chain) = &position.chain {
                        for blocker in &chain.blockers {
                            *summary.blockers[alignment]
                                .entry(format!(
                                    "{}/{}: {blocker}",
                                    position.site.func.0,
                                    role_name(position.kind, position.role)
                                ))
                                .or_default() += 1;
                        }
                    }
                }
                summary.all[alignment].add(position);
                summary.by_role[alignment].entry(role_name(position.kind, position.role)).or_default().add(position);
            }
        }
    }
    summary
}

fn assert_refinement(summaries: &[Summary; 4]) {
    for alignment in 0..2 {
        for mode in 2..4 {
            let (old, new) = (summaries[1].all[alignment], summaries[mode].all[alignment]);
            assert_eq!(old.positions[0..2], new.positions[0..2]);
            assert!(new.positions[2] <= old.positions[2]);
            assert_eq!(old.positions[2] + old.positions[3], new.positions[2] + new.positions[3]);
        }
        let (copy, recompute) = (summaries[0].all[alignment], summaries[1].all[alignment]);
        for counts in [(copy.positions, recompute.positions), (copy.instances, recompute.instances)] {
            assert_eq!(counts.0[0..2], counts.1[0..2], "same and copy witnesses retain priority");
            assert_eq!(counts.0[2], counts.1[2] + counts.1[3], "only irreducible words can become recomputed");
        }
    }
}

fn summaries(measurement: &JournalMeasurement, chains: bool) -> [Summary; 4] {
    let deep = if chains { classify_with_chains(measurement, false) } else { classify_with_recompute(measurement) };
    let tape = if chains { classify_with_chains(measurement, true) } else { classify_with_recompute(measurement) };
    [
        summarize(measurement, &classify(measurement)),
        summarize(measurement, &classify_with_recompute(measurement)),
        summarize(measurement, &deep),
        summarize(measurement, &tape),
    ]
}

// Names are resolved from measured function/slot IDs, not inferred from C source.
fn name_blockers(module: &ValidModule, summaries: &mut [Summary; 4]) {
    for summary in summaries {
        for blockers in &mut summary.blockers {
            *blockers = std::mem::take(blockers)
                .into_iter()
                .map(|(label, count)| {
                    let (root, reason) = label.split_once('/').expect("root function/role");
                    let func: usize = root.parse().expect("function ID");
                    let mut label = format!("{}/{}", module.functions[func].name, reason);
                    if let Some((prefix, origin)) = label.split_once("missing-origin f") {
                        let (func, slot) = origin.split_once(" slot").expect("origin IDs");
                        let func: usize = func.parse().expect("origin function");
                        let slot: usize = slot.parse().expect("origin slot");
                        label = format!(
                            "{prefix}missing-origin @{}/%{}",
                            module.functions[func].name, module.functions[func].slot_names[slot]
                        );
                    }
                    (label, count)
                })
                .collect();
        }
    }
}

fn run_summaries(module: &ValidModule, name: &str, args: &[u64]) -> [Summary; 4] {
    let measurement = measure(module, name, args);
    let mut summaries = summaries(&measurement, true);
    name_blockers(module, &mut summaries);
    assert_refinement(&summaries);
    summaries
}

const MODES: [&str; 4] = ["", "recompute-", "chain-", "tape-"];

const ALIGNMENTS: [&str; 2] = ["A", "B"];

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 { "-".to_owned() } else { format!("{:.1}", 100.0 * part as f64 / whole as f64) }
}

/// One line per alignment: the headline columns of the README's first table.
fn print_summary(mode: &str, group: &str, label: &str, summary: &Summary) {
    for (alignment, tally) in summary.all.iter().enumerate() {
        println!(
            "table\t{mode}workload\t{group}\t{label}\t{}\t{}\t{}\t{:.1}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            ALIGNMENTS[alignment],
            summary.applies,
            summary.branches,
            summary.steps as f64 / summary.branches.max(1) as f64,
            summary.max_words,
            tally.total_positions(),
            tally.positions[0],
            tally.positions[1],
            tally.positions[2],
            tally.partial,
            tally.control,
            percent(tally.instances[2], tally.total_instances()),
            percent(tally.positions[2], summary.max_words),
            percent(tally.instances[0], tally.total_instances()),
            percent(tally.instances[0] + tally.instances[1], tally.total_instances()),
            tally.positions[3],
            percent(tally.instances[3], tally.total_instances()),
        );
        if mode == "chain-" || mode == "tape-" {
            println!(
                "table\t{mode}time\t{group}\t{label}\t{}\t{}\t{}\t{:.3}\t{}\t{}\t{}\t{:.6}",
                ALIGNMENTS[alignment],
                tally.instructions,
                tally.recovered_instances,
                tally.instructions as f64 / tally.recovered_instances.max(1) as f64,
                tally.max_instructions,
                tally.max_depth,
                tally.individually_recovered,
                tally.instructions as f64 / summary.branches.max(1) as f64
            );
            for (blocker, count) in &summary.blockers[alignment] {
                println!("table\t{mode}blocker\t{group}\t{label}\t{}\t{count}\t{blocker}", ALIGNMENTS[alignment]);
            }
        }
    }
}

fn print_header() {
    println!(
        "table\tworkload\tgroup\tworkload\talignment\tapplies\tbranches\tsteps/branch\tmax words/branch\tpositions\t\
         same\trecoverable\tirreducible\tpartial\tirreducible, control only\tirreducible % of words\tirreducible positions % of max \
         words/branch\tsame % of words\tsame+recoverable % of words\trecomputed\trecomputed % of words"
    );
}

fn print_roles(mode: &str, group: &str, roles: &[BTreeMap<String, Tally>; 2]) {
    println!(
        "table\trole\tgroup\talignment\tkind/word\tinstances\tsame %\trecoverable %\tirreducible %\tpositions\t\
         irreducible positions\tirreducible positions, control only\trecomputed positions\trecomputed %"
    );
    for (alignment, by_role) in roles.iter().enumerate() {
        for (role, tally) in by_role {
            let total = tally.total_instances();
            println!(
                "table\t{mode}role\t{group}\t{}\t{role}\t{total}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                ALIGNMENTS[alignment],
                percent(tally.instances[0], total),
                percent(tally.instances[1], total),
                percent(tally.instances[2], total),
                tally.total_positions(),
                tally.positions[2],
                tally.control,
                tally.positions[3],
                percent(tally.instances[3], total),
            );
        }
    }
}

fn merge_roles(into: &mut [BTreeMap<String, Tally>; 2], from: &[BTreeMap<String, Tally>; 2]) {
    for (into, from) in into.iter_mut().zip(from) {
        for (role, tally) in from {
            let entry = into.entry(role.clone()).or_default();
            for class in 0..4 {
                entry.positions[class] += tally.positions[class];
                entry.instances[class] += tally.instances[class];
            }
            entry.partial += tally.partial;
            entry.control += tally.control;
        }
    }
}

/// The least-squares slope of `ys` against `xs`.
fn slope(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let covariance: f64 = xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let variance: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
    covariance / variance
}

/// The lookups at every repetition count: the per-workload table, the per-kind table, and slopes of
/// journal words and irreducible positions against steps per branch.
#[test]
fn journal_content_of_the_lookups() {
    print_lookup_tables(lookup_runs(), 0..2);
}

#[test]
#[ignore = "unbounded chains and their time price; run with --include-ignored"]
fn journal_content_of_chains() {
    let runs = LOOKUPS
        .into_iter()
        .flat_map(|lookup| REPS.map(|reps| (lookup, reps)))
        .map(|(lookup, reps)| ((lookup, reps), lookup_run(lookup, reps, true)))
        .collect();
    print_lookup_tables(&runs, 2..4);
}

fn print_lookup_tables(runs: &BTreeMap<(&str, u64), LookupRun>, modes: std::ops::Range<usize>) {
    print_header();
    for mode_index in modes {
        let mode = MODES[mode_index];
        let mut roles: [BTreeMap<String, Tally>; 2] = Default::default();
        let mut slopes = Vec::new();
        for lookup in LOOKUPS {
            let name = format!("coherent_{lookup}");
            let mut steps = Vec::new();
            let mut max_words = Vec::new();
            let mut time: [Vec<f64>; 2] = Default::default();
            let mut irreducible: [Vec<f64>; 2] = Default::default();
            let mut by_role: [BTreeMap<String, Vec<f64>>; 2] = Default::default();
            for (point, reps) in REPS.into_iter().enumerate() {
                let summary = &runs[&(lookup, reps)].summaries[mode_index];
                assert_eq!((summary.applies, summary.branches), (1, INPUTS as u64), "{name}({reps})");
                print_summary(mode, "lookups", &format!("{lookup} reps={reps}"), summary);
                merge_roles(&mut roles, &summary.by_role);
                steps.push(summary.steps as f64 / summary.branches as f64);
                max_words.push(summary.max_words as f64);
                for alignment in 0..2 {
                    time[alignment].push(summary.all[alignment].instructions as f64 / summary.branches as f64);
                    irreducible[alignment].push(summary.all[alignment].positions[2] as f64);
                    for (role, tally) in &summary.by_role[alignment] {
                        let series = by_role[alignment].entry(role.clone()).or_insert_with(|| vec![0.0; REPS.len()]);
                        series[point] = tally.positions[2] as f64;
                    }
                }
            }
            for alignment in 0..2 {
                if mode_index >= 2 {
                    println!(
                        "table\t{mode}time-slope\t{lookup}\t{}\t{:.6}",
                        ALIGNMENTS[alignment],
                        slope(&steps, &time[alignment])
                    );
                }
                let roles: Vec<String> = by_role[alignment]
                    .iter()
                    .map(|(role, series)| format!("{role}={:.3}", slope(&steps, series)))
                    .collect();
                slopes.push(format!(
                    "table\t{mode}slope\t{lookup}\t{}\t{:.3}\t{:.3}\t{}",
                    ALIGNMENTS[alignment],
                    slope(&steps, &max_words),
                    slope(&steps, &irreducible[alignment]),
                    roles.join(" "),
                ));
            }
        }
        print_roles(mode, "lookups", &roles);
        println!(
            "table\tslope\tworkload\talignment\tmax words per step\tirreducible positions per step\t\
         irreducible positions per step, by kind/word"
        );
        for line in slopes {
            println!("{line}");
        }
    }
}

/// Every function of the programs in `COHERENT_PROGRAMS` that applies ordinary code coherently, run
/// with every argument 1, and the hand-written examples that apply. About 35 s and 1.2 GB, almost all
/// of it the in-place updates' 2^16 and 2^17 branches, so it runs only when asked for.
#[test]
#[ignore = "the full measurement: about 35 s; run it with --include-ignored"]
fn journal_content_of_everything_else_that_runs_coherently() {
    print_header();
    let mut roles: [[BTreeMap<String, Tally>; 2]; 4] = Default::default();
    let mut runs = 0;
    for program in COHERENT_PROGRAMS {
        let module = lower(&c_dir().join(format!("{program}.c")), &format!("{program}.journal"), CLANG_FLAGS, &[]);
        for function in &module.functions {
            let applies = function.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Apply { .. }));
            let ordinary = function.params.iter().all(|p| matches!(p.ty, Type::Int(_)));
            if !applies || !ordinary || function.name.contains(".apply") {
                continue;
            }
            let args = vec![1; function.params.len()];
            for (mode_index, summary) in run_summaries(&module, &function.name, &args).iter().enumerate() {
                assert!(summary.applies > 0, "@{}", function.name);
                print_summary(MODES[mode_index], program, &function.name, summary);
                merge_roles(&mut roles[mode_index], &summary.by_role);
            }
            runs += 1;
        }
    }
    for (example, name, args) in [
        ("thesis", "main", &[][..]),
        ("thesis", "entangled", &[][..]),
        ("grover", "searched", &[2][..]),
        ("fault", "main", &[4][..]),
    ] {
        let path = root().join(format!("examples/{example}.bn"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let module = bennett_ir::load(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        for (mode_index, summary) in run_summaries(&module, name, args).iter().enumerate() {
            print_summary(MODES[mode_index], "examples", &format!("{example}.bn {name}"), summary);
            merge_roles(&mut roles[mode_index], &summary.by_role);
        }
        runs += 1;
    }
    for (mode, roles) in MODES.iter().zip(&roles) {
        print_roles(mode, "rest", roles);
    }
    assert_eq!(runs, 67, "the identical non-lookup workload set");
}
