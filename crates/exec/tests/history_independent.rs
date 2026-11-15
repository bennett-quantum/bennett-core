//! Independent probes of DESIGN.md “History: mark and rewind” and SYNTAX.md “History”.
mod common;

use bennett_exec::{Executor, Options, Outcome, ReverseError, Snapshot};
use bennett_ir::{Fault, Inst};
use common::{assert_reverses, assert_same_state, bounded, executor, load};
use num_complex::Complex64 as C;

fn options() -> Options {
    Options { reversible: true, ..bounded() }
}
fn at_rewind(e: &mut Executor<'_>, module: &bennett_ir::ValidModule) {
    for _ in 0..1000 {
        let s = e.snapshot();
        let f = s.classical.frames.last().expect("frame");
        if matches!(module.function(f.func).block(f.block).insts.get(f.inst), Some(Inst::Rewind { .. })) {
            return;
        }
        assert_eq!(e.step(), None);
    }
    panic!("rewind not reached");
}
fn same_except_steps(mut a: Snapshot, b: &Snapshot) {
    a.classical.steps = b.classical.steps;
    assert_same_state(&a, b, "forward replay");
}

#[test]
fn nested_gate_rewinds_and_rewind_of_rewind_replay_every_snapshot() {
    let module = load(
        r#"
memory 1024
func @callee(q2 %q) {
entry:
    %i = mark
    ry(0.47) %q[0]
    x %q[1] ctrl %q[0]
    rewind %i keep (i2 1 -> %q)
    ret
}
func @main() {
entry:
    %q = qalloc 2
    h %q[1]
    %outer = mark
    call @callee(%q)
    %inner = mark
    s %q[1]
    rewind %inner keep (i2 2 -> %q)
    rewind %inner keep (i2 3 -> %q)
    rewind %outer keep (i2 1 -> %q)
    ret
}
"#,
    );
    let mut e = executor(&module, "main", &[], options());
    let mut snapshots = vec![e.snapshot()];
    while e.step().is_none() {
        snapshots.push(e.snapshot());
    }
    snapshots.push(e.snapshot());
    for expected in snapshots[..snapshots.len() - 1].iter().rev() {
        assert_eq!(e.step_back(), Ok(true));
        assert_same_state(&e.snapshot(), expected, "each inverse step, including rewinds");
    }
    assert_eq!(e.step_back(), Ok(false));
}

#[test]
fn aliased_keep_parameters_and_all_width_boundaries_xor_values_read_now() {
    for width in [1, 2, 7, 8, 31, 32, 63, 64] {
        let max = if width == 64 { u64::MAX } else { (1 << width) - 1 };
        let text = format!(
            r#"
memory 512
func @keep(q{width} %a, q{width} %b, i{width} %v) {{
entry:
    %h = mark
    %w = xor i{width} %v, {max}
    rewind %h keep (%v -> %a, %w -> %b, i{width} {max} -> %a)
    ret
}}
func @main(i{width} %v) {{
entry:
    %q = qalloc {width}
    x %q[0]
    call @keep(%q, %q, %v)
    ret
}}
"#
        );
        let module = load(&text);
        for v in [0, 1, max / 2, max] {
            let mut e = executor(&module, "main", &[v], options());
            assert_eq!(e.run(), Outcome::Returned(None));
            assert_eq!(e.snapshot().quantum.amplitudes, vec![(vec![1], C::new(1.0, 0.0))], "width {width}, v {v}");
            assert_reverses(&module, "main", &[v], options());
        }
    }
}

#[test]
fn every_keep_is_checked_before_any_xor_or_unrun() {
    for (setup, expected) in
        [("%young = qalloc 1", Fault::BadRewind), ("qfree %q\n%young = qalloc 1", Fault::BadRewind)]
    {
        let module = load(&format!(
            "memory 512\nfunc @main() {{\nentry:\n%q = qalloc 1\n%r = qalloc 1\n%h = mark\neffect 65\n{setup}\nrewind %h keep (i1 1 -> %r, i1 1 -> %young)\nret\n}}"
        ));
        let mut e = executor(&module, "main", &[], options());
        at_rewind(&mut e, &module);
        let before = e.snapshot();
        let bytes = e.stats().journal_bytes;
        assert_eq!(e.step(), Some(Outcome::Faulted(expected)));
        let mut after = e.snapshot();
        after.classical.outcome = None;
        assert_same_state(&after, &before, "no partial first keep");
        assert_eq!(e.stats().journal_bytes, bytes);
    }
    let module = load(
        "memory 512\nfunc @main() {\nentry:\n%q = qalloc 1\n%r = qalloc 1\n%h = mark\nqfree %q\nrewind %h keep (i1 1 -> %r, i1 1 -> %q)\nret\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    at_rewind(&mut e, &module);
    let before = e.snapshot();
    assert_eq!(e.step(), Some(Outcome::Faulted(Fault::DanglingRegister)));
    let mut after = e.snapshot();
    after.classical.outcome = None;
    assert_same_state(&after, &before, "dangling second keep");
}

#[test]
fn fault_and_exit_between_mark_and_rewind_stop_first_and_reverse() {
    for (term, expected) in [
        ("fault unreachable", Outcome::Faulted(Fault::Unreachable)),
        ("%d = udiv i8 3, 0\nret %d", Outcome::Faulted(Fault::DivideByZero)),
        ("exit i8 9", Outcome::Returned(Some(9))),
    ] {
        let module = load(&format!(
            "memory 512\nfunc @f() -> i8 {{\nentry:\neffect 66\nstore i8 42, 16\n{term}\n}}\nfunc @main() -> i8 {{\nentry:\n%h = mark\n%r = call @f()\nrewind %h\nret 0\n}}"
        ));
        let mut e = executor(&module, "main", &[], options());
        let initial = e.snapshot();
        assert_eq!(e.run(), expected);
        assert_eq!(e.tape(), b"B");
        assert_eq!(e.snapshot().classical.memory[16], 42);
        assert_eq!(e.stats().rewound_steps, 0);
        e.reverse().expect("exit/fault under mark reverses");
        assert_same_state(&e.snapshot(), &initial, "marked failure");
    }
}

#[test]
fn faults_and_effects_inside_applied_marked_regions_leave_no_trace() {
    for (body, expected) in [
        ("effect 65\nret 3", Fault::EffectInRegion),
        ("exit i2 3", Fault::ExitInRegion),
        ("fault unreachable", Fault::Unreachable),
        ("%d = udiv i2 3, 0\nret %d", Fault::DivideByZero),
    ] {
        let module = load(&format!(
            "memory 1024\nfunc @f() -> i2 {{\nentry:\n%h = mark\nstore i8 27, 16\n{body}\n}}\nfunc @main() {{\nentry:\n%out = qalloc 2\n%e = qalloc 4\n%h = mark\napply @f() -> %out, %e budget 20\nrewind %h keep (i2 2 -> %out)\nret\n}}"
        ));
        let opts = Options { allow_region_faults: true, ..options() };
        let mut e = executor(&module, "main", &[], opts);
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_eq!(e.snapshot().classical.memory[16], 0);
        assert!(e.tape().is_empty());
        assert_eq!(e.snapshot().quantum.amplitudes, vec![(vec![2, 0], C::new(1.0, 0.0))]);
        assert_eq!(e.stats().region_faults, 2, "forward and unrun: {expected:?}");
        assert_eq!(e.stats().region_fault_weight, 2.0);
        assert_reverses(&module, "main", &[], opts);
    }
}

#[test]
fn measuring_before_mark_is_allowed_but_after_mark_is_an_atomic_barrier() {
    let module = load(
        "memory 512\nfunc @main() {\nentry:\n%q = qalloc 1\nh %q\n%m = measure %q\n%h = mark\nx %q\nrewind %h\nret\n}",
    );
    for seed in 0..16 {
        let mut e = executor(&module, "main", &[], Options { seed, ..options() });
        for _ in 0..3 {
            assert_eq!(e.step(), None);
        }
        let measured = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_eq!(e.reverse(), Err(ReverseError::PastMeasurement));
        assert_same_state(&e.snapshot(), &measured, "barrier restored");
    }
    let module = load(
        "memory 512\nfunc @main() {\nentry:\n%q = qalloc 1\n%h = mark\nh %q\n%m = measure %q\nrewind %h keep (i1 1 -> %q)\nret\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    at_rewind(&mut e, &module);
    let before = e.snapshot();
    assert_eq!(e.step(), Some(Outcome::Faulted(Fault::BadRewind)));
    let mut after = e.snapshot();
    after.classical.outcome = None;
    assert_same_state(&after, &before, "measurement barrier");
}

#[test]
fn a_rewind_has_finite_budget_even_in_an_infinite_loop() {
    let module = load("memory 256\nfunc @main() {\nentry:\n%h = mark\nbr loop\nloop:\nrewind %h\nbr loop\n}");
    for budget in [1, 2, 3, 4, 8, 32] {
        let mut e = executor(&module, "main", &[], Options { budget: Some(budget), ..options() });
        assert_eq!(e.run(), Outcome::Faulted(Fault::Budget));
        assert_eq!(e.stats().steps, budget);
        // Repeated dynamic visits reverse fully (F2, fixed); their work is pinned in `history_unrun.rs`.
        assert_eq!(e.reverse(), Ok(budget));
        assert_eq!(e.stats().steps, 0);
    }
}

#[test]
fn journal_content_refuses_rewind_in_a_region_but_observes_marks() {
    let source = "memory 512\nfunc @f() -> i2 {\nentry:\n%h = mark\nREWIND\nret 3\n}\nfunc @main() {\nentry:\n%q = qalloc 2\n%e = qalloc 4\napply @f() -> %q, %e budget 10\nret\n}";
    for raw in [false, true] {
        let module = load(&source.replace("REWIND", ""));
        let mut observed = executor(&module, "main", &[], options());
        observed.measure_journal_content(raw);
        assert_eq!(observed.run(), Outcome::Returned(None));
        let mut plain = executor(&module, "main", &[], options());
        plain.run();
        assert_same_state(&observed.snapshot(), &plain.snapshot(), "mark observer");
        let module = load(&source.replace("REWIND", "rewind %h"));
        let mut observed = executor(&module, "main", &[], options());
        observed.measure_journal_content(raw);
        let panic =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observed.run())).expect_err("must refuse rewind");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic text");
        assert!(message.contains("the journal-content measurement does not follow `rewind`"), "{message}");
    }
}

#[test]
fn text_roundtrip_preserves_execution_of_nested_marks_and_keep_lists() {
    let source = "memory 512\nfunc @f(q64 %q, q64 %alias) {\nentry:\n%h = mark\n%v = add i64 18446744073709551615, 1\n%g = mark\nrewind %g keep (i64 -1 -> %q)\nrewind %h keep (%v -> %q, i64 -1 -> %alias)\nret\n}\nfunc @main() {\nentry:\n%q = qalloc 64\ncall @f(%q, %q)\nret\n}";
    let parsed = bennett_ir::read(source).expect("read");
    let written = bennett_ir::write(&parsed);
    assert_eq!(bennett_ir::read(&written).expect("reread"), parsed);
    let a = load(source);
    let b = load(&written);
    let mut ea = executor(&a, "main", &[], options());
    let mut eb = executor(&b, "main", &[], options());
    assert_eq!(ea.run(), Outcome::Returned(None));
    assert_eq!(eb.run(), Outcome::Returned(None));
    assert_same_state(&ea.snapshot(), &eb.snapshot(), "round trip");
    assert_eq!(ea.snapshot().quantum.amplitudes, vec![(vec![u64::MAX], C::new(1.0, 0.0))]);
}

#[test]
fn simple_rewind_replays_identically_with_and_without_an_initial_journal() {
    let module = load(
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 2\n%h = mark\nx %q[0]\nrewind %h keep (i2 2 -> %q)\nret\n}",
    );
    let mut a = executor(&module, "main", &[], options());
    at_rewind(&mut a, &module);
    let before = a.snapshot();
    assert_eq!(a.step(), None);
    let after = a.snapshot();
    assert_eq!(a.step_back(), Ok(true));
    assert_same_state(&a.snapshot(), &before, "inverse rewind");
    assert_eq!(a.step(), None);
    same_except_steps(a.snapshot(), &after);
    a.run();
    let mut b = executor(&module, "main", &[], Options { reversible: false, ..options() });
    b.run();
    let mut sa = a.snapshot();
    let mut sb = b.snapshot();
    // Handles count records: the mark is 2 with an initial journal, 1 when it starts the journal.
    assert_eq!(sa.classical.slots[1], 2);
    assert_eq!(sb.classical.slots[1], 1);
    sa.classical.slots[1] = 0;
    sb.classical.slots[1] = 0;
    assert_same_state(&sa, &sb, "initial journal switch");
    assert_eq!(b.step_back(), Err(ReverseError::NotReversible));
}

/// DESIGN.md “History”: every journalled step is a bijection, including mark-to-rewind composites.
/// Rule 4 leaves the output unspecified when a kept input changed, but erasing its norm is not a bijection.
/// F1, fixed: the un-run finds the temporary not back to zero, so the rewind faults `bad_rewind` and
/// changes nothing (the interim decision until worlds that no longer match their history are decided).
#[test]
fn rewind_keep_must_not_erase_norm_when_a_temporary_read_the_destination() {
    let module = load(
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%h = mark\n%tmp = qalloc 1\nx %tmp ctrl %q\nrewind %h keep (i1 1 -> %q)\nret\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    at_rewind(&mut e, &module);
    let before = e.snapshot();
    assert_eq!(e.run(), Outcome::Faulted(Fault::BadRewind));
    let mut after = e.snapshot();
    after.classical.outcome = None;
    assert_same_state(&after, &before, "a refused rewind changes nothing");
    let norm: f64 = e.snapshot().quantum.amplitudes.iter().map(|(_, a)| a.norm_sqr()).sum();
    assert!((norm - 1.0).abs() < 1e-12, "rewind destroyed probability: norm {norm}, {:?}", e.snapshot().quantum);
}

/// DESIGN.md “History”, “Undoing a rewind”: re-run its region to restore the pre-rewind state. F2, fixed.
#[test]
fn undo_rewind_restores_the_second_dynamic_visit_to_the_same_instruction() {
    let module = load(
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%h = mark\nbr loop\nloop:\nrewind %h keep (i1 1 -> %q)\nbr loop\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    at_rewind(&mut e, &module);
    assert_eq!(e.step(), None); // first visit
    at_rewind(&mut e, &module);
    let before = e.snapshot(); // second visit
    assert_eq!(e.step(), None);
    assert_eq!(e.step_back(), Ok(true));
    assert_same_state(&e.snapshot(), &before, "undo second visit");
}

#[test]
fn strict_apply_checks_are_suspended_only_during_unrun_and_replay() {
    // The kept value changes an input of @f. No specific interference is promised by rule 4;
    // the documented apply permutation still defines its unrun even on newly faulting branches.
    let module = load(
        "memory 1024\nfunc @f(i1 %x) -> i1 {\nentry:\n%v = udiv i1 1, %x\nret %v\n}\nfunc @main() {\nentry:\n%x = qalloc 1\n%y = qalloc 1\n%e = qalloc 4\n%h = mark\nx %x\napply @f(%x) -> %y, %e budget 2\nrewind %h keep (i1 1 -> %x)\nret\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    let initial = e.snapshot();
    at_rewind(&mut e, &module);
    let before = e.snapshot();
    assert_eq!(e.stats().region_faults, 0);
    assert_eq!(e.step(), None, "strict mode must not stop the unrun apply");
    assert_eq!(e.stats().region_faults, 1);
    assert_eq!(e.snapshot().quantum.amplitudes, vec![(vec![1, 1, Fault::DivideByZero.code()], C::new(1.0, 0.0))]);
    assert_eq!(e.step_back(), Ok(true), "replay must not stop at newly faulting apply");
    assert_same_state(&e.snapshot(), &before, "strict replay");
    e.reverse().expect("restore entry");
    assert_same_state(&e.snapshot(), &initial, "strict entry");
}

#[test]
fn nested_replay_ignores_configuration_and_step_limits_but_restores_them_afterwards() {
    let module = load(
        "memory 512\nfunc @main() {\nentry:\n%q = qalloc 2\n%h = mark\nh %q[0]\nx %q[1] ctrl %q[0]\nh %q[0]\nrewind %h keep (i2 1 -> %q)\nret\n}",
    );
    // Forward reaches four configurations, rewind and its replay must not spend the forward budget.
    let opts = Options { budget: Some(7), max_configurations: Some(4), ..options() };
    let mut e = executor(&module, "main", &[], opts);
    let start = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None));
    assert_eq!(e.stats().steps, 7);
    assert_eq!(e.stats().rewound_steps, 3);
    assert_eq!(e.reverse(), Ok(7));
    assert_eq!(e.stats().rerun_steps, 3);
    assert_eq!(e.stats().budget, Some(7));
    assert_eq!(e.stats().max_configurations, Some(4));
    assert_same_state(&e.snapshot(), &start, "limited replay");
}

#[test]
fn stale_inner_and_returned_activation_marks_fail_without_partial_keeps() {
    let source = r#"
memory 512
func @old(q1 %q) {
entry:
    %a = mark
    %b = mark
    ret
}
func @new(q1 %q) {
entry:
    %a = mark
    %b = mark
    rewind %a
    rewind %b keep (i1 1 -> %q)
    ret
}
func @main() {
entry:
    %q = qalloc 1
    call @old(%q)
    call @new(%q)
    ret
}
func @stale() {
entry:
    %q = qalloc 1
    %a = mark
    %b = mark
    rewind %a
    rewind %b keep (i1 1 -> %q)
    ret
}
"#;
    let module = load(source);
    for name in ["main", "stale"] {
        let mut e = executor(&module, name, &[], options());
        at_rewind(&mut e, &module);
        assert_eq!(e.step(), None);
        at_rewind(&mut e, &module);
        let before = e.snapshot();
        if name == "main" {
            let base = before.classical.frames.last().expect("new activation").base;
            assert_eq!(
                before.classical.slots[base + 2],
                4,
                "foreign handle names old activation's still-journalled mark"
            );
        }
        assert_eq!(e.step(), Some(Outcome::Faulted(Fault::BadRewind)), "{name}");
        let mut after = e.snapshot();
        after.classical.outcome = None;
        assert_same_state(&after, &before, name);
        assert_reverses(&module, name, &[], options());
    }
}
