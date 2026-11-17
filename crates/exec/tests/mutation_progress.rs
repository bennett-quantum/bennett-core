//! Small progress and clock witnesses: bounded calls to the public stepping API, never polling loops.

use bennett_exec::{Executor, Options, Outcome};

#[test]
fn replaying_a_rewind_during_reversal_does_not_advance_the_machine_clock() {
    let module = bennett_ir::load(
        "memory 64
func @main() {
entry:
    %h = mark budget 2
    %x = add i8 17, 25
    rewind %h
    ret
}",
    )
    .expect("valid program");
    let mut e = Executor::new(
        &module,
        module.function_named("main").expect("entry"),
        &[],
        Options { reversible: true, budget: Some(8), ..Options::default() },
    )
    .expect("starts");
    for _ in 0..3 {
        assert_eq!(e.step(), None);
    }
    assert_eq!((e.stats().clock, e.stats().rewound_steps), (3, 1));
    assert_eq!(e.step_back(), Ok(true));
    assert_eq!(e.stats().rerun_steps, 1);
    assert_eq!(e.stats().clock, 3, "running history again is work, not a forward clock tick");
    assert_eq!(e.step_back(), Ok(true));
    assert_eq!(e.step_back(), Ok(true));
    assert_eq!(e.stats().clock, 0);
}
