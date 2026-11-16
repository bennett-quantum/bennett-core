//! Independent snapshot-reference checks where that reference has a determined
//! meaning, plus text round trips for the e4/e6 forms. The snapshot model does not
//! yet model finite mark budgets or call-with-budget; these probes do not pretend it does.

mod common;

use bennett_exec::{Options, Outcome};
use common::{
    bounded, executor, load,
    oracle_support::{
        worlds::{Model, basis},
        worlds_executor,
    },
};

/// The reference has snapshot history, not the production undo journal, and
/// independently counts the rounds run again. Recursive inner rewinds plus an
/// outer coherent rewind exercise e1 under run budgets below and above completion.
#[test]
fn recursive_replay_agrees_with_the_whole_world_reference() {
    let module = load(
        "memory 512
func @layer(i2 %n) -> i2 {
entry:
    %done = icmp eq i2 %n, 0
    br %done, base, work
base:
    ret 3
work:
    %next = sub i2 %n, 1
    %inner = mark budget -1
    %v = call @layer(%next)
    rewind %inner
    ret %n
}
func @classical(i2 %input) -> i2 {
entry:
    %m = mark
    %v = call @layer(%input)
    rewind %m
    ret %input
}
func @main() {
entry:
    %q = qalloc 2
    h %q[0]
    s %q[0]
    h %q[1]
    %out = qalloc 2
    %m = mark budget -1
    %v = call @layer(%q)
    rewind %m keep (%v -> %out)
    ret
}",
    );
    for input in 0..4 {
        for limit in [16, 24, 60, 128, 256] {
            let steps = worlds_executor::compare(&module, "classical", &[input], limit, false);
            assert!(steps >= 6, "reference must reach the calls, input={input}, limit={limit}");
        }
    }
    // Unequal branch lengths: the reference's joined evaluation compares the
    // completed coherent region, without asserting identical simulator tick policy.
    let model = Model::new(&module, "main");
    let reference = model.run_joined(basis(model.initial("main", &[])), 256);
    let mut actual = executor(&module, "main", &[], Options { machine_meaning: true, reversible: true, ..bounded() });
    assert_eq!(actual.run(), Outcome::Returned(None));
    worlds_executor::assert_snapshot(&model, &reference, &actual);
}

/// SYNTAX requires read(write(m)) == m. Vary result widths and both constant and
/// dynamic budgets, and exercise value+code, code-only from value/void functions,
/// and nested calls. Types come from the public grammar, not executor internals.
#[test]
fn generated_mark_and_call_budget_text_round_trips() {
    for width in [1, 2, 5, 8, 32, 64, 80, 128] {
        for budget in ["0", "1", "23", "-1", "%budget"] {
            let text = format!(
                "memory 2048
func @id(i{width} %v) -> i{width} {{
entry:
    ret %v
}}
func @void() {{
entry:
    ret
}}
func @main(i64 %budget) -> i5 {{
entry:
    %h = mark budget {budget}
    %y, %c = call @id(i{width} 1) budget {budget}
    %only = call @id(%y) budget {budget}
    %void = call @void() budget {budget}
    rewind %h
    ret 0
}}"
            );
            let module = load(&text);
            let written = bennett_ir::write(&module);
            assert_eq!(bennett_ir::read(&written).expect("written text reads"), *module, "i{width}, B={budget}");
            assert!(bennett_ir::load(&written).is_ok());
        }
    }
}
