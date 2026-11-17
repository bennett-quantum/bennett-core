//! The hand-written examples, with their expected results.

mod common;

use std::f64::consts::FRAC_1_SQRT_2;

use bennett_exec::{Options, Outcome, ReverseError, RunError};
use bennett_ir::Fault;
use common::{TOLERANCE, allowing_region_faults, assert_reverses, bounded, executor, load, run};
use num_complex::Complex64;

const SUM: &str = include_str!("../../../examples/sum.bn");
const CALLS: &str = include_str!("../../../examples/calls.bn");
const THESIS: &str = include_str!("../../../examples/thesis.bn");
const FAULT: &str = include_str!("../../../examples/fault.bn");
const GROVER: &str = include_str!("../../../examples/grover.bn");

#[test]
fn sum_counts_and_reverses() {
    let module = load(SUM);
    for n in [0, 1, 2, 10, 1000] {
        let (outcome, _) = run(&module, "main", &[n], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(n * n.saturating_sub(1) / 2))), "n = {n}");
        assert_eq!(assert_reverses(&module, "main", &[n], bounded()), outcome);
    }
}

#[test]
fn a_run_that_faults_keeps_its_journal_and_reverses_to_entry() {
    let module = load(SUM);
    let outcome = assert_reverses(&module, "main", &[1000], Options { budget: Some(100), ..bounded() });
    assert_eq!(outcome, Outcome::Faulted(Fault::Budget));
}

#[test]
fn calls_use_memory_tables_and_the_console() {
    let module = load(CALLS);
    let (outcome, executor) = run(&module, "main", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(64)));
    assert_eq!(executor.tape(), b"hello\n");
    assert_reverses(&module, "main", &[], bounded());
}

#[test]
fn the_thesis_state_is_entangled_after_one_application() {
    let module = load(THESIS);
    let (outcome, executor) = run(&module, "entangled", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(None));
    // Columns: q, y, e in allocation order.
    let state = executor.snapshot().quantum;
    let half = Complex64::new(FRAC_1_SQRT_2, 0.0);
    assert_eq!(state.amplitudes.len(), 2);
    assert!((state.amplitude(&[0, 0, 0]) - half).norm() <= TOLERANCE);
    assert!((state.amplitude(&[1, 10, 0]) - half).norm() <= TOLERANCE);
    assert_reverses(&module, "entangled", &[], bounded());
}

#[test]
fn the_thesis_interferes_back_to_zero_for_every_seed() {
    let module = load(THESIS);
    for seed in 0..64 {
        let (outcome, _) = run(&module, "main", &[], Options { seed, ..bounded() });
        assert_eq!(outcome, Outcome::Returned(Some(0)), "seed {seed}");
    }
}

#[test]
fn reversal_stops_at_a_measurement() {
    let module = load(THESIS);
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    executor.run();
    assert_eq!(executor.step_back(), Ok(true), "the return is undone");
    assert_eq!(executor.step_back(), Err(ReverseError::PastMeasurement));
    assert_eq!(executor.step_back(), Err(ReverseError::PastMeasurement), "and stays refused");
}

/// `@triangle(n)` takes 5n + 5 steps: under a budget of 20, n ≤ 3 succeeds and n ≥ 4 faults.
#[test]
fn fault_outcomes_follow_the_budget_and_leave_out_unchanged() {
    let module = load(FAULT);
    let initial_out = 5;
    for n in 0..16u64 {
        let (outcome, executor) = run(&module, "main", &[n], allowing_region_faults());
        let Outcome::Returned(Some(packed)) = outcome else { panic!("n = {n}: {outcome:?}") };
        assert_eq!(executor.stats().region_faults, u64::from(n > 3), "n = {n}: counted");
        // By default a region fault stops the run: fault.bn reads its fault register on purpose.
        let (strict, _) = run(&module, "main", &[n], bounded());
        let callee = module.function_named("triangle").expect("@triangle");
        let fault = RunError::RegionFault { callee, fault: Fault::Budget, faulting: 1, configurations: 1 };
        assert_eq!(strict, if n <= 3 { outcome } else { Outcome::Stopped(fault) }, "n = {n}");
        let (out, fault) = (packed & 0xff, packed >> 8);
        let expected =
            if n <= 3 { (initial_out ^ (n * n.saturating_sub(1) / 2), 0) } else { (initial_out, Fault::Budget.code()) };
        let expected = (u128::from(expected.0), u128::from(expected.1));
        assert_eq!((out, fault), expected, "n = {n}");
    }
}

#[test]
fn grover_succeeds_with_the_closed_form_probability() {
    let module = load(GROVER);
    let solutions = [2, 13];
    let theta = (solutions.len() as f64 / 16.0).sqrt().asin();
    for iterations in 0..=4u64 {
        let (outcome, executor) = run(&module, "searched", &[iterations], bounded());
        assert_eq!(outcome, Outcome::Returned(None));
        // Columns: x, kick, e in allocation order.
        let state = executor.snapshot().quantum;
        assert!(state.amplitudes.iter().all(|(config, _)| config[2] == 0), "the oracle never faults");
        let success: f64 = state
            .amplitudes
            .iter()
            .filter(|(config, _)| solutions.contains(&config[0]))
            .map(|(_, amplitude)| amplitude.norm_sqr())
            .sum();
        let expected = ((2 * iterations + 1) as f64 * theta).sin().powi(2);
        assert!((success - expected).abs() <= TOLERANCE, "{iterations} iterations: {success} vs {expected}");
    }
    assert_reverses(&module, "searched", &[2], bounded());
}

/// The limit on basis configurations belongs to the simulator, not to the program: reaching it stops the
/// run with a run error, outside the fault codes, leaving the state as it was before the gate.
#[test]
fn the_configuration_limit_stops_the_run_without_a_fault() {
    let module = load(GROVER);
    let limit = 8;
    let options = Options { max_configurations: Some(limit), reversible: true, ..bounded() };
    let mut executor = executor(&module, "main", &[], options);
    let at_entry = executor.snapshot();
    let outcome = executor.run();
    assert_eq!(outcome, Outcome::Stopped(RunError::ConfigurationLimit { limit }));
    assert!(executor.stats().configurations <= limit, "the gate that would exceed the limit did not happen");
    assert!(Fault::ALL.iter().all(|fault| fault.name() != "configuration_limit"), "not a fault code");
    executor.reverse().unwrap_or_else(|error| panic!("{error}"));
    common::assert_same_state(&executor.snapshot(), &at_entry, "reversed after the stop");
}

#[test]
fn gates_reject_physical_aliases_hidden_by_distinct_parameters() {
    for args in ["%q, %q, %r", "%q, %r, %r"] {
        let module = load(&format!(
            "memory 256
func @bad(q1 %target, q1 %a, q1 %b) {{
entry:
    x %target ctrl %a, %b
    ret
}}
func @main() {{
entry:
    %q = qalloc 1
    %r = qalloc 1
    h %q
    s %q
    call @bad({args})
    ret
}}
"
        ));
        let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
        for _ in 0..5 {
            assert_eq!(executor.step(), None);
        }
        let before = executor.snapshot();
        assert_eq!(executor.step(), Some(Outcome::Faulted(Fault::QubitAlias)));
        let mut after = executor.snapshot();
        after.classical.outcome = None;
        assert_eq!(after, before, "alias refusal preserves the complex amplitudes and classical state");
    }
}

#[test]
fn restoring_the_current_saved_stack_pointer_is_valid_with_or_without_allocations() {
    for size in [0, 1, 17] {
        let module = load(&format!(
            "memory 256\nfunc @main() -> i8 {{\nentry:\n %p = alloca {size}\n %sp = stacksave\n \
             stackrestore %sp\n ret 0\n}}\n"
        ));
        assert_eq!(assert_reverses(&module, "main", &[], bounded()), Outcome::Returned(Some(0)));
    }
}

#[test]
fn entry_results_respect_the_declared_integer_width() {
    for bits in [1, 2, 7, 8, 31, 32, 63, 64] {
        let module = load(&format!("memory 256\nfunc @main(i{bits} %x) -> i{bits} {{\nentry:\n ret %x\n}}\n"));
        let max = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
        for value in [0, max] {
            assert_eq!(run(&module, "main", &[value], bounded()).0, Outcome::Returned(Some(u128::from(value))));
        }
        if bits < 64 {
            for value in [max + 1, u64::MAX] {
                // The host may refuse or convert an argument; an accepted iN result must still fit N bits.
                if let Ok(mut executor) =
                    bennett_exec::Executor::new(&module, common::entry(&module, "main"), &[value], bounded())
                    && let Outcome::Returned(Some(result)) = executor.run()
                {
                    assert!(result <= u128::from(max), "i{bits} returned {result}");
                }
            }
        }
    }
}

#[test]
fn an_entry_frame_that_never_fit_is_unchanged_by_an_empty_reverse_step() {
    let module = load("memory 0\nfunc @main(i1 %x) -> i1 {\nentry:\n ret %x\n}\n");
    let mut executor = executor(&module, "main", &[0], Options { reversible: true, ..bounded() });
    assert_eq!(executor.outcome(), Some(Outcome::Faulted(Fault::StackOverflow)));
    let initial = executor.snapshot();
    assert_eq!(executor.step_back(), Ok(false));
    assert_eq!(executor.snapshot(), initial, "no step ran, so the entry fault is part of the initial state");
}

#[test]
fn a_multivalued_measurement_preserves_conditional_phases_and_reaches_every_outcome() {
    let module = load(
        "memory 256
func @main() -> i2 {
entry:
    %q = qalloc 2
    %r = qalloc 1
    h %q[0]
    h %q[1]
    h %r
    s %r
    x %r ctrl %q[0]
    %value = measure %q
    ret %value
}
",
    );
    let mut seen = [false; 4];
    for seed in 0..256 {
        let (outcome, executor) = run(&module, "main", &[], Options { seed, ..bounded() });
        let Outcome::Returned(Some(value)) = outcome else { panic!("{outcome:?}") };
        let value = usize::try_from(value).expect("a two-bit result");
        seen[value] = true;
        let state = executor.snapshot().quantum;
        assert_eq!(state.amplitudes.len(), 2);
        let plus_i = [Complex64::new(FRAC_1_SQRT_2, 0.0), Complex64::new(0.0, FRAC_1_SQRT_2)];
        let expected = if value & 1 == 0 { plus_i } else { [plus_i[1], plus_i[0]] };
        for (spectator, amplitude) in expected.into_iter().enumerate() {
            assert!(
                (state.amplitude(&[value as u64, spectator as u64]) - amplitude).norm() <= TOLERANCE,
                "outcome {value}, spectator {spectator}, seed {seed}"
            );
        }
    }
    // All four outcomes have Born weight 1/4; avoid fixing the mapping from a seed to an outcome.
    assert!(seen.into_iter().all(|value| value), "every positive-weight outcome must be sampled");
}
