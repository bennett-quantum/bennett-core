//! Hand-calculated definite outcomes and step counts for the applied-run rules.

mod common;

use bennett_exec::{Options, Outcome, RunError, Snapshot};
use bennett_ir::{Fault, ValidModule};
use common::{bounded, entry, executor, load};

const APPLY: &str = r#"
func @main(i64 %budget) {
entry:
    %out = qalloc 8
    %fault = qalloc 5
    apply @applied() -> %out, %fault budget %budget
    ret
}
"#;

const APPLY_BIT: &str = r#"
func @main(i64 %budget) {
entry:
    %arg = qalloc 1
    h %arg
    %out = qalloc 8
    %fault = qalloc 5
    apply @applied(%arg) -> %out, %fault budget %budget
    ret
}
"#;

fn options() -> Options {
    Options { machine_meaning: true, ..bounded() }
}

fn check(module: &ValidModule, args: &[u64], expected: &[&[u64]], steps: u64) -> Snapshot {
    let mut run = executor(module, "main", args, options());
    assert_eq!(run.run(), Outcome::Returned(None), "entry arguments {args:?}");
    let snapshot = run.snapshot();
    assert!(snapshot.classical.tape.is_empty());
    let configs: Vec<&[u64]> = snapshot.quantum.amplitudes.iter().map(|(basis, _)| basis.as_slice()).collect();
    assert_eq!(configs, expected, "entry arguments {args:?}");
    assert_eq!(run.stats().region_steps, steps, "entry arguments {args:?}");
    snapshot
}

fn single(body: &str, budget: u64, expected: (u64, u64), steps: u64) {
    let module = load(&format!("memory 128\nfunc @applied() -> i8 {{\nentry:\n{body}\n}}\n{APPLY}"));
    check(&module, &[budget], &[&[expected.0, expected.1]], steps);
}

#[test]
fn a_fault_before_any_jump_changes_only_fault_for_that_input() {
    let module = load(
        r#"
memory 128
func @applied(i1 %arg) -> i8 {
entry:
    %divisor = zext i1 %arg to i8
    %quotient = udiv i8 1, %divisor
    ret %quotient
}
func @main(i64 %budget) {
entry:
    %arg = qalloc 1
    h %arg
    z %arg
    %out = qalloc 8
    x %out[0]
    x %out[1]
    %fault = qalloc 5
    x %fault[2]
    apply @applied(%arg) -> %out, %fault budget %budget
    ret
}
"#,
    );
    // Columns are argument, out and fault. The second attempt faults for input 0 and costs zero.
    check(&module, &[1], &[&[0, 3, 5], &[1, 3, 5]], 2);
    check(&module, &[2], &[&[0, 3, 6], &[1, 3, 5]], 3);
    check(&module, &[3], &[&[0, 3, 6], &[1, 2, 4]], 4);

    let mut machine = executor(&module, "main", &[3], options());
    let mut strict = executor(&module, "main", &[3], bounded());
    for _ in 0..8 {
        assert_eq!(machine.step(), None);
        assert_eq!(strict.step(), None);
    }
    let before = machine.snapshot();
    assert_eq!(machine.step(), None);
    let after = machine.snapshot();
    assert_eq!(after.quantum.amplitudes[0].1, before.quantum.amplitudes[0].1);
    assert_eq!(after.quantum.amplitudes[1].1, before.quantum.amplitudes[1].1);
    let mut expected_classical = before.classical.clone();
    expected_classical.steps += 1;
    expected_classical.frames.last_mut().expect("the caller is still active").inst += 1;
    assert_eq!(after.classical, expected_classical);

    assert_eq!(
        strict.step(),
        Some(Outcome::Stopped(RunError::RegionFault {
            callee: entry(&module, "applied"),
            fault: Fault::DivideByZero,
            faulting: 1,
            configurations: 2,
        }))
    );
    let mut expected_stop = before;
    expected_stop.classical.outcome = strict.outcome();
    assert_eq!(strict.snapshot(), expected_stop);
}

#[test]
fn budget_is_tested_before_each_step_on_each_route_to_the_same_return() {
    let module = load(
        r#"
memory 128
func @applied(i1 %arg) -> i8 {
entry:
    br %arg, longer, done
longer:
    br done
done:
    ret 7
}
func @main(i64 %budget, i1 %input) {
entry:
    %arg = qalloc 1
    %out = qalloc 8
    %fault = qalloc 5
    br %input, one, apply_now
one:
    x %arg
    br apply_now
apply_now:
    apply @applied(%arg) -> %out, %fault budget %budget
    ret
}
"#,
    );
    // Each input is run on its own. Each expectation is (out, fault, completed steps).
    for (budget, zero, one) in [
        (0, (0, 1, 0), (0, 1, 0)),
        (1, (0, 1, 1), (0, 1, 1)),
        (2, (7, 0, 2), (0, 1, 2)),
        (3, (7, 0, 2), (7, 0, 3)),
        (4, (7, 0, 2), (7, 0, 3)),
    ] {
        for (input, (out, fault, steps)) in [(0, zero), (1, one)] {
            check(&module, &[budget, input], &[&[input, out, fault]], steps);
        }
    }
}

#[test]
fn frame_reservation_can_overflow_before_the_first_budget_test() {
    let source = r#"
data 0 = i8 0
func @applied() -> i8 {
entry:
    %unused = add i128 3, 4
    ret 7
}
"#;
    // The caller reserves 24 bytes. One static byte leaves seven or eight bytes for the applied slot.
    for (memory, code) in [(32, 6), (33, 1)] {
        let module = load(&format!("memory {memory}\n{source}{APPLY}"));
        check(&module, &[0], &[&[0, code]], 0);
        let mut strict = executor(&module, "main", &[0], bounded());
        let fault = Fault::from_code(code).expect("the test uses a named fault");
        assert_eq!(
            strict.run(),
            Outcome::Stopped(RunError::RegionFault {
                callee: entry(&module, "applied"),
                fault,
                faulting: 1,
                configurations: 1,
            }),
        );
    }
}

#[test]
fn a_back_edge_reads_both_parameters_before_writing_either() {
    single(
        r#"
    br loop(i8 0, i8 1, i1 0)
loop(i8 %a, i8 %b, i1 %again):
    br %again, done, loop(%b, %a, i1 1)
done:
    %left = shl i8 %a, 4
    %pair = or i8 %left, %b
    ret %pair
"#,
        6,
        (16, 0),
        6,
    );
}

#[test]
fn select_chooses_a_value_after_its_producers_have_run() {
    single(
        r#"
    %bad = sdiv i8 1, 0
    %chosen = select i8 1, 7, %bad
    ret %chosen
"#,
        3,
        (0, 2),
        0,
    );
    for (condition, expected) in [(0, 4), (1, 7)] {
        single(
            &format!(
                r#"
    %quotient = sdiv i8 12, 3
    %chosen = select i8 {condition}, 7, %quotient
    ret %chosen
"#
            ),
            3,
            (expected, 0),
            3,
        );
    }
}

#[test]
fn integer_instructions_use_the_declared_width() {
    for (instruction, expected) in [
        ("add i8 255, 2", 1),
        ("sub i8 0, 1", 255),
        ("mul i8 130, 2", 4),
        ("udiv i8 251, 3", 83),
        ("urem i8 251, 3", 2),
        ("sdiv i8 -128, -1", 128),
        ("srem i8 -7, 2", 255),
        ("and i8 15, 60", 12),
        ("or i8 15, 60", 63),
        ("xor i8 15, 60", 51),
        ("shl i8 129, 1", 2),
        ("lshr i8 129, 1", 64),
        ("ashr i8 129, 1", 192),
        ("shl i8 1, 8", 0),
        ("lshr i8 129, 9", 0),
        ("ashr i8 129, 8", 0),
        ("zext i4 8 to i8", 8),
        ("sext i4 8 to i8", 248),
        ("trunc i16 0xabcd to i8", 205),
    ] {
        single(&format!("    %v = {instruction}\n    ret %v"), 2, (expected, 0), 2);
    }
    for instruction in ["udiv", "urem", "sdiv", "srem"] {
        single(&format!("    %v = {instruction} i8 7, 0\n    ret %v"), 1, (0, 2), 0);
    }
    for (predicate, expected) in [
        ("eq", 0),
        ("ne", 1),
        ("ult", 0),
        ("ule", 0),
        ("ugt", 1),
        ("uge", 1),
        ("slt", 1),
        ("sle", 1),
        ("sgt", 0),
        ("sge", 0),
    ] {
        single(
            &format!("    %c = icmp {predicate} i8 255, 1\n    %v = zext i1 %c to i8\n    ret %v"),
            3,
            (expected, 0),
            3,
        );
    }
    single(
        "    %v = lshr i128 0x7b000000000000000000000000000000, 120\n    %byte = trunc i128 %v to i8\n    ret %byte",
        3,
        (123, 0),
        3,
    );
}

#[test]
fn switch_selects_the_first_matching_case_or_default() {
    for (input, expected) in [(0, 9), (1, 11), (2, 13)] {
        single(
            &format!(
                r#"
    switch i8 {input}, 1: done(i8 11), 2: done(i8 13), default: done(i8 9)
done(i8 %v):
    ret %v
"#
            ),
            2,
            (expected, 0),
            2,
        );
    }
}

#[test]
fn one_callee_returns_to_each_call_site_and_restores_the_stack_pointer() {
    let module = load(&format!(
        r#"
memory 256
func @applied(i1 %arg) -> i8 {{
entry:
    %before = stacksave
    br %arg, right, left
left:
    %l = call @callee(i8 2)
    %plus_one = add i8 %l, 1
    br done(%plus_one)
right:
    %fp = faddr @callee
    %r = call_indirect fn(i8) -> i8 %fp(i8 2)
    %plus_two = add i8 %r, 2
    br done(%plus_two)
done(i8 %v):
    %after = stacksave
    %same = icmp eq i64 %before, %after
    %result = select i8 %same, %v, 255
    ret %result
}}
func @callee(i8 %x) -> i8 {{
entry:
    %ptr = alloca 3
    ret %x
}}
{APPLY_BIT}
"#
    ));
    check(&module, &[11], &[&[0, 3, 0], &[1, 0, 1]], 22);
    check(&module, &[12], &[&[0, 3, 0], &[1, 4, 0]], 23);
}

#[test]
fn calls_and_returns_each_cost_a_step_and_void_calls_can_return() {
    let module = load(&format!(
        r#"
memory 128
func @applied() -> i8 {{
entry:
    call @callee()
    ret 7
}}
func @callee() {{
entry:
    ret
}}
{APPLY}
"#
    ));
    for (budget, expected, steps) in [(0, (0, 1), 0), (1, (0, 1), 1), (2, (0, 1), 2), (3, (7, 0), 3)] {
        check(&module, &[budget], &[&[expected.0, expected.1]], steps);
    }
}

#[test]
fn indirect_target_and_frame_checks_fault_before_entering_the_callee() {
    let module = load(&format!(
        r#"
memory 48
func @applied() -> i8 {{
entry:
    %nonexistent = faddr @wrong
    %v = call_indirect fn(i8) -> i8 %nonexistent(i8 7)
    ret %v
}}
func @wrong(i16 %x) -> i8 {{
entry:
    ret 8
}}
{APPLY}
"#
    ));
    check(&module, &[2], &[&[0, 4]], 1);

    let module = load(&format!(
        r#"
memory 39
func @applied() -> i8 {{
entry:
    %v = call @callee(i8 7)
    ret %v
}}
func @callee(i8 %x) -> i8 {{
entry:
    ret %x
}}
{APPLY}
"#
    ));
    check(&module, &[1], &[&[0, 6]], 0);
    single("    %v = call_indirect fn() -> i8 0()\n    ret %v", 1, (0, 4), 0);
}

#[test]
fn alloca_aligns_down_and_stackrestore_frees_without_erasing_bytes() {
    let source = r#"
func @applied() -> i8 {
entry:
    %saved = stacksave
    %p = alloca 1
    store i8 7, %p
    stackrestore %saved
    %q = alloca 1
    %v = load i8 %q
    %ptr = trunc i64 %q to i8
    %result = add i8 %ptr, %v
    ret %result
}
"#;
    // 147 - 24 - 48 = 75. Subtracting one gives 74 and rounding down gives 72.
    let module = load(&format!("memory 147\ndata 73 = i8 0\n{source}{APPLY}"));
    check(&module, &[9], &[&[0, 6]], 1);
    let module = load(&format!("memory 147\ndata 71 = i8 0\n{source}{APPLY}"));
    check(&module, &[9], &[&[79, 0]], 9);
    single("    %p = alloca 18446744073709551615\n    ret 7", 1, (0, 6), 0);
}

#[test]
fn stackrestore_checks_both_limits_and_accepts_the_current_pointer() {
    for (pointer, expected, steps) in
        [(87, (0, 12), 1), (88, (7, 0), 3), (89, (7, 0), 3), (96, (7, 0), 3), (97, (0, 12), 1)]
    {
        // The allocation lowers the pointer from 96 to 88, leaving [88, 96] to restore into.
        single(&format!("    %p = alloca 1\n    stackrestore {pointer}\n    ret 7"), 3, expected, steps);
    }
}

#[test]
fn a_conditional_store_is_visible_inside_its_run_only() {
    let module = load(&format!(
        r#"
memory 128
data 8 = i8 5
func @applied(i1 %arg) -> i8 {{
entry:
    br %arg, write, done
write:
    store i8 9, 8
    br done
done:
    %v = load i8 8
    ret %v
}}
{APPLY_BIT}
"#
    ));
    let after = check(&module, &[5], &[&[0, 5, 0], &[1, 9, 0]], 8);
    assert_eq!(after.classical.memory[8], 5);

    // A store made before a later fault is discarded as well.
    let module = load(&format!(
        r#"
memory 128
data 8 = i8 5
func @applied() -> i8 {{
entry:
    store i8 9, 8
    fault unreachable
}}
{APPLY}
"#
    ));
    let after = check(&module, &[2], &[&[0, 5]], 1);
    assert_eq!(after.classical.memory[8], 5);
}

#[test]
fn memory_is_little_endian_and_accesses_round_up_to_whole_bytes() {
    single(
        r#"
    store i16 0xa5ff, 127
    ret 7
"#,
        2,
        (0, 3),
        0,
    );
    single(
        r#"
    %v = load i8 18446744073709551615
    ret %v
"#,
        1,
        (0, 3),
        0,
    );
    single(
        r#"
    store i16 0xffff, 1
    store i9 0x1ab, 1
    %lo = load i8 1
    %hi = load i8 2
    %sum = add i8 %lo, %hi
    ret %sum
"#,
        6,
        (172, 0),
        6,
    );
    single(
        r#"
    store i16 0xbfef, 1
    %bits = load i9 1
    %high = lshr i9 %bits, 8
    %answer = trunc i9 %high to i8
    ret %answer
"#,
        5,
        (1, 0),
        5,
    );
}

#[test]
fn a_byte_holding_zero_qubits_faults_and_its_neighbour_loads() {
    let module = load(
        r#"
memory 128
data 8 = i8 0, i8 5
func @applied(i1 %arg) -> i8 {
entry:
    %i = zext i1 %arg to i64
    %p = add i64 8, %i
    %v = load i8 %p
    ret %v
}
func @main(i64 %budget) {
entry:
    x [8:0]
    x [8:0]
    %arg = qalloc 1
    h %arg
    %out = qalloc 8
    %fault = qalloc 5
    apply @applied(%arg) -> %out, %fault budget %budget
    ret
}
"#,
    );
    // The first column is the memory byte promoted by the gate. It still holds qubits at value zero.
    let after = check(&module, &[4], &[&[0, 0, 0, 17], &[0, 1, 5, 0]], 6);
    assert_eq!(after.quantum.cells, [(8, 0)]);
}

fn with_cell_at_last_byte(body: &str) -> ValidModule {
    load(&format!(
        r#"
memory 128
func @applied() -> i8 {{
entry:
{body}
}}
func @main(i64 %budget) {{
entry:
    x [127:0]
    %out = qalloc 8
    %fault = qalloc 5
    apply @applied() -> %out, %fault budget %budget
    ret
}}
"#
    ))
}

#[test]
fn bounds_checks_precede_qubit_byte_checks_for_accesses_and_jump_records() {
    for (body, code) in [
        ("    %v = load i16 127\n    ret 7", 3),
        ("    store i16 9, 127\n    ret 7", 3),
        ("    %r = setjmp 97\n    ret 7", 3),
        ("    longjmp 97, 0", 3),
        ("    %v = load i9 126\n    ret 7", 17),
        ("    store i9 9, 126\n    ret 7", 17),
        ("    %r = setjmp 96\n    ret 7", 17),
        ("    longjmp 96, 0", 17),
    ] {
        check(&with_cell_at_last_byte(body), &[1], &[&[1, 0, code]], 0);
    }
}

#[test]
fn output_exit_and_every_quantum_instruction_fault_before_acting() {
    for (body, code) in [
        ("    effect 65\n    ret 7", 7),
        ("    exit i16 7", 13),
        ("    %q = qalloc 1\n    ret 7", 8),
        ("    x [128:0]\n    ret 7", 8),
        ("    %v = measure i8 [128]\n    ret %v", 8),
        ("    fault unreachable", 5),
        ("    fault exit_type_mismatch", 14),
    ] {
        single(body, 1, (0, code), 0);
    }

    // A jump record can resume after qalloc without executing it. The region guard fires before
    // any of the skipped instructions' register slots are read.
    for (operation, sp) in
        [("qfree %q", 200), ("h %q", 200), ("%m = measure %q", 192), ("apply @applied() -> %out, %fault budget 0", 200)]
    {
        let module = load(&format!(
            r#"
memory 256
data 0 = i64 1, i64 0x0000000300000001, i64 232, i64 {sp}
func @applied() -> i8 {{
entry:
    longjmp 0, 0
resume:
    %q = qalloc 1
    %out = qalloc 8
    %fault = qalloc 5
    %r = setjmp 0
    {operation}
    ret 7
}}
{APPLY}
"#
        ));
        check(&module, &[2], &[&[0, 8]], 1);
    }
}

#[test]
fn a_jump_record_saves_the_continuation_and_longjmp_resumes_with_its_value() {
    let source = format!(
        r#"
memory 256
func @applied() -> i8 {{
entry:
    %saved = stacksave
    %r = setjmp 0
    %first = icmp eq i32 %r, 0
    br %first, go, done
go:
    %tmp = alloca 16
    call @jump(i32 9)
    fault unreachable
done:
    %after = stacksave
    %same = icmp eq i64 %after, %saved
    %value = trunc i32 %r to i8
    %result = select i8 %same, %value, 255
    ret %result
}}
func @jump(i32 %value) {{
entry:
    longjmp 0, %value
}}
{APPLY}
"#
    );
    let module = load(&source);
    // First seven steps end at longjmp, then two steps repeat and five more return the result.
    check(&module, &[6], &[&[0, 1]], 6);
    check(&module, &[7], &[&[0, 1]], 7);
    let after = check(&module, &[14], &[&[9, 0]], 14);
    assert_eq!(&after.classical.memory[0..32], &[0; 32]);
}

#[test]
fn jump_record_layout_and_zero_jump_value_are_exact() {
    let module = load(&format!(
        r#"
memory 256
func @decoy() {{
entry:
    ret
}}
func @applied() -> i8 {{
entry:
    br save
save:
    %saved = alloca 8
    %r = setjmp 0
    %identity = load i64 0
    %position = load i64 8
    %entry = load i64 16
    %sp = load i64 24
    %a = icmp eq i64 %identity, 0x0000000100000001
    %b = icmp eq i64 %position, 0x0000000100000001
    %c = icmp eq i64 %entry, 232
    %d = icmp eq i64 %sp, 96
    %ab = and i1 %a, %b
    %cd = and i1 %c, %d
    %record_ok = and i1 %ab, %cd
    %zero = icmp eq i32 %r, 0
    %ok = and i1 %record_ok, %zero
    %result = zext i1 %ok to i8
    ret %result
}}
{APPLY}
"#
    ));
    // The caller reserves 24 bytes, then the applied frame reserves 16 slots and allocates 8 bytes.
    check(&module, &[18], &[&[1, 0]], 18);

    single(
        r#"
    %r = setjmp 0
    %mark = load i8 32
    %first = icmp eq i8 %mark, 0
    br %first, go, done
go:
    store i8 1, 32
    longjmp 0, 0
done:
    %v = trunc i32 %r to i8
    ret %v
"#,
        12,
        (0, 0),
        11,
    );
}

#[test]
fn longjmp_cannot_target_an_outer_frame_or_an_invalid_record() {
    let module = load(
        r#"
memory 128
func @applied() -> i8 {
entry:
    longjmp 0, 0
}
func @main(i64 %budget) {
entry:
    %r = setjmp 0
    %out = qalloc 8
    %fault = qalloc 5
    apply @applied() -> %out, %fault budget %budget
    ret
}
"#,
    );
    check(&module, &[1], &[&[0, 16]], 0);

    // Corrupt one field at a time after saving a valid record.
    for (address, value) in [(0, 9), (4, 1), (8, 9), (12, 9), (16, 0), (24, 0), (24, 255)] {
        single(
            &format!(
                r#"
    %r = setjmp 0
    store i8 {value}, {address}
    longjmp 0, 0
"#
            ),
            3,
            (0, 16),
            2,
        );
    }
}

#[test]
fn retaddr_reads_the_suspended_outer_frames() {
    let module = load(
        r#"
memory 256
func @main(i64 %budget) {
entry:
    %here = qalloc 64
    %outer = qalloc 64
    %beyond = qalloc 64
    %fault = qalloc 5
    br apply_now
apply_now:
    apply @at_entry() -> %here, %fault budget %budget
    apply @at_outer() -> %outer, %fault budget %budget
    apply @past_outer() -> %beyond, %fault budget %budget
    ret
}
func @at_entry() -> i64 {
entry:
    %v = retaddr 0
    ret %v
}
func @at_outer() -> i64 {
entry:
    %v = retaddr 1
    ret %v
}
func @past_outer() -> i64 {
entry:
    %v = retaddr 2
    ret %v
}
"#,
    );
    check(&module, &[2], &[&[0x0000_0001_0000_0006, 0xffff_ffff_0000_0000, 0, 0]], 6);
}

#[test]
fn entry_copies_ordinary_and_register_arguments_in_parameter_order() {
    let module = load(
        r#"
memory 128
func @applied(i8 %first, i1 %bit, i8 %last) -> i8 {
entry:
    %b = zext i1 %bit to i8
    %diff = sub i8 %first, %last
    %answer = add i8 %diff, %b
    ret %answer
}
func @main(i8 %ordinary) {
entry:
    %arg = qalloc 1
    h %arg
    %out = qalloc 8
    %fault = qalloc 5
    apply @applied(%ordinary, %arg, i8 3) -> %out, %fault budget 4
    ret
}
"#,
    );
    check(&module, &[10], &[&[0, 7, 0], &[1, 8, 0]], 8);
}

#[test]
fn output_widths_are_checked_before_execution() {
    for (result, out, fault, expected) in [
        (4, 8, 5, "is q8, expected q4"),
        (8, 4, 5, "is q4, expected q8"),
        (65, 64, 5, "is q64, expected q65"),
        (8, 8, 4, "is q4, expected a register of at least 5 qubits"),
    ] {
        let source = format!(
            r#"
memory 128
func @applied() -> i{result} {{
entry:
    fault qubit_memory_access
}}
{APPLY}
"#
        )
        .replace("qalloc 8", &format!("qalloc {out}"))
        .replace("qalloc 5", &format!("qalloc {fault}"));
        let error = bennett_ir::load(&source).expect_err("invalid apply width").to_string();
        assert!(error.contains(expected), "{error}");
    }
    let module = load(
        &format!(
            r#"
memory 128
func @applied() -> i8 {{
entry:
    fault qubit_memory_access
}}
{APPLY}
"#
        )
        .replace("qalloc 5", "qalloc 6")
        .replace("    apply", "    x %fault[5]\n    apply"),
    );
    check(&module, &[1], &[&[0, 49]], 0);
}

#[test]
fn the_validator_rejects_reusing_a_target_slot() {
    for apply in [
        "apply @applied(%arg) -> %out, %out budget 1",
        "apply @applied(%arg) -> %arg, %fault budget 1",
        "apply @applied(%fault) -> %out, %fault budget 1",
    ] {
        let source = format!(
            r#"
memory 128
func @applied(i8 %x) -> i8 {{
entry:
    ret %x
}}
func @main() {{
entry:
    %arg = qalloc 8
    %out = qalloc 8
    %fault = qalloc 8
    {apply}
    ret
}}
"#
        );
        let module = bennett_ir::read(&source).expect("well-formed text");
        let errors = bennett_ir::validate(module).expect_err("a target slot was used twice");
        assert!(matches!(errors.0.as_slice(), [err] if matches!(err.problem, bennett_ir::Problem::RegionAlias(_))));
    }
}

#[test]
fn dynamic_target_aliases_and_dead_handles_fault_at_the_apply_itself() {
    let source = r#"
memory 256
func @applied(i8 %a, i8 %b) -> i8 {
entry:
    %sum = add i8 %a, %b
    ret %sum
}
func @invoke(q8 %a, q8 %b, q8 %out, q8 %fault) {
entry:
    apply @applied(%a, %b) -> %out, %fault budget 0
    ret
}
func @main() {
entry:
    %dead = qalloc 8
    qfree %dead
    %input = qalloc 8
    %out = qalloc 8
    %fault = qalloc 8
    call @invoke(ARGS)
    ret
}
"#;
    for (args, fault) in [
        ("%input, %input, %out, %out", Fault::QubitAlias),
        ("%out, %input, %out, %fault", Fault::QubitAlias),
        ("%dead, %input, %out, %out", Fault::DanglingRegister),
        ("%input, %input, %dead, %fault", Fault::DanglingRegister),
        ("%input, %input, %out, %dead", Fault::DanglingRegister),
    ] {
        let module = load(&source.replace("ARGS", args));
        let mut run = executor(&module, "main", &[], options());
        for _ in 0..6 {
            assert_eq!(run.step(), None);
        }
        let mut before = run.snapshot();
        assert_eq!(run.step(), Some(Outcome::Faulted(fault)));
        assert_eq!(run.stats().region_steps, 0);
        before.classical.outcome = run.outcome();
        assert_eq!(run.snapshot(), before);
    }
    let module = load(
        &source
            .replace("ARGS", "%input, %input, %out, %fault")
            .replace("budget 0", "budget 2")
            .replace("    %out = qalloc 8", "    x %input[0]\n    %out = qalloc 8"),
    );
    check(&module, &[], &[&[1, 2, 0]], 2);
}

#[test]
fn stackrestore_sets_an_unsaved_unaligned_pointer_in_range() {
    // Three slots leave the pointer at 80, and allocating one byte lowers it to 72.
    single(
        "    %p = alloca 1\n    stackrestore 73\n    %sp = stacksave\n    %result = trunc i64 %sp to i8\n    ret %result",
        5,
        (73, 0),
        5,
    );
}

#[test]
fn a_budget_fault_precedes_either_memory_check() {
    let module = with_cell_at_last_byte("    %v = load i16 127\n    ret 7");
    check(&module, &[0], &[&[1, 0, 1]], 0);
    check(&module, &[1], &[&[1, 0, 3]], 0);
}

#[test]
fn a_call_to_a_function_with_no_slots_reserves_zero_bytes() {
    let module = load(&format!(
        r#"
memory 24
func @applied() -> i8 {{
entry:
    call @recurse()
    ret 7
}}
func @recurse() {{
entry:
    call @recurse()
    ret
}}
{APPLY}
"#
    ));
    // The caller uses all 24 bytes before entering the applied function.
    check(&module, &[8], &[&[0, 1]], 8);
}

#[test]
fn documented_live_state_determines_the_result() {
    let module = load(
        r#"
memory 256
data 0 = i64 1, i64 0x0000000200000001, i64 240, i64 216
func @applied() -> i8 {
entry:
    longjmp 0, 0
resume:
    %unused = add i8 0, 0
    %v = zext i1 0 to i8
    %r = setjmp 0
    ret %v
}
func @prepare() {
entry:
    %q = qalloc 1
    h %q
    %bit = measure %q
    br %bit, clear, zero
clear:
    x %q
    br done
zero:
    z %q
    br done
done:
    qfree %q
    ret
}
func @main() {
entry:
    call @prepare()
    %out = qalloc 8
    %fault = qalloc 5
    apply @applied() -> %out, %fault budget 2
    ret
}
"#,
    );
    let mut before = None;
    for seed in 0..8 {
        let mut run = executor(&module, "main", &[], Options { seed, ..options() });
        for _ in 0..11 {
            assert_eq!(run.step(), None);
        }
        let snapshot = run.snapshot();
        if let Some(expected) = &before {
            assert_eq!(&snapshot, expected);
        } else {
            before = Some(snapshot);
        }
        assert_eq!(run.run(), Outcome::Returned(None));
        assert_eq!(run.snapshot().quantum.amplitudes[0].0, [0, 0], "seed {seed}");
    }
}
