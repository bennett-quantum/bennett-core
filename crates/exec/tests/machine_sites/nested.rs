//! A zero-wire settlement can create equal parked worlds after the merge pass has finished.
use super::{C, Options, Outcome, common};

fn nested_identity(levels: usize, prepare: &str) -> String {
    let marks: String = (0..levels).map(|n| format!("%m{n} = mark budget {}\n", (levels - n) * 100)).collect();
    let rewinds: String = (0..levels).rev().map(|n| format!("rewind %m{n}\n")).collect();
    format!(
        "memory 1024
func @id(i1 %c) -> i1 {{
entry:
ret %c
}}
func @main() -> i8 {{
entry:
%c = qalloc 1
{prepare}{marks}%v = call @id(%c)
{rewinds}h %c
ret 7
}}"
    )
}

fn check_identity(levels: usize, prep: &str, machine_meaning: bool, expected: [C; 2]) {
    let module = common::load(&nested_identity(levels, prep));
    let mut e =
        common::executor(&module, "main", &[], Options { machine_meaning, reversible: true, ..common::bounded() });
    let start = e.snapshot();
    let outcome = e.run();
    // Even the stopped witness reverses correctly: keep this assertion ahead of its failure.
    let end = e.snapshot();
    e.reverse().expect("nested identity reverses");
    common::assert_same_state(&e.snapshot(), &start, "nested identity");
    assert!(e.snapshot().worlds.is_empty());
    assert_eq!(outcome, Outcome::Returned(Some(7)), "levels={levels}, machine={machine_meaning}, prep={prep:?}");
    assert!(end.worlds.is_empty());
    assert_eq!(end.quantum.registers.len(), 1, "every zero wire is discarded");
    for (value, amplitude) in expected.into_iter().enumerate() {
        assert!((end.quantum.amplitude(&[value as u64]) - amplitude).norm() < 1e-12);
    }
}

#[test]
fn nested_identity_controls_pass_in_strict_mode_on_basis_states_and_with_two_marks() {
    let h = std::f64::consts::FRAC_1_SQRT_2;
    for levels in 1..=4 {
        for machine in [false, true] {
            check_identity(levels, "", machine, [C::new(h, 0.0), C::new(h, 0.0)]);
            check_identity(levels, "x %c\n", machine, [C::new(h, 0.0), C::new(-h, 0.0)]);
        }
        check_identity(levels, "h %c\n", false, [C::new(1.0, 0.0), C::new(0.0, 0.0)]);
        if levels <= 2 {
            check_identity(levels, "h %c\n", true, [C::new(1.0, 0.0), C::new(0.0, 0.0)]);
        }
    }
}

#[test]
#[ignore = "defect: three distinct faithful nested rewinds falsely stop Unaligned after zero-wire discard (P1)"]
fn three_distinct_faithful_rewinds_merge_after_discarding_their_zero_read_wires() {
    // COPY(c,w), followed by its inverse, is I, including the hidden wire. All three
    // marks are distinct, have ample finite nested budgets, and are rewound once each.
    // The state must remain |+> tensor |0...0>; the final H gives |0>. Strict mode
    // does so. Machine mode on the same source stops Unaligned after the third rewind:
    // two publicly identical parts remain after their zero wires were settled; the
    // scheduler resumes one and then treats the other as a late arrival. This is not
    // the documented different-site re-rewind or the open repeated-mark timing case.
    check_identity(3, "h %c\n", true, [C::new(1.0, 0.0), C::new(0.0, 0.0)]);
}
