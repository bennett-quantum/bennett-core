//! Independent whole-machine reference. Run with --nocapture for measured domain sizes.
#[allow(dead_code, reason = "this binary uses the whole-world oracle, not every dense helper")]
mod oracle_support;

use std::collections::BTreeMap;

use bennett_ir::{Fault, Type};
use num_complex::Complex64 as C;
use oracle_support::permutation::{check_bijection, words};
use oracle_support::worlds::{Action, Finish, Model, Record, Value, basis};
use oracle_support::worlds_executor::compare;

fn raw_constant<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source.find(&format!("const {name}: &str = r#\"")).expect("shared fixture name");
    source[start..].split_once("r#\"").expect("raw string").1.split_once("\"#;").expect("raw end").0
}

#[test]
fn history_and_jump_fixtures_match_the_independent_world_machine() {
    let history = include_str!("history.rs");
    let jumps = include_str!("setjmp.rs");
    let return_addresses = include_str!("retaddr.rs");
    let fixtures = [
        raw_constant(history, "RUNS"),
        raw_constant(history, "FAULTS"),
        raw_constant(history, "COSTS"),
        raw_constant(jumps, "STALE"),
        raw_constant(jumps, "CORRUPT"),
        raw_constant(jumps, "REGION"),
        raw_constant(jumps, "ESCAPE"),
        raw_constant(jumps, "REWIND"),
        raw_constant(return_addresses, "POSITIONS"),
        raw_constant(return_addresses, "REGION"),
        include_str!("../../../examples/history.bn"),
        include_str!("../../../examples/setjmp.bn"),
        include_str!("../../../examples/retaddr.bn"),
    ];
    let mut runs = 0;
    let mut steps = 0;
    for text in fixtures {
        let module = bennett_ir::load(text).expect("existing fixture");
        for f in &module.functions {
            if f.params.iter().any(|p| matches!(p.ty, Type::Qreg(_))) {
                continue;
            }
            let cases = if f.params.is_empty() {
                vec![vec![]]
            } else {
                vec![
                    vec![0; f.params.len()],
                    vec![1; f.params.len()],
                    f.params.iter().map(|p| if let Type::Int(w) = p.ty { 3 & w.mask() as u64 } else { 0 }).collect(),
                ]
            };
            for args in cases {
                steps += compare(&module, &f.name, &args, 256, true);
                runs += 1;
            }
        }
    }
    println!(
        "{runs} existing history/jump fixture runs, {steps} forward steps; snapshots and Born-conditioned measurements agree"
    );
}

#[test]
fn initial_worlds_split_classical_calls_and_keep_history_distinct() {
    // This call is the worlds extension: @main can be initialized as a superposition of complete
    // classical states today, without asking today's validator to accept a qreg as an integer call.
    let program=bennett_ir::load("memory 128\nfunc @f(i1 %x) -> i1 {\nentry:\nstore i1 %x, 0\nret %x\n}\nfunc @main(i1 %x) -> i1 {\nentry:\n%v = call @f(%x)\nret %v\n}").expect("valid reference fixture or inverse");
    let model = Model::new(&program, "main");
    let a = model.initial("main", &[0]);
    let b = model.initial("main", &[1]);
    let amp = C::new(std::f64::consts::FRAC_1_SQRT_2, 0.0);
    let (state, outcomes) =
        oracle_support::worlds::execute(&program, "main", BTreeMap::from([(a, amp), (b, amp * C::i())]), 20);
    assert_eq!(state.len(), 2);
    assert!((outcomes[&Finish::Returned(Some(0))] - 0.5).abs() < 1e-12);
    assert!((outcomes[&Finish::Returned(Some(1))] - 0.5).abs() < 1e-12);
    for (w, a) in state {
        let Value::Int(n) = w.classical.slots[0] else { panic!("integer") };
        assert_eq!(w.classical.finish, Some(Finish::Returned(Some(n))));
        assert_eq!(model.memory(&w)[0], n as u8);
        assert!((a - if n == 0 { amp } else { amp * C::i() }).norm() < 1e-12);
    }
}

/// The executor's count of journal records in a mark's handle, checked in the reference model's slots: a
/// handle is opaque to programs since 2026-11-16 (the validator refuses any use but `rewind`), so the count is
/// read from the machine state, not from a value the program computes.
#[test]
fn public_marks_count_reading_calls_once_even_with_repeated_register_arguments() {
    for call in ["call @read(%q, %q)", "call_indirect fn(i1, i1) -> i64 %p(%q, %q)"] {
        let text = format!(
            r#"
memory 512
func @read(i1 %a, i1 %b) -> i64 {{
entry:
    %h = mark
    ret 0
}}
func @identity(i64 %v) -> i64 {{
entry:
    ret %v
}}
func @main() -> i64 {{
entry:
    %q = qalloc 1
    h %q
    %p = faddr @read
    %out = qalloc 64
    %m = mark
    %r = {call}
    store i64 %r, 0
    %n = mark
    rewind %m keep (i64 5 -> %out)
    %result = call @identity(%out)
    ret %result
}}
"#
        );
        let p = bennett_ir::load(&text).expect("public mark fixture");
        let slot = |function: &str, name: &str| {
            let function = p.function(p.function_named(function).expect("function"));
            function.slot_names.iter().position(|n| n == name).expect("slot")
        };
        let handle = |w: &oracle_support::worlds::World, frame: usize, index: usize| {
            let Value::Int(n) = w.classical.slots[w.classical.frames[frame].base + index] else { panic!("integer") };
            n
        };
        let model = Model::new(&p, "main");
        let initial = basis(model.initial("main", &[]));
        let mut prefix = initial.clone();
        for steps in 1..=10 {
            prefix = model.step(&prefix);
            for w in prefix.keys() {
                match steps {
                    // Five caller steps, call, callee mark = 7. Both read annotations belong to that one call
                    // step.
                    7 => assert_eq!(handle(w, 1, slot("read", "h")), 7),
                    // call/mark/ret/store/mark gives a span of five records.
                    10 => {
                        assert_eq!(handle(w, 0, slot("main", "m")), 5);
                        assert_eq!(handle(w, 0, slot("main", "n")), 10);
                    }
                    _ => {}
                }
            }
        }
        let state = model.run_joined(initial.clone(), 30);
        for w in state.keys() {
            assert_eq!(w.classical.finish, Some(Finish::Returned(Some(5))));
            assert_eq!(model.memory(w)[0], 0, "rewind restores the store");
            assert_eq!(w.quantum[1], 5);
        }
        let mut reversed = state;
        while reversed.keys().any(|w| !w.history.is_empty()) {
            reversed = model.undo_state(&reversed).expect("reading calls reverse");
        }
        assert_eq!(reversed.keys().collect::<Vec<_>>(), initial.keys().collect::<Vec<_>>());
        for (w, a) in &reversed {
            assert!((*a - initial[w]).norm() < 1e-12, "full reference reversal, including public positions");
        }
        compare(&p, "main", &[], 30, false);
    }
}

#[test]
fn every_small_whole_state_step_is_bijective_on_dirty_history_and_restores_its_input() {
    let p=bennett_ir::load("memory 128\nfunc @main(i2 %x) -> i2 {\nentry:\n%h = mark\nstore i2 %x, 0\n%y = load i2 0\n%z = add i2 %y, 1\nret %z\n}").expect("valid reference fixture or inverse");
    let model = Model::new(&p, "main");
    let mut total = 0;
    // Whole states include all 2-bit assignments to overwritten and live slots, overlay byte,
    // and a dirty prefix word. Output space is constructed from the documented effects below.
    for pc in 0..4 {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for fields in words(&[2, 2, 2, 2]) {
            let mut w = model.initial("main", &[fields[0].into()]);
            w.classical.slots[2] = Value::Int(fields[1].into());
            w.classical.overlay.insert(0, fields[2] as u8);
            if fields[2] == 0 {
                w.classical.overlay.remove(&0);
            }
            let mut garbage = w.classical.clone();
            garbage.tape = vec![fields[3] as u8];
            w.history.push(Record { before: garbage, action: Action::Classical });
            w.classical.journal_records = 1;
            w.classical.frames[0].instruction = pc;
            let mut out = w.clone();
            let before = w.classical.clone();
            let action = match pc {
                0 => {
                    out.classical.slots[1] = Value::Int(2);
                    out.classical.marks.push(oracle_support::worlds::Mark {
                        handle: 2,
                        history_position: 2,
                        activation: 1,
                        valid: true,
                    });
                    Action::Mark
                }
                1 => {
                    if fields[0] == 0 {
                        out.classical.overlay.remove(&0);
                    } else {
                        out.classical.overlay.insert(0, fields[0] as u8);
                    }
                    Action::Classical
                }
                2 => {
                    out.classical.slots[2] = Value::Int(fields[2].into());
                    Action::Classical
                }
                3 => {
                    out.classical.slots[3] = Value::Int(((fields[1] + 1) & 3).into());
                    Action::Classical
                }
                _ => unreachable!(),
            };
            out.classical.frames[0].instruction += 1;
            out.classical.journal_records += 1;
            out.history.push(Record { before, action });
            inputs.push(w);
            outputs.push(out);
        }
        total += check_bijection(&inputs, &outputs, |w| {
            model.step(&basis(w.clone())).into_keys().next().expect("nonempty reference state")
        })
        .expect("valid reference fixture or inverse");
        for (input, output) in inputs.iter().zip(&outputs) {
            assert_eq!(model.undo(output.clone()).expect("bijection holds"), basis(input.clone()));
        }
    }
    assert_eq!(total, 1024);
    println!("{total} complete model states: mark/store/load/add, exhaustive dirty prefix and overwritten values");
}

#[test]
fn repeated_rewinds_replay_the_dynamic_visit_and_faults_remain_atomic() {
    for text in [
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%h = mark\nbr loop\nloop:\nrewind %h keep (i1 1 -> %q)\nbr loop\n}",
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%h = mark\n%tmp = qalloc 1\nx %tmp ctrl %q\nrewind %h keep (i1 1 -> %q)\nret\n}",
    ] {
        let p = bennett_ir::load(text).expect("valid reference fixture or inverse");
        compare(&p, "main", &[], 12, false);
        let model = Model::new(&p, "main");
        let start = basis(model.initial("main", &[]));
        let mut state = start.clone();
        for _ in 0..10 {
            let next = model.step(&state);
            if next.keys().any(|w| matches!(w.classical.finish, Some(Finish::Faulted(Fault::BadRewind)))) {
                break;
            }
            let mut restored = BTreeMap::new();
            for (w, a) in &next {
                for (out, b) in model.undo(w.clone()).expect("inverse exists") {
                    *restored.entry(out).or_default() += a * b;
                }
            }
            assert_eq!(restored, state, "exact complete model state including journal and dynamic visit");
            state = next;
        }
    }
}

/// cargo test -p bennett-exec --test worlds_reference all_existing_history_programs -- --ignored --nocapture
#[test]
#[ignore = "runs all four history/jump binaries with the independent whole-world adapter"]
fn all_existing_history_programs() {
    let result = std::process::Command::new("cargo")
        .args([
            "test",
            "-p",
            "bennett-exec",
            "--test",
            "history",
            "--test",
            "history_independent",
            "--test",
            "history_unrun",
            "--test",
            "setjmp",
            "--test",
            "retaddr",
            "--",
            "--nocapture",
        ])
        .env("CARGO_BUILD_JOBS", "3")
        .env("RUST_TEST_THREADS", "3")
        .env("BENNETT_WORLDS_REFERENCE", "1")
        .status()
        .expect("cargo history comparisons");
    assert!(result.success());
}

#[test]
fn a_register_passed_as_an_integer_runs_every_world_and_clean_rewind_keeps_coherence() {
    // Today's reader requires an integer call operand. Inject a register value in the initial
    // world at that operand: the model already supplies the documented next-step call meaning,
    // without changing today's validator or reading the worlds implementation.
    let p=bennett_ir::load("memory 256\nfunc @f(i1 %x) -> i1 {\nentry:\nstore i1 %x, 0\n%r = xor i1 %x, 1\nret %r\n}\nfunc @main(i1 %x) {\nentry:\n%out = qalloc 1\n%h = mark\n%v = call @f(%x)\nrewind %h keep (%v -> %out)\nret\n}").expect("valid reference fixture or inverse");
    let model = Model::new(&p, "main");
    let mut a = model.initial("main", &[0]);
    a.classical.slots[0] = Value::Register(0, 1);
    a.classical.registers.push(oracle_support::worlds::Register { width: Some(1), generation: 1, allocated_at: 0 });
    a.quantum.push(0);
    let mut b = a.clone();
    b.quantum[0] = 1;
    let half = C::new(std::f64::consts::FRAC_1_SQRT_2, 0.0);
    let initial = BTreeMap::from([(a, half), (b, half * C::i())]);
    let final_state = model.run(initial, 30);
    assert_eq!(final_state.len(), 2);
    let first = final_state.keys().next().expect("nonempty reference state");
    for (w, a) in &final_state {
        assert_eq!(w.classical.finish, Some(Finish::Returned(None)));
        assert_eq!(w.classical, first.classical, "all computed slots, private writes and frames cleaned");
        assert_eq!(w.history, first.history, "rewind must not journal the kept result");
        assert!(model.memory(w).iter().all(|&b| b == 0));
        assert_eq!(w.quantum[1], w.quantum[0] ^ 1);
        assert!((*a - if w.quantum[0] == 0 { half } else { half * C::i() }).norm() < 1e-12);
    }
}

#[test]
fn changed_classical_read_that_cannot_be_replayed_faults_bad_rewind_without_erasing_norm() {
    let p=bennett_ir::load("memory 256\nfunc @f(i1 %x) -> i1 {\nentry:\nret %x\n}\nfunc @main(i1 %x, q1 %dst) {\nentry:\n%h = mark\n%v = call @f(%x)\nrewind %h keep (%v -> %dst)\nret\n}").expect("valid reference fixture or inverse");
    let model = Model::new(&p, "main");
    let mut w = model.initial("main", &[0, 0]);
    w.classical.slots[0] = Value::Register(0, 1);
    w.classical.slots[1] = Value::Register(0, 1);
    w.classical.registers.push(oracle_support::worlds::Register { width: Some(1), generation: 1, allocated_at: 0 });
    w.quantum.push(1);
    let mut state = basis(w);
    for _ in 0..3 {
        state = model.step(&state);
    }
    let before = state.clone();
    state = model.step(&state);
    let mut actual = state.into_keys().next().expect("nonempty reference state");
    assert_eq!(actual.classical.finish.take(), Some(Finish::Faulted(Fault::BadRewind)));
    assert_eq!(basis(actual), before, "atomic refusal preserves every history and amplitude");
}

#[test]
fn nested_mark_rewind_composite_is_a_whole_state_bijection_including_dirty_prefixes() {
    let p=bennett_ir::load("memory 128\nfunc @main(i2 %x) {\nentry:\n%q = qalloc 2\n%h = mark\nstore i2 %x, 0\n%inner = mark\nrewind %inner keep (i2 1 -> %q)\nrewind %h keep (%x -> %q)\nret\n}").expect("valid reference fixture or inverse");
    let model = Model::new(&p, "main");
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for fields in words(&[2, 2, 2, 2]) {
        let mut w = model
            .step(&basis(model.initial("main", &[fields[0].into()])))
            .into_keys()
            .next()
            .expect("nonempty reference state");
        w.quantum[0] = fields[1];
        if fields[2] != 0 {
            w.classical.overlay.insert(0, fields[2] as u8);
        }
        w.history[0].before.tape = vec![fields[3] as u8];
        let at_mark = model.step(&basis(w)).into_keys().next().expect("nonempty reference state");
        let mut expected = at_mark.clone();
        expected.quantum[0] ^= fields[0];
        expected.classical.frames[0].instruction = 6;
        expected.classical.journal_records += 1;
        let start = Box::new(at_mark.classical.clone());
        let mut site = at_mark.classical.clone();
        site.frames[0].instruction = 5;
        expected.history.push(Record { before: site, action: Action::Rewind { start, count: 3 } });
        inputs.push(at_mark);
        outputs.push(expected);
    }
    let step = |w: &oracle_support::worlds::World| {
        let mut state = basis(w.clone());
        for _ in 0..4 {
            state = model.step(&state);
        }
        state.into_keys().next().expect("nonempty reference state")
    };
    assert_eq!(check_bijection(&inputs, &outputs, step).expect("bijection holds"), 256);
    for (input, output) in inputs.iter().zip(&outputs) {
        let before_rewind = model.undo(output.clone()).expect("valid reference fixture or inverse");
        let mut state = before_rewind;
        while state.keys().next().expect("nonempty reference state").history.len() > input.history.len() {
            state = model.undo_state(&state).expect("valid reference fixture or inverse");
        }
        assert_eq!(state, basis(input.clone()));
    }
    println!("256 full nested-mark/rewind images on independently constructed complete output domains");
}

#[test]
fn repeated_read_only_apply_arguments_are_preserved_and_only_destinations_must_be_disjoint() {
    // DESIGN.md permits repeated read-only arguments; out/fault may not occur among them.
    let p=bennett_ir::load("memory 256\nfunc @f(i1 %a, i1 %b) -> i1 {\nentry:\n%r = xor i1 %a, %b\nret %r\n}\nfunc @main() {\nentry:\n%q = qalloc 1\nh %q\n%out = qalloc 1\n%fault = qalloc 5\napply @f(%q, %q) -> %out, %fault budget 8\nret\n}").expect("valid duplicate read-only inputs");
    compare(&p, "main", &[], 20, false);
    let model = Model::new(&p, "main");
    let state = model.run(basis(model.initial("main", &[])), 20);
    for (w, a) in state {
        assert_eq!(w.classical.finish, Some(Finish::Returned(None)));
        assert_eq!(&w.quantum[1..], &[0, 0]);
        assert!((a - C::new(std::f64::consts::FRAC_1_SQRT_2, 0.0)).norm() < 1e-12);
    }
}

#[test]
fn measurement_checks_liveness_before_becoming_a_born_boundary() {
    let p = bennett_ir::load("memory 128\nfunc @main() {\nentry:\n%q = qalloc 1\nqfree %q\n%m = measure %q\nret\n}")
        .expect("valid program using a freed handle");
    compare(&p, "main", &[], 8, true);
    let model = Model::new(&p, "main");
    let state = model.run(basis(model.initial("main", &[])), 8);
    assert_eq!(
        state.keys().next().expect("one faulted world").classical.finish,
        Some(Finish::Faulted(Fault::DanglingRegister))
    );
}

#[test]
fn an_applied_setjmp_record_keeps_the_callers_physical_frame_depth() {
    // DESIGN.md Non-local jumps: the buffer stores frame depth. Private memory
    // does not remove the caller's physical activation while the function runs.
    let p=bennett_ir::load("memory 256\nfunc @f(i1 %x)->i8 {\nentry:\n%buf = alloca 32\n%r = setjmp %buf\n%depth = load i8 %buf\nret %depth\n}\nfunc @main() {\nentry:\n%q = qalloc 1\nh %q\n%out = qalloc 8\n%fault = qalloc 5\napply @f(%q) -> %out, %fault budget 16\nret\n}").expect("physical-depth fixture");
    compare(&p, "main", &[], 16, false);
    let model = Model::new(&p, "main");
    let state = model.run(basis(model.initial("main", &[])), 16);
    assert!(state.keys().all(|w| w.quantum[1] == 1 && w.quantum[2] == 0));
}
