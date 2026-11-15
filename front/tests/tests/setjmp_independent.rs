//! Independent native oracles for defined locals, control expressions, copied/nested records and VLAs.
use std::collections::BTreeMap;
use std::process::Command;

use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, entry, lower, native_build, options};

#[test]
fn directed_jumps_agree_at_both_native_optimization_levels_and_un_run() {
    let source = c_dir().join("setjmp_independent.c");
    let mut references = Vec::new();
    for opt in ["-O0", "-O2"] {
        let executable = native_build(&source, &format!("setjmp_independent{opt}"), &["-DNATIVE", opt]);
        let output = Command::new(executable).output().expect("runs natively");
        assert!(output.status.success());
        let rows: BTreeMap<(String, u64), u128> = String::from_utf8(output.stdout)
            .expect("native text")
            .lines()
            .map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                ((fields[0].to_owned(), fields[1].parse().expect("input")), fields[2].parse().expect("result"))
            })
            .collect();
        assert_eq!(rows.len(), 8 * 16);
        references.push(rows);
    }
    assert_eq!(references[0], references[1], "only defined C values are observed");
    let builds: [(&[&str], &[&str], &str); 4] = [
        (CLANG_FLAGS, &[], "o0"),
        (CLANG_O2_FLAGS, &[], "o2"),
        (&["-O0"], &[], "optnone"),
        (CLANG_FLAGS, &["--taint-all"], "taint"),
    ];
    for (clang, front, suffix) in builds {
        let module = lower(&source, &format!("setjmp_independent.{suffix}"), clang, front);
        for ((name, x), expected) in &references[0] {
            let mut exec =
                Executor::new(&module, entry(&module, name), &[*x], Options { reversible: true, ..options(0) })
                    .expect("starts");
            let start = exec.snapshot();
            assert_eq!(exec.run(), Outcome::Returned(Some(*expected)), "{name}({x}), {suffix}");
            let steps = exec.stats().steps;
            assert_eq!(exec.reverse().expect("un-runs"), steps);
            assert_eq!(exec.snapshot(), start, "{name}({x}), {suffix}: exact reversal");
        }
    }
}
