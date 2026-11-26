//! The programs in the handle interface (`front/include/bq.h`, `docs/HANDLES.md`) run natively against the
//! stand-in `front/tests/handles/standin.c`: the five example programs in `examples/handles/` and the
//! checks in `front/tests/handles/`. Each is built at `-O0` and at `-O2 -fno-finite-loops`. At each level
//! a measuring build gives every budget name its own unbounded value and reports, for each `apply`, the
//! budget it was passed and the most steps an input used; the result build gives each name the largest
//! count reported for it. The result runs must give the same output as the measuring runs (the measured
//! budgets change no result), the same output at both levels, and the right output, with every seed.
//! The stand-in claims agreement with the device only for the class of applied functions its header
//! comment states; every applied function of these programs is in it. Its step counts are its own.
use bennett_front_tests::{clang, out_dir, root};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The two optimisation levels the programs must agree at.
const LEVELS: [(&str, &[&str]); 2] = [("O0", &["-O0"]), ("O2", &["-O2", "-fno-finite-loops"])];
/// How the programs are compiled for the stand-in's step count and its table of function entries.
const COVERAGE: &str = "-fsanitize-coverage=trace-pc-guard,pc-table,no-prune";
/// The seeds of the stand-in's generator.
const SEEDS: [u64; 3] = [1, 2, 3];
/// How the stand-in ends a run at a failed check, a stop or a limit, and how `bq_faults_check` ends one.
const STANDIN_FAULT_STATUS: i32 = 3;
const BQ_FAULT_EXIT_STATUS: i32 = 4;

fn handles_dir() -> PathBuf {
    root().join("front/tests/handles")
}

fn examples_dir() -> PathBuf {
    root().join("examples/handles")
}

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(output.status.success(), "{command:?}: {}", String::from_utf8_lossy(&output.stderr));
    output
}

/// The stand-in, compiled once at `-O2` (it is the device, not the program).
fn standin() -> &'static Path {
    static OBJECT: OnceLock<PathBuf> = OnceLock::new();
    OBJECT.get_or_init(|| {
        let object = out_dir().join("handles-standin.o");
        checked(
            Command::new(clang())
                .args(["-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O2", "-c"])
                .arg(format!("-I{}", root().join("front/include").display()))
                .arg(handles_dir().join("standin.c"))
                .arg("-o")
                .arg(&object),
        );
        object
    })
}

/// One line `bq: apply <k> budget <b> inputs <e> steps <s> out-of-budget <n>` of the stand-in.
#[derive(Clone, Debug, PartialEq)]
struct Report {
    apply: usize,
    budget: u64,
    inputs: u64,
    steps: u64,
    ran_out: u64,
}

/// A run: its standard output, its standard error without the step reports, its status, and the reports.
#[derive(Clone, Debug)]
struct Run {
    stdout: String,
    stderr: String,
    status: i32,
    reports: Vec<Report>,
}

impl Run {
    /// The lines of standard output that are not probability tables.
    fn results(&self) -> Vec<&str> {
        self.stdout.lines().filter(|l| !l.starts_with("P[")).collect()
    }

    fn same_output(&self, other: &Run) -> bool {
        (&self.stdout, &self.stderr, self.status) == (&other.stdout, &other.stderr, other.status)
    }
}

fn run_with(command: &mut Command) -> Run {
    let output = command.output().unwrap_or_else(|e| panic!("{command:?}: {e}"));
    let mut reports = Vec::new();
    let mut stderr = String::new();
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        match line.strip_prefix("bq: apply ") {
            Some(rest) => {
                let words: Vec<&str> = rest.split(' ').collect();
                let names = (words.len(), words[1], words[3], words[5], words[7]);
                assert_eq!(names, (9, "budget", "inputs", "steps", "out-of-budget"), "{line}");
                let number = |i: usize| -> u64 { words[i].parse().unwrap_or_else(|e| panic!("{line}: {e}")) };
                reports.push(Report {
                    apply: number(0) as usize,
                    budget: number(2),
                    inputs: number(4),
                    steps: number(6),
                    ran_out: number(8),
                });
            }
            None => {
                stderr.push_str(line);
                stderr.push('\n');
            }
        }
    }
    let status = output.status.code().unwrap_or_else(|| panic!("{command:?} was killed: {}", output.status));
    Run { stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr, status, reports }
}

fn run(executable: &Path, seed: u64, args: &[&str]) -> Run {
    run_with(Command::new(executable).args(args).env("BQ_SEED", seed.to_string()).env("BQ_PROBS", "1"))
}

/// A program and how to build and judge it.
struct Program {
    source: PathBuf,
    /// The budget names it uses; each is defined to its measured maximum in the result build.
    budgets: &'static [&'static str],
    /// Further compiler arguments (include directories) and objects to link (per level).
    extra: Vec<String>,
    objects: Vec<(&'static str, PathBuf)>,
    /// The seeds of the runs.
    seeds: Vec<u64>,
    /// Names the build products, unique per test (tests run in parallel).
    label: String,
}

/// What a measuring build reported, per budget name, and the runs it made.
struct Measured {
    budgets: Vec<(String, u64)>,
    runs: Vec<Vec<Run>>,
}

impl Program {
    fn new(source: PathBuf, budgets: &'static [&'static str]) -> Self {
        let label = source.file_stem().expect("name").to_string_lossy().into_owned();
        let extra = vec![format!("-I{}", examples_dir().display())];
        Program { source, budgets, extra, objects: Vec::new(), seeds: SEEDS.to_vec(), label }
    }

    fn name(&self) -> String {
        self.source.file_stem().expect("name").to_string_lossy().into_owned()
    }

    /// Builds the program at `level` with the budget definitions `budgets`; `tag` names the executable.
    fn build(&self, level: &str, flags: &[&str], budgets: &[(String, String)], tag: &str) -> PathBuf {
        let executable = out_dir().join(format!("handles-{}-{level}-{tag}", self.label));
        let mut command = Command::new(clang());
        command
            .args(["-std=gnu11", "-Wall", "-Wextra", "-Werror", COVERAGE])
            .args(flags)
            .arg(format!("-I{}", root().join("front/include").display()))
            .args(&self.extra)
            .args(budgets.iter().map(|(name, value)| format!("-DBQ_BUDGET_{name}={value}")))
            .arg(&self.source)
            .args(self.objects.iter().filter(|(l, _)| *l == level).map(|(_, o)| o))
            .arg(standin())
            .arg("-lm")
            .arg("-o")
            .arg(&executable);
        checked(&mut command);
        executable
    }

    /// The build with every budget unbounded (for checks whose budgets do not matter).
    fn unbounded(&self, level: &str, flags: &[&str]) -> PathBuf {
        let budgets: Vec<_> = self.budgets.iter().map(|n| (n.to_string(), "BQ_UNBOUNDED".to_owned())).collect();
        self.build(level, flags, &budgets, "unbounded")
    }

    /// The measuring build and its runs with every seed and every argument list: the most steps reported
    /// for each budget name. Name `i` gets the value `BQ_UNBOUNDED - i`, so that a report names its budget.
    fn measure(&self, level: &str, flags: &[&str], arg_sets: &[&[&str]]) -> Measured {
        let mark = |i: usize| u64::MAX - i as u64;
        let values: Vec<(String, String)> = self
            .budgets
            .iter()
            .enumerate()
            .map(|(i, name)| (name.to_string(), format!("(BQ_UNBOUNDED-{i})")))
            .collect();
        let executable = self.build(level, flags, &values, "measure");
        let runs: Vec<Vec<Run>> =
            arg_sets.iter().map(|args| self.seeds.iter().map(|&seed| run(&executable, seed, args)).collect()).collect();
        let mut most = BTreeMap::new();
        for report in runs.iter().flatten().flat_map(|run| &run.reports) {
            let i = (0..self.budgets.len()).find(|&i| report.budget == mark(i));
            let i = i.unwrap_or_else(|| panic!("{}: apply with an unnamed budget: {report:?}", self.name()));
            assert_eq!(report.ran_out, 0, "{}: an unbounded apply ran out: {report:?}", self.name());
            let entry = most.entry(i).or_insert(0);
            *entry = report.steps.max(*entry);
        }
        let budgets = (0..self.budgets.len())
            .map(|i| {
                let steps =
                    *most.get(&i).unwrap_or_else(|| panic!("{}: no apply used {}", self.name(), self.budgets[i]));
                (self.budgets[i].to_string(), steps)
            })
            .collect();
        Measured { budgets, runs }
    }

    /// The result build at `level` with the budgets `budgets`.
    fn result(&self, level: &str, flags: &[&str], budgets: &[(String, u64)], tag: &str) -> PathBuf {
        let budgets: Vec<_> = budgets.iter().map(|(name, steps)| (name.clone(), steps.to_string())).collect();
        self.build(level, flags, &budgets, tag)
    }

    /// Measures, builds and runs at both levels with every seed and every argument list; asserts that each
    /// result run gives the output of its measuring run, with the same inputs out of budget in each apply,
    /// and that both levels give the same output. Returns the result runs at `-O0`, per argument list and seed.
    fn agree(&self, arg_sets: &[&[&str]]) -> Vec<Vec<Run>> {
        let runs: Vec<(&str, Vec<Vec<Run>>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = LEVELS
                .iter()
                .map(|(level, flags)| {
                    scope.spawn(move || {
                        let measured = self.measure(level, flags, arg_sets);
                        let executable = self.result(level, flags, &measured.budgets, "result");
                        let runs: Vec<Vec<Run>> = arg_sets
                            .iter()
                            .map(|args| self.seeds.iter().map(|&seed| run(&executable, seed, args)).collect())
                            .collect();
                        for (result, measuring) in runs.iter().flatten().zip(measured.runs.iter().flatten()) {
                            let name = self.name();
                            assert!(
                                result.same_output(measuring),
                                "{name} {level}: {result:?}\nmeasuring: {measuring:?}"
                            );
                            let outcome = |r: &Run| r.reports.iter().map(|r| (r.inputs, r.ran_out)).collect::<Vec<_>>();
                            assert_eq!(outcome(result), outcome(measuring), "{name} {level}: inputs out of budget");
                        }
                        (*level, runs)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().expect("level")).collect()
        });
        let (_, o0) = &runs[0];
        let (_, o2) = &runs[1];
        for (args, (a_runs, b_runs)) in arg_sets.iter().zip(o0.iter().zip(o2)) {
            for ((seed, a), b) in self.seeds.iter().zip(a_runs).zip(b_runs) {
                assert!(
                    a.same_output(b),
                    "{} {args:?}: -O0 and -O2 differ with seed {seed}\n{a:?}\n{b:?}",
                    self.name()
                );
            }
        }
        o0.clone()
    }
}

/// The runs of a single argument list, one per seed.
fn agree_alone(program: &Program) -> Vec<Run> {
    program.agree(&[&[]]).remove(0)
}

#[test]
fn the_thesis_program_entangles_the_result_with_the_qubit() {
    for run in agree_alone(&Program::new(examples_dir().join("thesis.c"), &["negate"])) {
        let lines = run.results();
        assert_eq!(lines.len(), 8, "{run:?}");
        assert!(lines.iter().all(|l| l.ends_with("q = 0, r = 1") || l.ends_with("q = 1, r = 0")), "{run:?}");
        assert_eq!((run.stderr.as_str(), run.status), ("", 0), "{run:?}");
        assert!(run.stdout.contains("P[measure 2]: 0:0.500000 1:0.500000"), "{run:?}");
    }
}

#[test]
fn grover_over_a_c_predicate_finds_the_marked_value() {
    for run in agree_alone(&Program::new(examples_dir().join("grover_predicate.c"), &["is_marked"])) {
        assert_eq!(run.results(), ["iterations 12, measured 235, is_marked = 1"], "{run:?}");
        assert_eq!((run.stderr.as_str(), run.status), ("", 0), "{run:?}");
        assert!(run.stdout.contains(" 235:0.999947"), "{run:?}");
    }
}

/// The probability tables `P[measure k]: y:p ...` of a run, in order.
fn tables(run: &Run) -> Vec<BTreeMap<u64, f64>> {
    let parse = |entry: &str| -> (u64, f64) {
        let (y, p) = entry.split_once(':').unwrap_or_else(|| panic!("{entry}"));
        (y.parse().unwrap_or_else(|e| panic!("{entry}: {e}")), p.parse().unwrap_or_else(|e| panic!("{entry}: {e}")))
    };
    let rows = run.stdout.lines().filter_map(|l| l.strip_prefix("P[measure ")?.split_once("]:"));
    rows.map(|(_, entries)| entries.split_whitespace().map(parse).collect()).collect()
}

/// Shor's distribution of the measured exponent register (`bits` qubits) for the period `period`: after
/// the residue register's partial trace the register holds the `m` exponents of one residue class, `m`
/// being the floor or the ceiling of 2^bits / period, and the inverse Fourier transform gives
/// P(y) = sin^2(pi m period y / 2^bits) / (2^bits m sin^2(pi period y / 2^bits)), and m / 2^bits where
/// period y is a multiple of 2^bits.
fn fourier(bits: u32, period: u64, m: u64) -> Vec<f64> {
    let size = 1u64 << bits;
    (0..size)
        .map(|y| {
            if (period * y).is_multiple_of(size) {
                return m as f64 / size as f64;
            }
            // sin^2 has period pi, so the products are reduced modulo 2^bits.
            let angle = |k: u64| std::f64::consts::PI * ((k * period * y) % size) as f64 / size as f64;
            angle(m).sin().powi(2) / (size as f64 * m as f64 * angle(1).sin().powi(2))
        })
        .collect()
}

/// Each attempt of a Shor run measures the fault register (no fault) and then the exponent register, whose
/// table is Shor's distribution for the period to the printed precision (the stand-in prints the outcomes
/// of probability at least 5e-7, with six decimals).
fn check_fourier(run: &Run, period: u64) {
    let bits = 8;
    let size = 1u64 << bits;
    let found = tables(run);
    let attempts = run.results().len();
    assert_eq!(found.len(), 2 * attempts, "{run:?}");
    let no_fault = BTreeMap::from([(0, 1.0)]);
    for pair in found.chunks(2) {
        let [faults, table] = pair else { unreachable!("chunks of two") };
        assert_eq!(faults, &no_fault, "{run:?}");
        let close = |m: u64| {
            let want = fourier(bits, period, m);
            (0..size).all(|y| match table.get(&y) {
                Some(p) => (p - want[y as usize]).abs() <= 1e-6,
                None => want[y as usize] < 1e-6,
            })
        };
        assert!(close(size / period) || close(size.div_ceil(period)), "period {period}: {table:?}\n{run:?}");
    }
}

/// Every attempt line of a Shor run has the expected form, and the run ends with a proper factorization.
fn check_shor(run: &Run, number: u64, factors: [u64; 2]) {
    let lines = run.results();
    let (last, attempts) = lines.split_last().expect("a line");
    for line in attempts {
        assert!(line.starts_with("attempt ") && line.ends_with(" - no factor, retry"), "{run:?}");
    }
    let [p, q] = factors;
    let proper = [format!(" - {number} = {p} x {q}"), format!(" - {number} = {q} x {p}")];
    assert!(last.starts_with("attempt ") && proper.iter().any(|f| last.ends_with(f.as_str())), "{run:?}");
    assert_eq!((run.stderr.as_str(), run.status), ("", 0), "{run:?}");
}

#[test]
fn shor_factors_15() {
    for run in agree_alone(&Program::new(examples_dir().join("shor15.c"), &["modexp"])) {
        check_shor(&run, 15, [3, 5]);
        // The period is 4, so the exponent register reads 0, 64, 128 or 192, each with probability 1/4.
        assert!(run.stdout.contains("0:0.250000 64:0.250000 128:0.250000 192:0.250000"), "{run:?}");
        check_fourier(&run, 4);
    }
}

#[test]
fn shor_factors_21() {
    let mut program = Program::new(examples_dir().join("shor21.c"), &["modexp"]);
    // With seed 404 the first measured frequency gives the candidate 12, a multiple of the period 6.
    program.seeds.push(404);
    for run in agree_alone(&program) {
        check_shor(&run, 21, [3, 7]);
        check_fourier(&run, 6);
    }
}

/// The header's own checks stop the run by name before any apply.
#[test]
fn the_header_stops_by_name() {
    let program = Program::new(handles_dir().join("stops.c"), &[]);
    let endings = [
        ("wide-slice", "bq: stop bad_argument\n"),
        ("field", "bq: stop bad_argument\n"),
        ("pop", "bq: stop bad_argument\n"),
        ("grow", "bq: stop out_of_memory\n"),
    ];
    for (level, flags) in LEVELS {
        let executable = program.unbounded(level, flags);
        for (ending, message) in endings {
            let stopped = run(&executable, 1, &[ending]);
            assert_eq!(stopped.stdout, "", "{level} {ending}: {stopped:?}");
            assert_eq!((stopped.stderr.as_str(), stopped.status), (message, STANDIN_FAULT_STATUS), "{level} {ending}");
        }
    }
}

#[test]
fn the_libraries_match_their_truth_tables() {
    for run in agree_alone(&Program::new(handles_dir().join("library.c"), &["library"])) {
        let lines = run.results();
        assert_eq!(lines.len(), 12, "{run:?}");
        assert!(lines[..10].iter().all(|l| l.ends_with("matches its truth table")), "{run:?}");
        assert!(lines[10].ends_with("matches the allocated widths"), "{run:?}");
        assert!(lines[11].ends_with("leaves the mean unchanged"), "{run:?}");
        assert_eq!((run.stderr.as_str(), run.status), ("", 0), "{run:?}");
    }
}

/// The stand-in's sampling, and its stops for some applied functions outside its class (a help, not a
/// check of the class: the stand-in's header comment).
#[test]
fn the_stand_in_samples_and_stops_by_name_at_its_limits() {
    let program = Program::new(handles_dir().join("limits.c"), &[]);
    for (level, flags) in LEVELS {
        let executable = program.unbounded(level, flags);
        // A draw of exactly 1/2 belongs to the second of two outcomes of weight 1/2.
        let midpoint = run(&executable, 3_453_682_501_520_545_093, &["midpoint"]);
        assert_eq!(midpoint.results(), ["midpoint 1"], "{level}: {midpoint:?}");
        // Applied functions outside the class: each stops the run by name and leaves the parent's files alone.
        let file = out_dir().join(format!("handles-limits-{level}.txt"));
        let file_arg = file.to_str().expect("a path");
        let outside: [(&[&str], &str); 7] = [
            (&["printf"], "system call write"),
            (&["descriptor"], "system call read"),
            (&["open", file_arg], "system call openat"),
            (&["clock"], "system call vdso"),
            (&["shared"], "shared memory"),
            // On the device free is a function of the program (D11); natively it is a shared library's.
            (&["foreign"], "function"),
            (&["inside"], "function"),
        ];
        for (args, what) in outside {
            std::fs::write(&file, "A").expect("write");
            let stopped = run(&executable, 1, args);
            assert_eq!(stopped.stdout, "", "{level} {args:?}: {stopped:?}");
            assert_eq!(stopped.stderr, format!("bq: limit {what} in bq_apply\n"), "{level} {args:?}: {stopped:?}");
            assert_eq!(stopped.status, STANDIN_FAULT_STATUS, "{level} {args:?}: {stopped:?}");
            assert_eq!(std::fs::read_to_string(&file).expect("read"), "A", "{level} {args:?}");
        }
        // The state of 24 qubits under a memory limit of half its bytes.
        let state_kib = (1u64 << 24) * 16 / 1024;
        let limited = format!("ulimit -v {} && exec \"$0\" state", state_kib / 2);
        let state = run_with(Command::new("sh").arg("-c").arg(limited).arg(&executable));
        assert_eq!(
            (state.stderr.as_str(), state.status),
            ("bq: limit memory in bq_alloc\n", STANDIN_FAULT_STATUS),
            "{state:?}"
        );
    }
}

/// The measured budget is exact for the stand-in's own count: one step less and an input runs out, and the
/// header's fault check stops the run with its message on standard output.
#[test]
fn one_step_below_the_measured_budget_runs_out() {
    let mut program = Program::new(examples_dir().join("thesis.c"), &["negate"]);
    program.label = "thesis-short".to_owned();
    for (level, flags) in LEVELS {
        let measured = program.measure(level, flags, &[&[]]);
        let short: Vec<_> = measured.budgets.iter().map(|(name, steps)| (name.clone(), steps - 1)).collect();
        let run = run(&program.result(level, flags, &short, "short"), 1, &[]);
        assert!(run.reports.iter().any(|r| r.ran_out > 0), "{level}: {:?}", run.reports);
        assert!(run.stdout.ends_with("fault budget in negate\n"), "{run:?}");
        assert_eq!((run.stderr.as_str(), run.status), ("", BQ_FAULT_EXIT_STATUS), "{run:?}");
    }
}

/// The header compiles as C++ too, with its layout assertions.
#[test]
fn the_header_compiles_as_cpp() {
    let source = out_dir().join("handles-header.cpp");
    std::fs::write(&source, "#include \"bq.h\"\nint main() { return bq_scope_depth() != 0; }\n").expect("write");
    checked(
        Command::new(clang())
            .args(["-x", "c++", "-std=c++17", "-Wall", "-Wextra", "-Werror", "-fsyntax-only"])
            .arg(format!("-I{}", root().join("front/include").display()))
            .arg(&source),
    );
}

#[test]
#[ignore = "needs the SQLite 3.54.1 amalgamation; run: SQLITE_DIR=<dir> front/test.sh --test handles_native -- \
            --ignored --nocapture"]
fn grover_over_sqlite_finds_the_higgs_boson() {
    let sqlite =
        PathBuf::from(std::env::var("SQLITE_DIR").expect("set SQLITE_DIR to the SQLite amalgamation directory"));
    let mut program = Program::new(examples_dir().join("grover_sqlite.c"), &["query"]);
    program.extra.push(format!("-I{}", sqlite.display()));
    for (level, flags) in LEVELS {
        let object = out_dir().join(format!("handles-sqlite3-{level}.o"));
        checked(
            Command::new(clang())
                .args([COVERAGE, "-DSQLITE_THREADSAFE=0", "-DSQLITE_OMIT_LOAD_EXTENSION"])
                .args(["-w", "-c"])
                .args(flags)
                .arg(sqlite.join("sqlite3.c"))
                .arg("-o")
                .arg(&object),
        );
        program.objects.push((level, object));
    }
    for run in agree_alone(&program) {
        assert_eq!(run.results(), ["iterations 3, measured id 15: Higgs boson, matches = 1"], "{run:?}");
        assert_eq!((run.stderr.as_str(), run.status), ("", 0), "{run:?}");
        assert!(run.stdout.contains(" 15:0.961319"), "{run:?}");
    }
}
