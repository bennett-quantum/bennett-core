//! Small, phase-sensitive checks of a literal reversible construction against the executor.
#[path = "compute_copy/boundaries.rs"]
mod boundaries;
mod common;
mod compute_copy_reference;
#[path = "compute_copy/swap_in.rs"]
mod swap_in;

use bennett_exec::{Options, Outcome, QuantumSnapshot};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};
use compute_copy_reference::{Reference, Vector, permute};
use num_complex::Complex;

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

// Amounts 0, 4, 8 and 12 from a register: the two at least the width give zero, `ashr` included.
const SHIFTS: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    %s = mul i8 %a, 4
    %l = shl i8 3, %s
    %r = lshr i8 200, %s
    %n = ashr i8 -128, %s
    %t = xor i8 %l, %r
    %u = add i8 %t, %n
    ret %u
}
"#;

// Divisors 0, 1, -2 and -1: truncation toward zero, remainders with the dividend's sign, the minimum
// divided by minus one wrapping, unsigned remainder of the same bits, and the zero divisor's fault.
const SIGNED_DIVISION: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %d = sext i2 %x to i8
    %q = sdiv i8 -7, %d
    %r = srem i8 -7, %d
    %m = srem i8 7, %d
    %u = urem i8 250, %d
    %w = sdiv i8 -128, %d
    %z = srem i8 -128, %d
    %r3 = mul i8 %r, 3
    %m5 = mul i8 %m, 5
    %u7 = mul i8 %u, 7
    %w11 = mul i8 %w, 11
    %z13 = mul i8 %z, 13
    %s1 = add i8 %q, %r3
    %s2 = add i8 %s1, %m5
    %s3 = add i8 %s2, %u7
    %s4 = add i8 %s3, %w11
    %s5 = add i8 %s4, %z13
    ret %s5
}
"#;

// `or` with shared bits (13 = 0b1101), signed and unsigned order on the same bits, `select` and a
// widened comparison result.
const OR_AND_ORDER: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    %b = or i8 %a, 13
    %s = sext i2 %x to i8
    %c = icmp slt i8 %s, 1
    %d = icmp ugt i8 %s, 1
    %e = select i8 %c, %b, 100
    %k = zext i1 %d to i8
    %g = shl i8 %k, 7
    %h = or i8 %e, %g
    ret %h
}
"#;

// A case with no arguments, two cases sharing a target with different arguments, and the default.
const SWITCH: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    switch i2 %x, 0: zero, 2: two(i8 20, %a), 3: two(i8 30, i8 1), default: other(%a)
zero:
    br other(i8 9)
two(i8 %p, i8 %q):
    %s = add i8 %p, %q
    ret %s
other(i8 %v):
    %w = mul i8 %v, 7
    ret %w
}
"#;

// Frames: `@f` 9 slots, `@h` 3, `@g` and `@k` 2, eight bytes each. With 146 bytes of memory the
// caller's four slots leave the stack pointer at 114, `@f` takes it to 42, `@h` to 18 and `@g` to 2,
// exactly the floor (data ends at 2), which fits. Input 0 calls `@h` twice in turn, so the second call
// fits only if the first return restored the pointer. Input 1 calls `@g` through its address, input 2
// through `@f`'s address, which has another signature (`bad_indirect`), and input 3 goes one frame
// deeper through `@k`, where `@g` no longer fits (`stack_overflow`).
const FRAMES: &str = r#"
func @g(i8 %a) -> i8 {
entry:
    %b = mul i8 %a, 3
    ret %b
}
func @h(i8 %a) -> i8 {
entry:
    %b = call @g(%a)
    %c = add i8 %b, 1
    ret %c
}
func @k(i8 %a) -> i8 {
entry:
    %b = call @h(%a)
    ret %b
}
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i8
    switch i2 %x, 0: twice, 1: good, 2: bad, default: deep
twice:
    %r1 = call @h(%a)
    %r2 = call @h(%r1)
    ret %r2
good:
    %p = faddr @g
    %r3 = call_indirect fn(i8) -> i8 %p(%a)
    ret %r3
bad:
    %q = faddr @f
    %r4 = call_indirect fn(i8) -> i8 %q(%a)
    ret %r4
deep:
    %r5 = call @k(%a)
    ret %r5
}
"#;

// 21 slots: with 256 bytes the caller leaves the stack pointer at 224 and `@f` at 56, the floor 2.
// Sizes 0, 21, 42 and 63 round down to reservations of 0, 24 and 48 bytes, and 63 is more than 56
// (`stack_overflow`). The restore goes back to the saved pointer (input 0), to an unsaved, unaligned
// pointer three bytes below it, still in range (input 1), or eight bytes above the top of the frame's
// allocations (`bad_stack_restore`, input 2). The result is the reserved bytes plus four times the
// distance below the saved pointer after the restore plus the byte stored in the reservation.
const STACK: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i64
    %n = mul i64 %a, 21
    %s = stacksave
    %p = alloca %n
    %q = stacksave
    store i8 9, %p
    %v = load i8 %p
    %d = sub i64 %s, %q
    %one = icmp eq i2 %x, 1
    %two = icmp eq i2 %x, 2
    %low = sub i64 %s, 3
    %high = add i64 %s, 8
    %t1 = select i64 %one, %low, %s
    %t = select i64 %two, %high, %t1
    stackrestore %t
    %u = stacksave
    %e = sub i64 %s, %u
    %e4 = mul i64 %e, 4
    %w = zext i8 %v to i64
    %r1 = add i64 %d, %e4
    %r2 = add i64 %r1, %w
    %r = trunc i64 %r2 to i8
    ret %r
}
"#;

// 10 slots: with 168 bytes `@f` starts its reservations at 56, the floor 2. Sizes 40 and 48 give 16 and
// 8. Size 49 gives 7, rounded down to 0, below the floor. Size 54 gives 2, the floor itself, before
// rounding and 0 after: the floor is compared with the rounded pointer, the one that would be set.
const ALLOCA_ROUNDING: &str = r#"
func @f(i2 %x) -> i8 {
entry:
    %a = zext i2 %x to i64
    %b = mul i64 %a, 8
    %n1 = add i64 %b, 40
    %two = icmp eq i2 %x, 2
    %three = icmp eq i2 %x, 3
    %n2 = select i64 %two, 49, %n1
    %n = select i64 %three, 54, %n2
    %p = alloca %n
    %r = trunc i64 %p to i8
    ret %r
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

fn program(functions: &str, memory: u64) -> String {
    format!(
        "memory {memory}
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Basis {
    argument: u64,
    out: u64,
    fault: u64,
}

fn initial_vector() -> Vector<Basis> {
    let phases = [Complex::new(1, 0), Complex::new(0, 1), Complex::new(-1, 0), Complex::new(0, -1)];
    let mut vector = Vector::new();
    for x in 0..4 {
        for y in 0..4 {
            for e in 0..4 {
                let basis = Basis { argument: x, out: 64 | y, fault: 16 | e };
                let target_phase = match y {
                    0 => Complex::new(1, 0),
                    1 | 2 => Complex::new(0, 1),
                    3 => Complex::new(-1, 0),
                    _ => unreachable!("two prepared target bits"),
                };
                vector.insert(basis, phases[x as usize] * target_phase * phases[e as usize]);
            }
        }
    }
    vector
}

fn assert_amplitudes(exact: &Vector<Basis>, actual: &QuantumSnapshot) {
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
    compare_in(functions, 512, budget)
}

fn compare_in(functions: &str, memory: u64, budget: usize) -> [(u64, u64); 4] {
    let module = load(&program(functions, memory));
    let reference = Reference::new(&module, "f", "main");
    let apply = |vector| {
        reference.apply(
            vector,
            budget,
            |b: &Basis| [b.argument],
            |b, (result, fault)| {
                b.out ^= result;
                b.fault ^= fault;
            },
        )
    };
    let before = initial_vector();
    let (after, computed) = apply(before.clone());
    assert_eq!(apply(after.clone()).0, before, "the whole map is an exact involution");

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
    assert_eq!(applied.classical.steps, prepared.classical.steps + 1, "apply costs one enclosing step");
    assert_eq!(applied.classical.stack_pointer, prepared.classical.stack_pointer);
    assert_eq!(applied.classical.slots, prepared.classical.slots);
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
        let finish = reference.finish(&[x as u64], budget);
        assert_eq!(finish.output, *outcome, "the reference's own run of input {x}");
        // The final attempt of a run that faults, unlike a return, costs no step.
        let successful_steps = match (finish.ticks, finish.output.1) {
            (None, _) => budget,
            (Some(ticks), 0) => ticks,
            (Some(ticks), _) => ticks.saturating_sub(1), // An entry fault took no attempts.
        };
        assert_eq!(definite.stats().region_steps, successful_steps as u64, "input {x}, budget {budget}");
    }
    assert_eq!(
        std::array::from_fn(|x| computed[&[x as u64]]),
        outcomes,
        "per-input results agree even if a target is invariant under an XOR"
    );
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
fn shift_amounts_from_a_register_at_and_beyond_the_width() {
    assert_eq!(compare(SHIFTS, 8), [(75, 0), (52, 0), (0, 0), (0, 0)]);
}

#[test]
fn signed_quotients_and_remainders_and_the_zero_divisor() {
    let outcomes = [(0, Fault::DivideByZero.code()), (121, 0), (155, 0), (93, 0)];
    assert_eq!(compare(SIGNED_DIVISION, 20), outcomes);
}

#[test]
fn or_and_signed_and_unsigned_order() {
    assert_eq!(compare(OR_AND_ORDER, 10), [(13, 0), (100, 0), (143, 0), (143, 0)]);
}

#[test]
fn switch_cases_sharing_a_target_and_the_default() {
    assert_eq!(compare(SWITCH, 5), [(63, 0), (7, 0), (22, 0), (31, 0)]);
}

#[test]
fn frames_reserved_restored_and_overflowing_at_the_floor() {
    let outcomes = [(4, 0), (3, 0), (0, Fault::BadIndirect.code()), (0, Fault::StackOverflow.code())];
    assert_eq!(compare_in(FRAMES, 146, 16), outcomes);
    // One byte less and input 0's `@g` no longer fits either.
    assert_eq!(compare_in(FRAMES, 145, 16)[0], (0, Fault::StackOverflow.code()));
}

#[test]
fn stack_reservations_restores_and_their_faults() {
    let outcomes = [(9, 0), (45, 0), (0, Fault::BadStackRestore.code()), (0, Fault::StackOverflow.code())];
    assert_eq!(compare_in(STACK, 256, 30), outcomes);
    let outcomes = [(16, 0), (8, 0), (0, Fault::StackOverflow.code()), (0, Fault::StackOverflow.code())];
    assert_eq!(compare_in(ALLOCA_ROUNDING, 168, 12), outcomes);
    for budget in 0..=24 {
        compare_in(STACK, 256, budget);
    }
    for budget in 0..=10 {
        compare_in(ALLOCA_ROUNDING, 168, budget);
    }
}

#[test]
fn one_budget_across_the_new_families() {
    for functions in [SHIFTS, SIGNED_DIVISION, OR_AND_ORDER, SWITCH] {
        for budget in 0..=20 {
            compare(functions, budget);
        }
    }
    for budget in 0..=16 {
        compare_in(FRAMES, 146, budget);
    }
}

#[test]
fn completed_ticks_invert_nonblank_records_and_invalid_encodings() {
    let module = load(&program(ARITHMETIC, 512));
    let reference = Reference::new(&module, "f", "main");
    let words = reference.initial_machine(8).len();
    for pattern in [0, 1, 0x123456789abcdef0, u64::MAX] {
        let machine = vec![pattern; words];
        let record: Vec<_> = (0..machine.len()).map(|i| pattern.wrapping_add(i as u64)).collect();
        for inverse_first in [false, true] {
            let (mut c, mut j) = (machine.clone(), record.clone());
            reference.tick(&mut c, &mut j, inverse_first);
            reference.tick(&mut c, &mut j, !inverse_first);
            assert_eq!((c, j), (machine.clone(), record.clone()));
        }
    }
}
