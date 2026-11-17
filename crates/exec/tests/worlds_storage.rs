//! Resource regression for the independent snapshot interpreter, with no executor adapter.
#[allow(dead_code)]
#[path = "oracle_support/dense.rs"]
mod dense;
#[allow(dead_code)]
#[path = "oracle_support/worlds.rs"]
mod reference;

use reference::{Model, Value, basis};

const WRITES: &str = "memory 4096
func @main(i64 %x) -> i64 {
entry:
store i64 %x, 0
%y = add i64 %x, 1
store i64 %y, 0
ret %y
}
";

#[test]
fn cloning_a_world_shares_history_and_unchanged_slots() {
    println!(
        "reference snapshot payload: Classical {} bytes, Record {} bytes (shared allocations excluded)",
        std::mem::size_of::<reference::Classical>(),
        std::mem::size_of::<reference::Record>()
    );
    let program = bennett_ir::load(WRITES).expect("fixture");
    let model = Model::new(&program, "main");
    let initial = basis(model.initial("main", &[41]));
    let state = model.step(&initial);
    let world = state.keys().next().expect("world");
    let copy = world.clone();
    // Structural resource checks, not a noisy wall-clock threshold. A world clone must not copy
    // its accumulated snapshots or the unchanged slots inside those snapshots.
    assert!(std::ptr::eq(&world.history[0], &copy.history[0]), "history copied on clone");
    assert!(std::ptr::eq(&world.classical.slots[0], &copy.classical.slots[0]), "slots copied on clone");
    let next = model.step(&state);
    assert_eq!(world.classical.slots[1], Value::Int(0), "retained snapshot mutated");
    assert_eq!(next.keys().next().expect("world").classical.slots[1], Value::Int(42));
    assert_eq!(model.undo_state(&next).expect("undo"), state);
    assert_eq!(model.undo_state(&state).expect("undo"), initial);
}

#[test]
fn retained_snapshots_survive_writes_and_independent_reversal() {
    let program = bennett_ir::load(WRITES).expect("fixture");
    let model = Model::new(&program, "main");
    for input in [0, 41, u64::MAX as u128] {
        let mut states = vec![basis(model.initial("main", &[input]))];
        for _ in 0..4 {
            states.push(model.step(states.last().expect("state")));
        }
        let final_world = states[4].keys().next().expect("world");
        let result = (input + 1) & u64::MAX as u128;
        assert_eq!(final_world.classical.finish, Some(reference::Finish::Returned(Some(result))));
        assert_eq!(&model.memory(final_world)[..8], &(result as u64).to_le_bytes());
        for step in (1..states.len()).rev() {
            let undone = model.undo_state(&states[step]).expect("undo");
            assert_eq!(undone, states[step - 1], "every retained machine state, input {input}, step {step}");
            assert_eq!(
                model.memory(undone.keys().next().expect("world")),
                model.memory(states[step - 1].keys().next().expect("world"))
            );
        }
        assert_eq!(model.memory(states[0].keys().next().expect("world")), vec![0; 4096]);
    }
}
