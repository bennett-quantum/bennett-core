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
fn version_copies_preserve_equality_and_disjoint_versions_count_toward_the_peak() {
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
    store i1 %zero, 10
    ret 0
}
"#,
    );
    let (gates, layout) = gates(&compiled, WIDTH, 13);
    assert_eq!((layout.scratch, gates.len()), (2, 4));
    assert_eq!(compiled.memory, MemoryCounts { retained_scratch_qubits: 2, gates: 4, peak_nonconstant_bits: 2 });
    bits::exhaustive(&gates, layout, |_| 0).expect("preserved data and clear scratch");
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
        assert_eq!(counts, MemoryCounts { retained_scratch_qubits: 3, gates: 6, peak_nonconstant_bits: 3 });
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
        let stores = usize::from(budget >= 2) + usize::from(budget >= 4);
        let counts = MemoryCounts {
            retained_scratch_qubits: 3 * stores,
            gates: 6 * stores,
            peak_nonconstant_bits: if stores == 0 { 0 } else { 3 },
        };
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
            route: "a proved definite memory address",
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
                MemoryCounts { retained_scratch_qubits: 2, gates: 8, peak_nonconstant_bits: 7 }
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
