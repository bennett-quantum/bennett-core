//! The thesis, end to end: C with a few marker calls, through clang and bennett-front, run by the
//! executor. Each program states its expectation in its own comment (`front/tests/c`).

use std::f64::consts::FRAC_1_SQRT_2;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, c_dir, column, entry, lower, lower_text, registers, run, state_before_first_measurement,
    state_without_fault_registers, write_c,
};
use bennett_ir::{Fault, FixedGate, Gate, Inst, ValidModule};
use num_complex::Complex64;

/// Seeds per program: measurement outcomes that should not depend on the seed are checked on this many.
const SEEDS: u64 = 64;

/// How far apart amplitudes may be after a handful of gates.
const TOLERANCE: f64 = 1e-12;

/// The front end runs its own pipeline, so it must also accept code clang has already optimised (without
/// vectorising, since the device program has no vector values).
const CLANG_O2_FLAGS: &[&str] =
    &["-O2", "-fno-vectorize", "-fno-slp-vectorize", "-gline-tables-only", "-fno-discard-value-names"];

fn program(name: &str, clang_flags: &[&str], front_flags: &[&str]) -> ValidModule {
    let tag = format!("{name}{}{}", if clang_flags == CLANG_FLAGS { "" } else { ".o2" }, front_flags.concat());
    lower(&c_dir().join(format!("{name}.c")), &tag, clang_flags, front_flags)
}

fn outcomes(module: &ValidModule) -> Vec<Outcome> {
    (0..SEEDS).map(|seed| run(module, "main", &[], seed)).collect()
}

fn assert_every_run(module: &ValidModule, expected: Outcome, context: &str) {
    for (seed, outcome) in outcomes(module).into_iter().enumerate() {
        assert_eq!(outcome, expected, "{context}, seed {seed}");
    }
}

/// The instructions of `@main`'s blocks, in order.
fn main_insts(module: &ValidModule) -> Vec<&Inst> {
    module.function(entry(module, "main")).blocks.iter().flat_map(|block| &block.insts).collect()
}

#[test]
fn the_thesis_program_measures_zero_on_every_run() {
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = program("thesis", flags, &[]);
        assert_every_run(&module, Outcome::Returned(Some(0)), "thesis.c");
        // Recorded, not required: LLVM proves that y ^= (the same value) is 0 and y unused, so the
        // dependence never reaches the device program. kickback.c is the test that exercises it.
        let computations = main_insts(&module)
            .into_iter()
            .filter(|inst| match inst {
                Inst::Apply { callee, .. } => !module.function(*callee).name.starts_with("bennett.copy"),
                _ => false,
            })
            .count();
        assert_eq!(computations, 0, "LLVM kept the dependence");
    }
}

#[test]
fn an_unused_dependence_leaves_nothing_behind() {
    let module = program("thesis_no_undo", CLANG_FLAGS, &[]);
    assert_every_run(&module, Outcome::Returned(Some(0)), "thesis_no_undo.c");
}

#[test]
fn a_kept_dependence_is_entangled_before_measurement() {
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = program("thesis_kept", flags, &[]);
        let columns = registers(&module, "main");
        let (b, y, fault) = (column(&columns, "b"), column(&columns, "y"), column(&columns, "y.fault"));
        let state = state_before_first_measurement(&module, "main", &[]).quantum;
        assert_eq!(state.amplitudes.len(), 2, "{:?}", state.amplitudes);
        let half = Complex64::new(FRAC_1_SQRT_2, 0.0);
        for (config, amplitude) in &state.amplitudes {
            assert!((amplitude - half).norm() <= TOLERANCE, "{config:?}: {amplitude}");
            assert_eq!(config[fault], 0, "no fault");
            assert_eq!(config[y], if config[b] == 1 { 10 } else { 0 }, "{config:?}");
        }
        assert_every_run(&module, Outcome::Returned(Some(1)), "thesis_kept.c: the measurements agree");
    }
}

#[test]
fn kickback_computes_and_uncomputes_in_the_device_program() {
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = program("kickback", flags, &[]);
        let function = module.function(entry(&module, "main"));
        let hit = function.slot_names.iter().position(|name| name == "hit").expect("a register %hit");
        let shape: Vec<&str> = main_insts(&module)
            .iter()
            .filter_map(|inst| match inst {
                Inst::Apply { out, .. } if out.index() == hit => Some("apply into hit"),
                Inst::Gate { gate: Gate::Fixed(FixedGate::Z), target, .. } if target.register.index() == hit => {
                    Some("z hit")
                }
                _ => None,
            })
            .collect();
        assert_eq!(shape, ["apply into hit", "z hit", "apply into hit"], "compute, act, uncompute");
        assert_every_run(&module, Outcome::Returned(Some(1)), "kickback.c");
    }
}

#[test]
fn without_the_uncomputation_the_discard_faults() {
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = program("kickback_no_uncompute", flags, &[]);
        assert_every_run(&module, Outcome::Faulted(Fault::DirtyFree), "kickback_no_uncompute.c");
    }
}

/// Coarser taint costs more, never correctness: treating everything that can run coherently as if it
/// depended on a qubit changes no outcome and no state.
#[test]
fn treating_everything_as_tainted_changes_no_result() {
    for name in ["thesis", "thesis_no_undo", "thesis_kept", "kickback", "kickback_no_uncompute"] {
        let precise = program(name, CLANG_FLAGS, &[]);
        let coarse = program(name, CLANG_FLAGS, &["--taint-all"]);
        assert_eq!(outcomes(&precise), outcomes(&coarse), "{name}");
    }
    let precise = state_without_fault_registers(&program("thesis_kept", CLANG_FLAGS, &[]), "main", &[]);
    let coarse = state_without_fault_registers(&program("thesis_kept", CLANG_FLAGS, &["--taint-all"]), "main", &[]);
    assert_eq!(precise, coarse);

    // coarse.c gives the switch something to change: ordinary inputs to a quantum computation.
    let source = c_dir().join("coarse.c");
    let precise_text = lower_text(&source, "coarse", CLANG_FLAGS, &[]).unwrap_or_else(|e| panic!("{e}"));
    let coarse_text =
        lower_text(&source, "coarse.taint-all", CLANG_FLAGS, &["--taint-all"]).unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(precise_text, coarse_text, "--taint-all changes the program");
    let precise = bennett_ir::load(&precise_text).unwrap_or_else(|e| panic!("{e}"));
    let coarse = bennett_ir::load(&coarse_text).unwrap_or_else(|e| panic!("{e}"));
    // Arithmetic the precise program does only in the control program, the coarse one also does inside
    // the functions it applies.
    let coherent_arithmetic = |module: &ValidModule| {
        let applied = module.functions.iter().filter(|function| function.name.starts_with("coarse.apply"));
        applied.flat_map(|f| &f.blocks).flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Binary { .. })).count()
    };
    assert!(coherent_arithmetic(&coarse) > coherent_arithmetic(&precise), "--taint-all runs more coherently");
    for param in [0, 1, 7, 200, 255] {
        let state = |module: &ValidModule| state_without_fault_registers(module, "coarse", &[param]);
        assert_eq!(state(&precise), state(&coarse), "coarse({param})");
        for seed in 0..SEEDS {
            let outcome = run(&precise, "coarse", &[param], seed);
            assert_eq!(outcome, run(&coarse, "coarse", &[param], seed), "coarse({param}), seed {seed}");
            let Outcome::Returned(Some(packed)) = outcome else { panic!("coarse({param}): {outcome:?}") };
            let (out, k, b) = (packed & 0xffff_ffff, packed >> 32 & 0xff, packed >> 40 & 0xff);
            let (first, copy, early) = (packed >> 48 & 1, packed >> 50 & 1, packed >> 56);
            assert_eq!(early, (param * 3 + 1) & 0xff, "coarse({param}): measuring a known value");
            let k_expected = param ^ 0x5a;
            let value = if b == 1 { param + k_expected } else { first * 5 + param };
            assert_eq!((out, k, copy), (value ^ first, k_expected, b), "coarse({param}), seed {seed}");
        }
    }
}

/// What this version does not handle is refused with a message naming it, never compiled to something
/// cheaper.
#[test]
fn what_is_not_handled_is_refused_by_name() {
    let cases = [
        ("returned", "return b;", "used by ordinary code"),
        ("overwritten", "b = 1; return (int)bennett_measure(&b);", "an overwrite of qubit variable 'b'"),
        (
            "stale",
            "_Bool t = b; bennett_h(&b); c ^= t; return (int)bennett_measure(&c);",
            "used after the variable changed",
        ),
        ("in_place", "b = b ? 0 : b; return (int)bennett_measure(&b);", "an overwrite of qubit variable 'b'"),
        ("controlled_gate", "if (b) bennett_x(&c); return (int)bennett_measure(&c);", "a quantum operation"),
        ("stored", "sink = b; return 0;", "used by ordinary code"),
    ];
    for (name, body, expected) in cases {
        let text = format!(
            "#include \"bennett.h\"\nint sink;\nint main(void) {{\n  _Bool b = 0, c = 0;\n  bennett_h(&b);\n  bennett_x(&c);\n  {body}\n}}\n"
        );
        let source = write_c(&format!("refused_{name}"), &text);
        match lower_text(&source, &format!("refused_{name}"), CLANG_FLAGS, &[]) {
            Ok(program) => panic!("{name}: accepted\n{program}"),
            Err(message) => assert!(
                message.starts_with("not handled yet: ") && message.contains(expected) && message.contains(".c:"),
                "{name}: {message}"
            ),
        }
    }
}
