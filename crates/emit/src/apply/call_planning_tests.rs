use std::fmt::Write;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{Fault, load};

use super::calls::Program;
use super::work::Scope;
use super::{ApplyInput, ApplyMode, Argument, compile_apply_with};

#[test]
fn straight_line_calls_schedule_each_context_once() {
    for count in [64, 256, 1024] {
        for indirect in [false, true] {
            let mut source = String::from(
                "memory 65536\nfunc @f(i1 %x) -> i8 {\nentry:\n call @sequence()\n ret 9\n}\nfunc @sequence() {\nentry:\n",
            );
            for index in 0..count {
                if indirect {
                    writeln!(source, " call_indirect fn() {}()", index + 3).expect("write indirect call");
                } else {
                    writeln!(source, " call @leaf{index}()").expect("write direct call");
                }
            }
            source.push_str(" ret\n}\n");
            for index in 0..count {
                writeln!(source, "func @leaf{index}() {{\nentry:\n ret\n}}").expect("write leaf");
            }
            let (frames, work) = plan(&source);
            assert_eq!(frames, count + 2);
            assert_eq!(work.block_visits, count + 2);
            assert_eq!(work.summary_table_writes, count + 1);
            assert_eq!(work.summary_table_reads, 2 * (count + 1));
            assert_eq!(work.call_lookups, 0);
            assert_eq!(work.literal_target_bits, if indirect { 64 * count } else { 0 });
            assert_eq!(work.fault_scans, count + 1);
        }
    }
}

fn plan(source: &str) -> (usize, super::WorkCounts) {
    let module = load(source).expect("valid call sequence");
    let input = ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("entry"),
        args: &[Argument::Register(0..1)],
        out: 1..9,
        fault: 9..14,
        budget: 0,
        entry_fault: None,
        stack_available: 0,
        memory: None,
        scratch_start: 14,
    };
    let scope = Scope::start();
    let (program, _) = Program::check(&input).expect("plan the sequence");
    (program.frames().count(), scope.counts())
}

#[test]
fn suspended_calls_do_not_visit_the_stopped_continuation() {
    for padding in [1, 64, 256, 1024] {
        let mut source = String::from(
            r#"
memory 65536
func @f(i1 %x) -> i8 {
entry:
    call @sequence()
    ret 9
}
func @sequence() {
entry:
    call @stop()
    call @unused()
    ret
}
func @stop() {
entry:
    fault unreachable
}
func @unused() {
entry:
"#,
        );
        for _ in 0..padding {
            source.push_str("    call @leaf()\n");
        }
        source.push_str("    ret\n}\nfunc @leaf() {\nentry:\n    ret\n}\n");
        let (frames, work) = plan(&source);
        assert_eq!(frames, 3);
        assert_eq!(work.block_visits, 3);
        assert_eq!(work.fault_scans, 2);
        assert_eq!(work.summary_table_writes, 2);
    }
}

#[test]
fn cold_stores_do_not_need_address_proofs() {
    let stores = "    store i8 1, %address\n".repeat(1024);
    let (_, work) = plan(&format!(
        r#"
memory 65536
data 0 = zero 1
func @f(i1 %x) -> i8 {{
entry:
    %address = zext i1 %x to i64
    call @choose(i1 0, %address)
    ret 9
}}
func @choose(i1 %pad, i64 %address) {{
entry:
    br %pad, unused, done
unused:
{stores}
    ret
done:
    ret
}}
"#,
    ));
    assert_eq!(work.block_visits, 3);
    assert_eq!(work.fault_scans, 2);
}

#[test]
fn a_discarded_call_leaves_proof_work_for_a_later_selector() {
    let discarded = "    effect 0\n".repeat(6144);
    let selector = "    effect 0\n".repeat(4608);
    let module = load(&format!(
        r#"
memory 4096
data 32 = i16 513
func @main(i64 %budget) {{
entry:
    %arg = qalloc 2
    h %arg[0]
    h %arg[1]
    s %arg[1]
    %out = qalloc 8
    %fault = qalloc 5
    x %out[2]
    x %fault[4]
    apply @f(%arg) -> %out, %fault budget %budget
    ret
}}
func @f(i2 %x) -> i8 {{
entry:
    %active = icmp ne i2 %x, 0
    br %active, work, skip
skip:
    ret 9
work:
    call @discarded()
    %tag = call @tag()
    switch i2 %tag, 2: done, default: recurse
done:
    ret 21
recurse:
    %again = call @cycle()
    ret %again
}}
func @discarded() {{
entry:
    call @finish()
    ret
unreached:
{discarded}
    ret
}}
func @finish() {{
entry:
    ret
}}
func @tag() -> i2 {{
entry:
    ret 2
unreached:
{selector}
    ret 3
}}
func @cycle() -> i8 {{
entry:
    %again = call @cycle()
    ret %again
}}
"#,
    ))
    .expect("valid copied call program");
    let main = module.function_named("main").expect("runner");
    let options = Options { machine_meaning: true, ..Options::default() };
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for (budget, gates, generated) in [(9, 15, 25), (10, 21, 31)] {
            let mut direct = Executor::new(&module, main, &[budget], options).expect("reference");
            let mut emitted = Executor::new(&module, main, &[budget], options).expect("gate runner");
            assert_eq!(direct.run(), Outcome::Returned(None));
            let mut compile = |input: ApplyInput<'_>| {
                let compiled = compile_apply_with(input, mode)?;
                assert_eq!(compiled.work.final_gates, gates);
                assert!(compiled.work.gates_generated <= generated, "{:?}", compiled.work);
                assert!(compiled.work.temporary_gates <= 10);
                Ok(compiled.block)
            };
            assert_eq!(emitted.run_emitted(&mut compile), Outcome::Returned(None), "{mode:?}, B={budget}");
            let expected = direct.snapshot();
            let actual = emitted.snapshot();
            assert_eq!(actual.quantum, expected.quantum);
            assert_eq!(actual.classical, expected.classical);
            assert_eq!(emitted.stats().nonzero_release_weight, 0.0);
            assert_eq!(actual.quantum.amplitudes.len(), 4);
            for (basis, _) in &actual.quantum.amplitudes {
                let (value, fault) = match (basis[0], budget) {
                    (0, _) => (9, 0),
                    (_, 9) => (0, Fault::Budget.code()),
                    (_, _) => (21, 0),
                };
                assert_eq!(*basis, [basis[0], 4 ^ value, 16 ^ fault]);
            }
        }
    }
}

#[test]
fn a_large_uncopied_entry_needs_only_structural_validation_of_its_cold_block() {
    let stores = "    store i8 0, 8\n".repeat(16_384);
    let module = load(&format!(
        r#"
memory 4096
func @f(i1 %x) -> i8 {{
entry:
    ret 7
cold:
{stores}
    ret 0
}}
"#,
    ))
    .expect("valid large entry");
    let compiled = compile_apply_with(
        ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("entry"),
            args: &[Argument::Register(0..1)],
            out: 1..9,
            fault: 9..14,
            budget: 1,
            entry_fault: None,
            stack_available: 4096,
            memory: None,
            scratch_start: 14,
        },
        ApplyMode::Automatic,
    )
    .expect("the limit applies when adding a child");
    assert_eq!(compiled.work.structural_cells, 16_386);
    assert_eq!(compiled.work.block_visits, 0);
    assert_eq!(compiled.work.fault_scans, 0);
    assert_eq!(compiled.definite_steps, 1);
}

#[test]
fn discarded_operand_circuits_count_even_at_device_budget_zero() {
    for (width, ceiling) in [(8, 78), (64, 638)] {
        let module = load(&format!(
            r#"
func @f(i{width} %a, i{width} %b) -> i8 {{
entry:
    %same = icmp eq i{width} %a, %b
    switch i1 %same, 1: yes, default: no
yes:
    ret 1
no:
    ret 0
}}
"#,
        ))
        .expect("valid switch");
        let end = 2 * width;
        let compiled = compile_apply_with(
            ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("entry"),
                args: &[Argument::Register(0..width), Argument::Register(width..end)],
                out: end..end + 8,
                fault: end + 8..end + 13,
                budget: 0,
                entry_fault: None,
                stack_available: 4096,
                memory: None,
                scratch_start: end + 13,
            },
            ApplyMode::Automatic,
        )
        .expect("compile the budget fault");
        assert!(compiled.work.temporary_gates > 0);
        assert!(compiled.work.temporary_gates <= ceiling);
        assert_eq!(compiled.work.gates_generated, compiled.work.temporary_gates + 1);
        assert_eq!(compiled.block.ops().len(), 3);
    }
}

#[test]
fn argument_independent_stop_leaves_allowance_for_a_later_selector() {
    check_stopping_contexts(8189, "i1", 1, "    fault unreachable", 25);
}

#[test]
fn argument_dependent_stops_leave_allowance_for_a_later_selector() {
    check_stopping_contexts(
        8185,
        "i2",
        2,
        "    %zero = icmp eq i2 %arg, 0\n    br %zero, stopped, other\nstopped:\n    fault unreachable\nother:\n    fault divide_by_zero",
        38,
    );
}

fn check_stopping_contexts(padding: usize, ty: &str, other: u8, stop: &str, operations: usize) {
    let cold = "    effect 0\n".repeat(padding);
    let module = load(&format!(
        r#"
memory 65536
func @main() {{
entry:
    %x = qalloc 1
    %y = qalloc 1
    h %x[0]
    h %y[0]
    %out = qalloc 8
    %fault = qalloc 5
    apply @f(%x, %y) -> %out, %fault budget 12
    ret
}}
func @f(i1 %x, i1 %y) -> i8 {{
entry:
    br %x, quick, fork
quick:
    br doomed({ty} 0)
fork:
    br %y, slow, later
doomed({ty} %arg):
    call @stop(%arg)
    ret 0
slow:
    br doomed({ty} {other})
later:
    br last
last:
    %tag = call @tag()
    switch i2 %tag, 2: done, default: recurse
done:
    ret 21
recurse:
    %again = call @cycle()
    ret %again
}}
func @stop({ty} %arg) {{
entry:
{stop}
cold:
{cold}
    ret
}}
func @tag() -> i2 {{
entry:
    ret 2
}}
func @cycle() -> i8 {{
entry:
    %again = call @cycle()
    ret %again
}}
"#,
    ))
    .expect("valid call program");
    let main = module.function_named("main").expect("runner");
    let options = Options { machine_meaning: true, ..Options::default() };
    let mut direct = Executor::new(&module, main, &[], options).expect("reference");
    assert_eq!(direct.run(), Outcome::Returned(None));
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let mut emitted = Executor::new(&module, main, &[], options).expect("gate runner");
        let mut compile = |input: ApplyInput<'_>| {
            let compiled = compile_apply_with(input, mode).expect("both stopping calls leave room for the selector");
            assert!(compiled.block.ops().len() <= operations);
            assert_eq!(compiled.work.context_schedules, if other == 1 { 2 } else { 3 });
            Ok(compiled.block)
        };
        assert_eq!(emitted.run_emitted(&mut compile), Outcome::Returned(None), "{mode:?}");
        assert_eq!(emitted.snapshot(), direct.snapshot());
        assert_eq!(emitted.stats().nonzero_release_weight, 0.0);
    }
}

#[test]
fn a_store_without_declarations_leaves_the_comparison_unproved() {
    check_store_proof("");
}

#[test]
fn a_declaration_does_not_activate_an_unrelated_store_proof() {
    check_store_proof("data 32 = zero 1\n");
}

fn check_store_proof(declaration: &str) {
    let module = load(&format!(
        r#"
memory 4096
{declaration}func @f(i64 %x, i64 %y) -> i1 {{
entry:
    %equal = icmp eq i64 %x, %y
    store i8 0, %x
    ret %equal
}}
"#,
    ))
    .expect("valid store");
    let held = Default::default();
    let memory = bennett_exec::ApplyMemory::new(&[0; 4096], &held);
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for (budget, gates, memory) in [(0, 1, None), (1, 639, Some(memory))] {
            let compiled = compile_apply_with(
                ApplyInput {
                    module: &module,
                    callee: module.function_named("f").expect("entry"),
                    args: &[Argument::Register(0..64), Argument::Register(64..128)],
                    out: 128..129,
                    fault: 129..134,
                    budget,
                    entry_fault: None,
                    stack_available: 4096,
                    memory,
                    scratch_start: 134,
                },
                mode,
            )
            .expect("compile the budget fault");
            assert_eq!(compiled.work.temporary_gates, 0, "{mode:?}, B={budget}");
            assert_eq!(compiled.work.gates_generated, gates);
            assert_eq!(compiled.work.final_gates, gates);
        }
    }
}

#[test]
fn reached_point_lookups_charge_the_table_meter() {
    use bennett_ir::BlockId;

    use super::{bits::Bit, definite::CallProofs, symbolic::Frame};

    let module = load(
        "func @f(i1 %x) -> i1 {\nentry:\n switch i1 %x, 0: yes, default: no\nyes:\n ret 1\nno:\n ret 0\ncold:\n ret 0\n}\n",
    )
    .expect("valid switch");
    let callee = module.function_named("f").expect("entry");
    let mut proofs = CallProofs::new(&module, 16_384);
    let frame =
        Frame::check(&module, callee, [Some(vec![Bit::input(0)])].into_iter(), &mut proofs).expect("plan the switch");
    let meter = Scope::start();
    for _ in 0..500 {
        assert_eq!(frame.reached_points(BlockId(1)), Some(1));
        assert_eq!(frame.reached_points(BlockId(3)), Some(0));
    }
    assert_eq!(meter.counts().table_entries, 1000);
}
