//! Small progress and clock witnesses: bounded calls to the public stepping API, never polling loops.

use bennett_exec::{Executor, Options, Outcome};

#[test]
fn one_step_executes_and_one_step_back_restores_the_captured_state() {
    let module = bennett_ir::load(
        "memory 64
func @main() -> i8 {
entry:
    %x = add i8 17, 25
    ret %x
}",
    )
    .expect("valid program");
    let mut e = Executor::new(
        &module,
        module.function_named("main").expect("entry"),
        &[],
        Options { reversible: true, budget: Some(4), ..Options::default() },
    )
    .expect("starts");
    let entry = e.snapshot();
    assert_eq!(e.step(), None);
    let after_add = e.snapshot();
    assert_ne!(after_add.classical.frames, entry.classical.frames);
    assert_eq!(after_add.classical.slots, [42]);
    assert_eq!((e.stats().steps, e.stats().world_steps), (1, 1));
    assert_eq!(e.step(), Some(Outcome::Returned(Some(42))));
    assert_eq!(e.step_back(), Ok(true));
    assert_eq!(e.snapshot(), after_add);
    assert_eq!(e.step_back(), Ok(true));
    assert_eq!(e.snapshot(), entry);
    assert_eq!(e.step_back(), Ok(false));
}

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
