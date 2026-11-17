//! `setjmp` and `longjmp`: jumps across frames, step-by-step reversal, and named faults for records that
//! name no live frame of the run or region.
mod common;

use bennett_exec::{Options, Outcome, RunError, WordClass, WordRole, classify_with_chains};
use bennett_ir::Fault;
use common::{allowing_region_faults, assert_reverses, assert_same_state, bounded, executor, load, run};

const EXAMPLE: &str = include_str!("../../../examples/setjmp.bn");

#[test]
fn the_example_jumps_back_three_times_and_reverses() {
    let module = load(EXAMPLE);
    let (outcome, _) = run(&module, "main", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(430)));
    assert_eq!(assert_reverses(&module, "main", &[], bounded()), outcome);
}

/// Every step undone returns exactly to the state before it, the jumps included.
#[test]
fn each_step_back_restores_the_state_before_that_step() {
    let module = load(EXAMPLE);
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    let mut before = vec![executor.snapshot()];
    while executor.step().is_none() {
        before.push(executor.snapshot());
    }
    assert_eq!(executor.outcome(), Some(Outcome::Returned(Some(430))));
    while let Some(expected) = before.pop() {
        assert!(executor.step_back().expect("reversible"));
        assert_same_state(&executor.snapshot(), &expected, &format!("after undoing step {}", before.len()));
    }
    assert!(!executor.step_back().expect("reversible"));
}

/// `@f` sets a jump record and returns; a later `longjmp` through it names a frame that is gone.
const STALE: &str = r#"
memory 512
func @f(i64 %buf) {
entry:
    %r = setjmp %buf
    ret
}
func @g(i64 %buf) {
entry:
    %pad = alloca 8
    longjmp %buf, 1
}
func @main() -> i32 {
entry:
    %buf = alloca 32
    call @f(%buf)
    call @g(%buf)
    ret 0
}
"#;

/// An invalid jump record and an invalid call target are told apart since fault codes are 5 bits wide: the
/// same garbage word as a jump record is `bad_longjmp`, as a call target `bad_indirect`.
#[test]
fn an_invalid_jump_record_is_not_a_bad_call_target() {
    let text = "memory 256\nfunc @g() {\nentry:\n ret\n}\nfunc @main(i1 %jump) {\nentry:\n store i64 77, 64\n \
                br %jump, jump, call\njump:\n longjmp 64, 1\ncall:\n %t = load i64 64\n call_indirect fn() %t()\n ret\n}\n";
    let module = load(text);
    assert_eq!(run(&module, "main", &[1], bounded()).0, Outcome::Faulted(Fault::BadLongjmp));
    assert_eq!(run(&module, "main", &[0], bounded()).0, Outcome::Faulted(Fault::BadIndirect));
    assert_reverses(&module, "main", &[1], bounded());
}

#[test]
fn a_jump_to_a_frame_that_returned_faults() {
    let module = load(STALE);
    assert_eq!(run(&module, "main", &[], bounded()).0, Outcome::Faulted(Fault::BadLongjmp));
    assert_reverses(&module, "main", &[], bounded());
}

/// A jump through a buffer the program overwrote between `setjmp` and `longjmp`; `%field` picks the
/// word, `%delta` what is added to it.
const CORRUPT: &str = r#"
memory 512
func @main(i64 %field, i64 %delta) -> i32 {
entry:
    %buf = alloca 32
    %r = setjmp %buf
    %again = icmp ne i32 %r, 0
    br %again, done, corrupt
corrupt:
    %offset = mul i64 %field, 8
    %at = add i64 %buf, %offset
    %old = load i64 %at
    %new = add i64 %old, %delta
    store i64 %new, %at
    longjmp %buf, 5
done:
    ret %r
}
"#;

#[test]
fn every_field_of_the_record_is_checked() {
    let module = load(CORRUPT);
    assert_eq!(run(&module, "main", &[0, 0], bounded()).0, Outcome::Returned(Some(5)));
    // Depth, function, block, instruction, entry stack pointer: any change names no setjmp of a live frame.
    for (field, delta) in [(0, 1), (0, 1 << 32), (1, 1), (1, 1 << 32), (1, u64::MAX << 32), (2, 8), (2, u64::MAX)] {
        let (outcome, _) = run(&module, "main", &[field, delta], bounded());
        assert_eq!(outcome, Outcome::Faulted(Fault::BadLongjmp), "word {field} + {delta:#x}");
    }
    // The stack pointer may not leave the frame's allocations: above them, or below where they stand.
    for delta in [40, 1 << 20, u64::MAX - 7] {
        let (outcome, _) = run(&module, "main", &[3, delta], bounded());
        assert_eq!(outcome, Outcome::Faulted(Fault::BadLongjmp), "stack pointer + {delta:#x}");
    }
    assert_reverses(&module, "main", &[3, 40], bounded());
}

#[test]
fn a_record_outside_memory_faults() {
    let text = "memory 64\nfunc @main() -> i32 {\nentry:\n %r = setjmp 40\n ret %r\n}\n";
    assert_eq!(run(&load(text), "main", &[], bounded()).0, Outcome::Faulted(Fault::OutOfBounds));
    let text = "memory 64\nfunc @main() {\nentry:\n longjmp 60, 1\n}\n";
    assert_eq!(run(&load(text), "main", &[], bounded()).0, Outcome::Faulted(Fault::OutOfBounds));
}

/// Inside a coherent region a jump within the region works on every branch; the record lives in the
/// region's private memory. `@jumpy(n)` returns `n + 1` through a `longjmp` from a callee.
const REGION: &str = r#"
memory 512
func @thrower(i64 %buf, i32 %v) {
entry:
    longjmp %buf, %v
}
func @jumpy(i2 %n) -> i2 {
entry:
    %buf = alloca 32
    %r = setjmp %buf
    %first = icmp eq i32 %r, 0
    br %first, throw, done
throw:
    %n32 = zext i2 %n to i32
    %v = add i32 %n32, 1
    call @thrower(%buf, %v)
    ret 0
done:
    %r2 = trunc i32 %r to i2
    ret %r2
}
func @main() -> i2 {
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %out = qalloc 2
    %fault = qalloc 5
    apply @jumpy(%q) -> %out, %fault budget 100
    apply @jumpy(%q) -> %out, %fault budget 100
    qfree %out
    qfree %fault
    h %q[0]
    h %q[1]
    %m = measure %q
    ret %m
}
"#;

#[test]
fn a_jump_inside_a_region_runs_on_every_branch() {
    let module = load(REGION);
    let (outcome, executor) = run(&module, "main", &[], bounded());
    // The second apply undoes the first, the registers are clean, and the Hadamards interfere back to 0.
    assert_eq!(outcome, Outcome::Returned(Some(0)));
    assert_eq!(executor.stats().region_faults, 0);
    // Without the measurement, the whole run reverses.
    let unmeasured = REGION.replace("    %m = measure %q\n    ret %m", "    ret 0");
    assert_reverses(&load(&unmeasured), "main", &[], bounded());
}

/// A region callee that jumps to a record its caller set: the frame is outside the region.
const ESCAPE: &str = r#"
memory 512
func @escape(i1 %b) -> i1 {
entry:
    %buf = load i64 16
    longjmp %buf, 1
}
func @main() -> i32 {
entry:
    %buf = alloca 32
    store i64 %buf, 16
    %r = setjmp %buf
    %again = icmp ne i32 %r, 0
    br %again, back, go
go:
    %q = qalloc 1
    h %q
    %out = qalloc 1
    %fault = qalloc 5
    apply @escape(%q) -> %out, %fault budget 100
    ret 0
back:
    ret 7
}
"#;

#[test]
fn a_jump_out_of_a_region_faults_on_every_branch() {
    let module = load(ESCAPE);
    let (outcome, _) = run(&module, "main", &[], bounded());
    let escape = common::entry(&module, "escape");
    let expected = RunError::RegionFault { callee: escape, fault: Fault::BadLongjmp, branches: 2, configurations: 2 };
    assert_eq!(outcome, Outcome::Stopped(expected));
    let (outcome, executor) = run(&module, "main", &[], allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(0)));
    assert!(executor.snapshot().quantum.amplitudes.iter().all(|(basis, _)| basis[2] == Fault::BadLongjmp.code()));
}

#[test]
fn the_text_form_round_trips() {
    let module = bennett_ir::read(EXAMPLE).expect("reads");
    let text = bennett_ir::write(&module);
    assert!(text.contains("%r = setjmp %buf") && text.contains("longjmp %buf, %v"), "{text}");
    assert_eq!(bennett_ir::read(&text).expect("reads back"), module);
}

#[test]
fn the_validator_checks_operand_types() {
    let bad_value = "memory 64\nfunc @main(i64 %b) {\nentry:\n longjmp %b, %b\n}\n";
    assert!(bennett_ir::load(bad_value).is_err());
    let bad_buffer = "memory 64\nfunc @main(i32 %b) -> i32 {\nentry:\n %r = setjmp %b\n ret %r\n}\n";
    assert!(bennett_ir::load(bad_buffer).is_err());
    let bad_result = "memory 64\nfunc @main(i64 %b) -> i64 {\nentry:\n %r = setjmp %b\n ret %r\n}\n";
    assert!(bennett_ir::load(bad_result).is_err());
}

/// The journal-content observer follows `setjmp` and `longjmp` records inside a region (its shadow frames
/// pop with the discarded frames) and changes nothing.
#[test]
fn the_journal_content_observer_follows_jumps() {
    let module = load(REGION);
    let mut off = executor(&module, "main", &[], bounded());
    let off_outcome = off.run();
    let mut on = executor(&module, "main", &[], bounded());
    on.measure_journal_content(true);
    assert_eq!(on.run(), off_outcome);
    assert_eq!(on.stats(), off.stats());
    assert_same_state(&on.snapshot(), &off.snapshot(), "observer on");
    let measurement = on.journal_measurement().expect("on");
    let classes = bennett_exec::classify(measurement);
    assert_eq!(classes.len(), 2, "two applies");
    let records: Vec<bennett_exec::Undo> =
        measurement.applies[0].branches[0].entries.iter().map(|record| record.kind).collect();
    assert!(records.contains(&bennett_exec::Undo::Setjmp) && records.contains(&bennett_exec::Undo::Longjmp));
    let mut counted = executor(&module, "main", &[], bounded());
    counted.measure_journal_content(false);
    assert_eq!(counted.run(), off_outcome);
    assert_eq!(counted.stats(), off.stats());
    assert_same_state(&counted.snapshot(), &off.snapshot(), "observer counts only");
    for (raw, counts) in measurement.applies.iter().zip(&counted.journal_measurement().expect("on").applies) {
        for (raw, counts) in raw.branches.iter().zip(&counts.branches) {
            assert_eq!((raw.records, raw.words), (counts.records, counts.words));
            assert!(counts.entries.is_empty());
        }
    }
    for tape in [false, true] {
        for (apply, classes) in measurement.applies.iter().zip(classify_with_chains(measurement, tape)) {
            for positions in [&classes.site_count, &classes.call_path] {
                assert_eq!(
                    positions.iter().map(|p| p.instances).sum::<usize>(),
                    apply.branches.iter().map(|b| b.words).sum()
                );
                assert!(
                    positions.iter().all(|p| p.instances == p.branches_present),
                    "one instance per branch and word"
                );
            }
        }
    }
}

#[test]
fn branch_only_longjmp_control_stays_opaque_even_when_live_slots_hold_its_numbers() {
    // Branch zero returns without jumping. The other branches discard one frame, and the live
    // %one aliases that control word. It does not certify which branches reached the jump.
    let text = REGION.replace("    %buf = alloca 32", "    %one = or i64 1, 0\n    %buf = alloca 32").replace(
        "    br %first, throw, done",
        "    %jumps = icmp ne i2 %n, 0\n    %both = and i1 %first, %jumps\n    br %both, throw, done",
    );
    let module = load(&text);
    let mut on = executor(&module, "main", &[], bounded());
    on.measure_journal_content(true);
    assert_eq!(on.run(), Outcome::Returned(Some(0)));
    let measurement = on.journal_measurement().expect("on");
    for tape in [false, true] {
        for classes in classify_with_chains(measurement, tape) {
            for positions in [&classes.site_count, &classes.call_path] {
                assert_eq!(
                    positions.iter().find(|p| p.role == WordRole::LongjmpResult).expect("old scalar result").class,
                    WordClass::Recoverable,
                    "the overwritten result is scalar data with a live copy, unlike control presence"
                );
                let control: Vec<_> =
                    positions.iter().filter(|p| matches!(p.role, WordRole::LongjmpControl(_))).collect();
                assert_eq!(control.len(), 5, "two discarded-frame words and three target/control words");
                for p in control {
                    assert_eq!((p.branches_present, p.branches_total), (3, 4));
                    assert_eq!(p.class, WordClass::Irreducible);
                    let diagnostic = p.chain.as_ref().expect("control diagnostic");
                    assert!(!diagnostic.recovered && !diagnostic.individually_recovered);
                    assert!(diagnostic.blockers.iter().any(|b| b == "control presence needs replay"));
                }
            }
        }
    }
}

/// A `rewind` whose history holds a jump: `@kept(x)` marks, sets a record, jumps back to it from a callee
/// with `x + 1`, and rewinds keeping the jump's value. The jump and every trace of it are un-run (the record,
/// the allocation); the value stays in `%out`. The rewind is itself undone by running forward again.
const REWIND: &str = r#"
memory 512
func @thrower(i64 %buf, i32 %v) {
entry:
    %pad = alloca 16
    longjmp %buf, %v
}
func @kept(q8 %out, i8 %x) -> i64 {
entry:
    %sp0 = stacksave
    %h = mark
    %buf = alloca 32
    %r = setjmp %buf
    %first = icmp eq i32 %r, 0
    br %first, go, back
go:
    %x32 = zext i8 %x to i32
    %v = add i32 %x32, 1
    call @thrower(%buf, %v)
    ret 0
back:
    %v8 = trunc i32 %r to i8
    rewind %h keep (%v8 -> %out)
    %sp1 = stacksave
    %record = sub i64 %sp0, 32
    %word = load i64 %record
    %moved = sub i64 %sp0, %sp1
    %both = or i64 %moved, %word
    ret %both
}
func @main(i8 %x) -> i8 {
entry:
    %out = qalloc 8
    %clean = call @kept(%out, %x)
    %zero = icmp eq i64 %clean, 0
    br %zero, ok, bad
ok:
    %m = measure %out
    ret %m
bad:
    fault unreachable
}
func @unmeasured(i8 %x) -> i64 {
entry:
    %out = qalloc 8
    %clean = call @kept(%out, %x)
    ret %clean
}
"#;

#[test]
fn a_rewind_un_runs_a_jump_and_keeps_its_value() {
    let module = load(REWIND);
    for x in [0u64, 1, 41, 255] {
        let (outcome, _) = run(&module, "main", &[x], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from((x + 1) % 256))), "x = {x}");
        assert_eq!(assert_reverses(&module, "unmeasured", &[x], bounded()), Outcome::Returned(Some(0)), "x = {x}");
    }
}

/// The front's `sigsetjmp` and `siglongjmp` as they are lowered (`front/src/Jumps.cpp`), with a memory word for
/// the signal mask and `@tail` for musl's `__sigsetjmp_tail`: before its `setjmp` each visit writes the buffer's
/// flag (here the run-time savemask, `i == 0`) and saves the mask at `buf + 72`; a jump restores the mask from
/// its target when the target's flag says so. One call site is visited twice, for buffer 0 (mask 1, saved) then
/// buffer 1 (mask 2, not saved), and buffer 0 is copied to buffer 2 between the visits. The jump from mask 9 to
/// buffer `t` lands with mask 1, 9, 1: what the target saved, not what the site's latest visit had. Around it a
/// `rewind` keeps that mask and un-runs everything else: the mask word and all three buffers are zero again.
const SIGSETJMP: &str = r#"
memory 2048
func @tail(i64 %buf, i32 %ret, i64 %mask) {
entry:
    %ss = add i64 %buf, 72
    %save = icmp eq i32 %ret, 0
    br %save, save, restore
save:
    %m = load i64 %mask
    store i64 %m, %ss
    ret
restore:
    %s = load i64 %ss
    store i64 %s, %mask
    ret
}
func @siglongjmp(i64 %buf, i32 %v, i64 %mask) {
entry:
    %flagp = add i64 %buf, 64
    %flag = load i64 %flagp
    %saved = icmp ne i64 %flag, 0
    br %saved, restore, go
restore:
    call @tail(%buf, i32 1, %mask)
    br go
go:
    longjmp %buf, %v
}
func @copy(i64 %to, i64 %from) {
entry:
    br word(i64 0)
word(i64 %at):
    %src = add i64 %from, %at
    %dst = add i64 %to, %at
    %w = load i64 %src
    store i64 %w, %dst
    %next = add i64 %at, 8
    %more = icmp ult i64 %next, 200
    br %more, word(%next), done
done:
    ret
}
func @visits(i64 %bufs, i64 %mask, i64 %t) -> i64 {
entry:
    br visit(i64 0)
visit(i64 %i):
    %m = add i64 %i, 1
    store i64 %m, %mask
    %off = mul i64 %i, 200
    %buf = add i64 %bufs, %off
    %first_visit = icmp eq i64 %i, 0
    %saves = zext i1 %first_visit to i64
    %flagp = add i64 %buf, 64
    store i64 %saves, %flagp
    br %first_visit, save, site
save:
    call @tail(%buf, i32 0, %mask)
    br site
site:
    %r = setjmp %buf
    %again = icmp eq i32 %r, 0
    br %again, next, landed
next:
    br %first_visit, copied, more
copied:
    %copy = add i64 %bufs, 400
    call @copy(%copy, %buf)
    br visit(i64 1)
more:
    store i64 9, %mask
    %toff = mul i64 %t, 200
    %target = add i64 %bufs, %toff
    call @siglongjmp(%target, i32 1, %mask)
    ret 0
landed:
    %got = load i64 %mask
    ret %got
}
func @kept(q8 %out, i64 %t) -> i64 {
entry:
    %bufs = alloca 600
    %mask = alloca 8
    %h = mark
    %got = call @visits(%bufs, %mask, %t)
    %g8 = trunc i64 %got to i8
    rewind %h keep (%g8 -> %out)
    %end = add i64 %bufs, 600
    %left = load i64 %mask
    br sum(%bufs, %left)
sum(i64 %at, i64 %acc):
    %w = load i64 %at
    %acc1 = or i64 %acc, %w
    %next = add i64 %at, 8
    %more = icmp ult i64 %next, %end
    br %more, sum(%next, %acc1), done
done:
    ret %acc1
}
func @main(i64 %t) -> i8 {
entry:
    %out = qalloc 8
    %clean = call @kept(%out, %t)
    %zero = icmp eq i64 %clean, 0
    br %zero, ok, bad
ok:
    %m = measure %out
    ret %m
bad:
    fault unreachable
}
func @unmeasured(i64 %t) -> i64 {
entry:
    %out = qalloc 8
    %clean = call @kept(%out, %t)
    ret %clean
}
"#;

#[test]
fn a_lowered_sigsetjmp_restores_its_targets_mask_and_rewinds_exactly() {
    let module = load(SIGSETJMP);
    for (t, mask) in [(0u64, 1u128), (1, 9), (2, 1)] {
        assert_eq!(run(&module, "main", &[t], bounded()).0, Outcome::Returned(Some(mask)), "target {t}");
        assert_eq!(assert_reverses(&module, "unmeasured", &[t], bounded()), Outcome::Returned(Some(0)), "target {t}");
        let mut executor = executor(&module, "unmeasured", &[t], Options { reversible: true, ..bounded() });
        let mut before = vec![executor.snapshot()];
        while executor.step().is_none() {
            before.push(executor.snapshot());
        }
        while let Some(expected) = before.pop() {
            assert!(executor.step_back().expect("reversible"));
            assert_same_state(&executor.snapshot(), &expected, &format!("target {t}, undoing step {}", before.len()));
        }
    }
}

#[test]
fn rewinding_a_same_frame_longjmp_does_not_count_a_return() {
    let module = load(
        "memory 256
func @main() -> i32 {
entry:
    %m = mark
    %value = setjmp 64
    %first = icmp eq i32 %value, 0
    br %first, jump, done
jump:
    longjmp 64, 1
done:
    rewind %m
    ret 0
}
",
    );
    assert_eq!(run(&module, "main", &[], bounded()).0, Outcome::Returned(Some(0)));
    assert_eq!(assert_reverses(&module, "main", &[], bounded()), Outcome::Returned(Some(0)));
}

#[test]
fn journal_cost_counts_read_cells_and_zero_frame_jump_metadata() {
    fn step_cost(executor: &mut bennett_exec::Executor<'_>) -> usize {
        let before = executor.stats().journal_bytes.expect("reversible journal");
        assert_eq!(executor.step(), None);
        executor.stats().journal_bytes.expect("reversible journal").checked_sub(before).expect("history grows")
    }
    let module = load(
        "memory 256
func @main() -> i32 {
entry:
    z [16]
    z [16]
    %plain = load i8 17
    %cell = load i8 16
    %value = setjmp 64
    %first = icmp eq i32 %value, 0
    br %first, jump, done
jump:
    longjmp 64, 1
done:
    ret 0
}
",
    );
    let mut executor = executor(&module, "main", &[], Options { reversible: true, ..bounded() });
    step_cost(&mut executor); // First gate promotes; the next one adds only a gate record.
    let record = step_cost(&mut executor);
    let slot = step_cost(&mut executor);
    let cell = step_cost(&mut executor);
    assert!(cell > slot, "a cell read keeps its byte beside the ordinary load's overwritten slot");
    let word = slot.checked_sub(record).expect("an overwritten slot adds a word");
    assert!(word > 0);
    for _ in 0..3 {
        step_cost(&mut executor); // setjmp, comparison, branch
    }
    let jump = step_cost(&mut executor);
    // DESIGN's non-local-jump journal: target position, stack pointer, count, old result slot;
    // even zero discarded frames have a count beside the marks. No struct sizes/padding are pinned.
    assert!(jump > record + 4 * word, "the discarded-frame side entry also occupies journal space");
    assert_eq!(executor.run(), Outcome::Returned(Some(0)));
}
