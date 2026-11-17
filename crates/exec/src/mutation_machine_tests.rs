//! Work counters whose intermediate values are not all part of a public snapshot.

use bennett_ir::Fault;

use crate::{ApplyMode, Executor, Options, Outcome};

#[test]
fn one_world_split_copies_every_caller_slot_exactly_once() {
    for (params, args, bytes) in [("", vec![], 32), ("i8 %a, i8 %b", vec![23, 71], 64)] {
        let text = format!(
            "memory 256
func @identity(i1 %value) -> i1 {{
entry:
    ret %value
}}
func @main({params}) -> i8 {{
entry:
    %q = qalloc 1
    h %q
    %value = call @identity(%q)
    ret 7
}}"
        );
        let module = bennett_ir::load(&text).expect("valid split witness");
        let entry = module.function_named("main").expect("main");
        let options = Options { budget: Some(20), reversible: true, ..Options::default() };
        let mut e = Executor::new(&module, entry, &args, options).expect("entry fits");
        assert_eq!(e.split_slot_bytes, 0);
        for _ in 0..3 {
            assert_eq!(e.step(), None);
        }
        assert_eq!(e.stats().splits, 1);
        // Two or four caller slots, each a 16-byte value, copied to the one extra world.
        assert_eq!(e.split_slot_bytes, bytes);
    }
}

#[test]
fn an_inspected_apply_preserves_prior_rewind_and_replay_work() {
    let module = bennett_ir::load(
        "memory 128
func @jump() -> i8 {
entry:
    %inner = mark
    %temporary = add i8 17, 25
    rewind %inner
    longjmp 16, 1
}
func @main() {
entry:
    h [16]
    %out = qalloc 8
    %fault = qalloc 5
    apply @jump() -> %out, %fault budget 20
    ret
}",
    )
    .expect("valid inspection witness");
    let entry = module.function_named("main").expect("main");
    for (rewound, replayed) in [(0, 0), (5, 7)] {
        let options = Options { budget: Some(40), reversible: true, ..Options::default() };
        let mut e = Executor::new(&module, entry, &[], options).expect("entry fits");
        e.set_apply_mode(ApplyMode::History);
        for _ in 0..3 {
            assert_eq!(e.step(), None);
        }
        // These are host work counters: the same apply must add the same work to any prior count.
        e.rewound_steps = rewound;
        e.rerun_steps = replayed;
        assert_eq!(e.run(), Outcome::Faulted(Fault::BadLongjmp));
        // Region history work is counted per configuration: each of the two configurations un-runs
        // one addition internally, then replays it when the boundary undoes the inner rewind.
        assert_eq!(e.stats().rewound_steps, rewound + 2);
        assert_eq!(e.stats().rerun_steps, replayed + 2);
    }
}

#[test]
fn a_region_partition_fits_at_the_last_representable_split_count() {
    let module = bennett_ir::load(
        "memory 256
func @identity(i2 %value) -> i2 {
entry:
    ret %value
}
func @main() {
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %out = qalloc 2
    %fault = qalloc 5
    apply @identity(%q) -> %out, %fault budget 2
    ret
}",
    )
    .expect("valid four-part region");
    let entry = module.function_named("main").expect("main");
    let options = Options { budget: Some(20), ..Options::default() };
    let mut e = Executor::new(&module, entry, &[], options).expect("entry fits");
    e.set_apply_mode(ApplyMode::History);
    for _ in 0..5 {
        assert_eq!(e.step(), None);
    }
    // Four parts add exactly three temporary splits. That still fits, and the region restores the
    // caller's count. Counting four/five splits, or multiplying by three, overflows needlessly.
    e.splits = u64::MAX - 3;
    assert_eq!(e.run(), Outcome::Returned(None));
    assert_eq!(e.stats().splits, u64::MAX - 3);
}
