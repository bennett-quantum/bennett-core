//! Compare mode must catch any change the history path makes to the run that the enumeration would not:
//! every field of the executor outside the cost report is restored before the enumeration and compared
//! after it. Each corruption below is made after the history path, before the enumeration (the testing
//! pass's mutation matrix, `tests/support/apply_mutations.py`, through a test-only hook instead of a
//! scratch copy of the source).

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::worlds::Status;
use crate::{ApplyMode, Executor, Options};

const PROBE: &str = "memory 512
func @f() -> i1 {
entry:
    ret 1
}
func @main() {
entry:
    %o = qalloc 1
    %e = qalloc 4
    apply @f() -> %o, %e budget 4
    ret
}";

type Corruption = fn(&mut Executor<'_>);

const CORRUPTIONS: &[(&str, Corruption)] = &[
    ("output", |e| e.quantum.xor_register(0, 1)),
    ("memory", |e| e.memory.write(0, &[1])),
    ("slot", |e| e.slots[0] ^= 1),
    ("journal", |e| e.journal.as_mut().expect("journal").clear()),
    ("tape", |e| e.tape.push(1)),
    ("region_steps", |e| e.region_steps += 1),
    ("region_faults", |e| e.region_faults += 1),
    ("fault_max_weight", |e| e.region_fault_max_weight += 1.0),
    ("rerun_steps", |e| e.rerun_steps += 1),
    ("mismatches", |e| e.mismatches += 1),
    ("split_memory_bytes", |e| e.split_memory_bytes += 1),
    ("split_journal_bytes", |e| e.split_journal_bytes += 1),
    ("rewound_steps", |e| e.rewound_steps += 1),
    ("splits", |e| e.splits += 1),
    ("fault_weight", |e| e.region_fault_weight += 1.0),
    ("status", |e| e.status = Status::Ended),
    ("partial", |e| e.partial = !e.partial),
    ("pending", |e| e.pending = !e.pending),
    ("merges", |e| e.merges += 1),
    ("peak_worlds", |e| e.peak_worlds += 1),
    ("steps", |e| e.steps += 1),
    ("budget", |e| e.budget = Some(0)),
    ("in_region", |e| e.in_region = true),
    ("backup", |e| {
        let copy = e.clone_world(None);
        e.backups.push(copy);
    }),
    ("deferring", |e| e.deferring = true),
    ("region_records", |e| e.region_records += 1),
    ("world_steps", |e| e.world_steps += 1),
    ("observations", |e| e.observations += 1),
    ("rng", |e| {
        let _: u64 = rand::Rng::random(&mut e.rng);
    }),
    ("forked", |e| {
        let copy = e.clone_world(None);
        e.forked.push(copy);
    }),
];

/// Runs the probe's `apply` in compare mode with `corrupt` made after the history path; the panic message
/// if the comparison caught it.
fn probe(corrupt: Option<Corruption>) -> Option<String> {
    let module = bennett_ir::load(PROBE).expect("probe program");
    let main = module.function_named("main").expect("main");
    let mut e = Executor::new(&module, main, &[], Options { reversible: true, ..Options::default() }).expect("entry");
    e.set_apply_mode(ApplyMode::Compare);
    e.corrupt_history = corrupt;
    catch_unwind(AssertUnwindSafe(|| {
        // Only the `apply` (the third step) is checked: nothing later may be what detects the corruption.
        for _ in 0..3 {
            assert_eq!(e.step(), None);
        }
    }))
    .err()
    .map(|panic| {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    })
}

#[test]
fn compare_mode_catches_every_corruption_of_the_history_path() {
    assert_eq!(probe(None), None, "the unchanged history path agrees");
    let missed: Vec<&str> = CORRUPTIONS
        .iter()
        .filter(|&&(_, corrupt)| !probe(Some(corrupt)).is_some_and(|message| message.contains("apply @f:")))
        .map(|&(name, _)| name)
        .collect();
    assert!(missed.is_empty(), "compare mode accepted a corrupted history path: {missed:?}");
}
