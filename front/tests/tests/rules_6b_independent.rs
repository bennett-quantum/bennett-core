//! Independent C-to-executor checks of whole applied regions, controlled gate loops and retained values.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, bitcode, column, entry, front, lower_text, lower_text_with_budget, options, registers,
    root, state_before_first_measurement, write_c,
};
use bennett_ir::{Function, Inst, ValidModule};
use num_complex::Complex64 as C;

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];
const S: f64 = std::f64::consts::FRAC_1_SQRT_2;

fn check_state<const N: usize>(
    module: &ValidModule,
    args: &[u64],
    names: [&str; N],
    expected: &BTreeMap<[u64; N], C>,
    context: &str,
) {
    let columns = registers(module, "entry");
    let outputs = names.map(|name| column(&columns, name));
    let state = state_before_first_measurement(module, "entry", args);
    let mut seen = BTreeSet::new();
    for (values, amplitude) in &state.quantum.amplitudes {
        let key = outputs.map(|i| values[i]);
        assert!(seen.insert(key), "{context}, args {args:?}: duplicate {key:?}");
        let wanted = expected.get(&key).unwrap_or_else(|| panic!("{context}, args {args:?}: unexpected {key:?}"));
        assert!(
            (amplitude - wanted).norm() < 1e-12,
            "{context}, args {args:?}, {key:?}: {amplitude}, expected {wanted}"
        );
        for (index, value) in values.iter().enumerate() {
            if !outputs.contains(&index) {
                assert_eq!(*value, 0, "{context}, args {args:?}: spare column {index}");
            }
        }
    }
    assert_eq!(seen.len(), expected.len(), "{context}, args {args:?}: configurations");
}

/// Counts edges to an ancestor in a depth-first traversal, including self-edges. Each of the
/// reducible loops in these fixtures contributes one such edge.
fn loop_edges(function: &Function) -> usize {
    fn visit(function: &Function, block: usize, visited: &mut [bool], active: &mut [bool]) -> usize {
        visited[block] = true;
        active[block] = true;
        let mut edges = 0;
        function.blocks[block].term.visit_targets(|target| {
            let next = target.block.index();
            if active[next] {
                edges += 1;
            } else if !visited[next] {
                edges += visit(function, next, visited, active);
            }
        });
        active[block] = false;
        edges
    }
    visit(function, 0, &mut vec![false; function.blocks.len()], &mut vec![false; function.blocks.len()])
}

fn before_measurements(function: &Function) -> impl Iterator<Item = &Inst> {
    function.blocks.iter().flat_map(|block| &block.insts).take_while(|inst| !matches!(inst, Inst::Measure { .. }))
}

fn measurement_count(function: &Function) -> usize {
    function.blocks.iter().flat_map(|block| &block.insts).filter(|inst| matches!(inst, Inst::Measure { .. })).count()
}

const WHOLE: &str = r#"#include "bennett.h"
__attribute__((noinline)) static unsigned step(unsigned t, unsigned j) {
    return (t * 29 + j * 11) ^ (t >> 1);
}

unsigned entry(unsigned stop, unsigned start) {
    unsigned char x = 0, parity = 0;
    unsigned short acc = 0;
    bennett_h(bennett_bit(&x, 0));
    bennett_h(bennett_bit(&x, 1));
    bennett_h(bennett_bit(&x, 2));
    bennett_s(bennett_bit(&x, 1));
    bennett_h(bennett_bit(&parity, 0));
    bennett_x(bennett_bit(&acc, 3));
    for (unsigned t = x; t != 0; t >>= 1) {
        for (unsigned j = 0; j <= (t & 3); ++j) {
            if (j == stop) break;
            unsigned delta = step(t, j);
            acc ^= (unsigned short)(start + delta);
            parity ^= delta & 1;
        }
        if (t == 2) break;
    }
    return (unsigned)(bennett_measure(&x) | bennett_measure(&parity) << 8 | bennett_measure(&acc) << 16);
}
"#;

#[test]
fn whole_nested_region_keeps_phases_and_packs_both_results_under_one_budget() {
    let source = write_c("rules-6b-whole", WHOLE);
    for (mode, clang, flags) in MODES {
        let text = lower_text_with_budget(&source, &format!("rules-6b-whole-{mode}"), clang, flags, 512)
            .expect("gate-free region with two XOR updates lowers");
        let module = bennett_ir::load(&text).expect("valid program");
        let control = module.function(entry(&module, "entry"));
        assert_eq!(loop_edges(control), 0, "{mode}: no source loop in control code");
        assert_eq!(
            module.functions.iter().map(measurement_count).sum::<usize>(),
            3,
            "{mode}: only the three explicit measurements"
        );
        let applies: Vec<_> = before_measurements(control)
            .filter_map(|inst| match inst {
                Inst::Apply { out, budget, .. } => Some((out, budget)),
                _ => None,
            })
            .collect();
        assert_eq!(applies.len(), 2, "{mode}: one compute and clear pair for both results");
        assert_eq!(applies[0], applies[1], "{mode}: same destination and one budget for each evaluation");
        let whole = module.function_named("entry.whole").expect("one outlined function");
        assert_eq!(loop_edges(module.function(whole)), 2, "{mode}: both source loops are inside the apply");
        assert!(text.contains("call @step("), "{mode}: ordinary helper call remains");
        for (stop, start) in [(0, 33), (1, 65535), (3, 7), (8, 31)] {
            let mut expected = BTreeMap::new();
            for x in 0..8 {
                for parity in 0..2 {
                    let (mut value, mut acc) = (parity, 8);
                    let mut t = x;
                    while t != 0 {
                        for j in 0..=(t & 3) {
                            if j == stop {
                                break;
                            }
                            let delta = (t * 29 + j * 11) ^ (t >> 1);
                            acc ^= (start + delta) & 65535;
                            value ^= delta & 1;
                        }
                        if t == 2 {
                            break;
                        }
                        t >>= 1;
                    }
                    let amplitude = if x & 2 != 0 { C::new(0.0, 0.25) } else { C::new(0.25, 0.0) };
                    expected.insert([x, value, acc], amplitude);
                }
            }
            check_state(&module, &[stop, start], ["x", "parity", "acc"], &expected, mode);
        }
    }
}

const NESTED_GATES: &str = r#"#include "bennett.h"
unsigned entry(unsigned n, unsigned m, unsigned enable) {
    unsigned char x = 0;
    _Bool y = 0, z = 0;
    bennett_h(bennett_bit(&x, 0));
    bennett_h(bennett_bit(&x, 1));
    bennett_s(bennett_bit(&x, 1));
    bennett_x(&z);
    if (enable && x == 2)
        for (unsigned i = 0; i < n; ++i) {
            bennett_h(&y);
            for (unsigned j = 0; j < m; ++j)
                if ((i + j) & 1) bennett_s(&y);
                else bennett_x(&z);
        }
    return (unsigned)(bennett_measure(&x) | bennett_measure(&y) << 8 | bennett_measure(&z) << 9);
}
"#;

#[test]
fn nested_ordinary_gate_loops_preserve_order_and_complex_amplitudes() {
    let source = write_c("rules-6b-nested-gates", NESTED_GATES);
    for (mode, clang, flags) in MODES {
        let text = lower_text(&source, &format!("rules-6b-nested-gates-{mode}"), clang, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid program");
        let control = module.function(entry(&module, "entry"));
        assert_eq!(loop_edges(control), 2, "{mode}: both source loops remain");
        assert_eq!(
            module.functions.iter().map(measurement_count).sum::<usize>(),
            3,
            "{mode}: only the three explicit measurements"
        );
        let controlled = before_measurements(control)
            .filter(|inst| matches!(inst, Inst::Gate { controls, .. } if !controls.is_empty()))
            .count();
        assert_eq!(controlled, 3, "{mode}: the three source gate sites");
        for (n, m, enable) in [(1, 0, 1), (1, 2, 1), (2, 2, 1), (3, 3, 1), (3, 2, 0)] {
            let mut expected = BTreeMap::new();
            for x in 0..4 {
                let mut y = [C::new(1.0, 0.0), C::new(0.0, 0.0)];
                let mut z = 1;
                if x == 2 && enable != 0 {
                    for i in 0..n {
                        y = [(y[0] + y[1]) * S, (y[0] - y[1]) * S];
                        for j in 0..m {
                            if (i + j) & 1 != 0 {
                                y[1] *= C::i();
                            } else {
                                z ^= 1;
                            }
                        }
                    }
                }
                let phase = if x & 2 != 0 { C::i() } else { C::new(1.0, 0.0) };
                for (bit, amplitude) in y.into_iter().enumerate().filter(|(_, a)| a.norm() > 1e-12) {
                    expected.insert([x, bit as u64, z], phase * amplitude * 0.5);
                }
            }
            check_state(&module, &[n, m, enable], ["x", "y", "z"], &expected, mode);
        }
    }
}

fn run_bounded(module: &ValidModule, args: &[u64], budget: u64) -> Outcome {
    let mut executor =
        Executor::new(module, entry(module, "entry"), args, Options { budget: Some(budget), ..options(0) })
            .expect("starts");
    executor.run()
}

#[test]
fn zero_outer_trip_count_skips_nested_gate_loops() {
    let source = write_c(
        "rules-6b-zero-outer",
        r#"#include "bennett.h"
unsigned entry(unsigned n, unsigned m) {
    _Bool q = 0, y = 0;
    bennett_h(&q);
    if (q)
        for (unsigned i = 0; i < n; ++i)
            for (unsigned j = 0; j < m; ++j)
                bennett_x(&y);
    return (unsigned)bennett_measure(&y);
}
"#,
    );
    let text = lower_text(&source, "rules-6b-zero-outer", CLANG_FLAGS, &[]).expect("lowers");
    let module = bennett_ir::load(&text).expect("valid program");
    for m in [0, 1] {
        assert_eq!(run_bounded(&module, &[0, m], 1_000), Outcome::Returned(Some(0)), "entry(n=0, m={m})");
    }
}

#[test]
fn ordinary_gate_site_position_does_not_spend_the_condition_apply_budget() {
    let source = write_c(
        "rules-6b-late-gate",
        r#"#include "bennett.h"
unsigned entry(unsigned n, unsigned site) {
    _Bool q = 0, y = 0;
    bennett_h(&q);
    if (q)
        for (unsigned i = 0; i < n; ++i)
            if (i == site) bennett_x(&y);
    return (unsigned)bennett_measure(&y);
}
"#,
    );
    let text = lower_text_with_budget(&source, "rules-6b-late-gate", CLANG_FLAGS, &[], 32).expect("lowers");
    let module = bennett_ir::load(&text).expect("valid program");
    for site in 0..=64 {
        let n = site + 1;
        let outcome = run_bounded(&module, &[n, site], 2_000);
        assert!(
            matches!(outcome, Outcome::Returned(Some(0 | 1))),
            "entry(n={n}, site={site}), apply budget 32: {outcome:?}"
        );
    }
}

#[test]
fn results_wider_than_one_register_still_use_one_applied_function() {
    let source = write_c(
        "rules-6b-wide-results",
        r#"#include "bennett.h"
unsigned long long entry(void) {
    _Bool q = 0, b = 0;
    unsigned long long a = 0;
    bennett_h(&q);
    if (q) { a = 1ULL << 63; b = 1; }
    return bennett_measure(&a) | bennett_measure(&b);
}
"#,
    );
    let text = lower_text(&source, "rules-6b-wide-results", CLANG_FLAGS, &[]).expect("lowers");
    let module = bennett_ir::load(&text).expect("valid program");
    let control = module.function(entry(&module, "entry"));
    let applies = before_measurements(control).filter(|inst| matches!(inst, Inst::Apply { .. })).count();
    let copies: Vec<_> = text.lines().filter(|line| line.starts_with("func @entry.whole")).collect();
    assert_eq!(
        copies.len(),
        1,
        "expected one outlined function for both variables, found {copies:?} with {applies} applies"
    );
}

#[test]
fn retained_overwrite_keeps_the_old_width_and_preserves_unrelated_phase() {
    let source = write_c(
        "rules-6b-retained",
        r#"#include "bennett.h"
unsigned entry(void) {
    unsigned short x = 0;
    _Bool y = 0;
    bennett_x(bennett_bit(&x, 1));
    bennett_h(bennett_bit(&x, 0));
    bennett_h(&y);
    bennett_s(&y);
    x = (unsigned short)(x / 3);
    return (unsigned)(bennett_measure(&x) | bennett_measure(&y) << 16);
}
"#,
    );
    let include = format!("-I{}", root().join("front/include").display());
    let clang: Vec<_> = CLANG_FLAGS.iter().copied().chain([include.as_str()]).collect();
    let bc = bitcode(&source, "rules-6b-retained", &clang);
    let output = Command::new(front())
        .args(["--stack-bytes=65536", "--region-budget=512"])
        .arg(bc)
        .output()
        .expect("front runs");
    let warnings = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{warnings}");
    let keeps: Vec<_> = warnings.lines().filter(|line| line.contains("keeps the old value")).collect();
    assert_eq!(keeps.len(), 1, "{warnings}");
    assert!(keeps[0].contains("'x'") && keeps[0].contains("in 16 qubits each time it runs"), "{warnings}");

    let module = bennett_ir::load(&String::from_utf8(output.stdout).expect("text")).expect("valid program");
    let state = state_before_first_measurement(&module, "entry", &[]);
    let columns = registers(&module, "entry");
    let x = column(&columns, "x");
    let y = column(&columns, "y");
    assert_eq!(state.quantum.amplitudes.len(), 4);
    let mut seen = BTreeSet::new();
    for (values, amplitude) in &state.quantum.amplitudes {
        let expected = if values[y] == 0 { C::new(0.5, 0.0) } else { C::new(0.0, 0.5) };
        assert!((amplitude - expected).norm() < 1e-12, "{values:?}: {amplitude}, expected {expected}");
        let old = *values.last().expect("retained column");
        assert!([2, 3].contains(&old) && values[y] <= 1, "{values:?}");
        assert_eq!(values[x], old / 3, "{values:?}");
        assert!(seen.insert((old, values[y])), "{values:?}");
    }
    let mut executor = Executor::new(&module, entry(&module, "entry"), &[], options(0)).expect("starts");
    assert!(matches!(executor.run(), Outcome::Returned(Some(_))));
    assert_eq!(executor.stats().leftover_qubits, 16);
    assert_eq!(executor.stats().nonzero_releases, 1);
}

#[test]
fn a_single_gate_loop_skips_zero_trips_with_a_mixed_guard() {
    let source = write_c(
        "rules-6b-single-zero",
        r#"#include "bennett.h"
unsigned entry(unsigned n, unsigned enable) {
    _Bool q = 0, y = 0;
    bennett_h(&q);
    if (enable && q)
        for (unsigned i = 0; i < n; ++i)
            bennett_x(&y);
    unsigned x = (unsigned)bennett_measure(&q);
    return x | (unsigned)bennett_measure(&y) << 1;
}
"#,
    );
    for (mode, clang, flags) in MODES {
        let text = lower_text(&source, &format!("rules-6b-single-zero-{mode}"), clang, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid program");
        for args in [[0, 1], [1, 0], [2, 1]] {
            let outcome = run_bounded(&module, &args, 128);
            assert!(matches!(outcome, Outcome::Returned(Some(0 | 1))), "{mode} entry{args:?}: {outcome:?}");
        }
    }
}

const MIXED_RESULTS: &str = r#"#include "bennett.h"
__attribute__((noinline)) static unsigned delta(unsigned t) { return t * 19 + 1; }
unsigned entry(void) {
    unsigned char x = 0, a = 0, b = 5;
    bennett_h(bennett_bit(&x, 0));
    bennett_h(bennett_bit(&x, 1));
    bennett_h(bennett_bit(&a, 0));
    for (unsigned t = x; t != 0; t >>= 1) {
        unsigned d = delta(t);
        a ^= (unsigned char)d;
        b = (unsigned char)(3 * b + d);
    }
    return (unsigned)(bennett_measure(&x) | bennett_measure(&a) << 8 | bennett_measure(&b) << 16);
}
"#;

#[test]
fn known_overwrite_and_xor_accumulation_give_the_expected_values() {
    let source = write_c("rules-6b-mixed-values", MIXED_RESULTS);
    for (mode, clang, flags) in MODES {
        let text = lower_text(&source, &format!("rules-6b-mixed-values-{mode}"), clang, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid program");
        let mut expected = BTreeMap::new();
        for x in 0..4 {
            for start in 0..2 {
                let (mut a, mut b, mut t) = (start, 5, x);
                while t != 0 {
                    let delta = 19 * t + 1;
                    a ^= delta;
                    b = (3 * b + delta) & 255;
                    t >>= 1;
                }
                expected.insert([x, a, b], C::new(0.5 * S, 0.0));
            }
        }
        check_state(&module, &[], ["x", "a", "b"], &expected, mode);
    }
}

#[test]
fn known_overwrite_and_xor_accumulation_share_the_same_applied_function() {
    let source = write_c("rules-6b-mixed-shape", MIXED_RESULTS);
    let text = lower_text(&source, "rules-6b-mixed-shape", CLANG_FLAGS, &[]).expect("lowers");
    let module = bennett_ir::load(&text).expect("valid program");
    let control = module.function(entry(&module, "entry"));
    let applied: Vec<_> = before_measurements(control)
        .filter_map(|inst| match inst {
            Inst::Apply { out, .. } => Some(control.slot_names[out.index()].as_str()),
            _ => None,
        })
        .collect();
    assert!(
        matches!(applied.as_slice(), [compute, clear] if compute == clear),
        "expected both evaluations to target the shared result register, got {applied:?}"
    );
}

#[test]
fn unsupported_gate_loops_name_the_reason_for_refusal() {
    let cases = [
        (
            "trip",
            "for (unsigned i = 0; i < x; ++i) bennett_x(&y);",
            "number of passes must be written as an ordinary value",
        ),
        (
            "target",
            "if (x == 2) for (unsigned i = 0; i < n; ++i) { bennett_x(&y); bennett_h(bennett_bit(&x, 1)); }",
            "the gate would change the condition",
        ),
        (
            "carried",
            "unsigned t = x; for (unsigned i = 0; i < n; ++i) { if (t & 2) bennett_z(&y); t = (t >> 1) ^ (t * 9); }",
            "table of control bits for all passes",
        ),
    ];
    for (name, body, reason) in cases {
        let key = format!("rules-6b-refused-{name}");
        let source = write_c(
            &key,
            &format!(
                r#"#include "bennett.h"
unsigned entry(unsigned n) {{
    unsigned char x = 0;
    _Bool y = 0;
    bennett_h(bennett_bit(&x, 0));
    bennett_h(bennett_bit(&x, 1));
    {body}
    return (unsigned)(bennett_measure(&x) | bennett_measure(&y) << 8);
}}
"#,
            ),
        );
        let error = lower_text(&source, &key, CLANG_FLAGS, &[]).expect_err("unsupported gate loop must be refused");
        assert!(error.starts_with("not handled yet:") && error.contains(reason), "{name}: {error}");
        if name == "trip" {
            assert!(error.contains("does not consume its loop variable"), "{error}");
        }
    }
}
