//! Observe live call frames and the old backing words that must survive reversal.

mod common;

use bennett_exec::{Options, Outcome, SitePos, Undo, WordRole, WordWitness};
use bennett_ir::{Fault, Inst, Operand, Slot, Terminator};
use common::{assert_same_state, bounded, entry, executor, load};
use num_complex::Complex64;

const CALL_WINDOWS: &str = r#"
memory 256
data 0 = i8 9
func @main(i8 %mode) -> i8 {
entry:
    %q = qalloc 1
    h %q
    %fp = faddr @wide
    %first = call @outer(%q, %fp)
    call @small()
    call @empty()
    %last = call @outer(%first, %fp)
    switch i8 %mode, 1: overflow, 2: bad, default: done
overflow:
    %p = alloca 200
    call @outer(%last, %fp)
    ret 7
bad:
    call_indirect fn(q1) -> q1 0(%last)
    ret 7
done:
    ret 7
}
func @outer(q1 %q, i64 %fp) -> q1 {
entry:
    call @small()
    %forwarded = call_indirect fn(q1) -> q1 %fp(%q)
    call @empty()
    call @small()
    ret %forwarded
}
func @small() {
entry:
    %tag = add i8 0, 0xa7
    ret
}
func @wide(q1 %q) -> q1 {
entry:
    %a = shl i128 1, 127
    %pass = call @identity(%q)
    %b = add i8 0, 0xb2
    %c = add i16 0, 0xc301
    %d = add i32 0, 0xd4020304
    %e = add i64 0, 0xe506070809101112
    %f = add i128 %a, 0xf6
    store i8 0x5a, 0
    effect 65
    s %pass
    ret %pass
}
func @identity(q1 %q) -> q1 {
entry:
    ret %q
}
func @empty() {
entry:
    ret
}
"#;

#[test]
fn repeated_call_windows_are_zeroed_and_restored_in_both_directions() {
    let module = load(CALL_WINDOWS);
    let outer = entry(&module, "outer");
    let small = entry(&module, "small");
    let wide = entry(&module, "wide");
    let identity = entry(&module, "identity");
    let empty = entry(&module, "empty");
    let half = std::f64::consts::FRAC_1_SQRT_2;
    for (mode, outcome) in [
        (0, Outcome::Returned(Some(7))),
        (1, Outcome::Faulted(Fault::StackOverflow)),
        (2, Outcome::Faulted(Fault::BadIndirect)),
    ] {
        for reversible in [false, true] {
            let mut run = executor(&module, "main", &[mode], Options { reversible, ..bounded() });
            let mut snapshots = vec![run.snapshot()];
            let mut entries = Vec::new();
            while run.outcome().is_none() {
                let before = snapshots.last().expect("initial snapshot");
                let _ = run.step();
                let after = run.snapshot();
                if after.classical.frames.len() > before.classical.frames.len() {
                    let top = after.classical.frames.last().expect("callee");
                    let handle = after.classical.slots[1];
                    assert_ne!(handle, 0);
                    let expected = if top.func == outer {
                        vec![handle, u128::from(wide.0) + 1, 0]
                    } else if top.func == small {
                        vec![0]
                    } else if top.func == wide {
                        vec![handle, 0, 0, 0, 0, 0, 0, 0]
                    } else if top.func == identity {
                        vec![handle]
                    } else {
                        assert_eq!(top.func, empty);
                        vec![]
                    };
                    assert_eq!(top.base, before.classical.slots.len());
                    assert_eq!(top.entry_stack_pointer, before.classical.stack_pointer);
                    assert_eq!(after.classical.stack_pointer, top.entry_stack_pointer - 8 * expected.len() as u64);
                    assert_eq!((top.block.0, top.inst), (0, 0));
                    assert_eq!(&after.classical.slots[top.base..], expected, "entry to {:?}", top.func);
                    entries.push(top.func);
                }
                if after.classical.frames.len() < before.classical.frames.len() {
                    let top = before.classical.frames.last().expect("returning frame");
                    if top.func == wide {
                        let handle = before.classical.slots[1];
                        assert_eq!(
                            &before.classical.slots[top.base..],
                            [handle, 1 << 127, handle, 0xb2, 0xc301, 0xd4020304, 0xe506070809101112, (1 << 127) + 0xf6],
                        );
                    }
                    assert_eq!(after.classical.stack_pointer, top.entry_stack_pointer);
                }
                if matches!(after.classical.outcome, Some(Outcome::Faulted(_))) {
                    let mut expected = before.clone();
                    expected.classical.outcome = Some(outcome);
                    assert_eq!(after, expected, "a failed call changes only the outcome");
                }
                snapshots.push(after);
            }
            assert_eq!(
                entries,
                [outer, small, wide, identity, empty, small, small, empty, outer, small, wide, identity, empty, small]
            );
            assert_eq!(run.outcome(), Some(outcome));
            let last = snapshots.last().expect("final snapshot");
            assert_eq!(last.classical.memory[0], 0x5a);
            assert_eq!(last.classical.tape, b"AA");
            assert_eq!(
                last.quantum.amplitudes,
                [(vec![0], Complex64::new(half, 0.0)), (vec![1], Complex64::new(-half, 0.0))]
            );
            if reversible {
                // A fault contributes no step and no journal record. Reversal clears it as it undoes
                // the preceding successful step.
                let completed = last.classical.steps as usize;
                for (step, expected) in snapshots[..completed].iter().enumerate().rev() {
                    assert!(run.step_back().expect("undo succeeds"));
                    assert_same_state(&run.snapshot(), expected, &format!("mode {mode}, undo to step {step}"));
                }
                assert!(!run.step_back().expect("journal exhausted"));
                for (step, expected) in snapshots.iter().enumerate().skip(1) {
                    assert_eq!(run.step(), expected.classical.outcome);
                    assert_same_state(&run.snapshot(), expected, &format!("mode {mode}, replay step {step}"));
                }
            }
        }
    }
}

const SKIPPED_DEFINITIONS: &str = r#"
memory 256
data 0 = i64 0x0000000100000001, i64 0x0000000100000001, i64 240, i64 200
data 32 = i64 0x0000000200000002, i64 0x0000000100000001, i64 200, i64 168
func @main() {
entry:
    %out = qalloc 8
    %fault = qalloc 5
    call @warm()
    apply @work() -> %out, %fault budget 12
    ret
}
func @work() -> i8 {
entry:
    longjmp 0, 0
resume:
    %skipped = add i8 0, 0x31
    %r = setjmp 0
    call @warm_inner()
    %inner = call @reader()
    %sum = add i8 %skipped, %inner
    %result = xor i8 %sum, 0x6c
    ret %result
}
func @reader() -> i8 {
entry:
    longjmp 32, 0
resume:
    %skipped = shl i128 1, 100
    %r = setjmp 32
    %hi = lshr i128 %skipped, 120
    %byte = trunc i128 %hi to i8
    ret %byte
}
func @warm() {
entry:
    %dirty = add i8 0, 0x31
    ret
}
func @warm_inner() {
entry:
    %dirty = shl i128 0x35, 120
    ret
}
"#;

#[test]
fn applied_entry_and_nested_calls_read_zero_when_their_definitions_are_skipped() {
    let module = load(SKIPPED_DEFINITIONS);
    for reversible in [false, true] {
        let mut run = executor(&module, "main", &[], Options { reversible, ..bounded() });
        for _ in 0..5 {
            assert_eq!(run.step(), None);
        }
        let before = run.snapshot();
        assert_eq!(before.quantum.amplitudes, [(vec![0, 0], Complex64::new(1.0, 0.0))]);
        // Both records resume just after setjmp without executing the producer above it.
        // The outer call suspends at an entry with two slots, @work has five, @reader has four.
        let mut after = before.clone();
        after.classical.steps += 1;
        after.classical.frames[0].inst += 1;
        after.quantum.amplitudes[0].0 = vec![0x6c, 0];
        assert_eq!(run.step(), None);
        assert_eq!(run.snapshot(), after);
        assert_eq!(run.stats().region_steps, 12);
        if reversible {
            assert!(run.step_back().expect("undo apply"));
            assert_eq!(run.snapshot(), before);
            assert!(run.step_back().expect("undo warm return"));
            assert_eq!(&run.snapshot().classical.slots[2..], [0x31]);
            assert_eq!(run.step(), None);
            assert_eq!(run.step(), None);
            assert_eq!(run.snapshot(), after);
        }
        assert_eq!(run.step(), Some(Outcome::Returned(None)));
    }
}

const OBSERVED_CALLS: &str = r#"
memory 256
func @main() {
entry:
    %out = qalloc 8
    %fault = qalloc 5
    call @warm()
    apply @probe(i8 20, i8 7) -> %out, %fault budget 6
    apply @probe(i8 30, i8 8) -> %out, %fault budget 6
    ret
}
func @warm() {
entry:
    %a = add i8 0, 11
    %b = add i8 0, 22
    %c = shl i128 3, 110
    %d = add i8 0, 44
    %e = add i8 0, 55
    %f = add i8 0, 66
    %g = shl i128 7, 110
    %h = add i8 0, 88
    ret
}
func @probe(i8 %x, i8 %y) -> i8 {
entry:
    %tmp = add i8 %x, %y
    %answer = call @leaf(%tmp, %y)
    ret %answer
}
func @leaf(i8 %x, i8 %y) -> i8 {
entry:
    %tmp = sub i8 %x, %y
    %unused = add i8 0, 1
    ret %tmp
}
"#;

fn observed_calls() -> bennett_ir::ValidModule {
    let mut module = bennett_ir::read(OBSERVED_CALLS).expect("well-formed text");
    // Text assigns parameters the first slots. The public IR also accepts interleaved slots.
    // Give parameter 0 a higher index than parameter 1 in both callees.
    let probe = &mut module.functions[2];
    probe.slot_names = ["tmp", "y", "answer", "x"].map(String::from).to_vec();
    probe.params[0].slot = Slot(3);
    let [Inst::Binary { dst, lhs, .. }, Inst::Call { dst: result, args, .. }] = probe.blocks[0].insts.as_mut_slice()
    else {
        panic!("probe fixture instructions");
    };
    *dst = Slot(0);
    *lhs = Operand::Slot(Slot(3));
    *result = Some(Slot(2));
    args[0] = Operand::Slot(Slot(0));
    probe.blocks[0].term = Terminator::Return(Some(Operand::Slot(Slot(2))));

    let leaf = &mut module.functions[3];
    leaf.slot_names = ["y", "unused", "x", "tmp"].map(String::from).to_vec();
    leaf.params[0].slot = Slot(2);
    leaf.params[1].slot = Slot(0);
    let [Inst::Binary { dst, lhs, rhs, .. }, Inst::Binary { dst: unused, .. }] = leaf.blocks[0].insts.as_mut_slice()
    else {
        panic!("leaf fixture instructions");
    };
    *dst = Slot(3);
    *lhs = Operand::Slot(Slot(2));
    *rhs = Operand::Slot(Slot(0));
    *unused = Slot(1);
    leaf.blocks[0].term = Terminator::Return(Some(Operand::Slot(Slot(3))));
    bennett_ir::validate(module).expect("valid interleaved slots")
}

#[test]
fn journal_reports_old_backing_words_by_slot_and_parameter_position() {
    use WordRole::{CallParam, CallSlot};

    let module = observed_calls();
    let probe = entry(&module, "probe");
    let mut run = executor(&module, "main", &[], bounded());
    run.measure_journal_content(true);
    assert_eq!(run.run(), Outcome::Returned(None));
    assert_eq!(run.snapshot().quantum.amplitudes, [(vec![20 ^ 30, 0], Complex64::new(1.0, 0.0))]);

    let measurement = run.journal_measurement().expect("observer enabled");
    assert_eq!(measurement.applies.len(), 2);
    for (apply, (x, y)) in measurement.applies.iter().zip([(20, 7), (30, 8)]) {
        assert_eq!(apply.callee, probe);
        let [recording] = apply.branches.as_slice() else {
            panic!("one definite argument assignment");
        };
        assert_eq!(recording.args, [x, y]);
        assert_eq!(recording.outcome, Outcome::Returned(Some(x)));
        assert_eq!((recording.steps, recording.records), (6, 7));
        let calls: Vec<_> = recording.entries.iter().filter(|record| record.kind == Undo::Call).collect();
        let [root, nested] = calls.as_slice() else {
            panic!("entry and nested call records");
        };
        for (record, pos, expected) in [
            (
                root,
                SitePos::Entry,
                [(CallSlot(0), 11), (CallParam(1), 22), (CallSlot(2), 3 << 110), (CallParam(0), 44)],
            ),
            (
                nested,
                SitePos::Inst(1),
                [(CallParam(1), 55), (CallSlot(1), 66), (CallParam(0), 7 << 110), (CallSlot(3), 88)],
            ),
        ] {
            assert_eq!((record.site.func, record.site.block.0, record.site.pos), (probe, 0, pos));
            assert_eq!(record.words.len(), expected.len());
            for (word, (role, value)) in record.words.iter().zip(expected) {
                assert_eq!((word.role, word.value), (role, value), "{pos:?}");
                assert_eq!(word.witness, WordWitness::FirstWrite, "{pos:?}");
                assert_eq!(word.recompute, None);
            }
        }
    }
}
