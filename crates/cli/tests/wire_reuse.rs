//! A register that reuses a freed range starts in zero and has a new name in the OpenQASM trace.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use bennett_exec::{Executor, Options, Outcome};

fn run(program: &Path, arguments: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_bennett"))
        .arg("run")
        .arg(program)
        .args(arguments)
        .output()
        .expect("ran bennett");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("device report")
}

fn result(report: &str) -> u128 {
    report
        .lines()
        .find_map(|line| line.strip_prefix("result: "))
        .expect("scalar result")
        .split_whitespace()
        .next()
        .expect("unsigned result")
        .parse()
        .expect("u128 result")
}

#[test]
fn a_reused_range_has_fresh_qasm_wires_and_agrees_with_the_simulator() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch directory");
    let program = dir.path().join("reuse.bn");
    let source = "memory 128
func @main() -> i128 {
entry:
    %old = qalloc 65
    %guard = qalloc 1
    x %old[64]
    x %guard
    x %old[64]
    qfree %old
    %new = qalloc 65
    x %new
    h %new[64]
    x %new ctrl %new[64]
    %value = measure %new
    qfree %new
    h %guard
    %guard_bit = measure %guard
    %guard_wide = zext i1 %guard_bit to i128
    %guard_shifted = shl i128 %guard_wide, 1
    %value_wide = zext i65 %value to i128
    %result = or i128 %value_wide, %guard_shifted
    ret %result
}";
    std::fs::write(&program, source).expect("write the program");
    let module = bennett_ir::load(source).expect("valid device program");
    let entry = module.function_named("main").expect("entry function");
    let counted = run(&program, &["--device", "count"]);
    // Six single-qubit gates and one CNOT. The guard is live beside either 65-qubit register.
    assert!(
        counted.contains("one-qubit gates: 6\ntwo-qubit gates: 1\nToffoli gates: 0\npeak qubits: 66\n"),
        "{counted}"
    );
    let mut seen = BTreeSet::new();
    for seed in 0..12 {
        let mut simulator = Executor::new(&module, entry, &[], Options { seed, ..Options::default() }).expect("starts");
        for _ in 0..5 {
            assert_eq!(simulator.step(), None);
        }
        let allocated = simulator.snapshot().quantum.wires;
        assert_eq!(allocated, [(0..65, 0), ((2 << 32)..(2 << 32) + 1, 128)]);
        assert_eq!(simulator.step(), None, "free the zeroed register");
        assert_eq!(simulator.snapshot().quantum.wires, allocated[1..]);
        assert_eq!(simulator.step(), None, "allocate the new register across the freed columns");
        let reused = simulator.snapshot().quantum;
        assert_eq!(reused.wires, allocated);
        assert_eq!(reused.amplitudes.len(), 1);
        assert_eq!(reused.amplitudes[0].0, [0, 0, 1], "the reused register starts in zero");
        let Outcome::Returned(Some(expected)) = simulator.run() else { panic!("seed {seed}: run did not return") };
        let stats = simulator.stats();
        assert_eq!((stats.live_qubits, stats.peak_live_qubits, stats.leftover_qubits), (66, 66, 65));

        let path = dir.path().join(format!("reuse-{seed}.qasm"));
        let seed = seed.to_string();
        // The low and high bits of the new register are opposite. The guard contributes bit 1.
        let without_guard = expected & !2;
        assert!(without_guard == 1 || without_guard == 1 << 64, "seed {seed}: {expected}");
        seen.insert(expected);
        let qasm =
            run(&program, &["--device", "qasm", "--seed", &seed, "--qasm-output", path.to_str().expect("UTF-8 path")]);
        assert_eq!(result(&qasm), expected, "seed {seed}");
        let trace = std::fs::read_to_string(path).expect("OpenQASM trace");
        assert_eq!(
            trace.lines().filter(|line| line.starts_with("qubit[") || line.starts_with("reset ")).collect::<Vec<_>>(),
            ["qubit[65] q0;", "reset q0;", "qubit[1] q1;", "reset q1;", "qubit[65] q2;", "reset q2;"],
            "seed {seed}"
        );
        assert!(trace.contains("\nctrl @ x q2[64], q2[0];\n"), "seed {seed}: the reused range names q2");
        let mut measurements = trace.lines().filter(|line| line.starts_with("measure "));
        for bit in 0..65 {
            let outcome = if bit == 1 { 0 } else { (expected >> bit) & 1 };
            assert_eq!(
                measurements.next(),
                Some(format!("measure q2[{bit}]; // outcome {outcome}").as_str()),
                "seed {seed}, q2[{bit}]"
            );
        }
        assert_eq!(
            measurements.next(),
            Some(format!("measure q1[0]; // outcome {}", (expected >> 1) & 1).as_str()),
            "seed {seed}, guard"
        );
        assert_eq!(measurements.next(), None, "seed {seed}");
    }
    assert_eq!(seen, BTreeSet::from([1, 3, 1 << 64, (1 << 64) + 2]));
}
