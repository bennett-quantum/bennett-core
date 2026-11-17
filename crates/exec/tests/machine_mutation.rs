//! Fast journal witnesses for machine mutation survivors. Import the production module so these
//! checks need no change to its visibility or to the executor's implementation files.

#[allow(dead_code, reason = "this test uses only the journal's state and deadline interface")]
#[path = "../src/journal.rs"]
mod journal;

use journal::{Journal, MarkEntry, Undo};

use bennett_exec::{Executor, Options, Outcome};

fn mark(position: usize, budget: u64, deadline: u64) -> MarkEntry {
    MarkEntry { position, allocations: 3, budget: Some(budget), deadline: Some(deadline) }
}

#[test]
fn different_undo_operations_are_different_histories_even_without_words() {
    // Both gate forms have no words; the operation to invert must still be distinguished.
    let mut register_gate = Journal::default();
    register_gate.push(Undo::Gate);
    let mut memory_gate = Journal::default();
    memory_gate.push(Undo::GateAt);
    assert_ne!(register_gate, memory_gate);
    assert_eq!(register_gate, register_gate.clone());
}

#[test]
fn a_marks_identity_includes_its_position_allocations_and_budget_but_not_the_clock() {
    let original = mark(1, 7, 10);
    assert_eq!(original, mark(1, 7, 12));
    assert_ne!(original, mark(2, 7, 10));
    assert_ne!(original, mark(1, 8, 10));
    assert_ne!(original, MarkEntry { allocations: 4, ..original });
}

#[test]
fn undoing_an_open_mark_removes_its_deadline() {
    let mut history = Journal::default();
    history.push_mark(mark(1, 7, 10), 1);
    history.push(Undo::Mark);
    assert_eq!(history.mark_deadline(0), Some(10));
    assert_eq!(history.pop(), Some(Undo::Mark));
    history.pop_mark();
    assert_eq!(history.mark_deadline(0), None);
}

#[test]
fn leaving_a_frame_closes_only_its_own_and_deeper_marks() {
    let mut history = Journal::default();
    history.push_mark(mark(1, 20, 21), 1);
    history.push(Undo::Mark);
    history.push_mark(mark(2, 7, 10), 2);
    history.push(Undo::Mark);
    assert_eq!(history.mark_deadline(0), Some(10));
    history.close_left(1);
    assert_eq!(history.mark_deadline(0), Some(21), "the caller's mark stays open");
    history.close_left(0);
    assert_eq!(history.mark_deadline(0), None);
}

#[test]
fn deadline_queries_exclude_the_boundary_and_closing_one_mark_keeps_the_other() {
    let mut history = Journal::default();
    history.push_mark(mark(1, 7, 10), 1);
    history.push(Undo::Mark);
    history.push_mark(mark(2, 20, 21), 1);
    history.push(Undo::Mark);
    assert_eq!(history.mark_deadline(0), Some(10));
    assert_eq!(history.mark_deadline(1), Some(21));
    assert_eq!(history.mark_deadline(2), None);
    history.close_mark(1);
    assert_eq!(history.mark_deadline(0), Some(21));
    history.close_mark(2);
    assert_eq!(history.mark_deadline(0), None);
}

#[test]
fn a_rewinds_consumed_budget_is_restored_only_at_its_own_record() {
    let mut history = Journal::default();
    history.push(Undo::Slot);
    history.push(Undo::Rewind);
    history.push_rewind_cost(7);
    history.push(Undo::Slot);
    assert_eq!(history.pop(), Some(Undo::Slot));
    assert_eq!(history.take_rewind_cost(2), 0, "an ordinary step consumed no rewind budget");
    assert_eq!(history.pop(), Some(Undo::Rewind));
    assert_eq!(history.take_rewind_cost(1), 7);
    assert_eq!(history.take_rewind_cost(1), 0, "the saved count is consumed once");
    history.push(Undo::Rewind);
    history.rewind_costs_restore(1, 7);
    assert_eq!(history.pop(), Some(Undo::Rewind));
    assert_eq!(history.take_rewind_cost(1), 7);
}

fn run_leftover_and_reverse(text: &str, leftover_qubits: u32) {
    let module = bennett_ir::load(text).expect("valid leftover witness");
    let entry = module.function_named("main").expect("main");
    // Each straight-line witness runs fewer than 30 instructions; this only bounds a broken replay.
    let options = Options { budget: Some(100), machine_meaning: true, reversible: true, ..Options::default() };
    let mut executor = Executor::new(&module, entry, &[], options).expect("entry fits");
    let start = executor.snapshot();
    assert_eq!(executor.run(), Outcome::Returned(Some(7)));
    assert_eq!(executor.stats().leftover_qubits, leftover_qubits);
    executor.reverse().expect("take back the leftovers and reverse");
    let restored = executor.snapshot();
    assert_eq!(restored.classical, start.classical);
    assert!(restored.quantum.distance(&start.quantum).is_some_and(|distance| distance < 1e-12));
    assert_eq!(restored.worlds.len(), start.worlds.len());
    assert_eq!(executor.stats().leftover_qubits, 0);
}

#[test]
fn a_changed_three_bit_read_takes_back_its_nonzero_value_from_a_later_column() {
    for outer in ["", "rewind %outer"] {
        let text = format!(
            "memory 256
func @identity(i3 %value) -> i3 {{
entry:
    ret %value
}}
func @main() -> i8 {{
entry:
    %padding = qalloc 1
    %q = qalloc 3
    x %q[0]
    x %q[2]
    %k = qalloc 1
    %outer = mark budget 60
    %inner = mark budget 20
    %value = call @identity(%q)
    x %q[1] ctrl %k
    rewind %inner keep (i1 1 -> %k)
    {outer}
    ret 7
}}"
        );
        run_leftover_and_reverse(&text, if outer.is_empty() { 3 } else { 0 });
    }
}

#[test]
fn a_changed_promotion_restores_a_nonzero_old_byte_at_a_nonzero_address() {
    for outer in ["", "rewind %outer"] {
        let text = format!(
            "memory 256
data 73 = i8 166
func @main() -> i8 {{
entry:
    %k = qalloc 1
    %outer = mark budget 60
    %inner = mark budget 20
    x [73:1] ctrl %k
    rewind %inner keep (i1 1 -> %k)
    {outer}
    ret 7
}}"
        );
        run_leftover_and_reverse(&text, if outer.is_empty() { 8 } else { 0 });
    }
}

#[test]
fn undoing_one_inner_rewind_charges_its_one_replayed_instruction() {
    let module = bennett_ir::load(
        "memory 256
func @main() -> i8 {
entry:
    %outer = mark
    %inner = mark
    %temporary = add i8 3, 4
    rewind %inner
    rewind %outer
    ret 7
}",
    )
    .expect("valid nested rewind");
    let entry = module.function_named("main").expect("main");
    let options = Options { budget: Some(20), reversible: true, ..Options::default() };
    let mut executor = Executor::new(&module, entry, &[], options).expect("entry fits");
    let start = executor.snapshot();
    assert_eq!(executor.run(), Outcome::Returned(Some(7)));
    // The outer rewind undoes the inner one by replaying the addition exactly once.
    assert_eq!(executor.stats().replay_charged, 1);
    executor.reverse().expect("finite replay reverses");
    assert_eq!(executor.snapshot().classical, start.classical);
    assert!(executor.snapshot().quantum.distance(&start.quantum).is_some_and(|distance| distance < 1e-12));
    assert_eq!(executor.stats().replay_charged, 0);
}

#[test]
fn one_reverse_step_after_a_world_split_actually_undoes_the_call() {
    let module = bennett_ir::load(
        "memory 256
func @identity(i1 %value) -> i1 {
entry:
    ret %value
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %value = call @identity(%q)
    ret 7
}",
    )
    .expect("valid split witness");
    let entry = module.function_named("main").expect("main");
    let options = Options { budget: Some(20), reversible: true, ..Options::default() };
    let mut executor = Executor::new(&module, entry, &[], options).expect("entry fits");
    assert_eq!(executor.step(), None); // allocate
    assert_eq!(executor.step(), None); // prepare |+>
    let before_call = executor.snapshot();
    assert_eq!(executor.step(), None); // read and split
    assert_eq!(executor.stats().splits, 1);
    assert_eq!(executor.step_back(), Ok(true));
    let restored = executor.snapshot();
    assert_eq!(restored.classical, before_call.classical);
    assert_eq!(restored.worlds.len(), before_call.worlds.len());
    assert!(restored.quantum.distance(&before_call.quantum).is_some_and(|distance| distance < 1e-12));
}
