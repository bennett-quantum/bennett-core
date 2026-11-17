//! Independent attacks on the region boundary, with analytic answers.
mod common;

use bennett_exec::{Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};

const CELL_FAULT_ORDER: &str = r#"
memory 512
func @f() -> i1 {
entry:
    %x = load i8 0
    %y = xor i8 %x, 1
    store i8 %y, 0
    %zero = icmp eq i8 %x, 0
    br %zero, a, b
a:
    fault divide_by_zero
b:
    fault unreachable
}
func @main() {
entry:
    h [0:0]
    %o = qalloc 1
    %e = qalloc 5
    apply @f() -> %o, %e budget 100
    ret
}
"#;

#[test]
fn strict_fault_uses_the_first_original_configuration() {
    let module = load(CELL_FAULT_ORDER);
    let mut e = executor(&module, "main", &[], bounded());
    assert_eq!(
        e.run(),
        Outcome::Stopped(RunError::RegionFault {
            callee: module.function_named("f").expect("f"),
            fault: Fault::DivideByZero,
            branches: 2,
            configurations: 2,
        })
    );
}

// A region whose function returns 2.
const RETURNS_TWO: &str = r#"
memory 512
func @f() -> i64 {
entry:
    ret 2
}
func @main() {
entry:
    %o = qalloc 64
    %e = qalloc 5
    apply @f() -> %o, %e budget 10
    ret
}
"#;

// Cell loads and stores run under the region's rules. The function reads both cells after their values
// have diverged, writes an overlapping i16, then jumps through a callee.
const MIXED: &str = r#"
memory 4096
data 0 = i16 513
func @jump(i8 %x) -> i8 {
entry:
    %p = alloca 32
    %j = setjmp %p
    %first = icmp eq i32 %j, 0
    br %first, throw, done
throw:
    longjmp %p, 3
done:
    %j8 = trunc i32 %j to i8
    %r = add i8 %x, %j8
    ret %r
}
func @f(i2 %x) -> i8 {
entry:
    %a = load i8 0
    %b = load i8 1
    %w = zext i8 %b to i16
    store i16 %w, 0
    %odd = icmp eq i8 %b, 0
    br %odd, short, long
short:
    br join
long:
    %t = mul i8 %a, 7
    store i8 %t, 80
    br join
join:
    %b2 = load i8 1
    %x8 = zext i2 %x to i8
    %sum = add i8 %a, %b2
    %sum2 = add i8 %sum, %x8
    %r = call @jump(%sum2)
    ret %r
}
func @main(i64 %budget) {
entry:
    h [0:0]
    h [1:1]
    %x = qalloc 2
    h %x[0]
    ry(0.7) %x[1]
    %s = qalloc 1
    h %s
    s %s
    %o = qalloc 8
    x %o[2]
    %e = qalloc 5
    x %e[3]
    apply @f(%x) -> %o, %e budget %budget
    ret
}
"#;

/// Runs `main(args)` reversibly, allowing region faults, to its return, and reverses it to its entry.
fn runs_and_reverses(text: &str, args: &[u64]) {
    let p = load(text);
    let mut e = executor(&p, "main", args, Options { reversible: true, allow_region_faults: true, ..bounded() });
    let entry = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None));
    e.reverse().expect("reverses");
    assert_same_state(&e.snapshot(), &entry, "full reverse");
}

#[test]
fn cell_loads_stores_and_jumps_in_a_region_run_and_reverse_at_every_budget() {
    for budget in [0, 1, 3, 5, 8, 12, 16, 20, 24, 28, 32, 40, 64] {
        runs_and_reverses(MIXED, &[budget]);
    }
}

const NESTED: &str = r#"
memory 512
func @inner() -> i1 {
entry:
    ret 1
}
func @outer() -> i1 {
entry:
    %q = qalloc 1
    %e = qalloc 5
    apply @inner() -> %q, %e budget 3
    ret 0
}
func @main() {
entry:
    %q = qalloc 1
    %e = qalloc 5
    apply @outer() -> %q, %e budget 10
    ret
}
"#;

#[test]
fn quantum_instructions_reached_through_calls_fault_inside_the_region() {
    // The outer region contains a nested apply; its first qalloc must fault before execution can
    // reach it. Effects and exit reached through the same call boundary have their own codes.
    for (body, fault) in [
        ("%q = qalloc 1", Fault::QuantumInRegion),
        // A measurement at an address is a quantum instruction like a register's, whatever the bytes hold.
        ("%m = measure i8 [16]", Fault::QuantumInRegion),
        ("effect 65", Fault::EffectInRegion),
        ("exit i1 0", Fault::ExitInRegion),
    ] {
        let start = NESTED.find("    %q = qalloc 1").expect("body");
        let end = NESTED[start..].find("\n}").expect("end") + start;
        let text = format!("{}    {body}\n    ret 0{}", &NESTED[..start], &NESTED[end..]);
        // Exit is a terminator, so remove the unreachable second terminator.
        let text = text.replace("exit i1 0\n    ret 0", "exit i1 0");
        let p = load(&text);
        let mut e = executor(&p, "main", &[], Options { allow_region_faults: true, ..bounded() });
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_eq!(e.snapshot().quantum.amplitudes[0].0, [0, fault.code()]);
        runs_and_reverses(&text, &[]);
    }
}

/// A large region with only four argument values; the other qubits are spectators. The opt-in peak
/// probe also supports BENNETT_TEST_BITS=20 and BENNETT_TEST_MODE=history|branches for /usr/bin/time -v.
fn large_region(bits: u32) -> String {
    let gates: String = (0..bits).map(|b| format!("    h %s[{b}]\n")).collect();
    format!(
        r#"
memory 1024
func @f(i2 %x) -> i2 {{
entry:
    %r = xor i2 %x, 3
    ret %r
}}
func @main() {{
entry:
    %s = qalloc {bits}
{gates}    %x = qalloc 2
    h %x[0]
    h %x[1]
    %o = qalloc 2
    %e = qalloc 5
    apply @f(%x) -> %o, %e budget 2
    ret
}}
"#
    )
}

#[test]
#[ignore = "large support/peak RSS probe: cargo test -p bennett-exec --test apply_independent amplitude_peak_probe -- --ignored --nocapture"]
fn amplitude_peak_probe() {
    let bits = std::env::var("BENNETT_TEST_BITS").map_or(15, |s| s.parse().expect("bits"));
    let p = load(&large_region(bits));
    let mut e = executor(&p, "main", &[], Options { max_configurations: Some(1 << (bits + 2)), ..bounded() });
    assert_eq!(e.run(), Outcome::Returned(None));
    let stats = e.stats();
    assert_eq!(stats.configurations, 1 << (bits + 2));
    assert_eq!(stats.region_steps, 2 * (1 << (bits + 2)));
    assert_eq!(stats.region_cost.runs, 1 << (bits + 2));
    println!("{stats:?}");
    // No snapshot: it would add another full copy and contaminate the peak measurement.
}

#[test]
fn two_identical_regions_must_xor_the_same_result() {
    let text = RETURNS_TWO.replace("    ret\n}", "    apply @f() -> %o, %e budget 10\n    ret\n}");
    let p = load(&text);
    let mut e = executor(&p, "main", &[], Options { reversible: true, ..bounded() });
    assert_eq!(e.run(), Outcome::Returned(None));
    assert_eq!(e.snapshot().quantum.amplitudes[0].0, [0, 0], ": apply is its own inverse");
}

#[test]
fn strict_budget_faults_leave_the_pre_apply_state_and_reverse() {
    let p = load(MIXED);
    for budget in [0, 1, 12, 24, 32, 64] {
        let mut e = executor(&p, "main", &[budget], Options { reversible: true, ..bounded() });
        let entry = e.snapshot();
        // Stop exactly at apply, preserving the complete visible state before a strict refusal.
        while e.snapshot().classical.frames.last().expect("frame").inst < 12 {
            assert_eq!(e.step(), None);
        }
        let before = e.snapshot();
        let result = e.step();
        if let Some(Outcome::Stopped(RunError::RegionFault { fault, .. })) = result {
            assert_eq!(fault, Fault::Budget);
            let mut after = e.snapshot();
            after.classical.outcome = before.classical.outcome;
            assert_same_state(&after, &before, "strict stop is atomic");
        } else {
            assert_eq!(result, None);
            assert_eq!(e.run(), Outcome::Returned(None));
        }
        e.reverse().expect("strict or successful run reverses");
        assert_same_state(&e.snapshot(), &entry, "strict reverse");
    }
}

#[test]
fn allowed_cell_fault_codes_and_private_writes_run_and_reverse() {
    runs_and_reverses(CELL_FAULT_ORDER, &[]);
}

#[test]
fn physical_return_addresses_and_an_entry_frame_that_cannot_fit() {
    let text = MIXED.replace("    %j8 = trunc i32 %j to i8", "    %ra = retaddr 1\n    %low = trunc i64 %ra to i32\n    %jr = xor i32 %j, %low\n    %j8 = trunc i32 %jr to i8");
    runs_and_reverses(&text, &[80]);
    let small = r#"
memory 64
func @f(i2 %x) -> i2 {
entry:
    %a = add i2 %x, 1
    %b = add i2 %a, 1
    %c = add i2 %b, 1
    %d = add i2 %c, 1
    %e = add i2 %d, 1
    %f = add i2 %e, 1
    ret %f
}
func @main() {
entry:
    %x = qalloc 2
    h %x[0]
    h %x[1]
    %o = qalloc 2
    %e = qalloc 5
    apply @f(%x) -> %o, %e budget 100
    ret
}
"#;
    runs_and_reverses(small, &[]);
    let p = load(small);
    let mut e = executor(&p, "main", &[], Options { allow_region_faults: true, ..bounded() });
    assert_eq!(e.run(), Outcome::Returned(None));
    for (c, _) in &e.snapshot().quantum.amplitudes {
        assert_eq!(c[1..], [0, Fault::StackOverflow.code()]);
    }
    assert_eq!(e.stats().region_steps, 0);
}

#[test]
#[ignore = "large analytic support: cargo test -p bennett-exec --test apply_independent more_than_65536 -- --ignored --nocapture"]
fn more_than_65536_configurations_have_the_analytic_answer() {
    let p = load(&large_region(15));
    let mut e = executor(&p, "main", &[], Options { max_configurations: Some(1 << 17), ..bounded() });
    assert_eq!(e.run(), Outcome::Returned(None));
    let state = e.snapshot();
    assert_eq!(state.quantum.amplitudes.len(), 1 << 17);
    let expected = 1.0 / f64::from(1 << 17).sqrt();
    for (c, a) in &state.quantum.amplitudes {
        assert_eq!(c[2..], [c[1] ^ 3, 0]);
        assert!((a.re - expected).abs() < 1e-15 && a.im == 0.0);
    }
    println!("131072 exact configuration images, amplitudes within 1e-15");
}

#[test]
fn apply_through_an_ordinary_call_keeps_the_real_caller_frames() {
    let text = MIXED.replace("func @main(i64 %budget)", "func @driver(i64 %budget)")
        + "\nfunc @main(i64 %budget) {\nentry:\n    call @driver(%budget)\n    ret\n}\n";
    for budget in [0, 24, 80] {
        runs_and_reverses(&text, &[budget]);
    }
}

// What the region's function observes is that of a run on a journal and memory of its own, never the
// caller's history. Each body runs after caller prefixes that journal different amounts (gates, an earlier
// `apply`), reversible or not; the state must not depend on the prefix, an `apply` twice must be no
// `apply`, and every reversible run must reverse to its entry.
const REGION_BODIES: &[&str] = &[
    // A constant.
    "    ret 2\n",
    // A store to private memory, then a value of x.
    "    store i8 100, 8\n    %x64 = zext i2 %x to i64\n    %s = shl i64 %x64, 8\n    %r = or i64 %s, 2\n    ret %r\n",
    // A load of a qubit in memory (its value in each run's private memory), paths of different lengths.
    "    %a = load i8 0\n    %c = icmp eq i8 %a, 0\n    br %c, p, q\np:\n    %u = add i8 %a, 1\n    br q\nq:\n    %x64 = zext i2 %x to i64\n    %a64 = zext i8 %a to i64\n    %r = add i64 %a64, %x64\n    ret %r\n",
    // A callee of the region.
    "    %k = zext i2 %x to i64\n    %r = call @g(%k)\n    ret %r\n",
    // A store on one path only, then a load of it.
    "    %c = icmp eq i2 %x, 1\n    br %c, p, q\np:\n    store i2 %x, 8\n    br q\nq:\n    %v = load i8 8\n    %r = zext i8 %v to i64\n    ret %r\n",
];

const REGION_PREFIXES: &[&str] = &[
    "",
    "    x %t\n    x %t\n",
    "    apply @zero(%x) -> %t, %te budget 8\n",
    "    apply @zero(%x) -> %t, %te budget 8\n    x %t\n    x %t\n",
];

fn region_program(body: &str, prefix: &str, applies: usize) -> String {
    let applies = "    apply @f(%x) -> %o, %e budget 200\n".repeat(applies);
    format!(
        "memory 512
func @zero(i2 %x) -> i1 {{
entry:
    ret 0
}}
func @g(i64 %k) -> i64 {{
entry:
    %r = add i64 %k, 4
    ret %r
}}
func @f(i2 %x) -> i64 {{
entry:
{body}}}
func @main() {{
entry:
    h [0:0]
    %x = qalloc 2
    h %x[0]
    h %x[1]
    %t = qalloc 1
    %te = qalloc 5
    %o = qalloc 64
    %e = qalloc 5
{prefix}{applies}    ret
}}
"
    )
}

fn final_state(text: &str, reversible: bool) -> bennett_exec::QuantumSnapshot {
    let p = load(text);
    let mut e = executor(&p, "main", &[], Options { reversible, ..common::allowing_region_faults() });
    let entry = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None), "\n{text}");
    let state = e.snapshot().quantum;
    if reversible {
        e.reverse().expect("reverses");
        assert_same_state(&e.snapshot(), &entry, "full reverse");
    }
    state
}

fn assert_same_quantum(
    actual: &bennett_exec::QuantumSnapshot,
    expected: &bennett_exec::QuantumSnapshot,
    context: &str,
) {
    let distance = actual.distance(expected);
    assert!(distance.is_some_and(|d| d <= 1e-12), "{context}: {distance:?}\n{actual:?}\n{expected:?}");
}

#[test]
fn what_a_region_sees_of_its_history_does_not_depend_on_the_caller() {
    for (b, body) in REGION_BODIES.iter().enumerate() {
        for reversible in [false, true] {
            let expected = final_state(&region_program(body, "", 1), reversible);
            for (p, prefix) in REGION_PREFIXES.iter().enumerate() {
                let none = final_state(&region_program(body, prefix, 0), reversible);
                let context = format!("body {b}, prefix {p}, reversible={reversible}");
                let once = final_state(&region_program(body, prefix, 1), reversible);
                assert_same_quantum(&once, &expected, &context);
                let twice = final_state(&region_program(body, prefix, 2), reversible);
                assert_same_quantum(&twice, &none, &format!("{context}: twice is no apply"));
            }
        }
    }
}

// The results of the bodies above, analytically, after every prefix.
#[test]
fn region_results_are_the_analytic_values_after_every_prefix() {
    // Body 4: only the run for x = 1 stores x at 8, in its own private memory.
    for (body, expected) in [
        (REGION_BODIES[0], [2; 4]),
        (REGION_BODIES[1], [2, 1 << 8 | 2, 2 << 8 | 2, 3 << 8 | 2]),
        (REGION_BODIES[3], [4, 5, 6, 7]),
        (REGION_BODIES[4], [0, 1, 0, 0]),
    ] {
        for prefix in REGION_PREFIXES {
            let state = final_state(&region_program(body, prefix, 1), true);
            // Columns: the cell, x, t, te, o, e.
            for (config, _) in &state.amplitudes {
                assert_eq!(config[4], expected[config[1] as usize], "{body}\n{prefix}");
                assert_eq!(config[5], 0);
            }
        }
    }
}

// The whole class of F2: a strict region fault is the first faulting configuration's, in the order of the
// configurations as they were before the `apply`,
// whatever the region writes to the qubits in memory it read. Two cells (byte 0 in four values, byte 1 in two),
// rewritten in place by every kind of permutation (none, XOR, increment, swap, reversal), and every
// assignment of outcomes to the eight configurations a salt chooses.
#[test]
fn strict_fault_is_the_first_original_configurations_whatever_the_region_writes_to_its_cells() {
    let rewrites = [
        "",
        "    %v = xor i8 %a, 3\n    store i8 %v, 0\n",
        "    %v = add i8 %a, 1\n    store i8 %v, 0\n",
        "    store i8 %b, 0\n    store i8 %a, 1\n",
        "    %v = sub i8 3, %a\n    store i8 %v, 0\n    %w = xor i8 %b, 1\n    store i8 %w, 1\n",
    ];
    let faults = [None, Some(Fault::DivideByZero), Some(Fault::Unreachable), Some(Fault::OutOfBounds)];
    for (r, rewrite) in rewrites.iter().enumerate() {
        for salt in 0..4u64 {
            let text = format!(
                "memory 512
func @f() -> i1 {{
entry:
    %a = load i8 0
    %b = load i8 1
{rewrite}    %k0 = mul i8 %a, 3
    %k1 = mul i8 %b, 5
    %k2 = add i8 %k0, %k1
    %k3 = add i8 %k2, {salt}
    %k = and i8 %k3, 3
    %z = icmp eq i8 %k, 0
    br %z, ok, n1
n1:
    %c1 = icmp eq i8 %k, 1
    br %c1, f1, n2
n2:
    %c2 = icmp eq i8 %k, 2
    br %c2, f2, f3
ok:
    ret 0
f1:
    fault divide_by_zero
f2:
    fault unreachable
f3:
    fault out_of_bounds
}}
func @main() {{
entry:
    h [0:0]
    h [0:1]
    h [1:0]
    %o = qalloc 1
    %e = qalloc 5
    apply @f() -> %o, %e budget 100
    ret
}}
"
            );
            // The configurations in order: byte 0's register is the first column, byte 1's the second.
            let outcome = |a: u64, b: u64| faults[((a * 3 + b * 5 + salt) & 3) as usize];
            let faulting: Vec<Fault> =
                (0..4).flat_map(|a| (0..2).map(move |b| (a, b))).filter_map(|(a, b)| outcome(a, b)).collect();
            let p = load(&text);
            let mut e = executor(&p, "main", &[], Options { reversible: true, ..bounded() });
            let entry = e.snapshot();
            let expected = match faulting.first() {
                Some(&fault) => Outcome::Stopped(RunError::RegionFault {
                    callee: p.function_named("f").expect("f"),
                    fault,
                    branches: faulting.len(),
                    configurations: 8,
                }),
                None => Outcome::Returned(None),
            };
            assert_eq!(e.run(), expected, "rewrite {r}, salt {salt}");
            e.reverse().expect("reverses");
            assert_same_state(&e.snapshot(), &entry, "full reverse");
        }
    }
}

#[test]
fn strict_fault_order_uses_fault_register_values_before_the_keep() {
    let module = load(
        "memory 512
func @f(i1 %x) -> i1 {
entry:
br %x, low, high
low:
fault divide_by_zero
high:
fault unreachable
}
func @main() {
entry:
%e = qalloc 5
%q = qalloc 1
%o = qalloc 1
h %q
s %q
apply @f(%q) -> %o, %e budget 4
ret
}",
    );
    let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let entry = e.snapshot();
    for _ in 0..5 {
        assert_eq!(e.step(), None);
    }
    let before = e.snapshot();
    assert_eq!(
        e.step(),
        Some(Outcome::Stopped(RunError::RegionFault {
            callee: module.function_named("f").expect("f"),
            fault: Fault::Unreachable,
            branches: 2,
            configurations: 2,
        }))
    );
    let mut after = e.snapshot();
    after.classical.outcome = None;
    assert_same_state(&after, &before, "strict refusal restores phases and the zero fault register");
    e.reverse().expect("reverses");
    assert_same_state(&e.snapshot(), &entry, "fault reverses");
}
