//! Fast tests of the actual experiment generator, with a memory-writing fixture
//! instead of SQLite. Real SQLite comparisons are the opt-in experiment.
use bennett_exec::{Executor, Options, Outcome};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn generated(n: usize, query: usize, k: usize) -> String {
    // The fixture checks both the forwarded query index and row value, and writes
    // a global byte that apply must restore. Only row 3 is marked.
    let body = format!(
        r#" %correct = icmp eq i32 %q, {query}
 %marked = icmp eq i32 %row, 3
 %b = and i1 %correct, %marked
 %v = zext i1 %b to i32
 ret %v"#
    );
    generated_with(n, query, k, &body)
}

/// The generator's program over a fixture whose `experiment_query(%q, %row)` body is `body`.
fn generated_with(n: usize, query: usize, k: usize, body: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("python3")
        .arg("-c")
        .arg("import sys; sys.path.insert(0, sys.argv[1]); from scale import program; print(program(sys.stdin.read(), *map(int, sys.argv[2:])))")
        .arg(root.join("experiments/sqlite-on-device"))
        .args([query.to_string(), n.to_string(), k.to_string()])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("python3");
    let base = format!(
        r#"memory 65536
func @experiment_setup() -> i32 {{
entry:
 store i8 91, 0
 ret 0
}}
func @experiment_query(i32 %q, i32 %row) -> i32 {{
entry:
 store i32 %row, 0
{body}
}}
"#
    );
    child.stdin.take().expect("stdin").write_all(base.as_bytes()).expect("write fixture");
    let output = child.wait_with_output().expect("generator");
    assert!(output.status.success());
    String::from_utf8(output.stdout).expect("device text")
}

#[test]
fn generated_sql_entries_preserve_memory_reverse_and_match_grover_amplitudes() {
    for n in [4, 8, 16, 32] {
        for q in [2, 4, 5, 6, 7] {
            let text = generated(n, q, 2);
            let module = bennett_ir::load(&text).expect("generated entries validate");
            for name in ["scale_classical", "scale_zero", "scale_one", "scale_grover"] {
                let entry = module.function_named(name).expect("entry");
                let mut exec = Executor::new(
                    &module,
                    entry,
                    &[],
                    Options { reversible: true, max_configurations: Some(2 * n), ..Options::default() },
                )
                .expect("start");
                let initial = exec.snapshot();
                if name == "scale_grover" {
                    loop {
                        let before = exec.snapshot();
                        let frame = before.classical.frames.last().expect("live frame");
                        let inst = module.function(frame.func).block(frame.block).insts.get(frame.inst);
                        if matches!(inst, Some(bennett_ir::Inst::Measure { .. })) {
                            assert_eq!(before.classical.memory[0], 91);
                            assert!(before.classical.memory[1..4].iter().all(|&v| v == 0));
                            assert!(before.worlds.is_empty());
                            assert_eq!(exec.stats().live_qubits, n.ilog2());
                            let theta = (1.0 / (n as f64).sqrt()).asin();
                            for row in 0..n {
                                let want = if row == 3 {
                                    (5.0 * theta).sin()
                                } else {
                                    (5.0 * theta).cos() / ((n - 1) as f64).sqrt()
                                };
                                let got = before.quantum.amplitude(&[row as u64]);
                                assert!(
                                    (got.re - want).abs() < 1e-12 && got.im.abs() < 1e-12,
                                    "N={n} q={q} row={row}: {got} != {want}"
                                );
                            }
                            break; // Reverse before measurement; it cannot be undone.
                        }
                        assert!(exec.step().is_none());
                    }
                } else {
                    assert_eq!(exec.run(), Outcome::Returned(Some(0)));
                    if name == "scale_classical" {
                        let want: Vec<_> = (0..n).map(|r| if r == 3 { b'1' } else { b'0' }).collect();
                        assert_eq!(exec.tape(), want);
                    } else {
                        let state = exec.snapshot();
                        assert_eq!(state.classical.memory[0], 91);
                        for row in 0..n {
                            let out = u64::from(name == "scale_one");
                            let got = state.quantum.amplitude(&[row as u64, out, 0]);
                            assert!((got.re - 1.0 / (n as f64).sqrt()).abs() < 1e-12);
                        }
                    }
                }
                assert_eq!(exec.stats().region_faults, 0);
                exec.reverse().expect("full reverse");
                let restored = exec.snapshot();
                assert_eq!(restored.classical, initial.classical);
                assert_eq!(restored.worlds, initial.worlds);
                assert!(restored.quantum.distance(&initial.quantum).expect("register table") < 1e-12);
            }
        }
    }
}

/// Runs `entry` of `module` reversibly until the instruction about to run satisfies `stop`, returning the
/// snapshot there (the executor is then reversed to its start, which must restore it exactly).
fn state_before(
    module: &bennett_ir::ValidModule,
    entry: &str,
    n: usize,
    mut stop: impl FnMut(&bennett_ir::Inst) -> bool,
) -> bennett_exec::Snapshot {
    let func = module.function_named(entry).expect("entry");
    let options = Options { reversible: true, max_configurations: Some(2 * n), ..Options::default() };
    let mut exec = Executor::new(module, func, &[], options).expect("start");
    let initial = exec.snapshot();
    let state = loop {
        let state = exec.snapshot();
        let frame = state.classical.frames.last().expect("live frame");
        if module.function(frame.func).block(frame.block).insts.get(frame.inst).is_some_and(&mut stop) {
            break state;
        }
        assert!(exec.step().is_none(), "{entry} stopped before the instruction");
    };
    assert_eq!(exec.stats().region_faults, 0);
    exec.reverse().expect("full reverse");
    let restored = exec.snapshot();
    assert_eq!(restored.classical, initial.classical);
    assert!(restored.quantum.distance(&initial.quantum).expect("register table") < 1e-12);
    state
}

#[test]
fn an_absent_key_in_superposition_is_not_marked() {
    // The driver's experiment_query returns -1 when the key is absent from the table (no row). Here row 5 is
    // absent, row 6 returns the count 3, row 3 returns 1 (SQL true) and every other row 0. Only row 3 is
    // marked: the lookup compares the result with 1. Truncating it to its low bit marked rows 3, 5 and 6.
    let (n, q, k) = (8, 2, 2);
    let body = r#" %five = icmp eq i32 %row, 5
 %six = icmp eq i32 %row, 6
 %three = icmp eq i32 %row, 3
 %a = select i32 %three, 1, 0
 %b = select i32 %six, 3, %a
 %v = select i32 %five, -1, %b
 ret %v"#;
    let module = bennett_ir::load(&generated_with(n, q, k, body)).expect("generated entries validate");
    let marked = |row: usize| row == 3;

    // Classical truth table: one '0'/'1' per row.
    let entry = module.function_named("scale_classical").expect("entry");
    let mut exec = Executor::new(&module, entry, &[], Options::default()).expect("start");
    assert_eq!(exec.run(), Outcome::Returned(Some(0)));
    assert_eq!(exec.tape(), (0..n).map(|r| if marked(r) { b'1' } else { b'0' }).collect::<Vec<_>>());

    // One coherent lookup over the uniform row register, the absent key among the rows: before the second
    // apply, |row, out, fault> has amplitude 1/sqrt(N) at out = marked(row), fault = 0, and nothing else.
    let mut applies = 0;
    let state = state_before(&module, "scale_zero", n, |inst| {
        applies += usize::from(matches!(inst, bennett_ir::Inst::Apply { .. }));
        applies == 2
    });
    for row in 0..n {
        for out in [0, 1] {
            let want = if out == u64::from(marked(row)) { 1.0 / (n as f64).sqrt() } else { 0.0 };
            let got = state.quantum.amplitude(&[row as u64, out, 0]);
            assert!((got.re - want).abs() < 1e-12 && got.im.abs() < 1e-12, "row={row} out={out}: {got} != {want}");
        }
    }

    // Grover, k=2 iterations of D·O from the uniform state, D = 2|s><s| - I, O = I - 2 Σ_marked |r><r|.
    // Dense calculation: start a_r = 1/sqrt(N); each iteration negates the marked amplitudes, then maps
    // a_r to 2·mean(a) - a_r. With M=1 of N=8 this gives sin(5θ) at row 3 and cos(5θ)/sqrt(7) elsewhere,
    // θ = asin(1/sqrt(8)) (amplitudes 0.9722 and -0.0884); three marked rows would give sin(5θ')/sqrt(3),
    // θ' = asin(sqrt(3/8)), instead.
    let mut dense = vec![1.0 / (n as f64).sqrt(); n];
    for _ in 0..k {
        for (row, a) in dense.iter_mut().enumerate() {
            if marked(row) {
                *a = -*a;
            }
        }
        let mean = dense.iter().sum::<f64>() / n as f64;
        dense.iter_mut().for_each(|a| *a = 2.0 * mean - *a);
    }
    let theta = (1.0 / (n as f64).sqrt()).asin();
    let phase = (2 * k + 1) as f64 * theta;
    for (row, &a) in dense.iter().enumerate() {
        let closed = if marked(row) { phase.sin() } else { phase.cos() / ((n - 1) as f64).sqrt() };
        assert!((a - closed).abs() < 1e-12, "dense row={row}: {a} != {closed}");
    }
    let state = state_before(&module, "scale_grover", n, |inst| matches!(inst, bennett_ir::Inst::Measure { .. }));
    assert_eq!(state.classical.memory[0], 91);
    for (row, &want) in dense.iter().enumerate() {
        let got = state.quantum.amplitude(&[row as u64]);
        assert!((got.re - want).abs() < 1e-12 && got.im.abs() < 1e-12, "Grover row={row}: {got} != {want}");
    }
}

#[test]
fn an_entry_frame_that_does_not_fit_returns_stack_overflow() {
    let module =
        bennett_ir::load("memory 1\nfunc @main() -> i32 {\nentry:\n %v = add i32 1, 2\n ret %v\n}\n").expect("valid");
    let entry = module.function_named("main").expect("entry");
    let mut exec = Executor::new(&module, entry, &[], Options::default()).expect("start");
    assert_eq!(exec.outcome(), Some(Outcome::Faulted(bennett_ir::Fault::StackOverflow)));
    assert_eq!(exec.run(), Outcome::Faulted(bennett_ir::Fault::StackOverflow));
}
