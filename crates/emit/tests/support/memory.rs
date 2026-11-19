//! Private memory across both emitted control paths.

use bennett_emit::{
    ApplyInput, ApplyMemory, ApplyMode, Argument, CompiledApply, Construction, MemoryCounts, compile_apply_with,
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
    load(&format!(
        r#"
memory {MEMORY}
{body}
func @main(i64 %budget) {{
entry:
{setup}
    %arg = qalloc {WIDTH}
    h %arg[0]
    h %arg[1]
    h %arg[2]
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
        let overwrite = rng.random::<bool>();
        let budget = rng.random_range(0..=18);
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
                choice = u8::from(overwrite)
            ),
            "    store i32 306247156, 32",
        );
        compare(&module, &format!("seeded case {case}"), budget, ApplyMode::Automatic);
    }
}
