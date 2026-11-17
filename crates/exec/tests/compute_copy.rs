//! Small, phase-sensitive checks of a literal reversible construction against the executor.
mod common;
mod compute_copy_reference;

use bennett_exec::{Options, Outcome, QuantumSnapshot};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};
use compute_copy_reference::{Reference, Vector, permute};

const ARITHMETIC: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    %b = mul i8 %a, 85
    %c = add i8 %b, 3
    %d = xor i8 %c, 6
    ret %d
}
"#;

const MEMORY: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    store i8 %a, 0
    %b = load i8 0
    %c = load i8 1
    %d = add i8 %b, %c
    ret %d
}
"#;

const CALL: &str = r#"
func @g(i8 %a) -> i8 {
entry:
    %b = mul i8 %a, 3
    %c = add i8 %b, 5
    ret %c
}
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    %b = call @g(%a)
    ret %b
}
"#;

const LOOP: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    br loop(i8 0, i8 0)
loop(i8 %i, i8 %s):
    %more = icmp ult i8 %i, %a
    br %more, body, done
body:
    %j = add i8 %i, 1
    %t = add i8 %s, %j
    br loop(%j, %t)
done:
    ret %s
}
"#;

// Neither loop is unrolled. Both trip counts depend on x; every loop and call consumes
// the same B scheduled steps for the whole function, rather than getting a fresh allowance.
const NESTED: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    %b = add i8 %a, 1
    br outer(i8 0, i8 0)
outer(i8 %i, i8 %s):
    %more = icmp ult i8 %i, %a
    br %more, begin, done
begin:
    br inner(i8 0, %s)
inner(i8 %j, i8 %t):
    %again = icmp ult i8 %j, %b
    br %again, body, advance
body:
    %k = add i8 %j, 1
    %u = add i8 %t, 1
    br inner(%k, %u)
advance:
    %next = add i8 %i, 1
    br outer(%next, %t)
done:
    ret %s
}
"#;

const DIVISION: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    store i8 %a, 0
    %b = udiv i8 12, %a
    ret %b
}
"#;

const BOUNDS: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i64
    %p = add i64 %a, 511
    %b = load i8 %p
    ret %b
}
"#;

fn conditional_fault(instruction: &str) -> String {
    format!(
        "func @f(i2 %x) -> i8 {{
entry:
    %c = icmp eq i2 %x, 0
    br %c, bad, ok
bad:
    store i8 99, 0
    {instruction}
ok:
    %a = zext i2 %x to i8
    ret %a
}}"
    )
}

fn program(functions: &str) -> String {
    format!(
        "memory 512
data 0 = i8 9, i8 7
{functions}
func @main(i64 %budget) {{
entry:
    %x = qalloc 2
    h %x[0]
    h %x[1]
    s %x[0]
    z %x[1]
    %o = qalloc 8
    x %o[6]
    h %o[0]
    h %o[1]
    s %o[0]
    s %o[1]
    %e = qalloc 5
    x %e[4]
    h %e[0]
    h %e[1]
    s %e[0]
    z %e[1]
    apply @f(%x) -> %o, %e budget %budget
    ret
}}
func @definite(i2 %x, i64 %budget) {{
entry:
    %o = qalloc 8
    %e = qalloc 5
    apply @f(%x) -> %o, %e budget %budget
    ret
}}"
    )
}

fn assert_amplitudes(exact: &Vector, actual: &QuantumSnapshot) {
    assert_eq!(actual.amplitudes.len(), exact.len());
    for (basis, amplitude) in exact {
        let actual = actual.amplitude(&[basis.argument, basis.out, basis.fault]);
        // Six H gates prepare magnitudes exactly 1/8 mathematically. The executor uses f64;
        // compare real and imaginary parts, without identifying states differing by a phase.
        assert!((actual.re - amplitude.re as f64 / 8.0).abs() < 1e-14, "{basis:?}: {actual}");
        assert!((actual.im - amplitude.im as f64 / 8.0).abs() < 1e-14, "{basis:?}: {actual}");
    }
}

// Returns the un-XORed outcome of each definite argument value, checked against analytic values
// by each test. All 64 argument/target/fault configurations carry nonzero complex amplitudes.
fn compare(functions: &str, budget: usize) -> [(u64, u64); 4] {
    let module = load(&program(functions));
    let reference = Reference::new(&module);
    let before = reference.initial_vector(budget);
    let (after, computed) = reference.apply(before.clone(), budget);
    assert_eq!(reference.apply(after.clone(), budget).0, before, "the whole map is an exact involution");

    let mut e = executor(
        &module,
        "main",
        &[budget as u64],
        Options { reversible: true, allow_region_faults: true, ..bounded() },
    );
    let entry = e.snapshot();
    // Step through preparation, stopping immediately before apply.
    for _ in 0..17 {
        assert_eq!(e.step(), None);
    }
    let prepared = e.snapshot();
    assert_amplitudes(&before, &prepared.quantum);
    assert_eq!(e.step(), None);
    let applied = e.snapshot();
    assert_amplitudes(&after, &applied.quantum);
    assert_eq!(applied.classical.memory, prepared.classical.memory, "private writes do not escape");
    assert_eq!(applied.classical.tape, prepared.classical.tape);
    assert_eq!(e.run(), Outcome::Returned(None));
    e.reverse().expect("executor reverses");
    assert_same_state(&e.snapshot(), &entry, "executor full reverse");

    let mut outcomes = [(0, 0); 4];
    for (x, outcome) in outcomes.iter_mut().enumerate() {
        // Evaluate each definite argument separately through apply, so effect/exit receive
        // their region fault codes. The executor is used only as the system under test.
        let mut definite = executor(
            &module,
            "definite",
            &[x as u64, budget as u64],
            Options { allow_region_faults: true, ..bounded() },
        );
        assert_eq!(definite.run(), Outcome::Returned(None));
        let state = definite.snapshot();
        assert_eq!(state.quantum.amplitudes.len(), 1);
        let config = &state.quantum.amplitudes[0].0;
        *outcome = (config[0], config[1]);
    }
    assert_eq!(computed, outcomes, "per-input results agree even if a target is invariant under an XOR");
    let expected = permute(before, |b| {
        let (result, fault) = outcomes[b.argument as usize];
        b.out ^= result;
        b.fault ^= fault;
    });
    assert_eq!(after, expected, "construction equals per-input evaluation, including phases");
    outcomes
}

#[test]
fn straight_line_arithmetic() {
    assert_eq!(compare(ARITHMETIC, 8), [(5, 0), (94, 0), (171, 0), (4, 0)]);
}

#[test]
fn private_memory_write_and_read() {
    assert_eq!(compare(MEMORY, 8), [(7, 0), (8, 0), (9, 0), (10, 0)]);
}

#[test]
fn ordinary_call() {
    assert_eq!(compare(CALL, 8), [(5, 0), (8, 0), (11, 0), (14, 0)]);
}

#[test]
fn argument_dependent_loop() {
    assert_eq!(compare(LOOP, 24), [(0, 0), (1, 0), (3, 0), (6, 0)]);
}

#[test]
fn two_argument_dependent_nested_loops() {
    assert_eq!(compare(NESTED, 100), [(0, 0), (2, 0), (6, 0), (12, 0)]);
}

#[test]
fn division_fault_after_private_write() {
    assert_eq!(compare(DIVISION, 8), [(0, Fault::DivideByZero.code()), (12, 0), (6, 0), (4, 0)]);
}

#[test]
fn out_of_range_access() {
    let fault = (0, Fault::OutOfBounds.code());
    assert_eq!(compare(BOUNDS, 8), [(0, 0), fault, fault, fault]);
}

#[test]
fn effect_and_exit_faults_after_private_writes() {
    for (instruction, fault) in [("effect 65\n    ret 0", Fault::EffectInRegion), ("exit i8 0", Fault::ExitInRegion)] {
        let functions = conditional_fault(instruction);
        assert_eq!(compare(&functions, 8), [(0, fault.code()), (1, 0), (2, 0), (3, 0)]);
        for budget in 0..=6 {
            compare(&functions, budget);
        }
    }
}

#[test]
fn one_budget_for_the_whole_function_including_calls_and_nested_loops() {
    for functions in [ARITHMETIC, MEMORY, CALL, DIVISION] {
        for budget in 0..=8 {
            compare(functions, budget);
        }
    }
    for functions in [LOOP, NESTED] {
        for budget in [0, 1, 4, 5, 6, 10, 16, 24, 40, 64, 100] {
            compare(functions, budget);
        }
    }
    let exhausted = (0, Fault::Budget.code());
    assert_eq!(compare(LOOP, 5), [(0, 0), exhausted, exhausted, exhausted]);
    assert_eq!(compare(NESTED, 6), [(0, 0), exhausted, exhausted, exhausted]);
}

#[test]
fn completed_ticks_invert_nonblank_records_and_invalid_encodings() {
    let module = load(&program(ARITHMETIC));
    let reference = Reference::new(&module);
    let basis = reference.initial_vector(1).into_keys().next().expect("basis");
    for pattern in [0, 1, 0x123456789abcdef0, u64::MAX] {
        let machine = vec![pattern; basis.machine.len()];
        let record: Vec<_> = (0..machine.len()).map(|i| pattern.wrapping_add(i as u64)).collect();
        for inverse_first in [false, true] {
            let (mut c, mut j) = (machine.clone(), record.clone());
            reference.tick(&mut c, &mut j, inverse_first);
            reference.tick(&mut c, &mut j, !inverse_first);
            assert_eq!((c, j), (machine.clone(), record.clone()));
        }
    }
}
