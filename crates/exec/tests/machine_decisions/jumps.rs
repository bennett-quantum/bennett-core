use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;

use crate::common::{assert_same_state, bounded, executor, load};

fn reverse_each_step(text: &str, expected: Outcome) {
    let module = load(text);
    let mut e = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let mut before = Vec::new();
    loop {
        let state = e.snapshot();
        let outcome = e.step();
        if matches!(outcome, Some(Outcome::Faulted(_) | Outcome::Stopped(_))) {
            let mut after = e.snapshot();
            after.classical.outcome = state.classical.outcome;
            assert_same_state(&after, &state, "faulting step is atomic");
        } else {
            before.push(state);
        }
        if outcome.is_some() {
            break;
        }
    }
    assert_eq!(e.outcome(), Some(expected));
    for state in before.into_iter().rev() {
        assert!(e.step_back().expect("reversible"));
        assert_same_state(&e.snapshot(), &state, "longjmp restores every step");
    }
    assert!(!e.step_back().expect("at start"));
}

/// SYNTAX, "Non-local jumps": all four record words have to match a live frame/position and
/// valid stack pointer. Here there are no dynamic allocations, so both valid stack bounds coincide.
#[test]
fn every_single_bit_corruption_of_a_jump_record_has_code_sixteen() {
    for word in 0..4 {
        for bit in 0..64 {
            let address = 16 + word * 8;
            let mask = 1u64 << bit;
            let text = format!(
                "memory 512\nfunc @main() -> i32 {{\nentry:\n %s = setjmp 16\n\
                 %first = icmp eq i32 %s, 0\n br %first, jump, done\njump:\n\
                 %old = load i64 {address}\n %bad = xor i64 %old, {mask}\n\
                 store i64 %bad, {address}\n longjmp 16, 7\ndone:\n ret %s\n}}"
            );
            reverse_each_step(&text, Outcome::Faulted(Fault::BadLongjmp));
        }
    }
}
