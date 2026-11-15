//! `mark` and `rewind`: what a rewind keeps, what it un-runs, how it is itself undone, when it faults,
//! and what it costs. The brute-force check that every journalled step is a bijection of (machine state,
//! history) is a unit test of the crate (`permutation_tests`), because it builds arbitrary states.

mod common;

use std::f64::consts::FRAC_1_SQRT_2;

use bennett_exec::{Options, Outcome, ReverseError, Stats};
use bennett_ir::Fault;
use common::{TOLERANCE, assert_reverses, assert_same_state, bounded, executor, executor as executor_of, load, run};
use num_complex::Complex64;

const HISTORY: &str = include_str!("../../../examples/history.bn");

/// Entry points that leave the example's states unmeasured, so that a run can be reversed.
const UNMEASURED: &str = r#"
func @copy_state(i8 %x) -> i8 {
entry:
    %out = qalloc 8
    %cell = call @copied(%out, %x)
    ret %cell
}

func @conjugate_state() {
entry:
    %q = qalloc 1
    call @conjugated(%q)
    ret
}

func @nest_state() {
entry:
    %out = qalloc 8
    call @nested(%out)
    ret
}
"#;

fn history() -> bennett_ir::ValidModule {
    load(&format!("{HISTORY}{UNMEASURED}"))
}

#[test]
fn a_rewind_keeps_the_result_and_erases_the_call() {
    let module = history();
    for x in [0u64, 1, 7, 85, 255] {
        let y = (3 * x + 1) % 256;
        let (outcome, _) = run(&module, "copy", &[x], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(y << 8))), "x = {x}: f(x) kept, memory clean");
        let (outcome, executor) = run(&module, "copy_state", &[x], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(0)), "x = {x}: memory clean");
        assert_eq!(executor.stats().rewound_steps, 5, "the call, f's three instructions and its return");
        let state = executor.snapshot().quantum;
        assert_eq!(state.amplitudes.len(), 1);
        assert!((state.amplitude(&[y]) - Complex64::new(1.0, 0.0)).norm() <= TOLERANCE, "x = {x}: {state:?}");
        assert_reverses(&module, "copy_state", &[x], bounded());
    }
}

#[test]
fn un_running_through_a_gate_conjugates() {
    let module = history();
    for seed in 0..32 {
        let (outcome, _) = run(&module, "conjugate", &[], Options { seed, ..bounded() });
        assert_eq!(outcome, Outcome::Returned(Some(1)), "seed {seed}");
    }
    assert_reverses(&module, "conjugate_state", &[], bounded());
}

#[test]
fn an_outer_rewind_un_runs_an_inner_one() {
    let module = history();
    assert_eq!(run(&module, "nest", &[], bounded()).0, Outcome::Returned(Some(3)));
    let (_, executor) = run(&module, "nest_state", &[], bounded());
    let stats = executor.stats();
    // The outer rewind undoes the inner rewind (running its empty region again) and the inner mark.
    assert_eq!((stats.rewound_steps, stats.rerun_steps), (2, 0));
    assert_reverses(&module, "nest_state", &[], bounded());
}

/// Conjugation against states written out by hand: mark, G, rewind keep (1 -> q) is G† X G.
#[test]
fn conjugated_xors_match_hand_computed_states() {
    let text = r#"
memory 64
func @hxh() {
entry:
    %q = qalloc 1
    ry(0.8) %q
    %h = mark
    h %q
    rewind %h keep (i1 1 -> %q)
    ret
}
; S† X S: |0> -> S† X |0> = S† |1> = -i|1>.
func @sxs() {
entry:
    %q = qalloc 1
    %h = mark
    s %q
    rewind %h keep (i1 1 -> %q)
    ret
}
; Controlled: on a = |+>, b = |0>: mark, H on b controlled by a, XOR 1 into b, un-run.
; a=0: b ^= 1 -> |0,1>; a=1: H X H = Z on |0> -> |1,0>. Result (|0,1> + |1,0>)/√2.
func @controlled() {
entry:
    %a = qalloc 1
    %b = qalloc 1
    h %a
    %h = mark
    h %b ctrl %a
    rewind %h keep (i1 1 -> %b)
    ret
}
"#;
    let module = load(text);
    let state = |name: &str| {
        let (outcome, executor) = run(&module, name, &[], bounded());
        assert_eq!(outcome, Outcome::Returned(None), "{name}");
        assert_reverses(&module, name, &[], bounded());
        executor.snapshot().quantum
    };
    let close = |a: Complex64, b: Complex64| (a - b).norm() <= TOLERANCE;
    // ry(0.8)|0> = cos 0.4 |0> + sin 0.4 |1>; Z flips the sign of |1>.
    let hxh = state("hxh");
    assert!(close(hxh.amplitude(&[0]), Complex64::new(0.4f64.cos(), 0.0)));
    assert!(close(hxh.amplitude(&[1]), Complex64::new(-(0.4f64.sin()), 0.0)), "{hxh:?}");
    let sxs = state("sxs");
    assert!(close(sxs.amplitude(&[1]), Complex64::new(0.0, -1.0)), "{sxs:?}");
    let controlled = state("controlled");
    let half = Complex64::new(FRAC_1_SQRT_2, 0.0);
    assert!(close(controlled.amplitude(&[0, 1]), half) && close(controlled.amplitude(&[1, 0]), half), "{controlled:?}");
    assert_eq!(controlled.amplitudes.len(), 2);
}

const RUNS: &str = r#"
memory 256

func @effects() -> i8 {
entry:
    effect 65
    %h = mark
    effect 66
    %c = alloca 8
    store i8 9, %c
    rewind %h
    effect 67
    ret 0
}

; Rewinds in a loop and across blocks: the mark is in one block, the rewind in another.
func @loop_body(q8 %out, i8 %n) {
entry:
    br head(i8 0)
head(i8 %i):
    %more = icmp ult i8 %i, %n
    br %more, body, done
body:
    %h = mark
    %twice = add i8 %i, %i
    br tail(%twice)
tail(i8 %v):
    rewind %h keep (%v -> %out)
    %next = add i8 %i, 1
    br head(%next)
done:
    ret
}

func @loop(i8 %n) -> i8 {
entry:
    %out = qalloc 8
    call @loop_body(%out, %n)
    %m = measure %out
    ret %m
}

func @loop_state(i8 %n) {
entry:
    %out = qalloc 8
    call @loop_body(%out, %n)
    ret
}

; A rewind in a region: allowed without keeps.
func @inner(i8 %x) -> i8 {
entry:
    %h = mark
    %y = add i8 %x, 1
    rewind %h
    %z = add i8 %x, 2
    ret %z
}

func @region() -> i8 {
entry:
    %out = qalloc 8
    %e = qalloc 4
    apply @inner(i8 4) -> %out, %e budget 100
    %m = measure %out
    ret %m
}

func @keep_in_region_body(i8 %x) -> i8 {
entry:
    %r = qalloc 8
    %h = mark
    rewind %h keep (%x -> %r)
    ret %x
}

func @keep_in_region() -> i8 {
entry:
    %out = qalloc 8
    %e = qalloc 4
    apply @keep_in_region_body(i8 4) -> %out, %e budget 100
    %m = measure %e
    %wide = zext i4 %m to i8
    ret %wide
}
"#;

#[test]
fn a_rewind_un_prints_console_output_and_frees_stack() {
    let module = load(RUNS);
    let (outcome, executor) = run(&module, "effects", &[], bounded());
    assert_eq!(outcome, Outcome::Returned(Some(0)));
    assert_eq!(executor.tape(), b"AC");
    assert_eq!(executor.snapshot().classical.stack_pointer, 256 - 8 * 2, "the alloca is undone, two slots stay");
    assert_reverses(&module, "effects", &[], bounded());
}

#[test]
fn rewinds_in_a_loop_xor_every_kept_value() {
    let module = load(RUNS);
    for n in [0u64, 1, 2, 5, 9] {
        let expected = (0..n).fold(0, |acc, i| acc ^ ((2 * i) % 256));
        let (outcome, _) = run(&module, "loop", &[n], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(expected))), "n = {n}");
        let (_, executor) = run(&module, "loop_state", &[n], bounded());
        let state = executor.snapshot().quantum;
        assert!((state.amplitude(&[expected]) - Complex64::new(1.0, 0.0)).norm() <= TOLERANCE, "n = {n}");
        assert_reverses(&module, "loop_state", &[n], bounded());
    }
}

#[test]
fn a_rewind_runs_inside_a_region_but_keeps_nothing_there() {
    let module = load(RUNS);
    assert_eq!(run(&module, "region", &[], bounded()).0, Outcome::Returned(Some(6)));
    let options = Options { allow_region_faults: true, ..bounded() };
    let (outcome, _) = run(&module, "keep_in_region", &[], options);
    assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))));
}

/// A run started without a journal keeps one from its first mark, but still cannot be reversed.
#[test]
fn a_run_that_is_not_reversible_rewinds_but_does_not_reverse() {
    let module = history();
    let mut plain = executor(&module, "copy", &[7], bounded());
    assert_eq!(plain.stats().journal_bytes, None);
    assert_eq!(plain.run(), Outcome::Returned(Some(22 << 8)));
    assert!(plain.stats().journal_bytes.is_some());
    assert_eq!(plain.step_back(), Err(ReverseError::NotReversible));
    let (reversible, _) = run(&module, "copy", &[7], Options { reversible: true, ..bounded() });
    assert_eq!(reversible, Outcome::Returned(Some(22 << 8)));
}

const FAULTS: &str = r#"
memory 256

; The second rewind's handle is stale: the outer rewind restored %h2 to what it held before its mark (0).
func @stale() {
entry:
    %h = mark
    %h2 = mark
    rewind %h
    rewind %h2
    ret
}

; A measurement since the mark cannot be un-run.
func @measured() {
entry:
    %q = qalloc 1
    %h = mark
    %m = measure %q
    rewind %h
    ret
}

; The kept value would go into a register allocated after the mark, whose allocation the rewind undoes.
func @young() {
entry:
    %h = mark
    %r = qalloc 2
    rewind %h keep (i2 1 -> %r)
    ret
}

; A handle of another activation: the rewind of %g restores %h to what its slot held before, which is
; @k's %b (both frames start at the same place), a mark of an activation that has returned.
func @k() {
entry:
    %a = mark
    %b = mark
    ret
}

func @m() {
entry:
    %g = mark
    %h = mark
    rewind %g
    rewind %h
    ret
}

func @other() {
entry:
    call @k()
    call @m()
    ret
}

func @dangling() {
entry:
    %r = qalloc 2
    %h = mark
    qfree %r
    rewind %h keep (i2 1 -> %r)
    ret
}

; Rewinding the same mark twice is allowed. The second rewind un-runs everything since the mark,
; including the first rewind and its keep: %r ends as 2, not 1 ^ 2.
func @twice() {
entry:
    %r = qalloc 2
    %h = mark
    rewind %h keep (i2 1 -> %r)
    rewind %h keep (i2 2 -> %r)
    ret
}
"#;

#[test]
fn rewinds_that_cannot_un_run_fault_and_change_nothing() {
    let module = load(FAULTS);
    for (name, fault) in [
        ("stale", Fault::BadRewind),
        ("measured", Fault::BadRewind),
        ("young", Fault::BadRewind),
        ("other", Fault::BadRewind),
        ("dangling", Fault::DanglingRegister),
    ] {
        let mut executor = executor(&module, name, &[], Options { reversible: true, ..bounded() });
        let mut before;
        let outcome = loop {
            before = executor.snapshot();
            if let Some(outcome) = executor.step() {
                break outcome;
            }
        };
        assert_eq!(outcome, Outcome::Faulted(fault), "{name}");
        let mut after = executor.snapshot();
        after.classical.outcome = None;
        assert_same_state(&after, &before, name);
    }
    let (_, executor) = run(&module, "twice", &[], bounded());
    assert!((executor.snapshot().quantum.amplitude(&[2]) - Complex64::new(1.0, 0.0)).norm() <= TOLERANCE);
    assert_reverses(&module, "twice", &[], bounded());
}

/// `steps` counts each rewind as one step and never goes back; the work of un-running is counted apart.
#[test]
fn steps_only_go_forward_and_un_running_is_counted_apart() {
    let module = history();
    let (_, executor) = run(&module, "copy_state", &[1], bounded());
    let Stats { steps, rewound_steps, rerun_steps, .. } = executor.stats();
    // qalloc, call; mark, call, f's three instructions and return, rewind, load, ret; ret.
    assert_eq!((steps, rewound_steps, rerun_steps), (12, 5, 0));
    let mut reversible = executor_of(&module, "copy_state", &[1], Options { reversible: true, ..bounded() });
    reversible.run();
    assert_eq!(reversible.reverse(), Ok(12), "reversal undoes each step once; a rewind's undo runs its region");
    assert_eq!(reversible.stats().rerun_steps, 5);
}

const COSTS: &str = r#"
memory 4096

; Some ordinary work: d + 4 steps.
func @work(i8 %d) -> i8 {
entry:
    %a = add i8 %d, 1
    %b = mul i8 %a, 3
    ret %b
}

; Cleaning up at every level: each level marks, works, calls the next level (which cleans up after
; itself), and rewinds keeping its work in %out.
func @clean(q8 %out, i8 %d) {
entry:
    %h = mark
    %w = call @work(%d)
    %leaf = icmp eq i8 %d, 0
    br %leaf, done, deeper
deeper:
    %d1 = sub i8 %d, 1
    call @clean(%out, %d1)
    br done
done:
    rewind %h keep (%w -> %out)
    ret
}

; The same calls with the callees' history left open: only the outermost level rewinds.
func @open(i8 %d) -> i8 {
entry:
    %w = call @work(%d)
    %leaf = icmp eq i8 %d, 0
    br %leaf, done, deeper
deeper:
    %d1 = sub i8 %d, 1
    %ignored = call @open(%d1)
    br done
done:
    ret %w
}

func @top_clean(i8 %d) -> i8 {
entry:
    %out = qalloc 8
    call @clean(%out, %d)
    %m = measure %out
    ret %m
}

func @top_open(i8 %d) -> i8 {
entry:
    %out = qalloc 8
    %h = mark
    %w = call @open(%d)
    rewind %h keep (%w -> %out)
    %m = measure %out
    ret %m
}

; A loop of n mark, call, rewind: the cost of each iteration must not grow with n.
func @repeat(i64 %n) {
entry:
    %out = qalloc 8
    br head(i64 0)
head(i64 %i):
    %more = icmp ult i64 %i, %n
    br %more, body, done
body:
    %h = mark
    %w = call @work(i8 7)
    rewind %h keep (%w -> %out)
    %next = add i64 %i, 1
    br head(%next)
done:
    ret
}
"#;

/// The known exponential (`docs/DESIGN.md`): cleaning up inside every call, inside callers that also
/// clean up, doubles the work per level, because un-running a rewind means running its region again.
/// Keeping the callees' history open until the outermost rewind keeps the work linear. Exact counts,
/// so a change in either shows here.
#[test]
fn cleaning_up_at_every_level_doubles_the_work_per_level() {
    let module = load(COSTS);
    let work = |name: &str, d: u64| {
        let (outcome, executor) = run(&module, name, &[d], Options { budget: None, ..bounded() });
        let expected = (u128::from(d) + 1) * 3 % 256;
        assert_eq!(outcome, Outcome::Returned(Some(expected)), "{name}({d}): both keep the outermost work");
        let stats = executor.stats();
        (stats.steps, stats.rewound_steps, stats.rerun_steps)
    };
    let mut clean = Vec::new();
    let mut open = Vec::new();
    for d in 0..=10 {
        clean.push(work("top_clean", d));
        open.push(work("top_open", d));
    }
    let total = |&(steps, rewound, rerun): &(u64, u64, u64)| steps + rewound + rerun;
    for d in 1..=10 {
        let (c0, c1) = (total(&clean[d - 1]), total(&clean[d]));
        assert!(c1 >= 2 * c0 && c1 <= 3 * c0, "cleaning up: level {d} costs {c1}, level {} {c0}", d - 1);
        let (o0, o1) = (total(&open[d - 1]), total(&open[d]));
        assert_eq!(o1 - o0, total(&open[1]) - total(&open[0]), "open history: a constant cost per level");
    }
    assert_eq!(clean[10], CLEAN_AT_10, "measured, recorded in docs/DESIGN.md");
    assert_eq!(open[10], OPEN_AT_10, "measured, recorded in docs/DESIGN.md");
}

/// Steps, steps undone by rewinds, steps run again: at depth 10, 36,847 in all against 221.
const CLEAN_AT_10: (u64, u64, u64) = (133, 18_420, 18_294);
const OPEN_AT_10: (u64, u64, u64) = (113, 108, 0);

/// The shape of the quadratic the earlier executor had (`docs/SALVAGE.md` §3.3): if a mark copied the
/// journal, the work per iteration would grow with the run. It is the same at 50 and 1,500 iterations.
#[test]
fn the_work_per_iteration_does_not_grow_with_the_run() {
    let module = load(COSTS);
    let per_iteration = |n: u64| {
        let (outcome, executor) = run(&module, "repeat", &[n], Options { budget: None, reversible: true, ..bounded() });
        assert_eq!(outcome, Outcome::Returned(None));
        let stats = executor.stats();
        let bytes = stats.journal_bytes.expect("reversible") as f64;
        ((stats.steps + stats.rewound_steps + stats.rerun_steps) as f64 / n as f64, bytes / n as f64)
    };
    let (short, long) = (per_iteration(50), per_iteration(1500));
    assert!(long.0 <= short.0 * 1.01 && long.0 >= short.0 * 0.99, "work per iteration {short:?} vs {long:?}");
    assert!(long.1 <= short.1 * 1.05, "journal bytes per iteration {short:?} vs {long:?}");
}
