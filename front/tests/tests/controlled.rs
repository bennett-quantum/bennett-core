//! Gates under qubit control, written as ordinary C (`front/tests/c/controlled.c`): the state just before
//! the first measurement, then the measurements; and how the gates are emitted.

use std::f64::consts::FRAC_1_SQRT_2;
use std::sync::OnceLock;

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, MAX_CONFIGURATIONS, branches, c_dir, entry, lower, run};
use bennett_ir::{Gate, Inst, ValidModule};
use num_complex::Complex64;

/// Seeds per program, for outcomes that should not depend on the seed.
const SEEDS: u64 = 32;

/// How far apart amplitudes may be after a handful of gates.
const TOLERANCE: f64 = 1e-12;

const CLANG_O2_FLAGS: &[&str] =
    &["-O2", "-fno-vectorize", "-fno-slp-vectorize", "-gline-tables-only", "-fno-discard-value-names"];

/// controlled.c lowered from unoptimised and from optimised C, and with --taint-all.
fn modules() -> &'static [(String, ValidModule)] {
    static MODULES: OnceLock<Vec<(String, ValidModule)>> = OnceLock::new();
    MODULES.get_or_init(|| {
        let source = c_dir().join("controlled.c");
        vec![
            ("O0".to_owned(), lower(&source, "controlled", CLANG_FLAGS, &[])),
            ("O2".to_owned(), lower(&source, "controlled.o2", CLANG_O2_FLAGS, &[])),
            ("taint-all".to_owned(), lower(&source, "controlled.taint-all", CLANG_FLAGS, &["--taint-all"])),
        ]
    })
}

/// The branches of `name(args)` as maps from C variable to value, with amplitudes.
fn state(module: &ValidModule, name: &str, args: &[u64]) -> Vec<(Vec<(String, u64)>, Complex64)> {
    let (columns, state) = branches(module, name, args, MAX_CONFIGURATIONS);
    state.into_iter().map(|(values, a)| (columns.iter().cloned().zip(values).collect(), a)).collect()
}

/// Asserts that the state of `name(args)` is `expected`: (values of the named variables, amplitude), in
/// any order, where the variables not named are 0.
fn assert_state(
    module: &ValidModule,
    context: &str,
    name: &str,
    args: &[u64],
    expected: &[(&[(&str, u64)], Complex64)],
) {
    let actual = state(module, name, args);
    assert_eq!(actual.len(), expected.len(), "{context} @{name}{args:?}: {actual:?}");
    for (values, amplitude) in expected {
        let found = actual.iter().find(|(branch, _)| {
            branch.iter().all(|(variable, value)| {
                *value == values.iter().find(|(v, _)| v == variable).map_or(0, |(_, value)| *value)
            })
        });
        let Some((_, a)) = found else { panic!("{context} @{name}{args:?}: no branch {values:?} in {actual:?}") };
        assert!((a - amplitude).norm() <= TOLERANCE, "{context} @{name}{args:?}: {values:?} has {a}, not {amplitude}");
    }
}

fn assert_every_run(module: &ValidModule, context: &str, name: &str, args: &[u64], allowed: &[u64]) -> Vec<u64> {
    (0..SEEDS)
        .map(|seed| match run(module, name, args, seed) {
            Outcome::Returned(Some(value)) if allowed.iter().any(|&a| u128::from(a) == value) => value as u64,
            other => panic!("{context} @{name}{args:?}, seed {seed}: {other:?}"),
        })
        .collect()
}

fn real(value: f64) -> Complex64 {
    Complex64::new(value, 0.0)
}

/// The gates `name` applies, as text, in order, for the ones that act on a C variable.
fn gates(module: &ValidModule, name: &str) -> Vec<String> {
    let function = module.function(entry(module, name));
    let qubit = |q: &bennett_ir::Qubit| format!("{}[{}]", function.slot_names[q.register.index()], q.bit);
    function
        .blocks
        .iter()
        .flat_map(|block| &block.insts)
        .filter_map(|inst| match inst {
            Inst::Gate { gate: Gate::Fixed(gate), target, controls } => {
                let controls: Vec<String> = controls.iter().map(qubit).collect();
                Some(format!("{} {} ctrl [{}]", gate.mnemonic(), qubit(target), controls.join(", ")))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_branch_on_a_qubit_around_a_gate_is_a_controlled_gate() {
    let half = real(FRAC_1_SQRT_2);
    for (context, module) in modules() {
        assert_state(module, context, "bell", &[], &[(&[], half), (&[("a", 1), ("b", 1)], half)]);
        assert_every_run(module, context, "bell", &[], &[0, 3]);
        let (cos, sin) = (0.5f64.cos(), 0.5f64.sin());
        assert_state(module, context, "anti", &[], &[(&[("b", 1)], real(cos)), (&[("a", 1)], real(sin))]);
        assert_every_run(module, context, "anti", &[], &[1, 2]);
        assert_state(module, context, "ghz", &[], &[(&[], half), (&[("a", 1), ("b", 1), ("c", 1)], half)]);
        assert_every_run(module, context, "ghz", &[], &[0, 7]);
        if context != "taint-all" {
            assert_eq!(gates(module, "bell"), ["h a[0] ctrl []", "x b[0] ctrl [a[0]]"], "{context}: a CNOT");
            let anti = ["x a[0] ctrl []", "x b[0] ctrl [a[0]]", "x a[0] ctrl []"];
            assert_eq!(gates(module, "anti"), anti, "{context}: a CNOT on a == 0");
        }
    }
}

#[test]
fn a_conjunction_of_qubits_is_a_toffoli() {
    for (context, module) in modules() {
        for input in 0..8u64 {
            let (a, b, c) = (input >> 2, input >> 1 & 1, input & 1);
            let c = c ^ (a & b);
            assert_state(module, context, "toffoli", &[input], &[(&[("a", a), ("b", b), ("c", c)], real(1.0))]);
            assert_every_run(module, context, "toffoli", &[input], &[a | b << 1 | c << 2]);
        }
        if context != "taint-all" {
            let gates = gates(module, "toffoli");
            assert_eq!(gates.last().map(String::as_str), Some("x c[0] ctrl [a[0], b[0]]"), "{context}: {gates:?}");
        }
    }
}

/// H, CZ, H on a: a phase is seen only by interference. Both a == 1 and a == 0 as the control.
#[test]
fn a_controlled_z_shows_by_interference() {
    for (context, module) in modules() {
        for flip in 0..2 {
            for negated in 0..2 {
                let args = [flip, negated];
                // With a negated control and b = 1 the state is -|1, 1>: a global phase.
                let sign = if negated == 1 && flip == 1 { -1.0 } else { 1.0 };
                assert_state(module, context, "phase", &args, &[(&[("a", flip), ("b", flip)], real(sign))]);
                assert_every_run(module, context, "phase", &args, &[flip | flip << 1]);
            }
        }
    }
}

#[test]
fn a_rotation_under_a_qubit_is_a_controlled_rotation() {
    let (cos, sin) = ((1.25f64 / 2.0).cos(), (1.25f64 / 2.0).sin());
    let half = FRAC_1_SQRT_2;
    for (context, module) in modules() {
        assert_state(
            module,
            context,
            "rotation",
            &[],
            &[(&[], real(half)), (&[("a", 1)], real(half * cos)), (&[("a", 1), ("b", 1)], real(half * sin))],
        );
    }
}

/// The condition f(p, q) == 3 is computed into a qubit of its own, which is 0 again afterwards (`branches`
/// checks every register the front made, and the executor's `qfree` faults if one is not 0).
#[test]
fn a_gate_under_a_computed_condition_leaves_the_condition_clean() {
    let f = |p: u64, q: u64| (p * 5 + q * 3) & 7;
    for (context, module) in modules() {
        let expected: Vec<(Vec<(&str, u64)>, Complex64)> = (0..16)
            .map(|i| {
                let (p, q) = (i & 3, i >> 2);
                (vec![("p", p), ("q", q), ("c", u64::from(f(p, q) == 3))], real(0.25))
            })
            .collect();
        let expected: Vec<(&[(&str, u64)], Complex64)> = expected.iter().map(|(v, a)| (v.as_slice(), *a)).collect();
        assert_state(module, context, "computed", &[], &expected);
        for (seed, value) in assert_every_run(module, context, "computed", &[], &(0..1 << 17).collect::<Vec<_>>())
            .into_iter()
            .enumerate()
        {
            let (p, q, c) = (value & 0xff, value >> 8 & 0xff, value >> 16);
            assert_eq!(c, u64::from(f(p, q) == 3), "{context}, seed {seed}");
        }
    }
}

/// A block under a condition, with a nested condition inside: every gate and store in it acts where its
/// path reaches it. p in 0..3: odd p sets d = p + 1 and c = [p == 3]; even p sets d = 7.
#[test]
fn every_operation_in_a_controlled_block_is_controlled() {
    for (context, module) in modules() {
        let expected: [(&[(&str, u64)], Complex64); 4] = [
            (&[("p", 0), ("d", 7)], real(0.5)),
            (&[("p", 1), ("d", 2)], real(0.5)),
            (&[("p", 2), ("d", 7)], real(0.5)),
            (&[("p", 3), ("d", 4), ("c", 1)], real(0.5)),
        ];
        assert_state(module, context, "block", &[], &expected);
    }
}
