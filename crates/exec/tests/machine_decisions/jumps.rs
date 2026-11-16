use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;

use crate::common::{assert_same_state, bounded, executor, load, oracle_support};
use oracle_support::worlds_executor::compare;

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

/// Positive execution control for the ignored validator witness: naming the same address literally
/// removes the phantom stale use while preserving the jump record, slot layout and every executed step.
#[test]
fn a_record_rebuilt_after_rewind_resumes_after_setjmp() {
    let text =
        include_str!("../../../ir/tests/fixtures/rewound_setjmp.bn").replace("%s = setjmp %buf", "%s = setjmp 16");
    reverse_each_step(&text, Outcome::Returned(Some(7)));
    compare(&load(&text), "main", &[], 32, false);
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
            compare(&load(&text), "main", &[], 32, false);
        }
    }
}

#[test]
fn a_jump_record_in_cells_must_agree_in_every_configuration() {
    for bit in [0, 7, 8, 63, 64, 127, 128, 191, 192, 255] {
        let address = 16 + bit / 8;
        let offset = bit % 8;
        for gate in ["z", "h"] {
            let text = format!(
                "memory 512\nfunc @main() -> i32 {{\nentry:\n %s = setjmp 16\n\
                 %first = icmp eq i32 %s, 0\n br %first, jump, done\njump:\n\
                 {gate} [{address}:{offset}]\n longjmp 16, 7\ndone:\n ret %s\n}}"
            );
            let expected = if gate == "z" { Outcome::Returned(Some(7)) } else { Outcome::Faulted(Fault::BadLongjmp) };
            reverse_each_step(&text, expected);
            compare(&load(&text), "main", &[], 32, false);
        }
    }
}

/// A jump discards two activations, each with a mark, and the caller's rewind crosses the jump.
/// DESIGN, "Non-local jumps" requires ordinary memory, tape, stack and discarded frames to un-run.
#[test]
fn longjmp_across_two_marked_activations_keeps_only_the_requested_value() {
    for depth in 1..=4 {
        for value in [0, 1, 7, u32::MAX] {
            let text = format!(
                "memory 8192\nfunc @throw(i64 %n) {{\nentry:\n %local = mark\n\
                 %p = alloca 16\n store i64 %n, %p\n effect 88\n\
                 %more = icmp ne i64 %n, 0\n br %more, recurse, jump\nrecurse:\n\
                 %next = sub i64 %n, 1\n call @throw(%next)\n ret\njump:\n longjmp 16, {value}\n}}\n\
                 func @main() -> i32 {{\nentry:\n %out = qalloc 32\n %outer = mark\n\
                 %s = setjmp 16\n %first = load i8 64\n %go = icmp eq i8 %first, 0\n\
                 br %go, go, done\ngo:\n store i8 1, 64\n call @throw(i64 {depth})\n ret 99\ndone:\n\
                 rewind %outer keep (%s -> %out)\n %zero = load i128 16\n %clean = icmp eq i128 %zero, 0\n\
                 br %clean, ok, bad\nok:\n %m = measure %out\n ret %m\nbad:\n fault unreachable\n}}"
            );
            let module = load(&text);
            let mut e = executor(&module, "main", &[], bounded());
            assert_eq!(e.run(), Outcome::Returned(Some(u128::from(value))), "depth {depth} value {value}");
            assert_eq!(&e.snapshot().classical.memory[16..65], &[0; 49]);
            assert!(e.snapshot().classical.tape.is_empty());
            compare(&module, "main", &[], 128, true);
            let unmeasured = text.replace("%m = measure %out\n ret %m", "ret 0");
            reverse_each_step(&unmeasured, Outcome::Returned(Some(0)));
        }
    }
}
