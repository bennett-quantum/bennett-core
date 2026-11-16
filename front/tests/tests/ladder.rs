//! NorthStar §8, C rungs 1–3. These exercise the available executor, in both
//! modes; they do not turn two modes into two independent implementations.
use bennett_exec::{Executor, Options, Outcome, Snapshot};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, entry, lower, root};
use bennett_ir::ValidModule;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

// Reuse the independently written interpreter unchanged. It imports no
// bennett-exec semantics; dense supplies its separate integer operations.
#[allow(dead_code)]
#[path = "../../../crates/exec/tests/oracle_support/dense.rs"]
mod dense;
#[allow(dead_code)]
#[path = "../../../crates/exec/tests/oracle_support/worlds.rs"]
mod reference;

fn fixtures() -> PathBuf {
    root().join("experiments/northstar-ladder/fixtures")
}

fn scratch() -> PathBuf {
    let path = root().join(".tmp/northstar-ladder");
    std::fs::create_dir_all(&path).expect("create experiment directory");
    path
}

fn checked(command: &mut Command) -> Output {
    let output = command.output().unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(output.status.success(), "{command:?}: {:?}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
    output
}

fn native(name: &str, sources: &[&str], flags: &[&str]) -> Vec<u8> {
    let executable = scratch().join(name);
    let mut compile = Command::new(std::env::var_os("CLANG").unwrap_or_else(|| "clang-18".into()));
    compile.args(flags);
    for source in sources {
        compile.arg(fixtures().join(source));
    }
    checked(compile.arg("-o").arg(&executable));
    let output = checked(&mut Command::new(executable));
    assert!(output.stderr.is_empty(), "native diagnostics: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}

fn options(reversible: bool) -> Options {
    Options { reversible, budget: Some(10_000_000), max_configurations: Some(1), ..Options::default() }
}

// Compare every field, but report the first differing byte rather than dumping
// megabytes of memory if a regression appears. Equality is still byte-exact.
fn assert_snapshot(mut actual: Snapshot, expected: &Snapshot, context: &str) {
    if actual.classical.memory != expected.classical.memory {
        let first = actual.classical.memory.iter().zip(&expected.classical.memory).position(|(a, b)| a != b);
        panic!(
            "{context}: memory lengths {} vs {}, first differing byte {first:?}: actual {:?}, expected {:?}",
            actual.classical.memory.len(),
            expected.classical.memory.len(),
            first.and_then(|i| actual.classical.memory.get(i)),
            first.and_then(|i| expected.classical.memory.get(i)),
        );
    }
    let mut rest = expected.clone();
    actual.classical.memory.clear();
    rest.classical.memory.clear();
    assert_eq!(actual, rest, "{context}: remaining complete state");
}

#[test]
#[ignore = "full Int8 sweep; run experiments/northstar-ladder/run.py explicitly"]
fn scalar_c_exhaustive_int8_native_forward_and_reverse() {
    let start = Instant::now();
    let sources = ["scalars.c", "scalar_native.c"];
    let expected = native("scalar-o2", &sources, &["-O2"]);
    let sanitized = native("scalar-sanitized", &sources, &["-O0", "-fsanitize=undefined", "-fno-sanitize-recover=all"]);
    assert_eq!(expected.len(), 131_840);
    assert_eq!(expected, sanitized, "native O2 and O0/UBSan differ");
    let mut cases = 0;
    let mut steps = 0;
    for (tag, flags) in [("o0", CLANG_FLAGS), ("o2", CLANG_O2_FLAGS)] {
        let p = lower(&fixtures().join("scalars.c"), &format!("ladder-scalar-{tag}"), flags, &["--entry=r1_sat_add"]);
        let mut offset = 0;
        for (name, arity) in
            [("r1_sat_add", 2), ("r1_gcd", 2), ("r1_popcount", 1), ("r1_sign_abs", 1), ("r1_lookup", 1)]
        {
            let function = entry(&p, name);
            for a in 0..256 {
                for b in 0..if arity == 2 { 256 } else { 1 } {
                    let args: &[u64] = if arity == 2 { &[a, b] } else { &[a] };
                    let want = Outcome::Returned(Some(u128::from(expected[offset])));
                    for reversible in [false, true] {
                        let mut e = Executor::new(&p, function, args, options(reversible)).expect("scalar starts");
                        let before = reversible.then(|| e.snapshot());
                        assert_eq!(e.run(), want, "{tag} {name}({args:?}) reversible={reversible}");
                        steps += e.stats().steps;
                        assert!(e.tape().is_empty(), "unexpected scalar console");
                        if let Some(before) = before {
                            let forward_steps = e.stats().steps;
                            assert_eq!(e.reverse().expect("scalar reverse"), forward_steps);
                            assert_snapshot(e.snapshot(), &before, &format!("reset: {tag} {name}({args:?})"));
                            assert!(!e.step_back().expect("empty history"));
                        }
                        cases += 1;
                    }
                    offset += 1;
                }
            }
        }
        assert_eq!(offset, expected.len(), "every native byte consumed");
    }
    println!(
        "scalar: {cases} native/device comparisons, {} complete reversals, {steps} forward steps, zero faults/exclusions, {:.3}s",
        cases / 2,
        start.elapsed().as_secs_f64()
    );
}

#[test]
#[ignore = "independent snapshot interpreter exhaustively runs and reverses the complete Int8 domain"]
fn scalar_c_independent_exhaustive_int8_native_and_reverse() {
    let expected = native("scalar-reference-native", &["scalars.c", "scalar_native.c"], &["-O2"]);
    assert_eq!(expected.len(), 131_840);
    let start = Instant::now();
    let mut cases = 0;
    for (tag, flags) in [("o0", CLANG_FLAGS), ("o2", CLANG_O2_FLAGS)] {
        let p =
            lower(&fixtures().join("scalars.c"), &format!("ladder-reference-{tag}"), flags, &["--entry=r1_sat_add"]);
        let mut offset = 0;
        for (name, arity) in
            [("r1_sat_add", 2), ("r1_gcd", 2), ("r1_popcount", 1), ("r1_sign_abs", 1), ("r1_lookup", 1)]
        {
            let model = reference::Model::new(&p, name);
            for a in 0..256 {
                for b in 0..if arity == 2 { 256 } else { 1 } {
                    let args: &[u128] = if arity == 2 { &[a, b] } else { &[a] };
                    let before = reference::basis(model.initial(name, args));
                    let mut state = model.run(before.clone(), 10_000);
                    assert_eq!(state.len(), 1, "{tag} {name}({args:?}): one basis world");
                    let world = state.keys().next().expect("one world");
                    assert_eq!(
                        world.classical.finish,
                        Some(reference::Finish::Returned(Some(u128::from(expected[offset])))),
                        "{tag} {name}({args:?})"
                    );
                    let records = world.history.len();
                    assert!(records > 0);
                    for _ in 0..records {
                        state = model.undo_state(&state).expect("reference scalar undo");
                    }
                    assert_eq!(state, before, "reference reset {tag} {name}({args:?})");
                    cases += 1;
                    offset += 1;
                }
            }
            println!(
                "independent scalar {tag} {name}: native agreement and exact reversal on complete Int8 domain, {:.3}s elapsed",
                start.elapsed().as_secs_f64()
            );
        }
        assert_eq!(offset, expected.len());
    }
    println!(
        "independent scalar: {cases} native comparisons and exact reversals, zero faults/exclusions, {:.3}s",
        start.elapsed().as_secs_f64()
    );
}

fn musl_program(name: &str, source: &str, level: &str) -> ValidModule {
    assert!(root().join("target/musl/libc.a").is_file(), "build musl first: python3 rt/musl/build.py --jobs 2");
    let program = scratch().join(format!("{name}{level}.bn"));
    checked(
        Command::new("python3")
            .arg(root().join("rt/musl/link.py"))
            .arg(fixtures().join(source))
            .arg(format!("--clang-flag={level}"))
            .arg("-o")
            .arg(&program),
    );
    load(&program)
}

fn load(path: &Path) -> ValidModule {
    bennett_ir::load(&std::fs::read_to_string(path).expect("device text")).expect("valid device program")
}

#[test]
#[ignore = "musl must be built first; archived ten-case table experiment"]
fn hashtable_native_forward_and_byte_exact_reverse() {
    let expected = native("table-o2", &["hashtable.c", "main_golden.c"], &["-O2"]);
    let sanitized = native(
        "table-sanitized",
        &["hashtable.c", "main_golden.c"],
        &["-O0", "-fsanitize=address,undefined", "-fno-sanitize-recover=all"],
    );
    assert_eq!(expected, sanitized, "native O2 and O0/ASan/UBSan differ");
    let text = std::str::from_utf8(&expected).expect("native table text");
    println!("native table results:\n{text}");
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 10);
    for (index, line) in lines.iter().enumerate() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let n = [0, 1, 7, 64, 1000][index % 5];
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0], if index >= 5 { "grow" } else { "basic" });
        assert_eq!(fields[1].parse::<u64>().expect("n"), n);
        fields[2].parse::<i64>().expect("full i64 checksum");
    }
    for level in ["-O0", "-O2"] {
        let p = musl_program("table", "table_entry.c", level);
        for reversible in [false, true] {
            let mut e =
                Executor::new(&p, entry(&p, "__bennett_entry"), &[], options(reversible)).expect("table starts");
            let before = e.snapshot();
            let start = Instant::now();
            let outcome = e.run();
            assert_eq!(
                outcome,
                Outcome::Returned(Some(0)),
                "{level} table reversible={reversible}; completed console: {}",
                String::from_utf8_lossy(e.tape())
            );
            assert_eq!(
                std::str::from_utf8(e.tape()).expect("table output"),
                text,
                "{level} all ten full i64 results, reversible={reversible}"
            );
            let stats = e.stats();
            if reversible {
                assert_eq!(e.reverse().expect("table reverse"), stats.steps);
                assert_snapshot(e.snapshot(), &before, &format!("table reset: {level}"));
                assert!(!e.step_back().expect("empty history"));
            }
            println!(
                "table {level} 10 native results, {} console bytes, reversible={reversible}: {} steps, {} journal bytes, {} memory bytes, {:.3}s",
                expected.len(),
                stats.steps,
                stats.journal_bytes.unwrap_or(0),
                p.memory_size,
                start.elapsed().as_secs_f64()
            );
        }
    }
}

#[test]
#[ignore = "full table on the snapshot interpreter; use run.py to bound the process's memory/time"]
fn hashtable_independent_native_forward_and_byte_exact_reverse() {
    assert_eq!(
        std::env::var("BENNETT_LADDER_REFERENCE_TABLE").as_deref(),
        Ok("bounded"),
        "run via experiments/northstar-ladder/run.py for a bounded reference process"
    );
    let expected = native("table-reference-native", &["hashtable.c", "main_golden.c"], &["-O2"]);
    for level in ["-O0", "-O2"] {
        // The preceding production test generated this exact same linked image.
        let p = load(&scratch().join(format!("table{level}.bn")));
        let model = reference::Model::new(&p, "__bennett_entry");
        let before = reference::basis(model.initial("__bennett_entry", &[]));
        let mut state = before.clone();
        let started = Instant::now();
        let mut steps = 0_usize;
        while state.keys().any(|w| w.classical.finish.is_none()) && steps < 10_000_000 {
            state = model.step(&state);
            steps += 1;
            if steps.is_power_of_two() {
                let world = state.keys().next().expect("world");
                let frame = world.classical.frames.last();
                let site = frame.map(|f| (&p.function(f.function).name, f.block, f.instruction));
                println!(
                    "independent table {level}: {steps} steps, {} history snapshots, {} overlay bytes, site {site:?}, {:.3}s",
                    world.history.len(),
                    world.classical.overlay.len(),
                    started.elapsed().as_secs_f64()
                );
            }
        }
        assert_eq!(state.len(), 1);
        let world = state.keys().next().expect("one world");
        assert_eq!(
            world.classical.finish,
            Some(reference::Finish::Returned(Some(0))),
            "{level}: reference table did not complete, {steps} steps"
        );
        assert_eq!(world.classical.tape, expected, "{level}: all ten native table results");
        let records = world.history.len();
        for _ in 0..records {
            state = model.undo_state(&state).expect("reference table undo");
        }
        assert_eq!(state, before, "{level}: independent table reset");
        println!(
            "independent table {level}: 10 native results, {steps} steps, complete reversal, {:.3}s",
            started.elapsed().as_secs_f64()
        );
    }
}

#[test]
#[ignore = "musl must be built first; hello reset and tape replay experiment"]
fn musl_hello_native_reset_and_deterministic_tape_replay() {
    let expected = native("hello-native", &["hello.c"], &["-O2"]);
    assert_eq!(expected, b"hello, world\n");
    for level in ["-O0", "-O2"] {
        let p = musl_program("hello", "hello.c", level);
        let mut forward = Executor::new(&p, entry(&p, "__bennett_entry"), &[], options(false)).expect("forward start");
        assert_eq!(forward.run(), Outcome::Returned(Some(0)));
        assert_eq!(forward.tape(), expected);
        let mut e = Executor::new(&p, entry(&p, "__bennett_entry"), &[], options(true)).expect("reversible start");
        let reset = e.snapshot();
        assert!(reset.classical.tape.is_empty());
        assert_eq!(e.run(), Outcome::Returned(Some(0)));
        assert_eq!(e.tape(), expected);
        let after = e.snapshot();
        assert_snapshot(after.clone(), &forward.snapshot(), "forward/reversible final state");
        let stats = e.stats();
        for replay in 1..=3 {
            assert_eq!(e.reverse().expect("hello un-run"), stats.steps);
            assert_snapshot(e.snapshot(), &reset, &format!("hello reset {level}, replay {replay}"));
            assert!(!e.step_back().expect("empty history"));
            assert_eq!(e.run(), Outcome::Returned(Some(0)));
            assert_eq!(e.tape(), expected, "native tape replay {replay}");
            assert_snapshot(e.snapshot(), &after, &format!("whole replay state {replay}"));
            assert_eq!(e.stats().journal_bytes, stats.journal_bytes, "replayed journal size");
        }
        e.reverse().expect("last un-run");
        assert_snapshot(e.snapshot(), &reset, "last hello reset");
        println!(
            "hello {level}: {} native bytes, {} steps, {} journal bytes, {} reset memory bytes, 3 identical replays, 4 exact resets",
            expected.len(),
            stats.steps,
            stats.journal_bytes.expect("journal"),
            p.memory_size
        );
    }
}
