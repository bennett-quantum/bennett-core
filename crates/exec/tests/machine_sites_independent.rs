//! Independent circuit checks of machine-sites. Each ordinary read is a CNOT into a fresh wire;
//! un-reading applies that CNOT again. A rewind is U† K U on the data AND those wires, never a
//! projection. Matrices below act on a dense vector, independent of the executor/world scheduler.
mod common;

use bennett_exec::{Executor, Options, Outcome};
use num_complex::Complex64 as C;
use std::collections::BTreeMap;
use std::f64::consts::FRAC_1_SQRT_2;

#[path = "machine_sites/dense.rs"]
mod dense;
use dense::{Circuit, ZERO, adjoint, matrix, prepare};

#[derive(Clone, Copy, Debug)]
enum Site {
    Read,
    Cell,
    Allocation,
    Promotion,
}

// Compare all amplitudes, retaining an explicit |0> for each discarded wire in the dense answer.
// Dense columns: c[0], c[1], k, a, read-wire. Only bit 0 of a byte is ever used.
fn assert_dense(e: &Executor<'_>, dense: &Circuit, site: Site, context: &str) {
    let snap = e.snapshot();
    assert!(snap.worlds.is_empty(), "{context}: unjoined worlds");
    assert!(snap.quantum.registers.len() <= 4, "{context}: unexpected registers");
    let mut actual = vec![ZERO; dense.0.len()];
    for (config, amp) in &snap.quantum.amplitudes {
        assert!(config.len() <= 4, "{context}: unexpected wires");
        let a = config.get(2).copied().unwrap_or(0) as usize;
        let wire = config.get(3).copied().unwrap_or(0) as usize;
        assert!(a <= 1 && wire <= 1, "{context}: byte not binary");
        let basis = config[0] as usize | (config[1] as usize) << 2 | a << 3 | wire << 4;
        actual[basis] += amp;
    }
    for (b, (actual, expected)) in actual.iter().zip(&dense.0).enumerate() {
        assert!(
            (actual - expected).norm() < 1e-11,
            "{context} {site:?} basis {b:05b}: {actual} != {expected}; {:?}",
            snap.quantum
        );
    }
    assert_eq!(e.stats().observations, 0, "{context}: no host sampling");
}

fn exercise(site: Site, input: usize, gate: &str, read_order: bool, nested: bool) {
    let mut dense = Circuit::new(5);
    let prep = prepare(&mut dense, input);
    let (before, after, target) = match site {
        Site::Read => ("%a = qalloc 1\n", "%v = call @id1(%a)\n", "%a"),
        Site::Cell => ("z [24:0]\n", "%v = load i8 24\n", "[24:0]"),
        Site::Allocation => ("", "%a = qalloc 1\n", "%a"),
        Site::Promotion => ("", "z [24:0]\n", "[24:0]"),
    };
    let split = "%split = call @id2(%c)\n";
    let (first, last) = if read_order { (split, "") } else { ("", split) };
    let outer_mark = if nested { "%outer = mark budget 400\n" } else { "" };
    let outer_rewind = if nested { "rewind %outer\n" } else { "" };
    let text = format!(
        "memory 2048
func @id1(i1 %a) -> i1 {{
entry:
ret %a
}}
func @id2(i2 %a) -> i2 {{
entry:
ret %a
}}
func @main() -> i8 {{
entry:
%c = qalloc 2
{prep}%k = qalloc 1
{before}{outer_mark}%m = mark budget 100
{first}{after}{gate} {target} ctrl %k, %c[0]
h {target} ctrl %k, %c[1]
{last}rewind %m keep (i1 1 -> %k)
{outer_rewind}h %c[0]
h %c[1]
ret 7
}}"
    );
    // Forward k=0 makes both controlled gates identities. The keep flips k. The inverse
    // applies H controlled by c[1], then G† controlled by c[0]. Un-reading copies a into
    // its old (zero) read wire. An outer rewind is the inverse of this entire dilation.
    if !nested {
        dense.gate(matrix("x"), 2, 0);
        dense.gate(matrix("h"), 3, (1 << 2) | (1 << 1));
        dense.gate(adjoint(matrix(gate)), 3, (1 << 2) | 1);
        if matches!(site, Site::Read | Site::Cell) {
            dense.gate(matrix("x"), 4, 1 << 3);
        }
        if matches!(site, Site::Promotion) {
            dense.gate(matrix("z"), 3, 0);
        }
    }
    dense.gate(matrix("h"), 0, 0);
    dense.gate(matrix("h"), 1, 0);
    let module = common::load(&text);
    let mut e = common::executor(&module, "main", &[], common::leftover::machine());
    let start = e.snapshot();
    let context = format!("{site:?} input={input} gate={gate} order={read_order} nested={nested}");
    assert_eq!(e.run(), Outcome::Returned(Some(7)), "{context}\n{text}");
    assert_dense(&e, &dense, site, &context);
    e.reverse().unwrap_or_else(|err| panic!("{context}: {err:?}"));
    common::assert_same_state(&e.snapshot(), &start, &context);
    assert!(e.snapshot().worlds.is_empty());
}

#[test]
fn generated_join_sites_match_a_dense_unitary_on_basis_and_complex_inputs() {
    let mut count = 0;
    for site in [Site::Read, Site::Cell, Site::Allocation, Site::Promotion] {
        for input in 0..6 {
            for gate in ["x", "h", "s", "t", "rx(0.73)", "ry(0.73)", "rz(0.73)"] {
                for order in [false, true] {
                    for nested in [false, true] {
                        exercise(site, input, gate, order, nested);
                        count += 1;
                    }
                }
            }
        }
    }
    eprintln!("{count} independently dense-checked site/input/gate/order/outer-rewind cases");
}

#[test]
fn a_replayed_two_byte_access_takes_back_wires_after_splitting_other_cells() {
    // Byte 24 was read as 0; the keep and inverse X change it to 1 and leave a wire 1.
    // Byte 25 has a complex superposition. Its zero wire is discarded. The outer rewind
    // replays the i16 load: split byte 25 first, take byte 24's wire back in every world.
    for input in 0..6 {
        for store in [false, true] {
            let mut dense = Circuit::new(5);
            let prep = prepare(&mut dense, input);
            let access = if store { "store i16 %same, 24\n" } else { "" };
            let text = format!(
                "memory 2048
func @main() -> i8 {{
entry:
%c = qalloc 2
{prep}%k = qalloc 1
z [24:0]
h [25:0]
s [25:0]
%outer = mark budget 400
%inner = mark budget 100
%same = load i16 24
{access}x [24:0] ctrl %k
rewind %inner keep (i1 1 -> %k)
rewind %outer
h %c[0]
h %c[1]
ret 7
}}"
            );
            dense.gate(matrix("h"), 0, 0);
            dense.gate(matrix("h"), 1, 0);
            let module = common::load(&text);
            let mut e = common::executor(&module, "main", &[], common::leftover::machine());
            let start = e.snapshot();
            assert_eq!(e.run(), Outcome::Returned(Some(7)), "input={input}, store={store}");
            let snap = e.snapshot();
            let mut expected = BTreeMap::new();
            for (b, a) in dense.0.iter().enumerate().filter(|(_, a)| a.norm() > 1e-12) {
                for cell in 0..2 {
                    expected.insert(
                        vec![(b & 3) as u64, 0, 0, cell],
                        a * if cell == 0 { C::new(FRAC_1_SQRT_2, 0.0) } else { C::new(0.0, FRAC_1_SQRT_2) },
                    );
                }
            }
            assert!(snap.worlds.is_empty());
            for (config, amp) in &snap.quantum.amplitudes {
                assert!((amp - expected.remove(config).unwrap_or(ZERO)).norm() < 1e-11, "{snap:?}");
            }
            assert!(expected.values().all(|a| a.norm() < 1e-11), "missing {expected:?}");
            assert_eq!(e.stats().leftover_qubits, 0);
            assert_eq!(e.stats().observations, 0);
            e.reverse().expect("mixed access replay reverses");
            common::assert_same_state(&e.snapshot(), &start, "mixed access replay");
        }
    }
}

fn interference_program(input: usize, suffix: &str) -> (String, Circuit) {
    let mut dense = Circuit::new(5);
    let prep = prepare(&mut dense, input);
    dense.gate(matrix("x"), 2, 0);
    dense.gate(matrix("h"), 3, 1 | (1 << 2));
    dense.gate(matrix("x"), 4, 1 << 3);
    dense.gate(matrix("h"), 0, 0);
    dense.gate(matrix("h"), 1, 0);
    (
        format!(
            "memory 2048
func @id1(i1 %a) -> i1 {{
entry:
ret %a
}}
func @id2(i2 %c) -> i2 {{
entry:
ret %c
}}
func @f(i2 %c) -> i2 {{
entry:
%v = xor i2 %c, 1
ret %v
}}
func @main() -> i8 {{
entry:
%c = qalloc 2
{prep}%k = qalloc 1
%a = qalloc 1
%m = mark budget 100
%v = call @id1(%a)
%split = call @id2(%c)
h %a ctrl %c[0], %k
rewind %m keep (i1 1 -> %k)
h %c[0]
h %c[1]
{suffix}
}}"
        ),
        dense,
    )
}

#[test]
fn apply_compare_and_both_implementations_preserve_the_dense_interference_with_leftovers() {
    for input in 0..6 {
        let (text, dense) = interference_program(
            input,
            "%out = qalloc 2\n%fault = qalloc 5\napply @f(%c) -> %out, %fault budget 20\nret 7",
        );
        let module = common::load(&text);
        let mut e = common::executor(&module, "main", &[], common::leftover::machine());
        let start = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(Some(7)), "input={input}");
        let snap = e.snapshot();
        let out = if e.stats().leftover_qubits == 0 { 3 } else { 4 };
        let mut actual = vec![ZERO; 32];
        for (config, amp) in &snap.quantum.amplitudes {
            assert_eq!(config[out], config[0] ^ 1);
            assert_eq!(config[out + 1], 0);
            let wire = if out == 3 { 0 } else { config[3] as usize };
            actual[config[0] as usize | (config[1] as usize) << 2 | (config[2] as usize) << 3 | wire << 4] += amp;
        }
        for (a, b) in actual.iter().zip(&dense.0) {
            assert!((a - b).norm() < 1e-11, "input={input}");
        }
        assert_eq!(e.stats().observations, 0);
        e.reverse().expect("apply after leftover reverses");
        common::assert_same_state(&e.snapshot(), &start, "apply after leftover");
    }
}

#[test]
fn measurement_statistics_after_the_join_match_the_dense_born_probabilities() {
    const SHOTS: u64 = 512;
    for input in [0, 1, 2, 3, 4, 5] {
        let (text, dense) = interference_program(input, "%m0 = measure %c\n%r = zext i2 %m0 to i8\nret %r");
        let module = common::load(&text);
        let mut probabilities = [0.0; 4];
        for (basis, amp) in dense.0.iter().enumerate() {
            probabilities[basis & 3] += amp.norm_sqr();
        }
        let mut counts = [0; 4];
        for seed in 0..SHOTS {
            let mut e = common::executor(&module, "main", &[], Options { seed, ..common::leftover::machine() });
            let Outcome::Returned(Some(value)) = e.run() else {
                panic!("measurement run: {:?}", e.snapshot());
            };
            counts[value as usize] += 1;
        }
        for (count, p) in counts.into_iter().zip(probabilities) {
            let expected = SHOTS as f64 * p;
            // Fixed seeds, a six-standard-deviation binomial bound, plus rounding at p=0/1.
            let bound = 6.0 * (SHOTS as f64 * p * (1.0 - p)).max(0.0).sqrt() + 1.0;
            assert!(
                (f64::from(count) - expected).abs() <= bound,
                "input={input}: counts={counts:?}, p={probabilities:?}"
            );
        }
    }
}

#[test]
fn replay_that_newly_returns_from_the_frame_is_an_atomic_named_refusal_in_both_modes() {
    let module = common::load(
        "memory 1024
func @id(i1 %x) -> i1 {
entry:
ret %x
}
func @main() -> i8 {
entry:
%k = qalloc 1
%outer = mark budget 100
%inner = mark budget 30
%v = call @id(%k)
br %v, leaves, done
leaves:
ret 9
done:
rewind %inner
rewind %outer keep (i1 1 -> %k)
ret 7
}",
    );
    for machine_meaning in [false, true] {
        let mut e =
            common::executor(&module, "main", &[], Options { machine_meaning, reversible: true, ..common::bounded() });
        let start = e.snapshot();
        loop {
            let before = e.snapshot();
            if let Some(outcome) = e.step() {
                assert_eq!(outcome, Outcome::Faulted(bennett_ir::Fault::BadRewind));
                let mut after = e.snapshot();
                after.classical.outcome = None;
                common::assert_same_state(&after, &before, "returning replay is atomic");
                assert_eq!(e.stats().observations, 0);
                e.reverse().expect("refused replay reverses");
                common::assert_same_state(&e.snapshot(), &start, "refused replay");
                break;
            }
        }
    }
}

#[path = "machine_sites/cancellation.rs"]
mod cancellation;

#[test]
fn splits_during_unrun_copy_existing_leftovers_and_make_new_ones_in_every_part() {
    for input in 0..6 {
        for outer in [false, true] {
            // Exactly the known false-Unaligned cohort; the minimal witness is in nested.rs.
            // These exclusions are not counted as passing dense comparisons.
            if outer && input >= 2 {
                continue;
            }
            let mut dense = Circuit::new(7);
            let prep = prepare(&mut dense, input);
            if !outer {
                dense.gate(matrix("x"), 2, 0);
                dense.gate(adjoint(matrix("rx(0.73)")), 4, (1 << 1) | (1 << 2));
                dense.gate(matrix("h"), 3, 1 | (1 << 2));
                dense.gate(matrix("x"), 5, 1 << 4);
                dense.gate(matrix("x"), 6, 1 << 3);
            }
            dense.gate(matrix("h"), 0, 0);
            dense.gate(matrix("h"), 1, 0);
            let text = format!(
                "memory 2048
func @id(i1 %a) -> i1 {{
entry:
ret %a
}}
func @id2(i2 %c) -> i2 {{
entry:
ret %c
}}
func @main() -> i8 {{
entry:
%c = qalloc 2
{prep}%k = qalloc 1
%a = qalloc 1
%b = qalloc 1
{outer_mark}%m = mark budget 300
%ra = call @id(%a)
%inner = mark budget 30
%rc = call @id2(%c)
rewind %inner
%rb = call @id(%b)
h %a ctrl %k, %c[0]
rx(0.73) %b ctrl %k, %c[1]
rewind %m keep (i1 1 -> %k)
{outer_rewind}h %c[0]
h %c[1]
ret 7
}}",
                outer_mark = if outer { "%outer = mark budget 1000\n" } else { "" },
                outer_rewind = if outer { "rewind %outer\n" } else { "" }
            );
            // The original inner read has merged before m's rewind. In the un-run, b's wire
            // exists before undoing the inner rewind splits c again, while a's is made after.
            let module = common::load(&text);
            let mut e = common::executor(&module, "main", &[], common::leftover::machine());
            let start = e.snapshot();
            assert_eq!(e.run(), Outcome::Returned(Some(7)), "input={input}, outer={outer}");
            let wb = dense.0.iter().enumerate().any(|(b, a)| b & (1 << 5) != 0 && a.norm() > 1e-12);
            let wa = dense.0.iter().enumerate().any(|(b, a)| b & (1 << 6) != 0 && a.norm() > 1e-12);
            let mut actual = vec![ZERO; 128];
            for (config, amp) in &e.snapshot().quantum.amplitudes {
                let b = config[0] as usize
                    | (config[1] as usize) << 2
                    | (config[2] as usize) << 3
                    | (config[3] as usize) << 4
                    | (if wb { config[4] as usize } else { 0 }) << 5
                    | (if wa { config[4 + usize::from(wb)] as usize } else { 0 }) << 6;
                actual[b] += amp;
            }
            for (a, b) in actual.iter().zip(&dense.0) {
                assert!((a - b).norm() < 1e-11, "input={input}, outer={outer}: {a} != {b}");
            }
            assert_eq!(e.stats().observations, 0);
            assert!(e.snapshot().worlds.is_empty());
            e.reverse().expect("splitting un-run reverses");
            common::assert_same_state(&e.snapshot(), &start, "splitting un-run");
            assert!(e.snapshot().worlds.is_empty());
        }
    }
}

#[path = "machine_sites/nested.rs"]
mod nested;
