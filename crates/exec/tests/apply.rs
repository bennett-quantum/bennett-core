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
                        Outcome::Returned(Some(r)) => [args.clone(), vec![t ^ r, e]].concat(),
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
func @main(i1 %which) -> i4 {
entry:
    %q = qalloc 1
    h %q
    %out = qalloc 1
    %e = qalloc 4
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
    %e = qalloc 4
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
    assert_eq!(outcome, Outcome::Returned(Some(Fault::EffectInRegion.code())));
    assert!(executor.tape().is_empty(), "the effect never happened");
    let (outcome, executor) = run(&module, "main", &[1], allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(Fault::QuantumInRegion.code())));
    assert_eq!(executor.stats().peak_live_qubits, 6, "the region allocated nothing");
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
func @main() -> i4 {
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %t = qalloc 1
    %e = qalloc 4
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
