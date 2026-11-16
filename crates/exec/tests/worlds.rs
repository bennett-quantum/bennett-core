//! Worlds: the quantum state as a superposition of whole machine states (`docs/DESIGN.md`, "Worlds").
//! Ordinary code reads a register as an integer when a call passes it to an integer parameter; a world
//! whose configurations disagree splits, every step keeps what it destroys, and `rewind` merges worlds
//! that are the same machine state again, so that their amplitudes interfere.

mod common;

use std::f64::consts::FRAC_1_SQRT_2;

use bennett_exec::{Options, Outcome, ReverseError, RunError, Snapshot};
use bennett_ir::Fault;
use common::{TOLERANCE, assert_same_state, bounded, executor, load, run};
use num_complex::Complex64;

const WORLDS: &str = r#"
memory 256
data 0 = i8 0

func @id(i1 %x) -> i1 {
entry:
    ret %x
}

; Different path lengths per value, a memory write and console output: all of it un-run.
func @f(i2 %x) -> i8 {
entry:
    %w = zext i2 %x to i8
    store i8 %w, 0
    effect 42
    br head(i2 0, i8 1)
head(i2 %i, i8 %acc):
    %more = icmp ult i2 %i, %x
    br %more, body, done
body:
    %next = mul i8 %acc, 3
    %j = add i2 %i, 1
    br head(%j, %next)
done:
    ret %acc
}

; H, then ordinary code reads q: two worlds, never merged; the host observes one at the end.
func @split() -> i1 {
entry:
    %q = qalloc 1
    h %q
    %v = call @id(%q)
    ret %v
}

; H, mark, read, rewind, H: the two worlds merge and interfere back to |0>.
func @interfere() -> i1 {
entry:
    %q = qalloc 1
    h %q
    %m = mark
    %v = call @id(%q)
    rewind %m
    h %q
    %r = measure %q
    ret %r
}

; The apply idiom: y ^= f(x) on every branch of x, with f's history un-run.
func @copied(q2 %x, q8 %y) {
entry:
    %m = mark
    %v = call @f(%x)
    rewind %m keep (%v -> %y)
    ret
}

func @copy_state() {
entry:
    %x = qalloc 2
    h %x[0]
    h %x[1]
    %y = qalloc 8
    call @copied(%x, %y)
    ret
}

; Compute, copy, uncompute twice: y is 0 again and x is back in |++>, so H H gives 00 every time.
func @copy_twice() -> i2 {
entry:
    %x = qalloc 2
    h %x[0]
    h %x[1]
    %y = qalloc 8
    call @copied(%x, %y)
    call @copied(%x, %y)
    qfree %y
    h %x[0]
    h %x[1]
    %r = measure %x
    ret %r
}
"#;

fn worlds() -> bennett_ir::ValidModule {
    load(WORLDS)
}

fn reversible() -> Options {
    Options { reversible: true, ..bounded() }
}

#[test]
fn reading_a_register_splits_the_world_and_the_host_observes_one_at_the_end() {
    let module = worlds();
    let mut seen = [0; 2];
    for seed in 0..64 {
        let (outcome, executor) = run(&module, "split", &[], Options { seed, ..bounded() });
        let Outcome::Returned(Some(value)) = outcome else { panic!("{outcome:?}") };
        seen[value as usize] += 1;
        let stats = executor.stats();
        assert_eq!((stats.peak_worlds, stats.worlds, stats.splits, stats.observations), (2, 1, 1, 1));
        // The surviving world is the one returned, its configuration the value read, renormalised.
        let state = executor.snapshot();
        assert!(state.worlds.is_empty());
        assert_eq!(state.quantum.amplitudes.len(), 1);
        assert!((state.quantum.amplitude(&[value as u64]) - Complex64::new(1.0, 0.0)).norm() <= TOLERANCE);
    }
    assert!(seen[0] > 16 && seen[1] > 16, "both worlds are observed about half the time: {seen:?}");
}

#[test]
fn a_rewind_merges_the_worlds_and_they_interfere() {
    let module = worlds();
    for seed in 0..32 {
        let (outcome, executor) = run(&module, "interfere", &[], Options { seed, ..bounded() });
        assert_eq!(outcome, Outcome::Returned(Some(0)), "seed {seed}");
        let stats = executor.stats();
        assert_eq!((stats.peak_worlds, stats.splits, stats.merges, stats.observations), (2, 1, 1, 0));
    }
}

#[test]
fn the_apply_idiom_copies_on_every_world_and_leaves_no_trace() {
    let module = worlds();
    let (outcome, executor) = run(&module, "copy_state", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(None));
    let state = executor.snapshot();
    assert!(state.worlds.is_empty(), "the four worlds merged");
    assert_eq!(state.classical.memory[0], 0, "f's store is un-run");
    assert!(state.classical.tape.is_empty(), "f's output is un-printed");
    let f = |x: u64| 3u64.pow(x as u32);
    assert_eq!(state.quantum.amplitudes.len(), 4);
    for x in 0..4 {
        let amplitude = state.quantum.amplitude(&[x, f(x)]);
        assert!((amplitude - Complex64::new(0.5, 0.0)).norm() <= TOLERANCE, "x = {x}: {state:?}");
    }
    let stats = executor.stats();
    assert_eq!((stats.peak_worlds, stats.splits, stats.merges), (4, 3, 3));
    for seed in 0..32 {
        let (outcome, _) = run(&module, "copy_twice", &[], Options { seed, ..bounded() });
        assert_eq!(outcome, Outcome::Returned(Some(0)), "seed {seed}");
    }
}

/// Forward then back returns exactly to the start, through splits and merges: un-running a merge runs
/// the region again, which splits the world again.
#[test]
fn reversal_through_splits_and_merges_restores_the_start() {
    let module = load(&format!("{WORLDS}{MORE}"));
    for name in ["copy_state", "nested_state", "alloc_inside_state", "jumps_state"] {
        let mut executor = executor(&module, name, &[], reversible());
        let at_entry = executor.snapshot();
        let outcome = executor.run();
        assert!(matches!(outcome, Outcome::Returned(_)), "{name}: {outcome:?}");
        assert!(executor.stats().peak_worlds > 1, "{name} splits");
        executor.reverse().unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_same_state(&executor.snapshot(), &at_entry, name);
    }
    // Two worlds that end apart are observed by the host, which reversal cannot cross.
    let mut executor = executor(&module, "split", &[], reversible());
    executor.run();
    assert_eq!(executor.reverse(), Err(ReverseError::PastMeasurement));
}

const MORE: &str = r#"
func @id2(i2 %x) -> i2 {
entry:
    ret %x
}

func @widen(i1 %x) -> i2 {
entry:
    %y = zext i1 %x to i2
    ret %y
}

; Q1's collision: keeping into the register that was read. q ^= q is 0 on both values of q, so the two
; worlds would become one with their amplitudes added.
func @collide(i2 %v) {
entry:
    %q = qalloc 2
    %c = xor i2 %v, 0
    call @prepare(%q, %c)
    %m = mark
    %r = call @id2(%q)
    rewind %m keep (%r -> %q)
    ret
}

func @prepare(q2 %q, i2 %how) {
entry:
    %superposed = icmp eq i2 %how, 3
    br %superposed, both, one
both:
    h %q[0]
    ret
one:
    %bit = trunc i2 %how to i1
    br %bit, set, done
set:
    x %q[0]
    br done
done:
    ret
}

; A kept value into another register that a later gate carries into the read one: o ^= 1, then un-running
; the CNOT from o to q flips q, so the read no longer matches.
func @conjugated_mismatch() {
entry:
    %q = qalloc 1
    %o = qalloc 1
    h %q
    %m = mark
    %r = call @id(%q)
    x %q ctrl %o
    rewind %m keep (i1 1 -> %o)
    ret
}

; The same with the CNOT the other way: X on its target commutes with it, so o ends 1 and q is as it was.
func @conjugated_match() {
entry:
    %q = qalloc 1
    %o = qalloc 1
    h %q
    %m = mark
    %r = call @id(%q)
    x %o ctrl %q
    rewind %m keep (i1 1 -> %o)
    ret
}

; A register allocated, rotated and read inside the region: each world un-runs its own part of the
; allocation; the parts sum to the empty register table again.
func @alloc_inside_state() {
entry:
    %m = mark
    %r = qalloc 1
    h %r
    %v = call @id(%r)
    rewind %m
    ret
}

; acc ^= b, by the apply idiom.
func @inner_copy(q1 %b, q2 %acc) {
entry:
    %m = mark
    %v = call @widen(%b)
    rewind %m keep (%v -> %acc)
    ret
}

; An outer region splits on a, an inner one on b and merges; the outer rewind un-runs the inner rewind,
; which runs the inner region again and splits each world on b again.
func @nested_state() {
entry:
    %a = qalloc 1
    %b = qalloc 1
    %acc = qalloc 2
    %out = qalloc 2
    h %a
    h %b
    %m = mark
    %x = call @widen(%a)
    call @inner_copy(%b, %acc)
    rewind %m keep (%x -> %out)
    ret
}

; Both worlds measure p while still apart: the measurement waits for both, observes one, then measures.
func @measure_apart() -> i2 {
entry:
    %q = qalloc 1
    %p = qalloc 1
    h %q
    h %p
    %v = call @widen(%q)
    %r = measure %p
    %w = zext i1 %r to i2
    %twice = shl i2 %v, 1
    %both = or i2 %twice, %w
    ret %both
}

func @divide(i2 %x) -> i2 {
entry:
    %y = udiv i2 3, %x
    ret %y
}

; Faults in the world where q is 0 only.
func @fault_in_one() -> i2 {
entry:
    %q = qalloc 2
    h %q[0]
    %v = call @divide(%q)
    ret %v
}

; A world-dependent longjmp, un-run by the rewind: (q=0, out=0) and (q=1, out=7).
func @jumper(i64 %buf, i1 %x) -> i1 {
entry:
    br %x, jump, stay
jump:
    longjmp %buf, 7
stay:
    ret 0
}

func @jumps(q1 %q, q8 %out) {
entry:
    %buf = alloca 32
    %m = mark
    %r = setjmp %buf
    %first = icmp eq i32 %r, 0
    br %first, call_it, after
call_it:
    %v = call @jumper(%buf, %q)
    br after
after:
    %r8 = trunc i32 %r to i8
    rewind %m keep (%r8 -> %out)
    ret
}

func @jumps_state() {
entry:
    %q = qalloc 1
    %out = qalloc 8
    h %q
    call @jumps(%q, %out)
    ret
}

; The worlds rewind the same mark at different places, so both go on; then the world at B rewinds it
; again at A, reaching the state the other world left.
func @unaligned() {
entry:
    %q = qalloc 1
    h %q
    %m = mark
    %v = call @id(%q)
    br %v, one, zero
zero:
    rewind %m
    ret
one:
    rewind %m
    br zero
}
"#;

fn more() -> bennett_ir::ValidModule {
    load(&format!("{WORLDS}{MORE}"))
}

/// The one-world states of a run, as (configuration, amplitude), checked against `expected`.
fn assert_amplitudes(snapshot: &Snapshot, expected: &[(&[u64], Complex64)], context: &str) {
    assert!(snapshot.worlds.is_empty(), "{context}: one world");
    assert_eq!(snapshot.quantum.amplitudes.len(), expected.len(), "{context}: {:?}", snapshot.quantum);
    for &(config, amplitude) in expected {
        let found = snapshot.quantum.amplitude(config);
        assert!((found - amplitude).norm() <= TOLERANCE, "{context}: {config:?} is {found}, not {amplitude}");
    }
}

#[test]
fn keeping_into_a_register_the_region_read_faults_and_changes_nothing() {
    let module = more();
    // One world (q holds 1): the read does not split, but un-running it would find q changed.
    let mut one = executor(&module, "collide", &[1], reversible());
    let steps = loop {
        let state = one.snapshot();
        if one.step().is_some() {
            break state;
        }
    };
    assert_eq!(one.outcome(), Some(Outcome::Faulted(Fault::BadRewind)));
    let mut after = one.snapshot();
    after.classical.outcome = None;
    assert_same_state(&after, &steps, "the fault changed nothing");
    // Two worlds (q is |0> + |1>): the world of q = 1 faults; the world of q = 0 keeps 0 and un-runs. By
    // default the fault in one world stops the run; allowed, the host observes either world. Either way
    // the worlds never become one, so no amplitudes are added.
    let (outcome, _) = run(&module, "collide", &[3], bounded());
    let expected = Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, worlds: 2 });
    assert_eq!(outcome, expected);
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..32 {
        let (outcome, executor) = run(&module, "collide", &[3], Options { seed, ..common::allowing_region_faults() });
        assert_eq!(executor.stats().merges, 0);
        seen.insert(format!("{outcome:?}"));
    }
    let expected = [Outcome::Faulted(Fault::BadRewind), Outcome::Returned(None)];
    assert_eq!(seen, expected.iter().map(|o| format!("{o:?}")).collect());
}

#[test]
fn a_kept_value_that_a_gate_carries_into_a_read_register_faults_and_is_restored() {
    let module = more();
    for seed in 0..8 {
        let options = Options { seed, ..common::allowing_region_faults() };
        let (outcome, executor) = run(&module, "conjugated_mismatch", &[], options);
        assert_eq!(outcome, Outcome::Faulted(Fault::BadRewind));
        // The observed world stands at the rewind as it was: q as read, o untouched.
        let state = executor.snapshot();
        assert_eq!(state.classical.frames[0].inst, 6, "at the rewind");
        assert_eq!(state.quantum.amplitudes.len(), 1);
        let (config, amplitude) = &state.quantum.amplitudes[0];
        assert_eq!(config[1], 0, "o untouched: {config:?}");
        assert!((amplitude - Complex64::new(1.0, 0.0)).norm() <= TOLERANCE);
        assert_eq!(state.classical.slots[3], u128::from(config[0]), "the value read is what q holds");
    }
    // The other direction carries nothing into q: the keep is X on o, conjugated by the CNOT.
    let (outcome, executor) = run(&module, "conjugated_match", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(None));
    let h = Complex64::new(FRAC_1_SQRT_2, 0.0);
    assert_amplitudes(&executor.snapshot(), &[(&[0, 1], h), (&[1, 1], h)], "conjugated_match");
}

#[test]
fn a_register_allocated_inside_the_region_is_un_allocated_part_by_part() {
    let module = more();
    let (outcome, executor) = run(&module, "alloc_inside_state", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(None));
    let state = executor.snapshot();
    assert_amplitudes(&state, &[(&[], Complex64::new(1.0, 0.0))], "alloc_inside");
    assert_eq!(executor.stats().merges, 1);
}

#[test]
fn an_outer_rewind_runs_an_inner_merged_region_again_and_splits_again() {
    let module = more();
    let (outcome, executor) = run(&module, "nested_state", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(None));
    // a, b, acc, out: acc is back to 0 (the inner keep is un-run), out holds a.
    let half = Complex64::new(0.5, 0.0);
    let expected: Vec<(Vec<u64>, Complex64)> = (0..4).map(|ab: u64| (vec![ab & 1, ab >> 1, 0, ab & 1], half)).collect();
    let expected: Vec<(&[u64], Complex64)> = expected.iter().map(|(c, a)| (c.as_slice(), *a)).collect();
    assert_amplitudes(&executor.snapshot(), &expected, "nested");
    let stats = executor.stats();
    assert!(stats.rerun_steps > 0, "the inner region ran again: {stats:?}");
    // Outer split (1) and inner splits in both worlds (2), and the inner splits again when run again (2).
    assert_eq!((stats.splits, stats.merges, stats.peak_worlds), (5, 5, 4), "{stats:?}");
}

#[test]
fn a_measurement_while_worlds_are_apart_observes_one_of_them() {
    let module = more();
    let mut counts = [0usize; 4];
    for seed in 0..400 {
        let (outcome, executor) = run(&module, "measure_apart", &[], Options { seed, ..bounded() });
        let Outcome::Returned(Some(value)) = outcome else { panic!("{outcome:?}") };
        counts[value as usize] += 1;
        assert_eq!(executor.stats().observations, 1, "the measurement observed which world; the end did not");
    }
    assert!(counts.iter().all(|&count| (70..=130).contains(&count)), "uniform over q and p: {counts:?}");
}

#[test]
fn a_fault_in_some_worlds_stops_the_run_unless_allowed() {
    let module = more();
    let (outcome, _) = run(&module, "fault_in_one", &[], bounded());
    assert_eq!(outcome, Outcome::Stopped(RunError::WorldFault { fault: Fault::DivideByZero, worlds: 2 }));
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..32 {
        let (outcome, _) = run(&module, "fault_in_one", &[], Options { seed, ..common::allowing_region_faults() });
        seen.insert(format!("{outcome:?}"));
    }
    let expected = [Outcome::Faulted(Fault::DivideByZero), Outcome::Returned(Some(3))];
    assert_eq!(seen, expected.iter().map(|o| format!("{o:?}")).collect(), "the host observes either world");
}

#[test]
fn a_jump_in_one_world_is_un_run_by_the_rewind() {
    let module = more();
    let (outcome, executor) = run(&module, "jumps_state", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(None));
    let h = Complex64::new(FRAC_1_SQRT_2, 0.0);
    assert_amplitudes(&executor.snapshot(), &[(&[0, 0], h), (&[1, 7], h)], "jumps");
}

#[test]
fn the_budget_counts_ticks_of_all_worlds() {
    let module = worlds();
    let (_, full) = run(&module, "copy_state", &[], bounded());
    let ticks = full.stats().steps;
    assert!(full.stats().world_steps > ticks, "several worlds stepped in some ticks: {:?}", full.stats());
    let (outcome, executor) = run(&module, "copy_state", &[], Options { budget: Some(ticks - 1), ..bounded() });
    assert_eq!(outcome, Outcome::Faulted(Fault::Budget));
    let _ = executor;
    let (outcome, _) = run(&module, "copy_state", &[], Options { budget: Some(ticks), ..bounded() });
    assert_eq!(outcome, Outcome::Returned(None));
}

#[test]
fn a_world_that_rewinds_into_a_state_another_world_left_stops_the_run() {
    let module = more();
    let (outcome, _) = run(&module, "unaligned", &[], bounded());
    assert_eq!(outcome, Outcome::Stopped(RunError::Unaligned));
}

const COSTS: &str = r#"
memory 4096

; Ordinary work that reads x: d + 4 steps whatever x is, so the worlds stay in step.
func @work(i1 %x, i8 %d) -> i8 {
entry:
    %a = add i8 %d, 1
    %b = mul i8 %a, 3
    %w = zext i1 %x to i8
    %c = xor i8 %b, %w
    ret %c
}

; Cleaning up at every level, each level reading x: two worlds from the outermost read on.
func @clean(q1 %x, q8 %out, i8 %d) {
entry:
    %h = mark
    %w = call @work(%x, %d)
    %leaf = icmp eq i8 %d, 0
    br %leaf, done, deeper
deeper:
    %d1 = sub i8 %d, 1
    call @clean(%x, %out, %d1)
    br done
done:
    rewind %h keep (%w -> %out)
    ret
}

func @open(q1 %x, i8 %d) -> i8 {
entry:
    %w = call @work(%x, %d)
    %leaf = icmp eq i8 %d, 0
    br %leaf, done, deeper
deeper:
    %d1 = sub i8 %d, 1
    %ignored = call @open(%x, %d1)
    br done
done:
    ret %w
}

func @top_clean(i8 %d) {
entry:
    %x = qalloc 1
    h %x
    %out = qalloc 8
    call @clean(%x, %out, %d)
    ret
}

func @top_open(i8 %d) {
entry:
    %x = qalloc 1
    h %x
    %out = qalloc 8
    %h = mark
    %w = call @open(%x, %d)
    rewind %h keep (%w -> %out)
    ret
}

; n iterations of the apply idiom on x: the work per iteration must not grow with n.
func @repeat(i16 %n) {
entry:
    %x = qalloc 1
    h %x
    %out = qalloc 8
    br head(i16 0)
head(i16 %i):
    %more = icmp ult i16 %i, %n
    br %more, body, done
body:
    %h = mark
    %w = call @work(%x, i8 5)
    rewind %h keep (%w -> %out)
    %next = add i16 %i, 1
    br head(%next)
done:
    ret
}
"#;

/// The known exponential with worlds (`docs/DESIGN.md`, "Worlds"): every level reads x, so from the
/// outermost read on there are two worlds, and each level's rewind merges them. Cleaning up at every
/// level still doubles the work per level; leaving the callees' history open until the outermost rewind
/// keeps it linear. Ticks, steps of all worlds, steps un-run and run again, and peak journal bytes.
#[test]
fn with_worlds_cleaning_up_at_every_level_still_doubles_the_work() {
    let module = load(COSTS);
    let work = |name: &str, d: u64| {
        let (outcome, executor) = run(&module, name, &[d], Options { budget: None, reversible: true, ..bounded() });
        assert_eq!(outcome, Outcome::Returned(None), "{name}({d})");
        let state = executor.snapshot();
        assert!(state.worlds.is_empty(), "{name}({d}): merged");
        // x stays in |+>, out holds 3(d + 1) XOR x.
        let y = |x: u64| (3 * (d + 1) % 256) ^ x;
        let h = Complex64::new(FRAC_1_SQRT_2, 0.0);
        assert_amplitudes(&state, &[(&[0, y(0)], h), (&[1, y(1)], h)], name);
        let stats = executor.stats();
        (stats.steps, stats.world_steps, stats.rewound_steps, stats.rerun_steps, stats.peak_worlds, stats.splits)
    };
    let mut clean = Vec::new();
    let mut open = Vec::new();
    for d in 0..=8 {
        clean.push(work("top_clean", d));
        open.push(work("top_open", d));
    }
    let total = |&(_, steps, rewound, rerun, _, _): &(u64, u64, u64, u64, usize, u64)| steps + rewound + rerun;
    for d in 1..=8 {
        let (c0, c1) = (total(&clean[d - 1]), total(&clean[d]));
        assert_eq!(c1, 2 * c0 + 47, "cleaning up: level {d} costs twice level {} and 47 more", d - 1);
        let (o0, o1) = (total(&open[d - 1]), total(&open[d]));
        assert_eq!(o1 - o0, 48, "open history: a constant cost per level");
    }
    assert_eq!(clean[8], CLEAN_AT_8, "measured, recorded in docs/DESIGN.md");
    assert_eq!(open[8], OPEN_AT_8, "measured, recorded in docs/DESIGN.md");
}

/// Ticks, steps of all worlds, un-run, run again, peak worlds, splits, at depth 8: 22,481 steps of work in
/// all against 430 with the history left open.
const CLEAN_AT_8: (u64, u64, u64, u64, usize, u64) = (128, 249, 11_236, 10_996, 2, 1);
const OPEN_AT_8: (u64, u64, u64, u64, usize, u64) = (112, 218, 212, 0, 2, 1);

/// A mark copies nothing and a split copies one world: the work and the journal per iteration of the
/// apply idiom on a superposed register are the same at 50 and 1,500 iterations.
#[test]
fn with_worlds_the_work_per_iteration_does_not_grow_with_the_run() {
    let module = load(COSTS);
    let per_iteration = |n: u64| {
        let (outcome, executor) = run(&module, "repeat", &[n], Options { budget: None, reversible: true, ..bounded() });
        assert_eq!(outcome, Outcome::Returned(None));
        let stats = executor.stats();
        assert_eq!((stats.splits, stats.merges), (n, n), "one split and one merge per iteration");
        let bytes = stats.journal_bytes.expect("reversible") as f64;
        let work = (stats.world_steps + stats.rewound_steps + stats.rerun_steps) as f64;
        (work / n as f64, bytes / n as f64)
    };
    let (short, long) = (per_iteration(50), per_iteration(1500));
    assert!(long.0 <= short.0 * 1.01 && long.0 >= short.0 * 0.99, "work per iteration {short:?} vs {long:?}");
    assert!(long.1 <= short.1 * 1.05, "journal bytes per iteration {short:?} vs {long:?}");
}

#[test]
fn replaying_a_reversed_register_split_still_refuses_a_dirty_free() {
    let module = load(
        "memory 64
func @read(i1 %x) {
entry:
    ret
}
func @main() {
entry:
    %dirty = qalloc 1
    x %dirty
    %q = qalloc 1
    h %q
    call @read(%q)
    qfree %dirty
    ret
}
",
    );
    let mut executor = executor(&module, "main", &[], reversible());
    for _ in 0..4 {
        assert_eq!(executor.step(), None);
    }
    let before = executor.snapshot();
    assert_eq!(executor.step(), None);
    assert_eq!(executor.stats().worlds, 2);
    assert_eq!(executor.step_back(), Ok(true));
    assert_eq!(executor.snapshot(), before, "undoing the call restores its full coherent input");
    assert_eq!(
        executor.run(),
        Outcome::Stopped(RunError::WorldFault { fault: Fault::DirtyFree, worlds: 2 }),
        "a host replay must still refuse to discard the register holding 1"
    );
}
