//! Operand proofs and gate costs for results of copied calls.

use bennett_emit::{ApplyError, ApplyInput, ApplyMode, Argument, Construction, CountingDevice, compile_apply_with};
use bennett_exec::{Device, Executor, Options, Outcome, RunError};
use bennett_ir::{Fault, ValidModule, load};

const MAIN: &str = r#"
func @main(i64 %budget) {
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
}
"#;

fn module(body: &str, helpers: &str) -> ValidModule {
    load(&format!(
        r#"
memory 4096
data 32 = i16 513
func @f(i2 %x) -> i8 {{
entry:
    %active = icmp ne i2 %x, 0
    br %active, work, skip
skip:
    ret 9
work:
    {body}
}}
{helpers}
{MAIN}
"#
    ))
    .expect("valid copied call program")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cost {
    construction: Construction,
    operations: usize,
    toffoli: u128,
    peak_qubits: usize,
}

fn compare(module: &ValidModule, budget: u64, expected: impl Fn(u64) -> (u64, u64)) -> Cost {
    compare_mode(module, budget, ApplyMode::Automatic, expected)
}

fn compare_mode(module: &ValidModule, budget: u64, mode: ApplyMode, expected: impl Fn(u64) -> (u64, u64)) -> Cost {
    let entry = module.function_named("main").expect("entry");
    let options = Options { machine_meaning: true, ..Options::default() };
    let mut direct = Executor::new(module, entry, &[budget], options).expect("reference");
    let mut emitted = Executor::new(module, entry, &[budget], options).expect("gate simulation");
    let mut cost = None;
    let mut compiler = |input: ApplyInput<'_>| {
        let [Argument::Register(argument)] = input.args else { panic!("one register argument") };
        let mut device = CountingDevice::default();
        device.allocate(argument.clone()).expect("allocate argument");
        device.allocate(input.out.clone()).expect("allocate result");
        device.allocate(input.fault.clone()).expect("allocate fault");
        let compiled = compile_apply_with(input, mode)?;
        compiled.send(&mut device).expect("count circuit");
        let counts = device.counts();
        assert!(
            cost.replace(Cost {
                construction: compiled.construction,
                operations: compiled.block.ops().len(),
                toffoli: counts.toffoli,
                peak_qubits: counts.peak_qubits,
            })
            .is_none()
        );
        Ok(compiled.block)
    };
    assert_eq!(direct.run(), Outcome::Returned(None), "B={budget}");
    assert_eq!(emitted.run_emitted(&mut compiler), Outcome::Returned(None), "B={budget}");
    let snapshot = direct.snapshot();
    let actual = emitted.snapshot();
    assert_eq!(actual.quantum, snapshot.quantum, "B={budget}");
    assert_eq!(actual.classical, snapshot.classical, "B={budget}");
    assert_eq!(emitted.stats().nonzero_release_weight, 0.0);
    assert!(snapshot.classical.tape.is_empty());
    assert_eq!(snapshot.quantum.amplitudes.len(), 4);
    for (basis, _) in &snapshot.quantum.amplitudes {
        let x = basis[0];
        let (out, fault) = expected(x);
        assert_eq!(*basis, [x, 4 ^ out, 16 ^ fault], "B={budget}, x={x}");
    }
    cost.expect("one applied circuit")
}

const ADDRESS: &str = r#"
func @address() -> i64 {
entry:
    %address = faddr @target
    ret %address
}
func @target(i2 %x, i1 %unused) -> i8 {
entry:
    %r = zext i2 %x to i8
    ret %r
}
func @other(i2 %x, i1 %unused) -> i8 {
entry:
    ret 28
}
"#;

const ONE: &str = "func @one() -> i2 {\nentry:\n    ret 1\n}";

const LOW_BIT: &str = r#"
func @low(i2 %x) -> i8 {
entry:
    %low = trunc i2 %x to i1
    %r = zext i1 %low to i8
    ret %r
}
"#;

const TABLE_ADDRESS: &str = r#"
func @table_address(i2 %x) -> i64 {
entry:
    %low = trunc i2 %x to i1
    %offset = zext i1 %low to i64
    %r = or i64 %offset, 32
    ret %r
}
"#;

const ECHO: &str = "func @echo(i2 %x) -> i2 {\nentry:\n    ret %x\n}";

#[test]
fn copied_call_results_feed_operands() {
    type Witness = (&'static str, &'static str, &'static str, fn(u64) -> u64, usize);
    let cases: [Witness; 5] = [
        (
            "indirect",
            "%address = call @address()\n    %r = call_indirect fn(i2, i1) -> i8 %address(%x, i1 0)\n    ret %r",
            ADDRESS,
            |x| x,
            56,
        ),
        (
            "switch",
            "%selector = call @one()\n    switch i2 %selector, 1: yes, default: no\nyes:\n    ret 17\nno:\n    ret 23",
            ONE,
            |_| 17,
            20,
        ),
        (
            "shift",
            "%amount = call @low(%x)\n    %r = lshr i8 128, %amount\n    ret %r",
            LOW_BIT,
            |x| 128 >> (x & 1),
            70,
        ),
        ("memory", "%p = call @table_address(%x)\n    %r = load i8 %p\n    ret %r", TABLE_ADDRESS, |x| 1 + (x & 1), 76),
        (
            "arithmetic",
            "%copy = call @echo(%x)\n    %different = xor i2 %copy, %x\n    %r = zext i2 %different to i8\n    ret %r",
            ECHO,
            |_| 0,
            38,
        ),
    ];
    for (label, body, helpers, expected, maximum_operations) in cases {
        let cost = compare(&module(body, helpers), 32, |x| (if x == 0 { 9 } else { expected(x) }, 0));
        println!("{label}: {cost:?}");
        assert!(cost.operations <= maximum_operations, "{label}: {cost:?}");
    }
}

const RECURSIVE: &str = r#"
func @cycle() -> i8 {
entry:
    %again = call @cycle()
    ret %again
}
"#;

#[test]
fn a_proved_return_address_excludes_an_unreachable_recursive_target() {
    let helpers = ADDRESS.replace("    ret 28", "    %again = call @other(%x, %unused)\n    ret %again");
    let module = module(
        "%address = call @address()\n    %r = call_indirect fn(i2, i1) -> i8 %address(%x, i1 0)\n    ret %r",
        &helpers,
    );
    compare(&module, 32, |x| (if x == 0 { 9 } else { x }, 0));
}

#[test]
fn a_proved_return_selector_excludes_an_unreachable_recursive_case() {
    let module = module(
        r#"
    %selector = call @one()
    switch i2 %selector, 1: yes, default: no
yes:
    ret 17
no:
    %r = call @cycle()
    ret %r
"#,
        &format!("{ONE}\n{RECURSIVE}"),
    );
    compare(&module, 32, |x| (if x == 0 { 9 } else { 17 }, 0));
}

#[test]
fn different_copy_arguments_keep_distinct_proofs_through_nested_calls() {
    let module = module(
        r#"
    %a = call @relay(i2 0)
    %b = call @relay(i2 1)
    %r = xor i8 %a, %b
    ret %r
"#,
        r#"
func @relay(i2 %x) -> i8 {
entry:
    %result = call @choose(%x)
    ret %result
}
func @choose(i2 %x) -> i8 {
entry:
    %zero = icmp eq i2 %x, 0
    br %zero, yes, no
yes:
    ret 7
no:
    ret 11
}
"#,
    );
    for budget in [15, 16, 17] {
        compare(&module, budget, |x| match (x, budget) {
            (0, _) => (9, 0),
            (_, 15) => (0, Fault::Budget.code()),
            (_, _) => (12, 0),
        });
    }
}

#[test]
fn the_successful_return_proof_preserves_faults_and_budget() {
    let helper = r#"
func @sometimes(i2 %x) -> i8 {
entry:
    %stop = icmp eq i2 %x, 1
    br %stop, stopped, success
stopped:
    effect 65
    ret 0
success:
    ret 7
}
"#;
    let module = module(
        r#"
    %value = call @sometimes(%x)
    switch i8 %value, 7: yes, default: no
yes:
    ret 14
no:
    %r = call @cycle()
    ret %r
"#,
        &format!("{helper}{RECURSIVE}"),
    );
    for budget in [5, 6, 7, 8, 9] {
        compare(&module, budget, |x| match (x, budget) {
            (0, _) => (9, 0),
            (1, 6..) => (0, Fault::EffectInRegion.code()),
            (_, 8..) => (14, 0),
            (_, _) => (0, Fault::Budget.code()),
        });
    }
}

#[test]
fn multiple_return_sites_share_only_their_common_bits() {
    let module = module(
        r#"
    %amount = call @choose(%x)
    %r = lshr i8 128, %amount
    ret %r
"#,
        r#"
func @choose(i2 %x) -> i8 {
entry:
    %odd = trunc i2 %x to i1
    br %odd, yes, no
yes:
    ret 5
no:
    ret 4
}
"#,
    );
    let cost = compare(&module, 32, |x| (if x == 0 { 9 } else { 8 >> (x & 1) }, 0));
    println!("multiple return sites: {cost:?}");
    assert_eq!(cost.operations, 82);
}

#[test]
fn multiple_indirect_targets_share_only_their_common_bits() {
    let module = module(
        r#"
    %odd = trunc i2 %x to i1
    %yes = faddr @odd_target
    %no = faddr @even_target
    %target = select i64 %odd, %yes, %no
    %amount = call_indirect fn() -> i8 %target()
    %r = lshr i8 128, %amount
    ret %r
"#,
        r#"
func @odd_target() -> i8 {
entry:
    ret 5
}
func @even_target() -> i8 {
entry:
    ret 4
}
"#,
    );
    let cost = compare(&module, 32, |x| (if x == 0 { 9 } else { 8 >> (x & 1) }, 0));
    println!("multiple indirect targets: {cost:?}");
    assert_eq!(cost.operations, 80);
}

const CALL_STOPPING_TARGET: &str = r#"
    %odd = trunc i2 %x to i1
    %a = faddr @seven
    %b = faddr @maybe
    %target = select i64 %odd, %a, %b
    %value = call_indirect fn(i1) -> i8 %target(i1 0)
    switch i8 %value, 7: yes, default: no
yes:
    ret 14
no:
    %unreachable = call @cycle()
    ret %unreachable
"#;

#[test]
fn an_indirect_target_can_stop_for_the_supplied_argument() {
    let module = module(
        CALL_STOPPING_TARGET,
        &format!(
            r#"
func @seven(i1 %flag) -> i8 {{
entry:
    ret 7
}}
func @maybe(i1 %flag) -> i8 {{
entry:
    br %flag, returns, stopped
returns:
    ret 8
stopped:
    effect 65
    ret 9
}}
{RECURSIVE}
"#
        ),
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in [8, 9, 10, 32] {
            compare_mode(&module, budget, mode, |x| match (x, budget) {
                (0, _) => (9, 0),
                (2, 9..) => (0, Fault::EffectInRegion.code()),
                (1 | 3, 10..) => (14, 0),
                (_, _) => (0, Fault::Budget.code()),
            });
        }
    }
}

#[test]
fn a_nested_call_can_stop_with_or_without_a_result_slot() {
    for receiver in ["%unused = ", ""] {
        let module = module(
            CALL_STOPPING_TARGET,
            &format!(
                r#"
func @seven(i1 %flag) -> i8 {{
entry:
    ret 7
}}
func @maybe(i1 %flag) -> i8 {{
entry:
    {receiver}call @inner(%flag)
    ret 8
}}
func @inner(i1 %flag) -> i8 {{
entry:
    br %flag, normal, stopped
normal:
    ret 8
stopped:
    effect 65
    ret 9
}}
{RECURSIVE}
"#
            ),
        );
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            for budget in (0..=16).chain([32]) {
                compare_mode(&module, budget, mode, |x| match (x, budget) {
                    (0, 3..) => (9, 0),
                    (2, 10..) => (0, Fault::EffectInRegion.code()),
                    (1 | 3, 10..) => (14, 0),
                    (_, _) => (0, Fault::Budget.code()),
                });
            }
        }
    }
}

#[test]
fn a_copy_can_stop_using_shared_local_argument_bits() {
    let module = module(
        r#"
    %sum = add i2 %x, 1
    %alias = or i2 %sum, 0
    %value = call @stop_if_equal(%sum, %alias)
    %unreachable = call @cycle()
    ret %unreachable
"#,
        &format!(
            r#"
func @stop_if_equal(i2 %a, i2 %b) -> i8 {{
entry:
    %equal = icmp eq i2 %a, %b
    br %equal, stopped, returns
stopped:
    effect 65
    ret 9
returns:
    ret 8
}}
{RECURSIVE}
"#
        ),
    );
    for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
        for budget in [7, 8, 9] {
            compare_mode(&module, budget, mode, |x| match (x, budget) {
                (0, _) => (9, 0),
                (_, 8..) => (0, Fault::EffectInRegion.code()),
                (_, _) => (0, Fault::Budget.code()),
            });
        }
    }
}

#[test]
fn a_reachable_recursive_call_is_still_refused() {
    let module = module("%r = call @cycle()\n    ret %r", RECURSIVE);
    let entry = module.function_named("main").expect("entry");
    let mut emitted = Executor::new(&module, entry, &[32], Options::default()).expect("gate simulation");
    let mut compiler = |input: ApplyInput<'_>| Ok(compile_apply_with(input, ApplyMode::Automatic)?.block);
    assert!(matches!(
        emitted.run_emitted(&mut compiler),
        Outcome::Stopped(RunError::Apply(ApplyError::Unsupported {
            operation: "call",
            route: "recursive call frames",
            ..
        }))
    ));
}
