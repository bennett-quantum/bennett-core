//! `apply` is the permutation it claims to be. For small widths, every argument value, every `out` value
//! and every `fault` value is prepared as a basis state, the region is applied, and the resulting state
//! is compared with evaluating the function directly. Each direct evaluation is also run reversibly and
//! reversed, and the harness checks that the entry state is restored, on success and on fault alike.

mod common;

use bennett_exec::{Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{allowing_region_faults, assert_reverses, bounded, executor, load, run};
use num_complex::Complex64;

const REGIONS: &str = r#"
memory 1024
data 0 = i8 7
data 8 = i64 @twice, i64 @negate, i64 @twice, i64 0

; Loops n times: 4 + 5n steps, so a budget of 20 lets n <= 3 finish.
func @steps(i3 %n) -> i3 {
entry:
    br loop(i3 0, i3 0)
loop(i3 %i, i3 %acc):
    %more = icmp ult i3 %i, %n
    br %more, body, done
body:
    %acc.next = add i3 %acc, 3
    %i.next = add i3 %i, 1
    br loop(%i.next, %acc.next)
done:
    ret %acc
}

; Faults on a zero divisor.
func @divide(i2 %a, i2 %b) -> i2 {
entry:
    %q = udiv i2 %a, %b
    ret %q
}

; Reads and writes static memory and the stack: every run must see the initial 7.
func @private(i2 %a) -> i2 {
entry:
    %old = load i8 0
    %wide = zext i2 %a to i8
    %new = add i8 %old, %wide
    store i8 %new, 0
    %cell = alloca 1
    store i8 %new, %cell
    %again = load i8 %cell
    %low = trunc i8 %again to i2
    ret %low
}

; Calls through a table whose last entry is not a function.
func @dispatch(i2 %k) -> i2 {
entry:
    %wide = zext i2 %k to i64
    %offset = mul i64 %wide, 8
    %address = add i64 %offset, 8
    %f = load i64 %address
    %r = call_indirect fn(i2) -> i2 %f(%k)
    ret %r
}

func @twice(i2 %x) -> i2 {
entry:
    %y = shl i2 %x, 1
    ret %y
}

func @negate(i2 %x) -> i2 {
entry:
    %y = sub i2 0, %x
    ret %y
}
"#;

/// A function to apply, with its parameter and result widths and the region's budget.
struct Region {
    name: &'static str,
    params: &'static [u32],
    result: u32,
    budget: u64,
}

const REGION_CASES: [Region; 4] = [
    Region { name: "steps", params: &[3], result: 3, budget: 20 },
    Region { name: "divide", params: &[2, 2], result: 2, budget: 20 },
    Region { name: "private", params: &[2], result: 2, budget: 20 },
    Region { name: "dispatch", params: &[2], result: 2, budget: 20 },
];

/// `@case(args..., t, e)`: prepares the basis state |args, t, e⟩ with X gates, one conditional block
/// per bit, then applies the region.
fn harness(region: &Region) -> String {
    let fault_bits = Fault::CODE_WIDTH.bits();
    let mut registers: Vec<(String, String, u32)> =
        region.params.iter().enumerate().map(|(i, &w)| (format!("r{i}"), format!("a{i}"), w)).collect();
    registers.push(("out".into(), "t".into(), region.result));
    registers.push(("fault".into(), "e".into(), fault_bits));
    let params: Vec<String> = registers.iter().map(|(_, source, w)| format!("i{w} %{source}")).collect();
    let mut text = format!("func @case({}) {{\nentry:\n", params.join(", "));
    for (register, _, width) in &registers {
        text += &format!("    %{register} = qalloc {width}\n");
    }
    text += "    br prepare0\n";
    let bits = registers.iter().flat_map(|(r, s, w)| (0..*w).map(move |bit| (r.clone(), s.clone(), *w, bit)));
    let mut count = 0;
    for (j, (register, source, width, bit)) in bits.enumerate() {
        text += &format!(
            "prepare{j}:\n    %s{j} = lshr i{width} %{source}, {bit}\n    %b{j} = and i{width} %s{j}, 1\n    \
             %on{j} = icmp eq i{width} %b{j}, 1\n    br %on{j}, flip{j}, prepare{next}\n\
             flip{j}:\n    x %{register}[{bit}]\n    br prepare{next}\n",
            next = j + 1
        );
        count = j + 1;
    }
    let args: Vec<String> = (0..region.params.len()).map(|i| format!("%r{i}")).collect();
    text += &format!(
        "prepare{count}:\n    apply @{}({}) -> %out, %fault budget {}\n    ret\n}}\n",
        region.name,
        args.join(", "),
        region.budget
    );
    text
}

/// Every tuple of values of the given widths.
fn all_values(widths: &[u32]) -> Vec<Vec<u64>> {
    widths.iter().fold(vec![Vec::new()], |tuples, &width| {
        tuples.iter().flat_map(|tuple| (0..1u64 << width).map(move |v| [tuple.clone(), vec![v]].concat())).collect()
    })
}

#[test]
fn apply_is_the_permutation_of_direct_evaluation() {
    let fault_values = 1u64 << Fault::CODE_WIDTH.bits();
    let mut faults_seen = Vec::new();
    for region in &REGION_CASES {
        let module = load(&format!("{REGIONS}\n{}", harness(region)));
        let initial_memory =
            executor(&module, region.name, &vec![0; region.params.len()], bounded()).snapshot().classical.memory;
        for args in all_values(region.params) {
            let budget = Some(region.budget);
            let (direct, _) = run(&module, region.name, &args, Options { budget, ..bounded() });
            assert_eq!(assert_reverses(&module, region.name, &args, Options { budget, ..bounded() }), direct);
            if let Outcome::Faulted(fault) = direct {
                faults_seen.push(fault);
            }
            for t in 0..1u64 << region.result {
                for e in 0..fault_values {
                    let case = [args.clone(), vec![t, e]].concat();
                    let (outcome, executor) = run(&module, "case", &case, allowing_region_faults());
                    assert_eq!(outcome, Outcome::Returned(None));
                    let expected = match direct {
                        Outcome::Returned(Some(r)) => [args.clone(), vec![t ^ r as u64, e]].concat(),
                        Outcome::Faulted(fault) => [args.clone(), vec![t, e ^ fault.code()]].concat(),
                        Outcome::Returned(None) => panic!("a region returns a value"),
                        Outcome::Stopped(error) => panic!("a classical run reaches no simulator limit: {error}"),
                    };
                    let state = executor.snapshot();
                    let context = format!("@{}{case:?}", region.name);
                    assert_eq!(state.quantum.amplitudes, [(expected, Complex64::new(1.0, 0.0))], "{context}");
                    assert_eq!(state.classical.memory, initial_memory, "{context}: memory is private to the region");
                }
            }
        }
    }
    for fault in [Fault::Budget, Fault::DivideByZero, Fault::BadIndirect] {
        assert!(faults_seen.contains(&fault), "some input faults with {fault}");
    }
}

/// `apply` equals mark, call, rewind-keep (`docs/DESIGN.md`, "Worlds") wherever no branch faults: on every
/// basis state of the exhaustive harness whose arguments do not fault, and on the arguments in uniform
/// superposition. Where some branch faults, the idiom has no fault register: by default the run stops,
/// as a region fault does.
#[test]
fn apply_equals_the_history_idiom_where_no_branch_faults() {
    let mut compared = 0;
    for region in &REGION_CASES {
        let text = format!("{REGIONS}\n{}", harness(region));
        let (module, idiom) = (load(&text), load(&common::as_history_idiom(&text)));
        for args in all_values(region.params) {
            let (direct, _) = run(&module, region.name, &args, Options { budget: Some(region.budget), ..bounded() });
            if !matches!(direct, Outcome::Returned(_)) {
                continue;
            }
            for t in 0..1u64 << region.result {
                let case = [args.clone(), vec![t, 0]].concat();
                let (expected, applied) = run(&module, "case", &case, bounded());
                let (outcome, rewound) = run(&idiom, "case", &case, bounded());
                let context = format!("@{}{case:?}", region.name);
                assert_eq!(outcome, expected, "{context}");
                assert_eq!(rewound.snapshot().quantum, applied.snapshot().quantum, "{context}");
                assert_eq!(rewound.snapshot().classical.memory, applied.snapshot().classical.memory, "{context}");
                compared += 1;
            }
        }
        // Every argument at once: H on each argument qubit before the region.
        let bits: u32 = region.params.iter().sum();
        let superposed = |text: &str| {
            let gates: String = (0..region.params.len())
                .flat_map(|i| (0..region.params[i]).map(move |bit| format!("    h %r{i}[{bit}]\n")))
                .collect();
            let prepare = format!("prepare{}:\n", bits + region.result + Fault::CODE_WIDTH.bits());
            text.replace(&prepare, &format!("{prepare}{gates}"))
        };
        let (module, idiom) = (load(&superposed(&text)), load(&common::as_history_idiom(&superposed(&text))));
        let zeros = vec![0; region.params.len() + 2];
        let (expected, applied) = run(&module, "case", &zeros, bounded());
        let (outcome, rewound) = run(&idiom, "case", &zeros, bounded());
        match expected {
            Outcome::Returned(None) => {
                assert_eq!(outcome, expected, "@{}", region.name);
                let distance = rewound.snapshot().quantum.distance(&applied.snapshot().quantum);
                assert!(distance.is_some_and(|d| d <= 1e-12), "@{} superposed: {distance:?}", region.name);
                assert_eq!(rewound.stats().peak_worlds, 1 << bits, "@{}: a world per argument value", region.name);
            }
            // The text idiom has no region budget (the executor's own `apply` has it: `docs/DESIGN.md`,
            // "`apply` as mark, call, rewind-keep"), so where only the budget faults, it runs to the end.
            Outcome::Stopped(RunError::RegionFault { fault: Fault::Budget, .. }) => {
                assert_eq!(outcome, Outcome::Returned(None), "@{}", region.name);
            }
            Outcome::Stopped(RunError::RegionFault { .. }) => {
                assert!(
                    matches!(outcome, Outcome::Stopped(RunError::WorldFault { .. })),
                    "@{}: {outcome:?}",
                    region.name
                );
            }
            other => panic!("@{}: {other:?}", region.name),
        }
    }
    // steps: n <= 3 of 8, times 8 outputs; divide: 12 nonzero divisors, times 4; private: 4 times 4;
    // dispatch: 3 valid table entries, times 4.
    assert_eq!(compared, 32 + 48 + 16 + 12, "basis states compared");
}

/// `apply` computed as mark, call, rewind-keep on worlds is branch enumeration, faults included
/// (`docs/DESIGN.md`, "`apply` as mark, call, rewind-keep"): in compare mode the executor computes every
/// `apply` both ways from the same state and stops at any difference in the result, the world's state and
/// journal, the amplitudes or the promised counts. Here on every basis state of the exhaustive harness
/// (faulting ones included), strict and allowing region faults, and on the arguments in uniform
/// superposition; each run is then reversed, which applies the region again in compare mode.
#[test]
fn apply_by_history_equals_branch_enumeration_faults_and_budgets_included() {
    let mut compared = 0;
    for region in &REGION_CASES {
        let text = format!("{REGIONS}\n{}", harness(region));
        let module = load(&text);
        let bits: u32 = region.params.iter().sum::<u32>() + region.result + Fault::CODE_WIDTH.bits();
        let gates: String = (0..region.params.len())
            .flat_map(|i| (0..region.params[i]).map(move |bit| format!("    h %r{i}[{bit}]\n")))
            .collect();
        let prepare = format!("prepare{bits}:\n");
        let superposed = load(&text.replace(&prepare, &format!("{prepare}{gates}")));
        let zeros = vec![0; region.params.len() + 2];
        let mut cases: Vec<(&bennett_ir::ValidModule, Vec<u64>)> = vec![(&superposed, zeros)];
        for args in all_values(region.params) {
            for t in 0..1u64 << region.result {
                cases.push((&module, [args.clone(), vec![t, 1]].concat()));
            }
        }
        for (module, case) in cases {
            for options in [bounded(), allowing_region_faults()] {
                let mut compare = executor(module, "case", &case, Options { reversible: true, ..options });
                compare.set_apply_mode(bennett_exec::ApplyMode::Compare);
                let at_entry = compare.snapshot();
                let outcome = compare.run();
                let mut history = executor(module, "case", &case, options);
                assert_eq!(history.run(), outcome, "@{}{case:?}", region.name);
                let mut branches = executor(module, "case", &case, options);
                branches.set_apply_mode(bennett_exec::ApplyMode::Branches);
                assert_eq!(branches.run(), outcome, "@{}{case:?}", region.name);
                let (h, b) = (history.stats(), branches.stats());
                assert_eq!(
                    (h.steps, h.region_steps, h.region_faults, h.region_fault_weight, h.splits, h.peak_worlds),
                    (b.steps, b.region_steps, b.region_faults, b.region_fault_weight, b.splits, b.peak_worlds),
                    "@{}{case:?}: the promised counts",
                    region.name
                );
                common::assert_same_state(&history.snapshot(), &branches.snapshot(), &format!("@{}", region.name));
                compare.reverse().unwrap_or_else(|error| panic!("@{}{case:?}: {error}", region.name));
                common::assert_same_state(&compare.snapshot(), &at_entry, &format!("@{} reversed", region.name));
                compared += 1;
            }
        }
    }
    // (8 x 8 + 16 x 4 + 4 x 4 + 4 x 4 basis states + 4 superposed) x 2 strictness.
    assert_eq!(compared, (64 + 64 + 16 + 16 + 4) * 2);
}

#[test]
fn applying_a_region_reverses_by_applying_it_again() {
    let region = &REGION_CASES[0];
    let module = load(&format!("{REGIONS}\n{}", harness(region)));
    for case in all_values(&[3, 3, 4]) {
        assert_reverses(&module, "case", &case, allowing_region_faults());
    }
}

const FORBIDDEN: &str = r#"
memory 64

func @noisy(i1 %b) -> i1 {
entry:
    effect 33
    ret %b
}

func @quantum(i1 %b) -> i1 {
entry:
    %q = qalloc 1
    ret %b
}

func @same(i1 %b) -> i1 {
entry:
    ret %b
}

; Applies @noisy or @quantum to |+⟩ and returns the fault register.
func @main(i1 %which) -> i5 {
entry:
    %q = qalloc 1
    h %q
    %out = qalloc 1
    %e = qalloc 5
    br %which, quantum, noisy
noisy:
    apply @noisy(%q) -> %out, %e budget 10
    br finish
quantum:
    apply @quantum(%q) -> %out, %e budget 10
    br finish
finish:
    %code = measure %e
    ret %code
}

; Passes the output register as the argument through a copy the validator cannot see through.
func @alias() {
entry:
    %out = qalloc 1
    %e = qalloc 5
    br copy(%out)
copy(q1 %same_register):
    apply @same(%same_register) -> %out, %e budget 10
    ret
}
"#;

#[test]
fn effects_and_quantum_instructions_inside_a_region_are_faults() {
    let module = load(FORBIDDEN);
    let (outcome, executor) = run(&module, "main", &[0], allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::EffectInRegion.code()))));
    assert!(executor.tape().is_empty(), "the effect never happened");
    let (outcome, executor) = run(&module, "main", &[1], allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))));
    assert_eq!(executor.stats().peak_live_qubits, 7, "the region allocated nothing: 1 + 1 + a 5-qubit fault register");
    // By default the faulting apply stops the run.
    let (outcome, executor) = run(&module, "main", &[0], bounded());
    let callee = module.function_named("noisy").expect("@noisy");
    let expected = RunError::RegionFault { callee, fault: Fault::EffectInRegion, branches: 2, configurations: 2 };
    assert_eq!(outcome, Outcome::Stopped(expected));
    assert!(executor.tape().is_empty(), "the effect never happened");
    let (outcome, _) = run(&module, "alias", &[], bounded());
    assert_eq!(outcome, Outcome::Faulted(Fault::QubitAlias));
}

/// `@check(q)` faults when its two-qubit argument is 0: on one branch of four in uniform superposition.
const PARTIAL: &str = r#"
memory 64

func @check(i2 %q) -> i1 {
entry:
    %zero = icmp eq i2 %q, 0
    br %zero, bad, good
bad:
    fault unreachable
good:
    ret 1
}

; Computes check(q) into t, then uncomputes it, with one fault register for both: on the branch q = 0
; both applications fault and their codes cancel in %e. The program cannot see that anything happened.
func @main() -> i5 {
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %t = qalloc 1
    %e = qalloc 5
    apply @check(%q) -> %t, %e budget 10
    apply @check(%q) -> %t, %e budget 10
    %code = measure %e
    ret %code
}
"#;

/// How far a weight may be from its exact value after two Hadamards.
const WEIGHT_TOLERANCE: f64 = 1e-12;

/// The simulator sees every branch: it counts each `apply` that faulted somewhere, with the weight of the
/// faulting branches, even when the program's own fault register shows nothing.
#[test]
fn region_faults_are_counted_with_their_weight_even_when_the_fault_register_cancels() {
    let module = load(PARTIAL);
    for seed in 0..8 {
        let (outcome, executor) = run(&module, "main", &[], Options { seed, ..allowing_region_faults() });
        assert_eq!(outcome, Outcome::Returned(Some(0)), "the shared fault register cancels (seed {seed})");
        let stats = executor.stats();
        assert_eq!(stats.region_faults, 2, "both executions faulted on one branch");
        assert!((stats.region_fault_weight - 0.5).abs() <= WEIGHT_TOLERANCE, "{}", stats.region_fault_weight);
        assert!((stats.region_fault_max_weight - 0.25).abs() <= WEIGHT_TOLERANCE, "{}", stats.region_fault_max_weight);
    }
}

/// By default the first `apply` that faults on some branch stops the run, before it changes anything,
/// and the run still reverses to its entry state.
#[test]
fn by_default_a_region_fault_stops_the_run_before_the_apply() {
    let module = load(PARTIAL);
    let callee = module.function_named("check").expect("@check");
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let at_entry = executor.snapshot();
    let mut before = executor.snapshot();
    let outcome = loop {
        if let Some(outcome) = executor.step() {
            break outcome;
        }
        before = executor.snapshot();
    };
    let expected = RunError::RegionFault { callee, fault: Fault::Unreachable, branches: 1, configurations: 4 };
    assert_eq!(outcome, Outcome::Stopped(expected));
    let after = executor.snapshot();
    assert_eq!(after.quantum, before.quantum, "the apply changed no amplitude");
    assert_eq!((&after.classical.frames, after.classical.steps), (&before.classical.frames, before.classical.steps));
    let stats = executor.stats();
    assert_eq!(stats.region_faults, 1);
    assert!((stats.region_fault_weight - 0.25).abs() <= WEIGHT_TOLERANCE, "{}", stats.region_fault_weight);
    executor.reverse().unwrap_or_else(|error| panic!("{error}"));
    common::assert_same_state(&executor.snapshot(), &at_entry, "reversed after the region fault");
}
