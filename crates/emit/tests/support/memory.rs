//! Private memory across both emitted control paths.

use bennett_emit::{
    ApplyError, ApplyInput, ApplyMemory, ApplyMode, Argument, CompiledApply, Construction, MemoryCounts,
    compile_apply_with,
};
use bennett_exec::{Executor, Options, Outcome, RunError};
use bennett_ir::{Fault, ValidModule, load};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::{bits, gates};

const MEMORY: usize = 1024;
const WIDTH: usize = 3;
const SAVED_VERSIONS: &str = r#"
func @f(i3 %x) -> i8 {
entry:
    %original = load i8 8
    store i3 %x, 8
    %private = load i8 8
    store i3 %x, 8
    store i8 34, 8
    %current = load i8 8
    %a = xor i8 %original, %private
    %result = xor i8 %a, %current
    ret %result
}
"#;

fn module(body: &str, setup: &str) -> ValidModule {
    module_with_width(body, setup, WIDTH)
}

fn module_with_width(body: &str, setup: &str, width: usize) -> ValidModule {
    let hadamards = (0..width).map(|bit| format!("    h %arg[{bit}]")).collect::<Vec<_>>().join("\n");
    load(&format!(
        r#"
memory {MEMORY}
{body}
func @main(i64 %budget) {{
entry:
{setup}
    %arg = qalloc {width}
{hadamards}
    %witness = qalloc 1
    x %witness ctrl %arg[0]
    s %witness
    %out = qalloc 8
    %fault = qalloc 5
    x %out[2]
    x %fault[4]
    apply @f(%arg) -> %out, %fault budget %budget
    ret
}}
"#
    ))
    .expect("valid memory program")
}

fn compare(
    module: &ValidModule,
    label: &str,
    budget: u64,
    mode: ApplyMode,
) -> (Construction, MemoryCounts, Vec<(u64, u64)>) {
    let entry = module.function_named("main").expect("main");
    let options = Options { machine_meaning: true, ..Options::default() };
    let mut direct = Executor::new(module, entry, &[budget], options).expect("executor");
    let mut emitted = Executor::new(module, entry, &[budget], options).expect("gate executor");
    let mut summary = None;
    let mut compiler = |input: ApplyInput<'_>| {
        let compiled = compile_apply_with(input, mode)?;
        println!("{label}, B={budget}, {mode:?}, @f: {}", compiled.memory);
        assert!(summary.replace((compiled.construction, compiled.memory)).is_none());
        Ok(compiled.block)
    };
    loop {
        let outcome = direct.step();
        let emitted_outcome = emitted.step_emitted(&mut compiler);
        assert_eq!(emitted_outcome, outcome, "{label}, B={budget}, {mode:?}");
        let actual = emitted.snapshot();
        let expected = direct.snapshot();
        assert!(actual == expected, "{label}, B={budget}, {mode:?}: snapshot mismatch");
        if let Some(outcome) = outcome {
            assert_eq!(outcome, Outcome::Returned(None));
            let columns = expected.quantum.registers.len();
            let values = expected
                .quantum
                .amplitudes
                .iter()
                .map(|(config, _)| (config[columns - 2] ^ 4, config[columns - 1] ^ 16))
                .collect();
            let (construction, memory) = summary.expect("an emitted apply");
            return (construction, memory, values);
        }
    }
}

fn bounds(budget: u64, at: u64, result: Result<u64, Fault>) -> (u64, u64) {
    if budget < at {
        return (0, Fault::Budget.code());
    }
    match result {
        Ok(output) => (output, 0),
        Err(fault) => (0, fault.code()),
    }
}

fn isolated(body: &str) -> CompiledApply {
    let module = module(body, "");
    let held = Default::default();
    let compiled = compile_apply_with(
        ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("f"),
            args: &[Argument::Register(0..3)],
            out: 3..11,
            fault: 11..16,
            budget: 8,
            entry_fault: None,
            stack_available: MEMORY as u64,
            memory: Some(ApplyMemory::new(&[0; MEMORY], &held)),
            scratch_start: 16,
        },
        ApplyMode::Automatic,
    )
    .expect("isolated memory example emits");
    println!("isolated @f: {}", compiled.memory);
    compiled
}

#[test]
fn definite_stores_have_no_physical_cost() {
    for store in [
        "store i8 255, 8",
        "%bit = trunc i3 %x to i1\n    %amount = zext i1 %bit to i8\n    %zero = shl i8 0, %amount\n    store i8 %zero, 8",
    ] {
        let compiled = isolated(&format!("func @f(i3 %x) -> i8 {{\nentry:\n    {store}\n    ret 0\n}}"));
        let (gates, layout) = gates(&compiled, WIDTH, 13);
        assert_eq!((layout.scratch, gates.len()), (0, 0), "{store}");
        assert_eq!(compiled.memory, MemoryCounts::default(), "{store}");
    }
}

#[test]
fn shared_source_bits_preserve_equality_and_count_each_live_byte_version() {
    let compiled = isolated(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %bit = trunc i3 %x to i1
    store i1 %bit, 8
    %copy = load i1 8
    store i1 %copy, 9
    %again = load i1 9
    %zero = xor i1 %again, %bit
    %answer = zext i1 %zero to i8
    ret %answer
}
"#,
    );
    let (gates, layout) = gates(&compiled, WIDTH, 13);
    bits::exhaustive(&gates, layout, |_| 0).expect("preserved data and clear scratch");
    assert_eq!((layout.scratch, gates.len()), (0, 0));
    assert_eq!(compiled.memory, MemoryCounts { peak_nonconstant_bits: 2, ..MemoryCounts::default() });
}

#[test]
fn a_computed_zero_is_a_definite_address() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %bit = trunc i3 %x to i1
    %amount = zext i1 %bit to i64
    %zero = shl i64 0, %amount
    %r = load i8 %zero
    ret %r
}
"#,
        "    store i8 5, 0",
    );
    for budget in 3..=6 {
        let (_, counts, values) = compare(&module, "computed address", budget, ApplyMode::Automatic);
        assert_eq!(values, vec![bounds(budget, 5, Ok(5)); 8]);
        assert_eq!(counts, MemoryCounts::default());
    }
}

#[test]
fn exhausted_scratch_cannot_prove_a_constant_product() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %low = trunc i3 %x to i2
    %complement = xor i2 %low, 3
    %product = mul i2 %low, %complement
    store i2 %product, 8
    %result = load i8 8
    ret %result
}
"#,
        "",
    );
    let held = Default::default();
    let input = ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("f"),
        args: &[Argument::Register(0..3)],
        out: 3..11,
        fault: 11..16,
        budget: 0,
        entry_fault: None,
        stack_available: MEMORY as u64,
        memory: Some(ApplyMemory::new(&[0; MEMORY], &held)),
        scratch_start: 16,
    };
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=7 {
            let (_, _, values) = compare(&module, "product of complements", budget, mode);
            for scratch_start in [u64::MAX, u64::MAX - 1, u64::MAX - 2] {
                let limited = compile_apply_with(ApplyInput { budget, scratch_start, ..input.clone() }, mode);
                if budget < 3 {
                    let compiled = limited.expect("budget expires before the product");
                    println!("limited scratch, B={budget}, {mode:?}, @f: {}", compiled.memory);
                    assert_eq!(compiled.memory, MemoryCounts::default());
                } else {
                    assert_eq!(limited.expect_err("product needs scratch"), ApplyError::WireSpace);
                }
            }
            let compiled = compile_apply_with(ApplyInput { budget, ..input.clone() }, mode).expect("room for product");
            println!("definite inputs, B={budget}, {mode:?}, @f: {}", compiled.memory);
            let (gates, layout) = gates(&compiled, WIDTH, 13);
            bits::exhaustive(&gates, layout, |x| {
                let (result, fault) = values[x as usize];
                u128::from(result | fault << 8)
            })
            .expect("executor outcomes with preserved inputs and zero scratch");
        }
    }
}

#[test]
fn arithmetic_copy_equality_proves_a_definite_store_and_load_address() {
    const PREFIX: &str = r#"
func @f(i3 %x) -> i8 {
entry:
    %bit = trunc i3 %x to i1
    %square = mul i1 %bit, %bit
    %zero = xor i1 %square, %bit
"#;
    let without = isolated(&format!("{PREFIX}    ret 0\n}}"));
    let with = isolated(&format!("{PREFIX}    store i1 %zero, 8\n    ret 0\n}}"));
    let (base_gates, base_layout) = gates(&without, WIDTH, 13);
    let (store_gates, store_layout) = gates(&with, WIDTH, 13);
    assert_eq!((store_layout.scratch, store_gates.len()), (base_layout.scratch, base_gates.len()));
    assert_eq!(with.memory, MemoryCounts::default());

    let module = module(
        &format!(
            "{PREFIX}    store i1 %zero, 8\n    %saved = load i1 8\n    %addr = zext i1 %saved to i64\n    %r = load i8 %addr\n    ret %r\n}}"
        ),
        "    store i8 5, 0\n    store i8 9, 1\n    store i8 1, 8",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=9 {
            let (_, counts, values) = compare(&module, "square, store and address", budget, mode);
            assert_eq!(counts, MemoryCounts::default());
            assert_eq!(values, vec![bounds(budget, 8, Ok(5)); 8]);
        }
    }
}

#[test]
fn an_arithmetic_copy_does_not_keep_equality_to_reused_scratch() {
    let module = module_with_width(
        r#"
func @f(i2 %x) -> i8 {
entry:
    %a = trunc i2 %x to i1
    %hi = lshr i2 %x, 1
    %b = trunc i2 %hi to i1
    %product = mul i1 %a, %b
    %sum = xor i1 %a, %b
    %either = xor i1 %product, %sum
    %result = zext i1 %either to i8
    ret %result
}
"#,
        "",
        2,
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let (_, counts, values) = compare(&module, "arithmetic scratch reuse", 8, mode);
        assert_eq!(counts, MemoryCounts::default());
        assert_eq!(values, [(0, 0), (1, 0), (1, 0), (1, 0)]);
    }
}

#[test]
fn memory_free_paths_after_a_load_compile_in_both_modes() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %value = load i8 8
    %odd = trunc i3 %x to i1
    br %odd, yes, no
yes:
    ret %value
no:
    ret 0
cold:
    store i8 42, 8
    ret 42
}
"#,
        "    store i8 7, 8",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=5 {
            let (_, counts, values) = compare(&module, "load before a choice", budget, mode);
            assert_eq!(counts, MemoryCounts::default());
            assert_eq!(values, (0..8).map(|x| bounds(budget, 4, Ok((x & 1) * 7))).collect::<Vec<_>>());
        }
    }
}

#[test]
fn a_conditional_fault_can_follow_the_last_load() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %n = load i8 8
    %odd = trunc i3 %x to i1
    %d = zext i1 %odd to i8
    %q = udiv i8 %n, %d
    ret %q
}
"#,
        "    store i8 7, 8",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 3..=6 {
            let (_, counts, values) = compare(&module, "load before division", budget, mode);
            assert_eq!(counts, MemoryCounts::default());
            assert_eq!(
                values,
                (0..8)
                    .map(|x| {
                        if x & 1 == 0 { bounds(budget, 4, Err(Fault::DivideByZero)) } else { bounds(budget, 5, Ok(7)) }
                    })
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn a_branch_can_exhaust_its_budget_before_a_guarded_store() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %odd = trunc i3 %x to i1
    br %odd, yes, no
yes:
    store i8 9, 8
    ret 3
no:
    ret 0
}
"#,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let (_, counts, values) = compare(&module, "before the guarded store", 2, mode);
        assert_eq!(counts, MemoryCounts::default());
        assert_eq!(values, vec![(0, Fault::Budget.code()); 8]);
    }
}

#[test]
fn a_store_keeps_qubit_dependent_bits_above_the_first_byte() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %wide = zext i3 %x to i16
    %high = shl i16 %wide, 8
    store i16 %high, 8
    %value = load i8 9
    ret %value
}
"#,
        "",
    );
    for budget in [5, 4, 6] {
        let (_, counts, values) = compare(&module, "high byte", budget, ApplyMode::Automatic);
        assert_eq!(values, (0..8).map(|x| bounds(budget, 5, Ok(x))).collect::<Vec<_>>());
        assert_eq!(counts, MemoryCounts { peak_nonconstant_bits: 3, ..MemoryCounts::default() });
    }
}

#[test]
fn stores_retain_loaded_versions_and_definite_stores_emit_nothing() {
    let module = module(SAVED_VERSIONS, "    store i8 192, 8");
    let mut image = [0; MEMORY];
    image[8] = 192;
    let held = Default::default();
    let input = ApplyInput {
        module: &module,
        callee: module.function_named("f").expect("f"),
        args: &[Argument::Register(0..3)],
        out: 3..11,
        fault: 11..16,
        budget: 0,
        entry_fault: None,
        stack_available: MEMORY as u64,
        memory: Some(ApplyMemory::new(&image, &held)),
        scratch_start: 16,
    };

    for budget in 0..=10 {
        let expected = |x| bounds(budget, 9, Ok(192 ^ x ^ 34));
        let (construction, stats, actual) = compare(&module, "versions", budget, ApplyMode::Automatic);
        assert_eq!(construction, Construction::Definite);
        assert_eq!(actual, (0..8).map(expected).collect::<Vec<_>>());
        let counts = MemoryCounts { peak_nonconstant_bits: if budget < 2 { 0 } else { 3 }, ..MemoryCounts::default() };
        assert_eq!(stats, counts);

        let compiled = compile_apply_with(ApplyInput { budget, ..input.clone() }, ApplyMode::Automatic).expect("bits");
        println!("versions, B={budget}, definite inputs, @f: {}", compiled.memory);
        assert_eq!(compiled.memory, counts);
        let (gates, layout) = gates(&compiled, WIDTH, 13);
        bits::exhaustive(&gates, layout, |x| {
            let (result, fault) = expected(x as u64);
            u128::from(result | fault << 8)
        })
        .expect("all arguments, patterned destinations, and clean scratch");
    }
}

#[test]
fn held_zero_and_out_of_bounds_respect_the_first_attempt() {
    let cases = [
        ("%v = load i16 8", Fault::QubitMemoryAccess),
        ("store i16 65535, 8", Fault::QubitMemoryAccess),
        ("%v = load i16 1023", Fault::OutOfBounds),
        ("store i16 65535, 1023", Fault::OutOfBounds),
        ("%v = load i8 18446744073709551615", Fault::OutOfBounds),
    ];
    for (operation, fault) in cases {
        let module = module(
            &format!("func @f(i3 %x) -> i8 {{\nentry:\n    {operation}\n    ret 5\n}}"),
            "    x [9:0]\n    x [9:0]\n    x [1023:7]\n    x [1023:7]",
        );
        for budget in 0..=2 {
            let (_, counts, actual) = compare(&module, operation, budget, ApplyMode::Automatic);
            assert_eq!(counts, MemoryCounts::default());
            assert_eq!(actual, vec![bounds(budget, 1, Err(fault)); 8]);
        }
    }
}

#[test]
fn a_varying_address_is_refused_only_when_attempted() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %p = zext i3 %x to i64
    %r = load i8 %p
    ret %r
}
"#,
        "",
    );
    compare(&module, "before varying address", 1, ApplyMode::Automatic);
    let entry = module.function_named("main").expect("main");
    let mut executor = Executor::new(&module, entry, &[2], Options::default()).expect("executor");
    let mut compile = |input: ApplyInput<'_>| Ok(compile_apply_with(input, ApplyMode::Automatic)?.block);
    assert!(matches!(
        executor.run_emitted(&mut compile),
        Outcome::Stopped(RunError::Apply(bennett_exec::ApplyError::Unsupported {
            operation: "load",
            route: "a proved address bound within a declared memory region",
            ..
        }))
    ));
}

#[test]
fn seeded_private_memory_programs_agree_on_complex_amplitudes() {
    let mut rng = ChaCha8Rng::seed_from_u64(0xd378_1594_62fe_961c);
    for case in 0..300 {
        let a = rng.random_range(32..36);
        let b = rng.random_range(32..36);
        let literal = rng.random::<u16>();
        let choice = ["0", "1", "%odd"][case % 3];
        let mode = [ApplyMode::Automatic, ApplyMode::StepMachine][case % 2];
        let budget = rng.random_range(0..=19);
        let module = module(
            &format!(
                r#"
func @f(i3 %x) -> i8 {{
entry:
    %expanded = zext i3 %x to i16
    %high = shl i16 %expanded, 8
    %both = or i16 %expanded, %high
    %value = xor i16 %both, {literal}
    call @write(%value)
    %odd = trunc i3 %x to i1
    br {choice}, overwrite, read
overwrite:
    %narrow = trunc i16 %value to i9
    store i9 %narrow, {b}
    br read
read:
    %left = load i16 {a}
    %right = load i16 {b}
    %sum = add i16 %left, %right
    %shift = lshr i16 %sum, 6
    %answer = trunc i16 %shift to i8
    ret %answer
}}
func @write(i16 %value) {{
entry:
    store i16 %value, {a}
    ret
}}
"#,
            ),
            "    store i32 306247156, 32",
        );
        compare(&module, &format!("seeded case {case}"), budget, mode);
    }
}

#[test]
fn guarded_stores_and_loads_share_only_their_own_updates() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %odd = trunc i3 %x to i1
    br %odd, write, read
write:
    %v = zext i3 %x to i9
    store i9 %v, 8
    br join
read:
    %old = load i16 8
    %low = trunc i16 %old to i8
    ret %low
join:
    %after = load i8 8
    ret %after
}
"#,
        "    store i16 4660, 8",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=8 {
            let (_, counts, values) = compare(&module, "guarded overlapping access", budget, mode);
            assert_eq!(
                values,
                (0..8)
                    .map(|x| if x & 1 == 0 { bounds(budget, 5, Ok(52)) } else { bounds(budget, 7, Ok(x)) })
                    .collect::<Vec<_>>()
            );
            let expected_counts = if budget < 4 {
                MemoryCounts::default()
            } else {
                MemoryCounts {
                    retained_scratch_qubits: 2,
                    gates: 8,
                    peak_nonconstant_bits: 7,
                    ..MemoryCounts::default()
                }
            };
            assert_eq!(counts, expected_counts);
        }
    }
}

#[test]
fn guarded_memory_faults_precede_later_operations() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    switch i3 %x, 0: held, 1: bounds, default: done
held:
    store i16 1, 8
    ret 1
bounds:
    %v = load i16 1023
    ret 1
done:
    ret 2
}
"#,
        "    x [9:0]\n    x [9:0]\n    x [1023:0]\n    x [1023:0]",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=4 {
            let (_, counts, values) = compare(&module, "guarded first fault", budget, mode);
            assert_eq!(counts, MemoryCounts::default());
            assert_eq!(values[0], bounds(budget, 2, Err(Fault::QubitMemoryAccess)));
            assert_eq!(values[1], bounds(budget, 2, Err(Fault::OutOfBounds)));
            assert_eq!(values[2..], [bounds(budget, 2, Ok(2)); 6]);
        }
    }
}

#[test]
fn copied_stack_frames_restore_their_own_pointers_and_keep_bytes() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %before = stacksave
    %odd = trunc i3 %x to i1
    br %odd, write, done
write:
    %p = alloca 3
    %v = zext i3 %x to i8
    store i8 %v, %p
    %from_child = call @child(%p)
    %after = stacksave
    %same = icmp eq i64 %after, %p
    stackrestore %before
    %restored = stacksave
    %again = icmp eq i64 %restored, %before
    %ok = and i1 %same, %again
    %v2 = load i8 %p
    %r = xor i8 %from_child, %v2
    %answer = select i8 %ok, %r, 255
    ret %answer
done:
    ret 17
}
func @child(i64 %parent) -> i8 {
entry:
    %own = stacksave
    %p = alloca 17
    %delta = sub i64 %own, %p
    %value = trunc i64 %delta to i8
    %old = load i8 %parent
    %v = add i8 %value, %old
    %unaligned = sub i64 %own, 1
    stackrestore %unaligned
    ret %v
}
"#,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=27 {
            let (_, _, values) = compare(&module, "copied stack frames", budget, mode);
            assert_eq!(
                values,
                (0..8)
                    .map(|x| if x & 1 == 0 { bounds(budget, 4, Ok(17)) } else { bounds(budget, 26, Ok((x + 24) ^ x)) })
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn a_callee_cannot_restore_one_byte_above_its_own_ceiling() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %odd = trunc i3 %x to i1
    br %odd, enter, done
enter:
    %r = call @child()
    ret %r
done:
    ret 3
}
func @child() -> i8 {
entry:
    %top = stacksave
    %above = add i64 %top, 1
    stackrestore %above
    ret 19
}
"#,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=9 {
            let (_, counts, values) = compare(&module, "callee restore ceiling", budget, mode);
            assert_eq!(counts, MemoryCounts::default());
            let expected = (0..8).map(|x| {
                if x & 1 == 0 { bounds(budget, 3, Ok(3)) } else { bounds(budget, 6, Err(Fault::BadStackRestore)) }
            });
            assert_eq!(values, expected.collect::<Vec<_>>());
        }
    }
}

#[test]
fn stack_faults_freeze_at_the_failing_attempt() {
    let cases = [
        ("%small = alloca %size\n    %r = call @child()\n    ret %r", 3, Fault::StackOverflow),
        (
            "%small = alloca %size\n    %above = add i64 %top, %size\n    stackrestore %above\n    ret 7",
            4,
            Fault::BadStackRestore,
        ),
        (
            "%small = alloca %size\n    %below = sub i64 %small, 1\n    stackrestore %below\n    ret 7",
            4,
            Fault::BadStackRestore,
        ),
        ("%small = alloca %huge\n    ret 7", 2, Fault::StackOverflow),
    ];
    for (body, fault_at, code) in cases {
        let module = module(
            &format!(
                r#"
func @f(i3 %x) -> i8 {{
entry:
    %top = stacksave
    %odd = trunc i3 %x to i1
    br %odd, try, done
try:
    %size = select i64 %odd, 912, 0
    %huge = select i64 %odd, 18446744073709551615, 0
    {body}
done:
    ret 4
}}
func @child() -> i8 {{
entry:
    %one = stacksave
    %two = stacksave
    %three = stacksave
    %four = stacksave
    %five = stacksave
    %six = stacksave
    %seven = stacksave
    %eight = stacksave
    ret 7
}}
"#
            ),
            "",
        );
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in 0..=10 {
                let (_, counts, values) = compare(&module, "stack first fault", budget, mode);
                assert_eq!(counts, MemoryCounts::default());
                assert_eq!(
                    values,
                    (0..8)
                        .map(|x| if x & 1 == 0 {
                            bounds(budget, 4, Ok(4))
                        } else {
                            bounds(budget, 4 + fault_at, Err(code))
                        })
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}

#[test]
fn nested_memory_loops_spend_one_shared_budget() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %limit = trunc i3 %x to i2
    br outer(%limit)
outer(i2 %left):
    %done = icmp eq i2 %left, 0
    br %done, finish, inner(%left)
inner(i2 %in):
    %old = load i8 8
    %new = add i8 %old, 1
    store i8 %new, 8
    %next = sub i2 %in, 1
    %again = icmp ne i2 %next, 0
    br %again, inner(%next), leave
leave:
    %remaining = sub i2 %left, 1
    br outer(%remaining)
finish:
    %r = load i8 8
    ret %r
}
"#,
        "    store i8 5, 8",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=55 {
            let (_, _, values) = compare(&module, "nested guarded stores", budget, mode);
            let expected = (0..8).map(|x| {
                let n = x & 3;
                let stores = n * (n + 1) / 2;
                bounds(budget, 6 + 4 * n + 6 * stores, Ok(5 + stores))
            });
            assert_eq!(values, expected.collect::<Vec<_>>());
        }
    }
}

#[test]
fn stack_allocation_checks_the_floor_after_alignment() {
    let module = module(
        r#"
data 956 = i8 0
func @f(i3 %x) -> i8 {
entry:
    %p = alloca 9
    %q = stacksave
    ret 5
}
"#,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=4 {
            let (_, counts, values) = compare(&module, "stack alignment at the floor", budget, mode);
            assert_eq!(counts, MemoryCounts::default());
            assert_eq!(values, vec![bounds(budget, 1, Err(Fault::StackOverflow)); 8]);
        }
    }
}

#[test]
fn conditional_and_switch_destinations_can_share_the_same_load() {
    for instruction in ["br %x, read, read", "switch i1 %x, 0: read, default: read"] {
        let module = module_with_width(
            &format!(
                r#"
func @f(i1 %x) -> i8 {{
entry:
    {instruction}
read:
    %v = load i8 0
    ret %v
}}
"#
            ),
            "    store i8 5, 0",
            1,
        );
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in 0..=4 {
                let (_, counts, values) = compare(&module, instruction, budget, mode);
                assert_eq!(counts, MemoryCounts::default());
                assert_eq!(values, [bounds(budget, 3, Ok(5)); 2]);
            }
        }
    }
}

#[test]
fn declared_lookup_reads_caller_and_private_definite_bytes() {
    let module = module(
        r#"
data 32 = i64 578437695752307201
func @f(i3 %x) -> i8 {
entry:
    %offset = zext i3 %x to i64
    %addr = or i64 %offset, 32
    store i8 91, 34
    %value = load i8 %addr
    ret %value
}
"#,
        "    store i8 46, 35",
    );
    let table = [1, 2, 91, 46, 5, 6, 7, 8];
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=6 {
            let (_, counts, values) = compare(&module, "declared lookup", budget, mode);
            assert_eq!(values, table.map(|value| bounds(budget, 5, Ok(value))));
            assert_eq!(counts.peak_nonconstant_bits, 0);
            if budget < 4 {
                assert_eq!(counts, MemoryCounts::default());
            } else {
                assert_eq!((counts.varying_loads, counts.varying_stores, counts.candidate_bytes), (1, 0, 8));
                assert_eq!(counts.address_scratch_qubits, counts.retained_scratch_qubits);
                assert!(counts.gates > 0 && counts.address_toffolis > 0);
            }
        }
    }
}

#[test]
fn varying_load_preserves_fault_order_and_the_successful_guard() {
    let module = module(
        r#"
data 16 = i32 12584
func @f(i3 %x) -> i8 {
entry:
    %skip = icmp eq i3 %x, 7
    br %skip, done, choose
choose:
    %odd = trunc i3 %x to i1
    %outside = icmp uge i3 %x, 4
    %low = select i64 %odd, 18, 16
    %high = select i64 %odd, 18446744073709551615, 1023
    %addr = select i64 %outside, %high, %low
    %word = load i16 %addr
    %value = trunc i16 %word to i8
    store i8 9, 32
    ret %value
done:
    ret 55
}
"#,
        "    store i16 4660, 16\n    x [18:0]\n    x [18:0]\n    x [1023:0]\n    x [1023:0]",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=12 {
            let (_, _, values) = compare(&module, "varying load faults", budget, mode);
            let expected = [
                bounds(budget, 11, Ok(52)),
                bounds(budget, 8, Err(Fault::QubitMemoryAccess)),
                bounds(budget, 11, Ok(52)),
                bounds(budget, 8, Err(Fault::QubitMemoryAccess)),
                bounds(budget, 8, Err(Fault::OutOfBounds)),
                bounds(budget, 8, Err(Fault::OutOfBounds)),
                bounds(budget, 8, Err(Fault::OutOfBounds)),
                bounds(budget, 3, Ok(55)),
            ];
            assert_eq!(values, expected);
        }
    }
}

#[test]
fn every_load_candidate_touching_the_same_held_byte_faults() {
    let module = module(
        r#"
data 32 = i64 0
func @f(i3 %x) -> i8 {
entry:
    %low = trunc i3 %x to i2
    %offset = zext i2 %low to i64
    %p = or i64 %offset, 32
    %word = load i16 %p
    %r = trunc i16 %word to i8
    ret %r
}
"#,
        "    x [33:0]\n    x [33:0]",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=7 {
            let (_, _, values) = compare(&module, "overlapping held-byte candidates", budget, mode);
            let expected = (0..8).map(|x| {
                if x & 3 < 2 { bounds(budget, 4, Err(Fault::QubitMemoryAccess)) } else { bounds(budget, 6, Ok(0)) }
            });
            assert_eq!(values, expected.collect::<Vec<_>>());
        }
    }
}

#[test]
fn no_configuration_runs_a_store_after_a_faulting_lookup() {
    let module = module(
        r#"
data 16 = i16 0
func @f(i3 %x) -> i8 {
entry:
    %odd = trunc i3 %x to i1
    %p = select i64 %odd, 1023, 16
    %word = load i16 %p
    %q = zext i3 %x to i64
    store i16 %word, %q
    ret 5
}
"#,
        "    x [16:0]\n    x [16:0]\n    x [1023:0]\n    x [1023:0]",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=7 {
            let (_, _, values) = compare(&module, "lookup faults on every input", budget, mode);
            assert_eq!(
                values,
                (0..8)
                    .map(|x| bounds(
                        budget,
                        3,
                        Err(if x & 1 == 1 { Fault::OutOfBounds } else { Fault::QubitMemoryAccess })
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn known_address_bits_bound_a_lookup_with_a_wider_argument() {
    let module = load(
        r#"
memory 1024
data 32 = i64 578437695752307201
func @f(i9 %x) -> i8 {
entry:
    %i = trunc i9 %x to i3
    %o = zext i3 %i to i64
    %p = or i64 %o, 32
    %r = load i8 %p
    ret %r
}
"#,
    )
    .expect("wider table module");
    let held = Default::default();
    let mut image = [0u8; MEMORY];
    image[32..40].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=6 {
            let compiled = compile_apply_with(
                ApplyInput {
                    module: &module,
                    callee: module.function_named("f").expect("f"),
                    args: &[Argument::Register(0..9)],
                    out: 9..17,
                    fault: 17..22,
                    budget,
                    entry_fault: None,
                    stack_available: 900,
                    memory: Some(ApplyMemory::new(&image, &held)),
                    scratch_start: 22,
                },
                mode,
            )
            .expect("lookup using known address bits");
            println!("wider table, B={budget}, {mode:?}, @f: {}", compiled.memory);
            let (gates, layout) = gates(&compiled, 9, 13);
            let states = (0..(1 << layout.x)).flat_map(|x| [0, 0x1004].map(|y| (x, y)));
            bits::check_states(&gates, layout, states, |x| {
                let (value, fault) = bounds(budget, 5, Ok((x & 7) as u64 + 1));
                u128::from(value | fault << 8)
            })
            .expect("all nine-bit input values");
        }
    }
}

fn check_wide_lookup(declaration: &str, address: &str, width: usize, expected: [Result<u16, Fault>; 2]) {
    let module = load(&format!(
        r#"
memory {MEMORY}
{declaration}
func @f(i9 %x) -> i{width} {{
entry:
    %bit = trunc i9 %x to i1
    %low = zext i1 %bit to i64
{address}
    %value = load i{width} %p
    ret %value
}}
"#
    ))
    .expect("wide lookup module");
    let mut image = [0; MEMORY];
    for segment in &module.data {
        let address = segment.address as usize;
        let bytes = segment.encode();
        image[address..address + bytes.len()].copy_from_slice(&bytes);
    }
    let held = Default::default();
    let fault_start = (9 + width) as u64;
    let scratch_start = fault_start + u64::from(Fault::CODE_WIDTH.bits());
    // Compile the access in isolation so data may reach the last byte without reserving a caller frame.
    let compiled = compile_apply_with(
        ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("f"),
            args: &[Argument::Register(0..9)],
            out: 9..fault_start,
            fault: fault_start..scratch_start,
            budget: 7,
            entry_fault: None,
            stack_available: MEMORY as u64,
            memory: Some(ApplyMemory::new(&image, &held)),
            scratch_start,
        },
        ApplyMode::StepMachine,
    )
    .expect("wide lookup emits a value or an out-of-bounds fault");
    let (gates, layout) = gates(&compiled, 9, (scratch_start - 9) as usize);
    let states = (0..512).flat_map(|x| [0, (16 << width) | 4].map(|y| (x, y)));
    bits::check_states(&gates, layout, states, |x| match expected[(x & 1) as usize] {
        Ok(value) => u128::from(value),
        Err(fault) => u128::from(fault.code()) << width,
    })
    .expect("exact wide lookup values and faults");
}

#[test]
fn wide_lookup_faults_when_all_addresses_are_outside_memory() {
    check_wide_lookup("", "    %p = or i64 %low, 1024", 8, [Err(Fault::OutOfBounds); 2]);
}

#[test]
fn wide_lookup_can_read_the_last_byte_or_fault() {
    check_wide_lookup(
        "data 1023 = i8 167",
        "    %high = shl i64 %low, 10\n    %p = or i64 %high, 1023",
        8,
        [Ok(167), Err(Fault::OutOfBounds)],
    );
}

#[test]
fn wide_lookup_reads_two_bytes_at_either_start() {
    for start in [0, 32] {
        check_wide_lookup(
            &format!("data {start} = i8 18, i8 52, i8 86"),
            &format!("    %p = or i64 %low, {start}"),
            16,
            [Ok(0x3412), Ok(0x5634)],
        );
    }
}

#[test]
fn seeded_declared_lookups_match_the_executor() {
    let mut rng = ChaCha8Rng::seed_from_u64(0x1d29_4c1d_3816_83e0);
    for case in 0..120 {
        let table = rng.random::<u64>();
        let overwrite = rng.random::<u8>();
        let index = rng.random_range(0..8);
        let budget = rng.random_range(3..=6);
        let mode = [ApplyMode::Automatic, ApplyMode::StepMachine][case % 2];
        let module = module(
            &format!(
                r#"
data 32 = i64 {table}
func @f(i3 %x) -> i8 {{
entry:
    %i = zext i3 %x to i64
    %p = or i64 %i, 32
    %value = load i8 %p
    ret %value
}}
"#
            ),
            &format!("    store i8 {overwrite}, {}", 32 + index),
        );
        compare(&module, &format!("seeded table {case}"), budget, mode);
    }
}

#[test]
fn a_varying_load_checks_the_full_span_against_its_declaration() {
    let module = module(
        r#"
data 32 = i16 513
func @f(i3 %x) -> i8 {
entry:
    %low = trunc i3 %x to i1
    %i = zext i1 %low to i64
    %p = or i64 %i, 32
    %word = load i16 %p
    %r = trunc i16 %word to i8
    ret %r
}
"#,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        compare(&module, "before an undeclared span", 3, mode);
        let entry = module.function_named("main").expect("main");
        let mut executor = Executor::new(&module, entry, &[4], Options::default()).expect("executor");
        let mut compiler = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
        assert!(matches!(
            executor.run_emitted(&mut compiler),
            Outcome::Stopped(RunError::Apply(ApplyError::Unsupported {
                operation: "load",
                route: "a proved address bound within a declared memory region",
                ..
            }))
        ));
    }
}

#[test]
fn a_uniform_lookup_needs_no_memory_qubits_or_gates() {
    let module = module(
        r#"
data 32 = i64 0
func @f(i3 %x) -> i8 {
entry:
    %offset = zext i3 %x to i64
    %address = or i64 %offset, 32
    %value = load i8 %address
    ret %value
}
"#,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 2..=5 {
            let (_, counts, values) = compare(&module, "uniform declared lookup", budget, mode);
            let expected = if budget < 3 {
                MemoryCounts::default()
            } else {
                MemoryCounts { varying_loads: 1, candidate_bytes: 8, ..MemoryCounts::default() }
            };
            assert_eq!(counts, expected);
            assert_eq!(values, vec![bounds(budget, 4, Ok(0)); 8]);
        }
    }
}

#[test]
fn overlapping_unaligned_store_candidates_commit_one_version_per_byte() {
    let module = module(
        r#"
data 32 = i32 0
func @f(i3 %x) -> i8 {
entry:
    %index = trunc i3 %x to i1
    %offset = zext i1 %index to i64
    %p = or i64 %offset, 32
    %shift = lshr i3 %x, 1
    %data = zext i3 %shift to i16
    %complement = xor i16 %data, 3
    %high = shl i16 %complement, 8
    %v = or i16 %data, %high
    store i16 %v, %p
    %r = load i16 %p
    %low = trunc i16 %r to i8
    %upper = lshr i16 %r, 8
    %hi = trunc i16 %upper to i8
    %shifted = shl i8 %hi, 2
    %result = or i8 %low, %shifted
    ret %result
}
"#,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=17 {
            let (_, counts, values) = compare(&module, "overlapping unaligned candidates", budget, mode);
            let expected = (0..8).map(|x| {
                let low = x >> 1;
                bounds(budget, 16, Ok(low | (low ^ 3) << 2))
            });
            assert_eq!(values, expected.collect::<Vec<_>>());
            if budget == 9 {
                // There are two varying bits in each of three bytes. The middle byte
                // uses both address guards and has only two retained storage wires.
                assert_eq!(counts.retained_scratch_qubits, 6);
                assert_eq!(counts.address_scratch_qubits, 6);
                assert_eq!(counts.peak_nonconstant_bits, 6);
                assert_eq!(counts.address_toffolis, 16);
                assert_eq!((counts.varying_loads, counts.varying_stores, counts.candidate_bytes), (0, 1, 4));
            }
            if budget >= 10 {
                assert_eq!((counts.varying_loads, counts.varying_stores, counts.candidate_bytes), (1, 1, 8));
                assert_eq!(counts.peak_nonconstant_bits, 6);
            }
        }
    }
}

#[test]
fn dynamic_stores_fault_before_updating_any_byte() {
    let module = module(
        r#"
data 16 = i32 0
func @f(i3 %x) -> i8 {
entry:
    %odd = trunc i3 %x to i1
    %outside = icmp uge i3 %x, 4
    %low = select i64 %odd, 18, 16
    %high = select i64 %odd, 18446744073709551615, 1023
    %addr = select i64 %outside, %high, %low
    store i16 441, %addr
    %r = load i8 16
    ret %r
}
"#,
        "    x [18:0]\n    x [18:0]\n    x [1023:0]\n    x [1023:0]",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=9 {
            let (_, counts, values) = compare(&module, "varying store faults", budget, mode);
            let expected = (0..8).map(|x| match x {
                0 | 2 => bounds(budget, 8, Ok(185)),
                1 | 3 => bounds(budget, 6, Err(Fault::QubitMemoryAccess)),
                _ => bounds(budget, 6, Err(Fault::OutOfBounds)),
            });
            assert_eq!(values, expected.collect::<Vec<_>>());
            if budget >= 6 {
                assert_eq!((counts.varying_loads, counts.varying_stores, counts.candidate_bytes), (0, 1, 4));
            }
        }
    }
}

#[test]
fn seeded_overlapping_stores_and_varying_loads_match_the_executor() {
    let mut rng = ChaCha8Rng::seed_from_u64(0xc9a6_d718_05bb_882f);
    for case in 0..100 {
        let table = rng.random::<u64>();
        let mask = rng.random::<u16>();
        let budget = rng.random_range(3..=10);
        let address_delta = rng.random_range(0..8);
        let mode = [ApplyMode::Automatic, ApplyMode::StepMachine][case % 2];
        let module = module(
            &format!(
                r#"
data 32 = i64 {table}, i8 0
func @f(i3 %x) -> i8 {{
entry:
    %i = zext i3 %x to i64
    %p = or i64 %i, 32
    %input = zext i3 %x to i16
    %v = xor i16 %input, {mask}
    store i16 %v, %p
    %q = xor i64 %p, {address_delta}
    %r = load i16 %q
    %result = trunc i16 %r to i8
    ret %result
}}
"#
            ),
            "",
        );
        compare(&module, &format!("seeded varying writes {case}"), budget, mode);
    }
}

#[test]
fn address_selection_respects_the_instruction_dispatch_guard() {
    let module = module(
        r#"
data 32 = i32 21828
func @f(i3 %x) -> i8 {
entry:
    %write = trunc i3 %x to i1
    %shift = lshr i3 %x, 1
    %bit = trunc i3 %shift to i1
    %offset = zext i1 %bit to i64
    %p = or i64 %offset, 32
    br %write, put, read
put:
    store i16 258, %p
    %new = load i8 33
    ret %new
read:
    %old = load i8 33
    ret %old
}
"#,
        "",
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=10 {
            let (_, counts, values) = compare(&module, "guarded address selection", budget, mode);
            let expected = (0..8)
                .map(|x| if x & 1 == 0 { bounds(budget, 8, Ok(85)) } else { bounds(budget, 9, Ok(1 + (x >> 1 & 1))) });
            assert_eq!(values, expected.collect::<Vec<_>>());
            if budget >= 7 {
                assert_eq!((counts.varying_stores, counts.candidate_bytes), (1, 4));
            }
        }
    }
}

#[test]
fn a_complemented_dispatch_guard_proves_the_only_address_zero() {
    let module = module_with_width(
        r#"
data 0 = i8 42
func @f(i1 %x) -> i8 {
entry:
    %p = zext i1 %x to i64
    br %x, done, read
read:
    %v = load i8 %p
    ret %v
done:
    ret 7
}
"#,
        "",
        1,
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in 0..=5 {
            let (_, counts, values) = compare(&module, "guard proves address zero", budget, mode);
            assert_eq!(values, [bounds(budget, 4, Ok(42)), bounds(budget, 3, Ok(7))]);
            assert_eq!(counts, MemoryCounts::default());
        }
    }
}

#[test]
fn a_dispatch_guard_proves_address_thirty_three() {
    let module = module_with_width(
        r#"
data 0 = i8 12, i8 24
data 33 = i8 42
func @f(i1 %x) -> i8 {
entry:
    %offset = zext i1 %x to i64
    %p = or i64 %offset, 32
    br %x, access, done
access:
    %v = load i8 %p
    store i8 42, %p
    ret %v
done:
    ret 7
}
"#,
        "",
        1,
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let (_, counts, values) = compare(&module, "guard proves address 33", 6, mode);
        assert_eq!(values, [(7, 0), (42, 0)]);
        assert_eq!(counts, MemoryCounts::default());
    }
}

#[test]
#[ignore = "heavy: 2^20 gate replay cutoff"]
fn address_replay_includes_the_gate_limit_and_stops_above_it() {
    const LIMIT: u64 = 1 << 20;
    const XORS_PER_PASS: u64 = 64;
    const PASSES: u64 = LIMIT / (16 * XORS_PER_PASS);
    const PADDING_END: u64 = 8 + (XORS_PER_PASS + 3) * PASSES;
    let padding = (0..XORS_PER_PASS).map(|i| format!("    %padding{i} = xor i8 %pad, %rotated\n")).collect::<String>();
    let module = load(&format!(
        r#"
memory {MEMORY}
data 33 = i8 42
func @f(i8 %pad, i1 %extra) -> i8 {{
entry:
    %active = trunc i8 %pad to i1
    %offset = zext i1 %active to i64
    %p = or i64 %offset, 32
    %lower = lshr i8 %pad, 1
    %upper = shl i8 %pad, 7
    %rotated = or i8 %lower, %upper
    %other = trunc i8 %rotated to i1
    br emit(i16 0)
emit(i16 %pass):
{padding}    %next = add i16 %pass, 1
    %finished = icmp eq i16 %next, {PASSES}
    br %finished, tail, emit(%next)
tail:
    br %extra, one_more, dispatch
one_more:
    %extra_gate = and i1 %active, %other
    br dispatch
dispatch:
    br %active, read, done
read:
    %v = load i8 %p
    ret %v
done:
    ret 7
}}
"#
    ))
    .expect("gate replay module");
    let held = Default::default();
    let mut image = [0; MEMORY];
    image[33] = 42;
    let compile = |extra, budget| {
        compile_apply_with(
            ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("f"),
                args: &[Argument::Register(0..8), Argument::Value(extra)],
                out: 8..16,
                fault: 16..21,
                budget,
                entry_fault: None,
                stack_available: MEMORY as u64,
                memory: Some(ApplyMemory::new(&image, &held)),
                scratch_start: 21,
            },
            ApplyMode::StepMachine,
        )
    };

    // Each XOR uses two CNOTs per bit. The budget fault at the end of the
    // padding adds its XOR, and the reverse path repeats the padding.
    let padded = compile(0, PADDING_END).expect("emit the padding at the replay limit");
    assert_eq!(padded.block.ops().len() as u64, 2 * LIMIT + 2 + u64::from(Fault::Budget.code().count_ones()));

    // The extra AND adds one gate. Only replay can exclude undeclared byte 32.
    let budget = PADDING_END + 16;
    assert!(
        matches!(
            compile(1, budget),
            Err(ApplyError::Unsupported {
                operation: "load",
                route: "a proved address bound within a declared memory region",
                ..
            })
        ),
        "address proof must fall back above the gate limit"
    );
    let at_limit = compile(0, budget).expect("the guard proves address 33 at the gate limit");
    assert_eq!(at_limit.memory.varying_loads, 0);
}

#[test]
fn unconditional_store_aliases_the_widest_valid_span_under_the_budget() {
    for (address, valid) in [(1008, true), (1009, false)] {
        let source = format!(
            r#"
func @f(i3 %x) -> i8 {{
entry:
    %wide = zext i3 %x to i128
    %high = shl i128 %wide, 120
    store i128 %high, {address}
    %result = load i8 1023
    ret %result
}}
"#
        );
        let module = module(&source, "");
        for budget in [2, 3, 4, 5] {
            let (_, counts, values) = compare(&module, "wide fixed store", budget, ApplyMode::Automatic);
            let expected = (0..8)
                .map(|x| if valid { bounds(budget, 5, Ok(x)) } else { bounds(budget, 3, Err(Fault::OutOfBounds)) });
            assert_eq!(values, expected.collect::<Vec<_>>());
            assert_eq!((counts.retained_scratch_qubits, counts.gates), (0, 0));
            assert_eq!(counts.peak_nonconstant_bits, if valid && budget >= 3 { 3 } else { 0 });
        }
    }
}

#[test]
fn a_guarded_fixed_store_keeps_the_previous_byte_for_other_inputs() {
    let module = module(
        r#"
data 24 = i8 7
func @f(i3 %x) -> i8 {
entry:
    %take = trunc i3 %x to i1
    %value = lshr i3 %x, 1
    br %take, write, skip
write:
    store i3 %value, 24
    br read
skip:
    %unchanged = add i8 0, 0
    br read
read:
    %result = load i8 24
    ret %result
}
"#,
        "",
    );
    let (construction, counts, values) = compare(&module, "guarded fixed store", 7, ApplyMode::Automatic);
    assert!(matches!(construction, Construction::StepMachine { .. }));
    let expected = (0..8).map(|x| (if x & 1 == 0 { 7 } else { x >> 1 }, 0)).collect::<Vec<_>>();
    assert_eq!(values, expected);
    assert!(counts.retained_scratch_qubits > 0 && counts.gates > 0);
}

#[test]
fn stored_complement_is_materialized_once_for_many_consumers() {
    let compiled = isolated(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %bit = trunc i3 %x to i1
    %inverted = xor i1 %bit, 1
    store i1 %inverted, 24
    %read = load i1 24
    %answer = sext i1 %read to i8
    ret %answer
}
"#,
    );
    let (gates, layout) = gates(&compiled, WIDTH, 13);
    bits::exhaustive(&gates, layout, |x| if x & 1 == 0 { 255 } else { 0 }).expect("complemented output");
    // Two X gates and one CNOT in each direction, plus eight output CNOTs.
    assert_eq!(gates.len(), 14);
    assert_eq!(layout.scratch, 1);
}

#[test]
fn stored_complement_keeps_the_later_address_provable() {
    let module = module(
        r#"
func @f(i3 %x) -> i8 {
entry:
    %low = trunc i3 %x to i1
    %opposite = xor i1 %low, 1
    store i1 %opposite, 64
    %saved = load i1 64
    %left = sext i1 %saved to i128
    %shifted = lshr i3 %x, 1
    %second = trunc i3 %shifted to i1
    %right = sext i1 %second to i128
    br accumulate(i16 0, i128 0)
accumulate(i16 %passed, i128 %total):
    %intersection = and i128 %left, %right
    %updated = xor i128 %total, %intersection
    %advance = add i16 %passed, 1
    %last = icmp eq i16 %advance, 1651
    br %last, read, accumulate(%advance, %updated)
read:
    %narrow = trunc i3 %x to i2
    %unit = select i2 %low, 1, -1
    %offset = srem i2 %narrow, %unit
    %address = zext i2 %offset to i64
    %byte = load i8 %address
    %answer = trunc i128 %updated to i8
    %result = xor i8 %answer, %byte
    ret %result
}
"#,
        "",
    );
    let (_, _, values) =
        compare(&module, "address after repeated use of stored complement", 8272, ApplyMode::Automatic);
    let expected = (0..8).map(|x| (if x & 3 == 2 { 255 } else { 0 }, 0)).collect::<Vec<_>>();
    assert_eq!(values, expected);
}

fn private_polarity_case(invert_store: bool, invert_read: bool, guarded: bool) -> String {
    let merge = if guarded {
        r#"
    br %choose, positive, negative
positive:
    store i1 %left, 10
    br read
negative:
    store i1 %right, 10
    br read
read:"#
    } else {
        "    store i1 %left, 10\n    store i1 %right, 10"
    };
    format!(
        r#"
func @f(i3 %x) -> i8 {{
entry:
    %bit = trunc i3 %x to i1
    %stored = xor i1 %bit, {store_mask}
    store i1 %stored, 8
    %first = load i1 8
    store i1 %first, 9
    %second = load i1 9
    %left = xor i1 %first, {read_mask}
    %right = xor i1 %second, {read_mask}
    %shifted = lshr i3 %x, 1
    %choose = trunc i3 %shifted to i1
{merge}
    %value = load i1 10
    %fanout = sext i1 %value to i8
    %mask = sext i1 %choose to i8
    %out = and i8 %fanout, %mask
    ret %out
}}
"#,
        store_mask = u32::from(invert_store),
        read_mask = u32::from(invert_read),
    )
}

#[test]
fn private_byte_sharing_keeps_store_and_read_polarities_within_the_copy_cost() {
    // Measured with each stored bit retained on its own wire.
    const COPY_COSTS: [(usize, usize); 8] =
        [(32, 12), (36, 11), (40, 12), (44, 11), (36, 12), (40, 11), (44, 12), (48, 11)];
    for invert_store in [false, true] {
        for invert_read in [false, true] {
            for guarded in [false, true] {
                let source = private_polarity_case(invert_store, invert_read, guarded);
                let module = module(&source, "");
                let held = Default::default();
                let input = ApplyInput {
                    module: &module,
                    callee: module.function_named("f").expect("test entry"),
                    args: &[Argument::Register(0..3)],
                    out: 3..11,
                    fault: 11..16,
                    budget: 24,
                    entry_fault: None,
                    stack_available: MEMORY as u64,
                    memory: Some(ApplyMemory::new(&[0; MEMORY], &held)),
                    scratch_start: 16,
                };
                for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                    let compiled = compile_apply_with(input.clone(), mode).expect("compile the polarity matrix");
                    let (gates, layout) = gates(&compiled, WIDTH, 13);
                    let expected = |x| {
                        let set = x & 1 != 0;
                        if x & 2 != 0 && (set ^ invert_store ^ invert_read) { 255 } else { 0 }
                    };
                    bits::check_states(&gates, layout, (0..8).flat_map(|x| [(x, 0), (x, 0x15a5)]), expected)
                        .expect("preserve inputs and clear the scratch");
                    let index = 4 * usize::from(invert_store) + 2 * usize::from(invert_read) + usize::from(guarded);
                    let (copy_gates, copy_scratch) = COPY_COSTS[index];
                    assert!(
                        gates.len() <= copy_gates && layout.scratch <= copy_scratch,
                        "store inverted {invert_store}, read inverted {invert_read}, guarded {guarded}, {mode:?}: {} gates and {} scratch above ({copy_gates}, {copy_scratch})",
                        gates.len(),
                        layout.scratch
                    );
                    println!(
                        "POLARITY {invert_store} {invert_read} {guarded} {mode:?}: {} gates, {} scratch",
                        gates.len(),
                        layout.scratch
                    );
                }
            }
        }
    }
}

#[test]
fn merging_inverted_reads_preserves_the_replay_budget_for_a_later_load() {
    let source = private_polarity_case(true, true, true);
    let (prefix, _) = source.split_once("    %fanout =").expect("prepared byte and mask");
    let source = format!(
        r#"{prefix}
    %fanout = sext i1 %value to i128
    %mask = sext i1 %choose to i128
    br loop(i16 0, i128 0)
loop(i16 %count, i128 %acc):
    %selected = and i128 %fanout, %mask
    %sum = xor i128 %acc, %selected
    %next = add i16 %count, 1
    %done = icmp eq i16 %next, 1651
    br %done, finish, loop(%next, %sum)
finish:
    %small = trunc i3 %x to i2
    %unit = select i2 %bit, 1, -1
    %offset = srem i2 %small, %unit
    %address = zext i2 %offset to i64
    %base = load i8 %address
    %low = trunc i128 %sum to i8
    %out = xor i8 %low, %base
    ret %out
}}
"#,
    );
    let module = module(&source, "");
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        let (_, _, values) = compare(&module, "load after inverted guarded versions", 8280, mode);
        assert_eq!(values, (0..8).map(|x| (if x & 3 == 3 { 255 } else { 0 }, 0)).collect::<Vec<_>>());
    }
}
