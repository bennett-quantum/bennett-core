//! Fast tests of the actual experiment generator, with a memory-writing fixture
//! instead of SQLite. Real SQLite comparisons are the opt-in experiment.
use bennett_exec::{Executor, Options, Outcome};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn generated(n: usize, query: usize, k: usize) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("python3")
        .arg("-c")
        .arg("import sys; sys.path.insert(0, sys.argv[1]); from scale import program; print(program(sys.stdin.read(), *map(int, sys.argv[2:])))")
        .arg(root.join("experiments/sqlite-on-device"))
        .args([query.to_string(), n.to_string(), k.to_string()])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("python3");
    // The fixture checks both the forwarded query index and row value, and writes
    // a global byte that apply must restore. Only row 3 is marked.
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
 %correct = icmp eq i32 %q, {query}
 %marked = icmp eq i32 %row, 3
 %b = and i1 %correct, %marked
 %v = zext i1 %b to i32
 ret %v
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

#[test]
fn an_entry_frame_that_does_not_fit_returns_stack_overflow() {
    let module =
        bennett_ir::load("memory 1\nfunc @main() -> i32 {\nentry:\n %v = add i32 1, 2\n ret %v\n}\n").expect("valid");
    let entry = module.function_named("main").expect("entry");
    let mut exec = Executor::new(&module, entry, &[], Options::default()).expect("start");
    assert_eq!(exec.outcome(), Some(Outcome::Faulted(bennett_ir::Fault::StackOverflow)));
    assert_eq!(exec.run(), Outcome::Faulted(bennett_ir::Fault::StackOverflow));
}
