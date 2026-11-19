//! The acceptance target of applied calls (`docs/DESIGN.md`, "Applied calls"): unmodified SQLite, set up by
//! ordinary code, queried under `apply` with a key that depends on a three-qubit register, written in C
//! (`experiments/sqlite-on-device/applied.c`). Each of the SQL oracle's eight queries, for each of the eight
//! row values, is compared with the native driver's result, and the classical memory, the database
//! included, is the same before and after each `apply`.
use bennett_exec::{Executor, Options};
use bennett_front_tests::{front, options, out_dir, registers, root, state_around_first_apply};
use num_complex::Complex64;
use std::collections::BTreeMap;
use std::process::Command;
use std::time::Instant;

/// Rows in the table, and the values of the three-qubit row register.
const ROWS: u64 = 8;
/// Queries of the SQL oracle (`driver.c` with `SQL_ORACLE=1`).
const QUERIES: usize = 8;
/// The one budget of each applied query: far above any row's need (README, stage 1: at most 16,131 steps
/// per query at N=16).
const QUERY_BUDGET: u64 = 1_000_000;
/// Device stack, the experiment's default (README, "Configuration").
const STACK_BYTES: u64 = 1 << 20;
/// A bound on the steps of setting the database up and applying one query, far above what it takes.
const RUN_BUDGET: u64 = 1_000_000_000;

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(output.status.success(), "{command:?}: {}", String::from_utf8_lossy(&output.stderr));
    output
}

#[test]
#[ignore = "needs the SQLite 3.54.1 amalgamation; run: SQLITE_DIR=<dir> front/test.sh --test sqlite_applied -- \
            --ignored --nocapture; measured 2026-11-17: 84 s (builds 54 s, front 26 s, eight queries 0.2 s each)"]
fn sqlite_queries_under_apply_from_c_match_native_for_each_row() {
    let sqlite = std::env::var("SQLITE_DIR").expect("set SQLITE_DIR to the SQLite amalgamation directory");
    let workspace = root().canonicalize().expect("workspace");
    let experiment = workspace.join("experiments/sqlite-on-device");
    let build = out_dir().join("sqlite-applied");
    let start = Instant::now();
    checked(Command::new("python3").arg(workspace.join("rt/musl/build.py")));
    checked(
        Command::new("sh")
            .arg(experiment.join("build.sh"))
            .env("SQLITE_DIR", &sqlite)
            .env("NROWS", ROWS.to_string())
            .env("SQL_ORACLE", "1")
            .env("OUT", &build),
    );
    println!("native and device builds of the driver: {:.1} s", start.elapsed().as_secs_f64());

    // Native results, one line `name row value` per query and row.
    let native = checked(&mut Command::new(build.join("native")));
    let mut expected: BTreeMap<(usize, u64), u64> = BTreeMap::new();
    let mut names = Vec::new();
    for line in String::from_utf8_lossy(&native.stdout).lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        if fields.len() != 3 || fields[0] == "memory" || fields[0] == "vfs" {
            continue;
        }
        if !names.contains(&fields[0].to_owned()) {
            names.push(fields[0].to_owned());
        }
        let query = names.iter().position(|n| n == fields[0]).expect("named");
        let value: i64 = fields[2].parse().expect("native value");
        expected.insert((query, fields[1].parse().expect("row")), value as u32 as u64);
    }
    assert_eq!((names.len(), expected.len()), (QUERIES, QUERIES * ROWS as usize), "{names:?}");

    // The C entries, linked with the unchanged lowered driver and lowered by the front.
    let lowering = Instant::now();
    let applied = build.join("applied.bc");
    checked(
        Command::new("clang-18")
            .args(["-O0", "-Xclang", "-disable-O0-optnone", "-gline-tables-only", "-fno-discard-value-names"])
            .arg(format!("-I{}", workspace.join("front/include").display()))
            .args(["-emit-llvm", "-c"])
            .arg(experiment.join("applied.c"))
            .arg("-o")
            .arg(&applied),
    );
    let linked = build.join("applied-linked.bc");
    checked(Command::new("llvm-link-18").arg(build.join("link/linked.bc")).arg(&applied).arg("-o").arg(&linked));
    let program = build.join("applied.bn");
    let front = checked(
        Command::new(front())
            .arg(&linked)
            .arg("-o")
            .arg(&program)
            .arg(format!("--stack-bytes={STACK_BYTES}"))
            .arg(format!("--region-budget={QUERY_BUDGET}")),
    );
    let diagnostics = String::from_utf8_lossy(&front.stderr);
    println!("front: {:.1} s; diagnostics: {diagnostics:?}", lowering.elapsed().as_secs_f64());
    let module = bennett_ir::load(&std::fs::read_to_string(&program).expect("device text")).expect("valid");

    let weight = Complex64::new(1.0 / (ROWS as f64).sqrt(), 0.0);
    for (query, name) in names.iter().enumerate() {
        let running = Instant::now();
        let entry = format!("experiment_applied_{query}");
        let options = Options { budget: Some(RUN_BUDGET), ..options(0) };
        let (before, after) = state_around_first_apply(&module, &entry, options);
        assert_eq!(after.classical.memory, before.classical.memory, "{name}: the query left a write behind");
        let columns = registers(&module, &entry);
        let at = |n: &str| columns.iter().position(|c| c == n).unwrap_or_else(|| panic!("no %{n} in {columns:?}"));
        let (row, result, fault) = (at("row"), at("result"), at("result.fault"));
        assert_eq!(after.quantum.amplitudes.len(), ROWS as usize, "{name}");
        for (config, amplitude) in &after.quantum.amplitudes {
            let want = expected[&(query, config[row])];
            assert_eq!((config[result], config[fault]), (want, 0), "{name}, row {}", config[row]);
            assert!((amplitude - weight).norm() < 1e-12, "{name}, row {}: {amplitude}", config[row]);
        }
        let function = module.function_named(&entry).expect("entry");
        let mut whole = Executor::new(&module, function, &[], options).expect("executor");
        let outcome = whole.run();
        let stats = whole.stats();
        println!(
            "steps {name}: control {} applied {} (summed over configurations), outcome {outcome:?}",
            stats.steps, stats.region_steps
        );
        println!("{name}: 8 rows equal to native, memory unchanged ({:.1} s)", running.elapsed().as_secs_f64());
    }
    println!("total {:.1} s", start.elapsed().as_secs_f64());
}
