//! All permutations of three register reads, followed by a second read with repeated aliases.
//! Hand circuit: q[j] ^= c << j during the inverse; every read leaves a separate wire c << j.
//! Replaying takes wires back in argument order, with other calls' wires still retained.
mod common;
use bennett_exec::{Options, Outcome};
use num_complex::Complex64 as C;
use std::collections::BTreeMap;
const HALF: f64 = std::f64::consts::FRAC_1_SQRT_2;

const PREP: [(&str, [C; 2]); 5] = [
    ("", [C::new(1.0, 0.0), C::new(0.0, 0.0)]),
    ("x %c", [C::new(0.0, 0.0), C::new(1.0, 0.0)]),
    ("h %c", [C::new(HALF, 0.0), C::new(HALF, 0.0)]),
    ("h %c\ns %c", [C::new(HALF, 0.0), C::new(0.0, HALF)]),
    ("ry(0.74) %c\nsdg %c", [C::new(0.9323273456060345, 0.0), C::new(0.0, -0.361615431964962)]),
];

fn program(order: [usize; 3], prepare: &str, outer: bool, worlds: bool) -> String {
    let args = |indices: &[usize]| indices.iter().map(|j| format!("%q{j}")).collect::<Vec<_>>().join(", ");
    let alias = [order[0], order[1], order[0], order[2], order[1]];
    let function = |n: usize| {
        let params = (0..n).map(|j| format!("i3 %a{j}")).collect::<Vec<_>>().join(", ");
        format!(
            "func @read{n}({params}) {{\nentry:\n{}ret\n}}\n",
            (0..n).map(|j| format!("store i3 %a{j}, {}\n", 64 + j)).collect::<String>()
        )
    };
    let allocated = (0..3)
        .map(|j| {
            format!(
                "%q{j} = qalloc 3\n{}",
                (0..3).filter(|b| [1, 3, 5][j] & (1 << b) != 0).map(|b| format!("x %q{j}[{b}]\n")).collect::<String>()
            )
        })
        .collect::<String>();
    format!(
        "memory 8192\n{}{}func @id(i1 %x) -> i1 {{\nentry:\nret %x\n}}\nfunc @main() {{\nentry:\n%c = qalloc 1\n{prepare}\n%k = qalloc 1\n{allocated}{}%outer = mark budget 2000\n{}%m = mark budget 200\ncall @read3({})\ncall @read5({})\nx %q0[0] ctrl %c, %k\nx %q1[1] ctrl %c, %k\nx %q2[2] ctrl %c, %k\nrewind %m keep (i1 1 -> %k)\n{}h %c\nret\n}}",
        function(3),
        function(5),
        if worlds { "%w = qalloc 1\nh %w\n" } else { "" },
        if worlds { "%branch = call @id(%w)\nstore i1 %branch, 80\n" } else { "" },
        args(&order),
        args(&alias),
        if outer { "rewind %outer\n" } else { "" }
    )
}

fn check(text: &str, order: [usize; 3], input: [C; 2], outer: bool, worlds: bool) {
    let alias = [order[0], order[1], order[0], order[2], order[1]];
    let mut expected = BTreeMap::<Vec<u64>, C>::new();
    for (c, a) in input.into_iter().enumerate() {
        for w in 0..if worlds { 2 } else { 1 } {
            let mut config = vec![c as u64, u64::from(!outer)];
            config.extend((0..3).map(|j| [1, 3, 5][j] ^ if outer { 0 } else { (c as u64) << j }));
            if worlds {
                config.push(w);
            }
            if !outer {
                config.extend(alias.iter().chain(order.iter()).map(|j| (c as u64) << j));
            }
            for bit in 0..2 {
                let mut out = config.clone();
                out[0] = bit;
                *expected.entry(out).or_default() +=
                    a * if bit & c as u64 == 1 { -HALF } else { HALF } * if worlds { HALF } else { 1.0 };
            }
        }
    }
    let p = common::load(text);
    let mut e =
        common::executor(&p, "main", &[], Options { reversible: true, machine_meaning: true, ..common::bounded() });
    let initial = e.snapshot();
    let mut before = vec![];
    while e.outcome().is_none() {
        before.push(e.snapshot());
        e.step();
        assert!(before.len() < 300);
    }
    assert_eq!(e.outcome(), Some(Outcome::Returned(None)), "{text}");
    let snapshot = e.snapshot();
    assert!(snapshot.worlds.is_empty(), "{snapshot:?}");
    let columns = 5 + usize::from(worlds) + if outer { 0 } else { 8 };
    let actual: BTreeMap<_, _> = snapshot
        .quantum
        .amplitudes
        .iter()
        .map(|(c, a)| {
            assert!(c.len() <= columns, "{snapshot:?}");
            let mut c = c.clone();
            c.resize(columns, 0);
            (c, *a)
        })
        .collect();
    for key in actual.keys().chain(expected.keys()) {
        assert!(
            (actual.get(key).copied().unwrap_or_default() - expected.get(key).copied().unwrap_or_default()).norm()
                < 1e-11,
            "{text}\nactual {actual:?}\nexpected {expected:?}"
        );
    }
    for snapshot in before.into_iter().rev() {
        e.step_back().expect("inverse exists");
        whole_state(&e.snapshot(), &snapshot, "wire-order inverse snapshot");
    }
    whole_state(&e.snapshot(), &initial, "all wires taken back");
}

#[test]
fn every_read_order_and_repeated_alias_preserves_wires_phases_and_inverse_snapshots() {
    for order in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        for (prepare, input) in PREP {
            for outer in [false, true] {
                check(&program(order, prepare, outer, false), order, input, outer, false);
            }
        }
    }
}

#[test]
fn read_wires_in_interleaved_worlds_are_taken_back_before_the_worlds_join() {
    let order = [2, 0, 1];
    for (prepare, input) in PREP {
        check(&program(order, prepare, true, true), order, input, true, true);
    }
}

#[test]
fn whole_world_reference_takes_back_reordered_and_aliased_read_wires() {
    use common::oracle_support::worlds::{Model, basis};
    use common::oracle_support::worlds_executor::assert_snapshot;
    for order in [[0, 1, 2], [1, 2, 0], [2, 0, 1]] {
        for (prepare, _) in PREP {
            for outer in [false, true] {
                for worlds in [false, true] {
                    if worlds && !outer {
                        continue;
                    }
                    let text = program(order, prepare, outer, worlds);
                    let p = common::load(&text);
                    let mut m = Model::new(&p, "main");
                    m.machine_meaning = true;
                    let initial = basis(m.initial("main", &[]));
                    let expected = m.run_joined(initial.clone(), 300);
                    let mut e = common::executor(
                        &p,
                        "main",
                        &[],
                        Options { reversible: true, machine_meaning: true, ..common::bounded() },
                    );
                    assert_eq!(e.run(), Outcome::Returned(None));
                    assert_snapshot(&m, &expected, &e);
                    let reversed = m.reverse_joined(expected).expect("reference read wires reverse");
                    assert_eq!(reversed.keys().collect::<Vec<_>>(), initial.keys().collect::<Vec<_>>());
                    assert!((reversed.values().next().expect("entry amplitude") - C::new(1.0, 0.0)).norm() < 1e-11);
                    e.reverse().expect("production read wires reverse");
                    assert_snapshot(&m, &initial, &e);
                }
            }
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

#[test]
fn nested_allocation_and_read_lifetimes_preserve_complex_phases() {
    for (prepare, input) in PREP {
        let text = format!(
            "memory 2048\nfunc @id(i1 %n) -> i1 {{\nentry:\nret %n\n}}\nfunc @main() {{\nentry:\n%c = qalloc 1\n{prepare}\n%q = qalloc 1\n%k = qalloc 1\n%r = qalloc 1\n%outer = mark budget 200\n%inner = mark\n%a = qalloc 1\nx %a ctrl %c, %k\nrewind %inner keep (i1 1 -> %k)\n%v = call @id(%q)\nx %q ctrl %c, %r\nrewind %outer keep (i1 1 -> %r)\nh %c\nret\n}}"
        );
        let mut expected = BTreeMap::<Vec<u64>, C>::new();
        for (c, a) in input.into_iter().enumerate() {
            for bit in 0..2 {
                *expected.entry(vec![bit, c as u64, 0, 1, 0, c as u64]).or_default() +=
                    a * if bit & c as u64 == 1 { -HALF } else { HALF };
            }
        }
        let p = common::load(&text);
        let mut e =
            common::executor(&p, "main", &[], Options { reversible: true, machine_meaning: true, ..common::bounded() });
        let entry = e.snapshot();
        let mut before = vec![];
        while e.outcome().is_none() {
            before.push(e.snapshot());
            e.step();
            assert!(before.len() < 100);
        }
        assert_eq!(e.outcome(), Some(Outcome::Returned(None)));
        let actual: BTreeMap<_, _> = e
            .snapshot()
            .quantum
            .amplitudes
            .iter()
            .map(|(c, a)| {
                let mut c = c.clone();
                assert!(c.len() <= 6);
                c.resize(6, 0);
                (c, *a)
            })
            .collect();
        for c in actual.keys().chain(expected.keys()) {
            assert!(
                (actual.get(c).copied().unwrap_or_default() - expected.get(c).copied().unwrap_or_default()).norm()
                    < 1e-11
            );
        }
        for snapshot in before.into_iter().rev() {
            e.step_back().expect("nested inverse exists");
            whole_state(&e.snapshot(), &snapshot, "nested lifetime inverse");
        }
        whole_state(&e.snapshot(), &entry, "nested lifetime entry");
    }
}
