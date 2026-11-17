//! Code without gates whose control flow depends on a qubit, written inline, that writes qubit variables:
//! one applied function for all of its results (docs/DESIGN.md, "Code under a qubit"). Each body of
//! `region_whole.c` runs natively for every value of x; on the device x is in uniform superposition, and the
//! state just before the first measurement must hold, for each x, exactly the native values, with every other
//! register (results, fault registers) zero.
use std::process::Command;
use std::sync::OnceLock;

use bennett_exec::{Outcome, RunError};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, column, lower, lower_text, lower_text_with_budget, native_build, registers,
    run, state_before_first_measurement, write_c,
};
use bennett_ir::Fault;

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];
const VALUES: u64 = 8;

/// For each x: a, b, c of `nested`, y of `parity`, c of `known`.
fn native() -> &'static [[u64; 5]; VALUES as usize] {
    static TABLE: OnceLock<[[u64; 5]; VALUES as usize]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let executable = native_build(&c_dir().join("region_whole.c"), "region-whole", &["-DNATIVE"]);
        std::array::from_fn(|x| {
            let output = Command::new(&executable).arg(x.to_string()).output().expect("native run");
            assert!(output.status.success(), "native x={x}: {output:?}");
            String::from_utf8(output.stdout)
                .expect("native output")
                .split_whitespace()
                .map(|word| word.parse().expect("native value"))
                .collect::<Vec<u64>>()
                .try_into()
                .expect("five native values")
        })
    })
}

/// Checks the state of `entry` before its first measurement: for each x one configuration of amplitude
/// 1/sqrt(8) whose registers `named` hold `expected(x)`, every other register zero.
fn check(
    module: &bennett_ir::ValidModule,
    entry: &str,
    named: &[&str],
    expected: impl Fn(usize) -> Vec<u64>,
    mode: &str,
) {
    let state = state_before_first_measurement(module, entry, &[]);
    let columns = registers(module, entry);
    let x = column(&columns, "x");
    let named: Vec<usize> = named.iter().map(|name| column(&columns, name)).collect();
    assert_eq!(state.quantum.amplitudes.len(), VALUES as usize, "{mode} @{entry}: configurations");
    let mut seen = [false; VALUES as usize];
    for (config, amplitude) in &state.quantum.amplitudes {
        let value = usize::try_from(config[x]).expect("x");
        assert!(value < VALUES as usize && !seen[value], "{mode} @{entry}: x={value} twice or out of range");
        seen[value] = true;
        assert!((amplitude.norm() - 1.0 / (VALUES as f64).sqrt()).abs() < 1e-12, "{mode} @{entry}: amplitude");
        let actual: Vec<u64> = named.iter().map(|&c| config[c]).collect();
        assert_eq!(actual, expected(value), "{mode} @{entry}, x={value}: {columns:?} = {config:?}");
        for (c, name) in columns.iter().enumerate() {
            if c != x && !named.contains(&c) {
                assert_eq!(config[c], 0, "{mode} @{entry}, x={value}: register %{name} not returned to 0");
            }
        }
    }
}

#[test]
fn nested_loops_parity_and_known_start_agree_with_native_c_in_every_mode() {
    let table = native();
    for (mode, clang, flags) in MODES {
        let module = lower(&c_dir().join("region_whole.c"), &format!("region-whole-{mode}"), clang, flags);
        check(&module, "nested_entry", &["a", "b", "c"], |x| table[x][..3].to_vec(), mode);
        check(&module, "parity_entry", &["y"], |x| vec![table[x][3]], mode);
        check(&module, "known_entry", &["c"], |x| vec![table[x][4]], mode);
    }
}

/// The two nested loops whose trip counts depend on x run under one budget, inside one applied function:
/// the control program has no loop at all, and the pair of applies for the two XORed results each take the
/// whole region.
#[test]
fn the_nested_loops_are_one_applied_function_with_one_budget() {
    let text = lower_text(&c_dir().join("region_whole.c"), "region-whole-shape", CLANG_FLAGS, &[]).expect("lowers");
    let body = text.split("func @nested_entry(").nth(1).expect("@nested_entry").split("\n}\n").next().expect("body");
    // No block of the control program is entered from a later one: every loop of the C is inside an apply.
    let labels: Vec<&str> = body.lines().filter_map(|line| line.strip_suffix(':')).collect();
    let mut current = 0;
    for line in body.lines() {
        if let Some(label) = line.strip_suffix(':') {
            current = labels.iter().position(|l| *l == label).expect("label");
        } else if let Some(targets) = line.trim().strip_prefix("br ") {
            for target in targets.split(", ").filter_map(|t| t.split_whitespace().last()) {
                if let Some(index) = labels.iter().position(|l| *l == target) {
                    assert!(index > current, "@nested_entry jumps back to {target}: a loop in the control program");
                }
            }
        }
    }
    let applies = body.lines().filter(|line| line.trim().starts_with("apply ")).count();
    assert!(applies >= 2, "@nested_entry: {applies} applies");
}

/// The smallest budget with which no value of x faults in @nested_entry, measured by bisection
/// (`SMALLEST_BUDGET=search front/test.sh --test region_whole one_budget -- --nocapture`).
const SMALLEST_BUDGET: u64 = 128;

/// How many values of x fault in the first `apply` that faults in @nested_entry with `budget`.
fn faulting(budget: u64) -> usize {
    let source = c_dir().join("region_whole.c");
    let text = lower_text_with_budget(&source, &format!("region-whole-budget-{budget}"), CLANG_FLAGS, &[], budget)
        .expect("lowers");
    let module = bennett_ir::load(&text).expect("valid");
    match run(&module, "nested_entry", &[], 0) {
        Outcome::Stopped(RunError::RegionFault { fault: Fault::Budget, faulting, .. }) => faulting,
        Outcome::Returned(_) => 0,
        other => panic!("budget {budget}: {other:?}"),
    }
}

/// One budget for everything the region does, both loops included: with the smallest sufficient budget no
/// value faults, and one step less makes some, not all, of the values fault (the longest runs).
#[test]
fn one_budget_covers_both_loops_and_one_step_less_faults_the_longest_inputs() {
    if std::env::var("SMALLEST_BUDGET").is_ok_and(|v| v == "search") {
        let (mut low, mut high) = (1, bennett_front_tests::REGION_BUDGET);
        while high - low > 1 {
            let middle = (low + high) / 2;
            if faulting(middle) == 0 { high = middle } else { low = middle }
        }
        println!("smallest budget for every x: {high} steps");
    }
    assert_eq!(faulting(SMALLEST_BUDGET), 0, "budget {SMALLEST_BUDGET}");
    let short = faulting(SMALLEST_BUDGET - 1);
    assert!((1..VALUES as usize).contains(&short), "budget {}: {short} of {VALUES} values fault", SMALLEST_BUDGET - 1);
}

#[test]
fn a_loop_that_does_more_than_xor_into_a_variable_of_unknown_value_is_refused_by_name() {
    let source = write_c(
        "region-whole-refused",
        r#"#include "bennett.h"
unsigned long long refused_entry(void) {
    unsigned char x = 0, y = 0;
    bennett_h(bennett_bit(&x, 0));
    bennett_h(bennett_bit(&x, 1));
    bennett_x(bennett_bit(&y, 2));
    for (unsigned t = x; t != 0; t >>= 1) y = (unsigned char)(y * y + t);
    return bennett_measure(&x) | bennett_measure(&y) << 8;
}
"#,
    );
    let error = lower_text(&source, "region-whole-refused", CLANG_FLAGS, &[]).expect_err("refused");
    assert!(
        error.starts_with("not handled yet:")
            && error.contains("'y'")
            && error.contains("old value would have to be kept"),
        "{error}"
    );
}
