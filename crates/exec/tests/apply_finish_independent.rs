//! Independent finite-state calculations of `apply`. Expectations are explicit arithmetic/step counts and
//! sparse matrix products, never another executor's answers.
//! Zero ancillas may be omitted, but the maps below are identical for every amplitude support.
mod common;

use bennett_exec::{Options, Outcome, Snapshot};
use bennett_ir::Fault;
use num_complex::Complex64 as C;
use std::collections::BTreeMap;

const HALF: f64 = std::f64::consts::FRAC_1_SQRT_2;
type State = BTreeMap<Vec<u64>, C>;

fn assert_vector(snapshot: &Snapshot, expected: &State, columns: usize) {
    assert!(snapshot.worlds.is_empty(), "completed circuit must have joined: {snapshot:?}");
    let mut actual = State::new();
    for (config, amplitude) in &snapshot.quantum.amplitudes {
        assert!(config.len() <= columns, "unexpected extra wires: {snapshot:?}");
        let mut config = config.clone();
        config.resize(columns, 0);
        *actual.entry(config).or_default() += amplitude;
    }
    for key in expected.keys().chain(actual.keys()) {
        assert!(
            (actual.get(key).copied().unwrap_or_default() - expected.get(key).copied().unwrap_or_default()).norm()
                < 1e-11,
            "configuration {key:?}: actual {actual:?}; expected {expected:?}"
        );
    }
}

fn h(mut state: State, column: usize) -> State {
    let mut out = State::new();
    for (config, a) in &mut state {
        for bit in 0..2 {
            let mut next = config.clone();
            next[column] ^= (next[column] & 1) ^ bit;
            *out.entry(next).or_default() += *a * if bit & config[column] == 1 { -HALF } else { HALF };
        }
    }
    out
}

fn check(text: &str, expected: &State, columns: usize) {
    let p = common::load(text);
    let mut e =
        common::executor(&p, "main", &[], Options { reversible: true, machine_meaning: true, ..common::bounded() });
    let initial = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None), "\n{text}");
    let snapshot = e.snapshot();
    assert_vector(&snapshot, expected, columns);
    assert_eq!(snapshot.classical.memory[64..80], [0; 16], "callee writes restored");
    e.reverse().unwrap_or_else(|error| panic!("{error}\n{text}"));
    whole_state(&e.snapshot(), &initial, "entire run reverses");
}

/// Four input/dirty-target/spectator factors. H after the oracle makes relative phases visible.
/// f(x) either XORs a result into o, or a fault code into e, once per iteration.
fn expected_apply(width: u32, repeats: usize, f: impl Fn(u64) -> Result<u64, Fault>) -> State {
    let high = 1u64 << (width - 1);
    let mut state = State::new();
    for bit in 0..2 {
        let x = if width == 1 { bit } else { high | bit };
        let ax = if bit == 0 { C::new(HALF, 0.0) } else { C::new(0.0, HALF) };
        for low in 0..2 {
            let o = if width == 1 { low } else { high | low };
            let ao = if width == 1 && low == 1 { -HALF } else { HALF };
            for fault_bit in 0..2 {
                let e = 16 | (fault_bit << 1);
                let ae = if fault_bit == 0 { C::new(HALF, 0.0) } else { C::new(0.0, HALF) };
                for spectator in 0..2 {
                    let a = if spectator == 0 { C::new(0.37f64.cos(), 0.0) } else { C::new(0.0, -0.37f64.sin()) };
                    let (mut o, mut e) = (o, e);
                    if !repeats.is_multiple_of(2) {
                        match f(x) {
                            Ok(value) => o ^= value,
                            Err(fault) => e ^= fault.code(),
                        }
                    }
                    *state.entry(vec![x, o, e, spectator]).or_default() += ax * ao * ae * a;
                }
            }
        }
    }
    h(state, 0)
}

fn harness(width: u32, function: &str, budget: u64, repeats: usize) -> String {
    format!(
        "memory 8192\n{function}\nfunc @main() {{\nentry:\n%q = qalloc {width}\n%o = qalloc {width}\n%e = qalloc 5\n%s = qalloc 1\nh %q[0]\ns %q[0]\n{}x %o[{}]\nh %o[0]\nx %e[4]\nh %e[1]\ns %e[1]\nry(0.74) %s\nsdg %s\nbr loop(i8 0)\nloop(i8 %i):\napply @f(%q) -> %o, %e budget {budget}\n%j = add i8 %i, 1\n%c = icmp ult i8 %j, {repeats}\nbr %c, loop(%j), done\ndone:\nh %q[0]\nret\n}}",
        if width > 1 { format!("x %q[{}]\n", width - 1) } else { String::new() },
        width - 1
    )
}

#[test]
fn every_register_keep_width_with_dirty_complex_targets_and_looped_applies() {
    let mut runs = 0;
    for width in 1..=64 {
        let mask = u64::MAX >> (64 - width);
        let k = 0xa93d_b68f_2475_01ceu64 & mask;
        // Three instructions: xor, store, ret. B=2 faults; B=3 completes exactly.
        let f = format!(
            "func @f(i{width} %x) -> i{width} {{\nentry:\n%r = xor i{width} %x, {k}\nstore i{width} %r, 64\nret %r\n}}"
        );
        for budget in [0, 2, 3, 4] {
            for repeats in [1, 2, 3] {
                let expected =
                    expected_apply(width, repeats, |x| if budget < 3 { Err(Fault::Budget) } else { Ok(x ^ k) });
                check(&harness(width, &f, budget, repeats), &expected, 4);
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 768);
}

#[test]
fn branch_faults_and_variable_work_match_hand_counts() {
    // loop makes n=x+1 additions, 3 setup steps + 4n loop steps + return = 4x+8.
    let f = "func @f(i2 %x) -> i2 {\nentry:\n%n = zext i2 %x to i8\n%limit = add i8 %n, 1\nbr loop(i8 0, i2 1)\nloop(i8 %i, i2 %sum):\n%a = add i2 %sum, 3\n%j = add i8 %i, 1\n%c = icmp ult i8 %j, %limit\nbr %c, loop(%j, %a), done(%a)\ndone(i2 %r):\nret %r\n}";
    for budget in 0..=21 {
        check(
            &harness(2, f, budget, 1),
            &expected_apply(2, 1, |x| if budget < 4 * x + 8 { Err(Fault::Budget) } else { Ok((1 + 3 * (x + 1)) & 3) }),
            4,
        );
    }
    // Odd inputs fault divide_by_zero on step 3; even inputs return 3 on step 4.
    let f = "func @f(i2 %x) -> i2 {\nentry:\n%b = and i2 %x, 1\n%d = xor i2 %b, 1\n%r = udiv i2 3, %d\nret %r\n}";
    for budget in 0..=6 {
        let expected = expected_apply(2, 1, |x| {
            if budget < 3 {
                Err(Fault::Budget)
            } else if x & 1 != 0 {
                Err(Fault::DivideByZero)
            } else if budget < 4 {
                Err(Fault::Budget)
            } else {
                Ok(3)
            }
        });
        check(&harness(2, f, budget, 1), &expected, 4);
    }
}

#[test]
fn jumps_inside_a_region_and_escaping_a_region_and_nested_apply_faults() {
    let local = "func @f(i2 %x) -> i2 {\nentry:\n%p = alloca 32\n%j = setjmp %p\n%first = icmp eq i32 %j, 0\nbr %first, jump, done\njump:\nlongjmp %p, 3\ndone:\n%n = trunc i32 %j to i2\n%r = xor i2 %x, %n\nret %r\n}";
    check(&harness(2, local, 40, 1), &expected_apply(2, 1, |x| Ok(x ^ 3)), 4);
    // A valid caller setjmp is below the region boundary, so jumping to it is bad_longjmp,
    // independent of the fact that its record is a valid record for ordinary code.
    let escape = "func @f(i2 %x) -> i2 {\nentry:\nlongjmp 128, 1\n}";
    let text = harness(2, escape, 20, 1).replace("%q = qalloc", "%saved = setjmp 128\n%q = qalloc");
    check(&text, &expected_apply(2, 1, |_| Err(Fault::BadLongjmp)), 4);
    // A region cannot acquire the quantum targets for an inner apply: integer-only parameters
    // cannot carry them, and the first allocation is already quantum_in_region.
    let nested = "func @id(i2 %x) -> i2 {\nentry:\nret %x\n}\nfunc @f(i2 %x) -> i2 {\nentry:\n%a = qalloc 2\n%b = qalloc 5\napply @id(%x) -> %a, %b budget 4\nret %x\n}";
    check(&harness(2, nested, 20, 1), &expected_apply(2, 1, |_| Err(Fault::QuantumInRegion)), 4);
}

#[test]
fn textual_forms_and_applies_under_nested_rewinds_have_the_same_dense_answer() {
    let f = "func @f(i2 %x) -> i2 {\nentry:\n%r = xor i2 %x, 3\nret %r\n}";
    let original = harness(2, f, 4, 1);
    let textual = original.replace(
        "apply @f(%q) -> %o, %e budget 4",
        "%m = mark budget 7\n%y, %code = call @f(%q) budget 4\nrewind %m keep (%y -> %o, %code -> %e)",
    );
    // Caller SSA slots differ in the literal expansion; quantum/memory and independent inverses agree.
    let expected = expected_apply(2, 1, |x| Ok(x ^ 3));
    check(&textual, &expected, 4);
    // Inner work is undone, the outer work is undone, then one remaining apply supplies f(x).
    // Ample finite bounds cover the forms' permitted waits. No mark is rewound twice.
    let nested = original.replace("apply @f(%q) -> %o, %e budget 4", "%outer = mark budget 300\napply @f(%q) -> %o, %e budget 4\n%inner = mark budget 100\napply @f(%q) -> %o, %e budget 4\nrewind %inner\nrewind %outer\napply @f(%q) -> %o, %e budget 4");
    check(&nested, &expected, 4);
}

#[test]
fn replay_work_is_charged_to_each_nested_call_budget() {
    // Six forward steps: mark, add, rewind, add, rewind, ret. The second rewind
    // undoes the first rewind by running its single add again: one replay step.
    // Thus work succeeds at B=7, and gives the code budget below that boundary.
    let body = "func @work(i2 %x) -> i2 {\nentry:\n%m = mark\n%a = add i2 %x, 1\nrewind %m\n%b = add i2 %x, 2\nrewind %m\nret %x\n}";
    for inner in 0..=12 {
        for outer in [1, 64] {
            let f = format!(
                "{body}\nfunc @f(i2 %x) -> i2 {{\nentry:\n%y, %code = call @work(%x) budget {inner}\n%r = trunc i5 %code to i2\n%v = xor i2 %y, %r\nret %v\n}}"
            );
            let expected = expected_apply(2, 1, |x| {
                if outer == 1 {
                    Err(Fault::Budget)
                } else if inner < 7 {
                    Ok(Fault::Budget.code() & 3)
                } else {
                    Ok(x)
                }
            });
            check(&harness(2, &f, outer, 1), &expected, 4);
        }
    }
}

fn whole_state(actual: &bennett_exec::Snapshot, expected: &bennett_exec::Snapshot, context: &str) {
    common::assert_same_state(actual, expected, context);
    assert_eq!(actual.worlds.len(), expected.worlds.len(), "{context}: other worlds");
    for (actual, expected) in actual.worlds.iter().zip(&expected.worlds) {
        whole_state(actual, expected, context);
    }
}
