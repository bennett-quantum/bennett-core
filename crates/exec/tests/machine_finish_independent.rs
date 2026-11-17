//! Independent circuit calculations for the machine-finish changes. The oracle below enumerates a
//! small dense input, applies the stated XOR maps, and traces out leftover wires by tensor contraction.
//! It never asks the executor to calculate an expected value. Only finite mark budgets are used.

mod common;

use std::collections::BTreeMap;

use bennett_exec::{ApplyMode, Executor, Options, Outcome, Snapshot};
use bennett_ir::Fault;
use common::{bounded, executor, load};
use num_complex::Complex64 as C;

const EPS: f64 = 1e-12;
const HALF: f64 = std::f64::consts::FRAC_1_SQRT_2;
type State = BTreeMap<Vec<u64>, C>;

/// Same circuit, different support and relative phases, including unequal complex amplitudes.
fn preparations() -> Vec<(&'static str, [C; 2])> {
    vec![
        ("", [C::new(1.0, 0.0), C::new(0.0, 0.0)]),
        ("x %c", [C::new(0.0, 0.0), C::new(1.0, 0.0)]),
        ("h %c", [C::new(HALF, 0.0), C::new(HALF, 0.0)]),
        ("h %c\nz %c", [C::new(HALF, 0.0), C::new(-HALF, 0.0)]),
        ("h %c\ns %c", [C::new(HALF, 0.0), C::new(0.0, HALF)]),
        ("h %c\nsdg %c", [C::new(HALF, 0.0), C::new(0.0, -HALF)]),
        ("ry(0.74) %c\ns %c", [C::new(0.37f64.cos(), 0.0), C::new(0.0, 0.37f64.sin())]),
    ]
}

fn image(input: [C; 2], map: impl Fn(u64) -> Vec<u64>) -> State {
    input.into_iter().enumerate().filter(|(_, a)| a.norm() > EPS).map(|(c, a)| (map(c as u64), a)).collect()
}

/// Standard H on the control, after the un-run: phase errors become probabilities as well.
fn hadamard(state: State) -> State {
    let mut out = State::new();
    for (config, a) in state {
        for c in 0..2 {
            let mut next = config.clone();
            next[0] = c;
            *out.entry(next).or_default() += a * if c == 1 && config[0] == 1 { -HALF } else { HALF };
        }
    }
    out.retain(|_, a| a.norm() > EPS);
    out
}

/// Missing clean wires mean |0> ancillas, so pad them before comparing across amplitude supports.
fn padded(snapshot: &Snapshot, columns: usize) -> State {
    assert!(snapshot.worlds.is_empty(), "the un-run must join its worlds");
    snapshot
        .quantum
        .amplitudes
        .iter()
        .map(|(c, a)| {
            assert!(c.len() <= columns, "unexpected extra wire: {snapshot:?}");
            let mut c = c.clone();
            c.resize(columns, 0);
            (c, *a)
        })
        .collect()
}

fn state_eq(actual: &State, expected: &State) {
    for c in actual.keys().chain(expected.keys()) {
        let difference = actual.get(c).copied().unwrap_or_default() - expected.get(c).copied().unwrap_or_default();
        assert!(difference.norm() < EPS, "{c:?}: actual {actual:?}, expected {expected:?}");
    }
}

/// rho_live[x,y] = sum_wire psi[x,wire] conj(psi[y,wire]). Keep all off-diagonal entries.
fn trace_wires(state: &State, live: usize) -> BTreeMap<(Vec<u64>, Vec<u64>), C> {
    let mut density = BTreeMap::new();
    for (x, a) in state {
        for (y, b) in state {
            if x[live..] == y[live..] {
                *density.entry((x[..live].to_vec(), y[..live].to_vec())).or_default() += a * b.conj();
            }
        }
    }
    density
}

fn assert_dense(e: &Executor<'_>, expected: State, live: usize) {
    let columns = expected.keys().next().expect("nonempty expected state").len();
    let actual = padded(&e.snapshot(), columns);
    // Check the complete wire values, then separately the reduced density operator requested by the
    // machine meaning. The latter catches accidental coherence between distinct leftover values.
    state_eq(&actual, &expected);
    let actual = trace_wires(&actual, live);
    let expected = trace_wires(&expected, live);
    for key in actual.keys().chain(expected.keys()) {
        assert!(
            (actual.get(key).copied().unwrap_or_default() - expected.get(key).copied().unwrap_or_default()).norm()
                < EPS
        );
    }
}

fn whole_eq(actual: &Snapshot, expected: &Snapshot) {
    common::assert_same_state(actual, expected, "complete inverse snapshot");
    assert_eq!(actual.worlds.len(), expected.worlds.len());
    for (a, b) in actual.worlds.iter().zip(&expected.worlds) {
        whole_eq(a, b);
    }
}

/// Capture every step externally; inverse snapshots and a second forward run must agree exactly.
fn reverse_and_repeat(e: &mut Executor<'_>, text: &str, expected: State, live: usize) {
    let start = e.snapshot();
    let mut before = Vec::new();
    while e.outcome().is_none() {
        before.push(e.snapshot());
        e.step();
        assert!(before.len() < 200, "bounded straight-line probe: {text}");
    }
    assert_eq!(e.outcome(), Some(Outcome::Returned(Some(7))), "{text}");
    assert_eq!(&e.snapshot().classical.memory[16..24], &[0; 8], "ordinary stores un-run");
    assert!(e.snapshot().classical.tape.is_empty(), "effects un-run");
    assert_dense(e, expected.clone(), live);
    let end = e.snapshot();
    for previous in before.iter().rev() {
        assert_eq!(e.step_back(), Ok(true));
        whole_eq(&e.snapshot(), previous);
    }
    whole_eq(&e.snapshot(), &start);
    assert_eq!(e.stats().leftover_qubits, 0);
    assert_eq!(e.run(), Outcome::Returned(Some(7)));
    whole_eq(&e.snapshot(), &end);
    assert_dense(e, expected, live);
    e.reverse().expect("second run reverses");
    whole_eq(&e.snapshot(), &start);
}

fn machine() -> Options {
    Options { machine_meaning: true, reversible: true, ..bounded() }
}

fn run_dense(text: &str, expected: State, live: usize) {
    let module = load(text);
    let mut e = executor(&module, "main", &[], machine());
    e.set_apply_mode(ApplyMode::Compare);
    reverse_and_repeat(&mut e, text, expected, live);
}

fn forward_dense(text: &str, expected: State, live: usize) {
    let module = load(text);
    let mut e = executor(&module, "main", &[], machine());
    assert_eq!(e.run(), Outcome::Returned(Some(7)), "{text}");
    assert_eq!(&e.snapshot().classical.memory[16..24], &[0; 8], "ordinary stores un-run");
    assert!(e.snapshot().classical.tape.is_empty(), "effects un-run");
    assert_dense(&e, expected, live);
}

fn read_program(prepare: &str, reads: &str, outer: bool) -> String {
    format!(
        "memory 1024
func @pair(i3 %a, i2 %b) -> i8 {{
entry:
    %aa = zext i3 %a to i8
    %bb = zext i2 %b to i8
    %v = add i8 %aa, %bb
    ret %v
}}
func @twice(i3 %a, i3 %b) -> i8 {{
entry:
    %aa = zext i3 %a to i8
    %bb = zext i3 %b to i8
    %v = add i8 %aa, %bb
    ret %v
}}
func @main() -> i8 {{
entry:
    %c = qalloc 1
    {prepare}
    %q = qalloc 3
    x %q[0]
    x %q[2]
    %r = qalloc 2
    x %r[1]
    %k = qalloc 1
    {outer_mark}
    %m = mark budget 60
    {reads}
    store i8 93, 16
    effect 81
    x %q[1] ctrl %c, %k
    x %r[0] ctrl %c, %k
    rewind %m keep (i1 1 -> %k)
    {outer_rewind}
    h %c
    ret 7
}}",
        outer_mark = if outer { "%outer = mark budget 200" } else { "" },
        outer_rewind = if outer { "rewind %outer" } else { "" }
    )
}

#[test]
fn nonzero_multi_register_reads_leave_exact_xor_wires_and_trace_correctly_forward() {
    for (prepare, input) in preparations() {
        let text = read_program(prepare, "%v = call @pair(%q, %r)", false);
        // Values read: 5, 2. Current values: 5 XOR 2c, 2 XOR c. Wires: 2c, c.
        forward_dense(&text, hadamard(image(input, |c| vec![c, 5 ^ (2 * c), 2 ^ c, 1, 2 * c, c])), 4);
        let module = load(&text);
        let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
        let start = e.snapshot();
        assert_eq!(
            e.run(),
            if input[1].norm() > EPS { Outcome::Faulted(Fault::BadRewind) } else { Outcome::Returned(Some(7)) }
        );
        e.reverse().expect("strict diagnostic reverses");
        whole_eq(&e.snapshot(), &start);
    }
}

#[test]
fn two_arguments_reading_the_same_register_keep_two_independent_wires_forward() {
    for (prepare, input) in preparations() {
        let text = read_program(prepare, "%v = call @twice(%q, %q)", false);
        forward_dense(&text, hadamard(image(input, |c| vec![c, 5 ^ (2 * c), 2 ^ c, 1, 2 * c, 2 * c])), 4);
    }
}

fn memory_read_program(prepare: &str) -> String {
    format!(
        "memory 512
func @main() -> i8 {{
entry:
    %c = qalloc 1
    {prepare}
    %k = qalloc 1
    store i8 5, 24
    z [24:1]
    %m = mark budget 40
    %v = load i8 24
    x [24:1] ctrl %c, %k
    rewind %m keep (i1 1 -> %k)
    h %c
    ret 7
}}"
    )
}

#[test]
fn changed_nonzero_memory_read_has_the_independent_dense_state() {
    let text = memory_read_program("h %c\ns %c");
    // Columns c, k, cell, wire. Read value 5, current cell 5 XOR 2c: wire = 2c.
    let expected = image([C::new(HALF, 0.0), C::new(0.0, HALF)], |c| vec![c, 1, 5 ^ (2 * c), 2 * c]);
    run_dense(&text, hadamard(expected), 3);
}

#[test]
fn a_faithful_memory_read_runs_and_changed_reads_are_strict_diagnostics() {
    run_dense(&memory_read_program(""), hadamard(image([C::new(1.0, 0.0), C::new(0.0, 0.0)], |_| vec![0, 1, 5, 0])), 3);
    for (prepare, input) in preparations().into_iter().skip(1) {
        let text = memory_read_program(prepare);
        let module = load(&text);
        let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
        let start = e.snapshot();
        assert!(input[1].norm() > EPS);
        assert_eq!(e.run(), Outcome::Faulted(Fault::BadRewind));
        e.reverse().expect("diagnostic reverses");
        whole_eq(&e.snapshot(), &start);
    }
}

#[test]
#[ignore = "defect: deferred un-run checks fault every world instead of retaining the read XOR wire"]
fn a_deferred_nonzero_read_keeps_the_dense_phase_and_reduced_state() {
    let text = "memory 512
func @id(i1 %b) -> i1 {
entry:
    ret %b
}
func @main() -> i8 {
entry:
    %c = qalloc 1
    h %c
    s %c
    %q = qalloc 1
    x %q
    %k = qalloc 1
    %m = mark budget 40
    %a = call @id(%q)
    x %q ctrl %c, %k
    %b = call @id(%c)
    rewind %m keep (i1 1 -> %k)
    h %c
    ret 7
}";
    // The first call read 1. At its un-run q = 1 XOR c, and its wire = c.
    run_dense(text, hadamard(image([C::new(HALF, 0.0), C::new(0.0, HALF)], |c| vec![c, 1 ^ c, 1, c])), 3);
}

#[test]
fn a_single_changed_nonzero_read_reverses_each_step_and_an_outer_rewind_takes_it_back() {
    for (prepare, input) in preparations() {
        for outer in [false, true] {
            let text = read_program(prepare, "%v = call @pair(%q, i2 2)", outer);
            let expected = if outer {
                image(input, |c| vec![c, 5, 2, 0])
            } else {
                image(input, |c| vec![c, 5 ^ (2 * c), 2 ^ c, 1, 2 * c])
            };
            run_dense(&text, hadamard(expected), 4);
        }
    }
}

/// Minimal deterministic witness. Forward: two read values 0, current registers 1, so both
/// leftover wires must hold 1. Inverse replay must restore the initial empty snapshot. Today it
/// reclaims the older wire first and panics. No amplitudes, deadlines or world scheduling are involved.
#[test]
fn two_leftover_read_wires_reverse_to_the_empty_entry() {
    for alias in [false, true] {
        let args = if alias { "%a, %a" } else { "%a, %b" };
        let text = format!(
            "memory 512
func @read(i1 %a, i1 %b) {{
entry:
    ret
}}
func @main() -> i8 {{
entry:
    %a = qalloc 1
    %b = qalloc 1
    %k = qalloc 1
    %m = mark budget 20
    call @read({args})
    x %a ctrl %k
    x %b ctrl %k
    rewind %m keep (i1 1 -> %k)
    ret 7
}}"
        );
        run_dense(&text, BTreeMap::from([(vec![1, 1, 1, 1, 1], C::new(1.0, 0.0))]), 3);
    }
}

/// Nonzero old bytes must be XORed out at un-promotion, rather than copied into the leftover.
/// A fresh allocation after the rewind must not alias any wire left over by it.
#[test]
fn promotion_of_a_nonzero_byte_and_a_dirty_allocation_keep_their_circuit_values() {
    for promotion in [false, true] {
        for outer in [false, true] {
            for (prepare, input) in preparations() {
                let (setup, operation, old) = if promotion {
                    ("store i8 165, 24", "x [24:1] ctrl %c, %k", 165)
                } else {
                    ("", "%a = qalloc 2\nx %a[1] ctrl %c, %k", 0)
                };
                let text = format!(
                    "memory 1024
func @main() -> i8 {{
entry:
    %c = qalloc 1
    {prepare}
    %k = qalloc 1
    {setup}
    {outer_mark}
    %m = mark budget 60
    {operation}
    rewind %m keep (i1 1 -> %k)
    {outer_rewind}
    %fresh = qalloc 1
    x %fresh
    h %c
    ret 7
}}",
                    outer_mark = if outer { "%outer = mark budget 200" } else { "" },
                    outer_rewind = if outer { "rewind %outer" } else { "" }
                );
                let module = load(&text);
                let mut e = executor(&module, "main", &[], machine());
                let start = e.snapshot();
                assert_eq!(e.run(), Outcome::Returned(Some(7)));
                assert!(e.snapshot().quantum.cells.is_empty());
                assert_eq!(e.snapshot().classical.memory[24], old);
                // When c has no |1> support, the temporary is clean and its column is reused by
                // fresh. Otherwise it stays as the leftover and fresh gets the following column.
                let dirty = !outer && input[1].norm() > EPS;
                let expected =
                    hadamard(image(
                        input,
                        |c| {
                            if dirty { vec![c, 1, 2 * c, 1] } else { vec![c, u64::from(!outer), 1] }
                        },
                    ));
                // Compare the complete state (columns here include the leftover between live ones).
                assert_dense(&e, expected, if dirty { 4 } else { 3 });
                e.reverse().expect("allocation or promotion reverses");
                whole_eq(&e.snapshot(), &start);
                assert_eq!(e.stats().leftover_qubits, 0);
            }
        }
    }
}

/// A replay of an earlier rewind can itself leave a temporary over while a later read's wire is
/// waiting to be taken back. These are distinct logical wires; their lifetime intervals overlap.
fn nested_program(prepare: &str) -> String {
    format!(
        "memory 1024
func @id(i1 %n) -> i1 {{
entry:
    ret %n
}}
func @main() -> i8 {{
entry:
    %c = qalloc 1
    {prepare}
    %q = qalloc 1
    %k = qalloc 1
    %r = qalloc 1
    %outer = mark budget 200
    %inner = mark
    %a = qalloc 1
    x %a ctrl %c, %k
    rewind %inner keep (i1 1 -> %k)
    %v = call @id(%q)
    x %q ctrl %c, %r
    rewind %outer keep (i1 1 -> %r)
    ret 7
}}"
    )
}

#[test]
fn nested_clean_lifetimes_run_and_strict_mode_diagnoses_changed_ones() {
    run_dense(&nested_program(""), image([C::new(1.0, 0.0), C::new(0.0, 0.0)], |_| vec![0, 0, 0, 1]), 4);
    for (prepare, _) in preparations().into_iter().skip(1) {
        let text = nested_program(prepare);
        let module = load(&text);
        let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
        let start = e.snapshot();
        assert_eq!(e.run(), Outcome::Faulted(Fault::BadRewind));
        e.reverse().expect("strict nested diagnostic reverses");
        whole_eq(&e.snapshot(), &start);
    }
}

#[test]
fn nested_leftover_lifetimes_complete_and_reverse_on_a_basis_state() {
    // Outer inverse takes back and then clears a (column 4), clears k, and leaves the read
    // wire (column 5) with q = 1. It must return, then reverse to the empty entry.
    run_dense(&nested_program("x %c"), image([C::new(0.0, 0.0), C::new(1.0, 0.0)], |c| vec![c, c, 0, 1, 0, c]), 4);
}
