//! `apply` to a block (`docs/SYNTAX.md`, "Applying to a block"; `docs/HANDLES.md`, section 6): the handle
//! interface's `apply`. Checked: the unitary of 6.4 against the device's earlier `apply` of the same
//! function on integers; the image and the in fields in the block; faults as values in the fault register,
//! never a stop; controls; fields wider than 64 qubits; each check of 6.2 in order with the state unchanged;
//! placement and setup failures as fault codes; the step report; forward then reverse back to every earlier
//! state; `quantum_in_region` first inside an applied function.

mod common;

use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use common::{bounded, executor, run};

/// The fields at 64: `x` (handle 1, three qubits) in at byte 0, `o` (handle 2) out at byte 1. The image at
/// 160 has 7 at byte 2. `@f` computes `(3 x + image[2]) mod 8` into byte 1. `%k` is the control register
/// (handle 4), listed at 256 when `%n` is 1.
fn program(apply: &str, f_body: &str) -> String {
    format!(
        "memory 1024
data 64 = i64 1, i64 0, i64 3, i64 0, i64 1, i64 2, i64 0, i64 3, i64 1, i64 2
data 160 = i8 5, i8 0, i8 7, i8 0
data 256 = i64 4, i64 0
applied func @f(i64 %b) {{
entry:
{f_body}
}}
func @main(i64 %n) -> i64 {{
entry:
    %x = qalloc i64 3
    %o = qalloc i64 3
    %e = qalloc i64 5
    %k = qalloc i64 1
    h %x[0]
    h %x[1]
    h %x[2]
    h %k[0]
    %fa = faddr @f
    {apply}
    %rx = measure i64 %x[0, 3]
    %ro = measure i64 %o[0, 3]
    %re = measure i64 %e[0, 5]
    %rk = measure i64 %k[0, 1]
    %a = shl i64 %ro, 3
    %c = shl i64 %re, 6
    %d = shl i64 %rk, 11
    %s = or i64 %rx, %a
    %t = or i64 %s, %c
    %r = or i64 %t, %d
    ret %r
}}"
    )
}

/// The program ending right after its `apply`, before any measurement.
fn unmeasured(text: &str) -> String {
    let start = text.find("    %rx = measure").expect("the measurements");
    let end = text[start..].find("}").expect("the end of main") + start;
    format!("{}    ret 0\n{}", &text[..start], &text[end..])
}

const AFFINE: &str = "    %x = load i8 %b
    %c2 = add i64 %b, 2
    %k = load i8 %c2
    %y0 = mul i8 %x, 3
    %y1 = add i8 %y0, %k
    %y = and i8 %y1, 7
    %o = add i64 %b, 1
    store i8 %y, %o
    ret";

const APPLY: &str = "apply %fa budget 100, fault %e, image(160, 4), fields(64, 2), ctrl list(256, %n)";

fn outcomes(text: &str, n: u64, options: Options) -> Vec<u64> {
    let module = common::load(text);
    (0..24)
        .map(|seed| match run(&module, "main", &[n], Options { seed, ..options }).0 {
            Outcome::Returned(Some(value)) => value as u64,
            other => panic!("seed {seed}: {other:?}"),
        })
        .collect()
}

/// Splits a packed result into x, o, e, k.
fn split(value: u64) -> (u64, u64, u64, u64) {
    (value & 7, value >> 3 & 7, value >> 6 & 31, value >> 11 & 1)
}

#[test]
fn each_input_gets_the_out_value_of_its_run_on_the_block() {
    let mut seen = std::collections::BTreeSet::new();
    for value in outcomes(&program(APPLY, AFFINE), 0, bounded()) {
        let (x, o, e, _) = split(value);
        assert_eq!((o, e), ((3 * x + 7) % 8, 0), "x = {x}");
        seen.insert(x);
    }
    assert!(seen.len() > 4, "{seen:?}");
}

/// The same function written for the device's earlier `apply` (integers in, an integer out) gives the same
/// state, configuration by configuration, before any measurement.
#[test]
fn the_state_after_it_is_that_of_the_earlier_apply() {
    let state = |text: &str| {
        let module = common::load(&unmeasured(text));
        let (outcome, executor) = run(&module, "main", &[0], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(0)));
        executor.snapshot().quantum
    };
    let block = state(&program(APPLY, AFFINE));
    let earlier = program("apply @g(%x) -> %o, %e budget 100", "    ret")
        .replace("%x = qalloc i64 3", "%x = qalloc 3")
        .replace("%o = qalloc i64 3", "%o = qalloc 3")
        .replace("%e = qalloc i64 5", "%e = qalloc 5")
        .replace(
            "func @main",
            "func @g(i3 %x) -> i3 {
entry:
    %y0 = mul i3 %x, 3
    %y = add i3 %y0, 7
    ret %y
}
func @main",
        );
    let earlier = state(&earlier);
    assert_eq!(block.distance(&earlier), Some(0.0));
    assert_eq!(block.amplitudes.len(), 16, "eight values of x, two of the control register's qubit");
}

/// A fault of the applied function is a value in the fault register: `x = 0` divides by zero, the other
/// inputs return; the run goes on also in the simulator's strict mode (the default), and the out field of
/// the faulting input is unchanged.
#[test]
fn a_fault_is_a_value_in_the_fault_register_never_a_stop() {
    let divide = "    %x = load i8 %b
    %q = udiv i8 6, %x
    %o = add i64 %b, 1
    store i8 %q, %o
    ret";
    let mut faulted = false;
    for value in outcomes(&program(APPLY, divide), 0, bounded()) {
        let (x, o, e, _) = split(value);
        match 6u64.checked_div(x) {
            None => {
                assert_eq!((o, e), (0, Fault::DivideByZero.code()), "x = 0");
                faulted = true;
            }
            Some(quotient) => assert_eq!((o, e), (quotient & 7, 0), "x = {x}"),
        }
    }
    assert!(faulted, "some seed measured x = 0");
}

/// With a control in (|0⟩ + |1⟩)/√2 the out field changes only where it is 1.
#[test]
fn a_control_list_limits_the_apply_to_its_configurations() {
    let mut both = [false; 2];
    for value in outcomes(&program(APPLY, AFFINE), 1, bounded()) {
        let (x, o, e, k) = split(value);
        let expected = if k == 1 { (3 * x + 7) % 8 } else { 0 };
        assert_eq!((o, e), (expected, 0), "x = {x}, k = {k}");
        both[k as usize] = true;
    }
    assert_eq!(both, [true, true]);
}

/// A field of 100 qubits: the in field's 13 bytes (the last with its four high bits 0, over an image byte of
/// 255) copied to the out field, which XORs them into a second register of 100 qubits.
#[test]
fn fields_wider_than_64_qubits_are_bytes_of_the_block() {
    let module = common::load(
        "memory 1024
data 64 = i64 1, i64 0, i64 100, i64 0, i64 1, i64 2, i64 0, i64 100, i64 16, i64 2
data 172 = i8 255
applied func @copy(i64 %b) {
entry:
    %w0 = load i64 %b
    %a8 = add i64 %b, 8
    %w1 = load i32 %a8
    %a12 = add i64 %b, 12
    %w2 = load i8 %a12
    %top = lshr i8 %w2, 4
    %clean = icmp eq i8 %top, 0
    br %clean, store, dirty
dirty:
    fault unreachable
store:
    %d0 = add i64 %b, 16
    store i64 %w0, %d0
    %d8 = add i64 %b, 24
    store i32 %w1, %d8
    %d12 = add i64 %b, 28
    store i8 %w2, %d12
    ret
}
func @main() -> i64 {
entry:
    %x = qalloc i64 100
    %o = qalloc i64 100
    %e = qalloc i64 5
    x %x[0]
    x %x[63]
    x %x[64]
    x %x[99]
    h %x[70]
    %fa = faddr @copy
    apply %fa budget 100, fault %e, image(160, 32), fields(64, 2), ctrl list(0, 0)
    %lo = measure i64 %o[0, 64]
    %hi = measure i64 %o[64, 36]
    %f = measure i64 %e[0, 5]
    %same = measure i64 %x[64, 36]
    %d = xor i64 %hi, %same
    %c = icmp eq i64 %d, 0
    %z = zext i1 %c to i64
    %h = shl i64 %f, 1
    %r = or i64 %h, %z
    %l = xor i64 %lo, -9223372036854775807
    %ok = icmp eq i64 %l, 0
    %k = zext i1 %ok to i64
    %kk = shl i64 %k, 8
    %rr = or i64 %r, %kk
    ret %rr
}",
    );
    for seed in 0..8 {
        let (outcome, _) = run(&module, "main", &[], Options { seed, ..bounded() });
        // The low 64 qubits of o equal x's (bits 0 and 63), the high 36 x's, and no fault.
        assert_eq!(outcome, Outcome::Returned(Some(1 << 8 | 1)), "seed {seed}");
    }
}

/// The checks of 6.2 in order, each the fault with the state unchanged.
#[test]
fn the_checks_run_in_the_interfaces_order() {
    let check = |apply: &str, data: &str| {
        let text = program(apply, AFFINE).replace("data 256 = i64 4, i64 0", data);
        let module = common::load(&text);
        let mut executor = executor(&module, "main", &[1], bounded());
        // Four allocations, four gates, the function address.
        for _ in 0..9 {
            assert_eq!(executor.step(), None, "{apply}");
        }
        let before = executor.snapshot();
        let outcome = executor.run();
        let after = executor.snapshot();
        if let Outcome::Faulted(_) = outcome {
            assert_eq!((after.quantum, after.classical.memory), (before.quantum, before.classical.memory), "{apply}");
        }
        outcome
    };
    let faulted = Outcome::Faulted;
    let list = "data 256 = i64 4, i64 0";
    let with = |field: &str| format!("data 256 = i64 4, i64 0\ndata 300 = {field}");
    let fields_at =
        |count: u64| format!("apply %fa budget 100, fault %e, image(160, 4), fields(300, {count}), ctrl list(256, %n)");
    for (apply, data, expected) in [
        // 1. An apply target.
        (
            "apply 0 budget 100, fault 0, image(0, 0), fields(0, 0), ctrl list(0, 0)".to_owned(),
            list.to_owned(),
            Fault::BadIndirect,
        ),
        (
            "apply 2 budget 100, fault %e, image(160, 4), fields(64, 2), ctrl list(256, %n)".to_owned(),
            list.to_owned(),
            Fault::BadIndirect,
        ),
        // 2. The fault register, then its width.
        (
            "apply %fa budget 100, fault 9, image(0, 0), fields(0, 0), ctrl list(0, 0)".to_owned(),
            list.to_owned(),
            Fault::DanglingRegister,
        ),
        (
            "apply %fa budget 100, fault %x, image(0, 0), fields(0, 0), ctrl list(0, 0)".to_owned(),
            list.to_owned(),
            Fault::BadRange,
        ),
        // 3. The size, the lists' byte counts.
        (
            "apply %fa budget 100, fault %e, image(160, 0), fields(64, 2), ctrl list(256, %n)".to_owned(),
            list.to_owned(),
            Fault::BadArgument,
        ),
        (
            "apply %fa budget 100, fault %e, image(0, 1025), fields(64, 2), ctrl list(256, %n)".to_owned(),
            list.to_owned(),
            Fault::BadArgument,
        ),
        (
            "apply %fa budget 100, fault %e, image(160, 4), fields(64, 461168601842738791), ctrl list(256, %n)"
                .to_owned(),
            list.to_owned(),
            Fault::BadArgument,
        ),
        (
            "apply %fa budget 100, fault %e, image(160, 4), fields(64, 2), ctrl list(256, 1152921504606846976)"
                .to_owned(),
            list.to_owned(),
            Fault::BadArgument,
        ),
        // 4. The image and the field list readable, then the control list's own checks.
        (
            "apply %fa budget 100, fault %e, image(1020, 5), fields(64, 2), ctrl list(256, %n)".to_owned(),
            list.to_owned(),
            Fault::BadArgument,
        ),
        (
            "apply %fa budget 100, fault %e, image(160, 4), fields(1000, 1), ctrl list(256, %n)".to_owned(),
            list.to_owned(),
            Fault::BadArgument,
        ),
        (APPLY.to_owned(), "data 256 = i64 9, i64 0".to_owned(), Fault::DanglingRegister),
        (APPLY.to_owned(), "data 256 = i64 4, i64 1".to_owned(), Fault::BadRange),
        // 5. Each field: direction, handle, width, slice, bytes in the block.
        (fields_at(1), with("i64 1, i64 0, i64 3, i64 0, i64 3"), Fault::BadArgument),
        (fields_at(1), with("i64 9, i64 0, i64 3, i64 0, i64 1"), Fault::BadArgument),
        (fields_at(1), with("i64 1, i64 0, i64 0, i64 0, i64 1"), Fault::BadArgument),
        (fields_at(1), with("i64 1, i64 1, i64 3, i64 0, i64 1"), Fault::BadArgument),
        (fields_at(1), with("i64 1, i64 0, i64 3, i64 4, i64 1"), Fault::BadArgument),
        // 6. Overlaps.
        (
            fields_at(2),
            with("i64 1, i64 0, i64 3, i64 0, i64 1, i64 2, i64 0, i64 3, i64 0, i64 2"),
            Fault::BadArgument,
        ),
        (
            fields_at(2),
            with("i64 2, i64 0, i64 2, i64 0, i64 2, i64 2, i64 1, i64 2, i64 1, i64 2"),
            Fault::BadArgument,
        ),
        (
            fields_at(2),
            with("i64 1, i64 0, i64 3, i64 0, i64 1, i64 1, i64 2, i64 1, i64 1, i64 2"),
            Fault::BadArgument,
        ),
        (fields_at(1), with("i64 3, i64 0, i64 3, i64 0, i64 1"), Fault::BadArgument),
        (fields_at(1), with("i64 4, i64 0, i64 1, i64 0, i64 2"), Fault::BadArgument),
        (
            "apply %fa budget 100, fault %e, image(160, 4), fields(64, 2), ctrl list(400, 1)".to_owned(),
            "data 256 = i64 4, i64 0\ndata 400 = i64 3, i64 4".to_owned(),
            Fault::BadArgument,
        ),
    ] {
        assert_eq!(check(&apply, &data), faulted(expected), "{apply} {data}");
    }
    // A qubit in several in fields, and in the control list and an in field, is allowed.
    let allowed = (
        fields_at(3),
        with("i64 1, i64 0, i64 3, i64 0, i64 1, i64 1, i64 0, i64 3, i64 2, i64 1, i64 2, i64 0, i64 3, i64 1, i64 2"),
    );
    assert!(matches!(check(&allowed.0, &allowed.1), Outcome::Returned(_)));
    let controlled_in = "apply %fa budget 100, fault %e, image(160, 4), fields(64, 2), ctrl list(400, 1)";
    assert!(matches!(check(controlled_in, "data 256 = i64 4, i64 0\ndata 400 = i64 1, i64 0"), Outcome::Returned(_)));
}

/// A block with no room between the stack floor and the stack pointer gives every input `out_of_memory`; a
/// block that leaves no room for the applied function's frame gives `stack_overflow`; a budget of 0 gives
/// `budget`. Each is a value in the fault register.
#[test]
fn placement_setup_and_budget_failures_are_fault_codes() {
    for (apply, code) in [
        ("apply %fa budget 100, fault %e, image(0, 1024), fields(64, 2), ctrl list(0, 0)", Fault::OutOfMemory),
        ("apply %fa budget 100, fault %e, image(0, 600), fields(64, 2), ctrl list(0, 0)", Fault::StackOverflow),
        ("apply %fa budget 0, fault %e, image(160, 4), fields(64, 2), ctrl list(0, 0)", Fault::Budget),
    ] {
        for value in outcomes(&program(apply, AFFINE), 0, bounded()) {
            let (_, o, e, _) = split(value);
            assert_eq!((o, e), (0, code.code()), "{apply}");
        }
    }
}

/// The step report of a run asked for it: the inputs evaluated and the largest count.
#[test]
fn the_step_report_counts_each_input_once() {
    let module = common::load(&program(APPLY, AFFINE));
    let mut executor = executor(&module, "main", &[0], bounded());
    executor.keep_apply_reports();
    assert!(matches!(executor.run(), Outcome::Returned(_)));
    let [report] = executor.apply_reports() else { panic!("one report") };
    // Eight values of x; nine steps each, the return included.
    assert_eq!(
        (report.position, report.budget, report.inputs, report.largest, report.at_budget),
        (1, 100, 8, Some(9), 0)
    );
}

#[test]
fn forward_then_reverse_restores_each_earlier_state() {
    let module = common::load(&unmeasured(&program(APPLY, AFFINE)));
    for n in [0, 1] {
        let mut executor = executor(&module, "main", &[n], Options { reversible: true, ..bounded() });
        let mut before = vec![executor.snapshot()];
        while executor.step().is_none() {
            before.push(executor.snapshot());
        }
        assert_eq!(executor.outcome(), Some(Outcome::Returned(Some(0))));
        while let Some(expected) = before.pop() {
            assert!(executor.step_back().expect("reversible"), "{n}");
            common::assert_same_state(&executor.snapshot(), &expected, &format!("{n}: step {}", before.len()));
        }
    }
}

/// Inside an applied function the new `apply` is a quantum instruction: with constant operands and one step
/// of budget left it gives `quantum_in_region`, before its own checks (here, function 0 is no apply target).
#[test]
fn inside_an_applied_function_it_is_quantum_in_region_first() {
    let module = common::load(
        "memory 256
func @f(i64 %a) -> i1 {
entry:
    apply 0 budget 0, fault 0, image(0, 0), fields(0, 0), ctrl list(0, 0)
    ret 0
}
func @main() -> i5 {
entry:
    %o = qalloc 1
    %f = qalloc 5
    apply @f(i64 0) -> %o, %f budget 1
    %m = measure %f
    ret %m
}",
    );
    let (outcome, _) = run(&module, "main", &[], common::allowing_region_faults());
    assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))));
}

/// A block that cannot be placed ends each enabled input's run before any step: the report counts the input,
/// with a largest count of 0 and none at the budget; with the control at 0 no input is enabled and the report
/// counts none.
#[test]
fn a_placement_failure_is_a_finished_input_of_no_steps_in_the_report() {
    for (control, inputs, largest) in [("x %c", 1, Some(0)), ("", 0, None)] {
        let module = common::load(&format!(
            "memory 256
data 0 = i64 1, i64 0
applied func @f(i64 %b) {{
entry:
    ret
}}
func @main() -> i5 {{
entry:
    %c = qalloc i64 1
    %e = qalloc i64 5
    {control}
    apply 1 budget 0, fault %e, image(0, 256), fields(0, 0), ctrl list(0, 1)
    %r = measure i64 %e[0, 5]
    %t = trunc i64 %r to i5
    ret %t
}}"
        ));
        let mut executor = executor(&module, "main", &[], bounded());
        executor.keep_apply_reports();
        let code = if inputs == 1 { Fault::OutOfMemory.code() } else { 0 };
        assert_eq!(executor.run(), Outcome::Returned(Some(u128::from(code))), "{control}");
        let [report] = executor.apply_reports() else { panic!("one report") };
        assert_eq!((report.inputs, report.largest, report.at_budget), (inputs, largest, 0), "{control}");
    }
}

/// An input is a value of the in fields (`docs/HANDLES.md` 6.1), counted once however many basis
/// configurations carry it: a spectator qubit in superposition doubles the configurations, not the inputs.
#[test]
fn the_report_counts_inputs_not_configurations() {
    for (prepare, inputs) in [("h %s", 1), ("h %x", 2), ("h %x\n    h %s", 2)] {
        let module = common::load(&format!(
            "memory 256
data 64 = i64 1, i64 0, i64 1, i64 0, i64 1
applied func @f(i64 %b) {{
entry:
    ret
}}
func @main() {{
entry:
    %x = qalloc i64 1
    %s = qalloc i64 1
    %e = qalloc i64 5
    {prepare}
    apply 1 budget 1, fault %e, image(0, 1), fields(64, 1), ctrl list(0, 0)
    ret
}}"
        ));
        let mut executor = executor(&module, "main", &[], bounded());
        executor.keep_apply_reports();
        assert_eq!(executor.run(), Outcome::Returned(None));
        assert_eq!(executor.apply_reports()[0].inputs, inputs, "{prepare}");
    }
}

/// The journal's observer must not take the memory at the call for the old value of a block byte: the block
/// holds the image and the in fields, set before the run. A first store over the in field's byte is not
/// recoverable from the memory at the call (it held 0 there, the input 0 or 1).
#[test]
fn a_first_store_over_the_block_is_not_credited_to_the_memory_at_the_call() {
    use bennett_exec::{WordClass, WordRole, classify};
    let module = common::load(
        "memory 256
data 16 = i64 1, i64 0, i64 1, i64 0, i64 1
applied func @f(i64 %b) {
entry:
    store i8 2, %b
    ret
}
func @main() {
entry:
    %x = qalloc 1
    %e = qalloc 5
    h %x
    apply 1 budget 2, fault %e, image(0, 1), fields(16, 1), ctrl list(0, 0)
    ret
}",
    );
    let mut executor = executor(&module, "main", &[], bennett_exec::Options::default());
    executor.measure_journal_content(true);
    assert_eq!(executor.run(), Outcome::Returned(None));
    let journal = executor.journal_measurement().expect("measured");
    assert_eq!(journal.applies[0].branches.len(), 2, "two inputs");
    let classes = classify(journal);
    let store = classes[0].site_count.iter().find(|p| p.role == WordRole::Store).expect("the store's old byte");
    assert_ne!(store.class, WordClass::Recoverable, "{store:?}");
}
