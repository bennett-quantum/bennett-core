//! Costs and outcomes when a conditional jump proves a countdown argument definite.

use bennett_emit::{ApplyInput, ApplyMode, Argument, Construction, CountingDevice, compile_apply_with};
use bennett_exec::{Device, Executor, Options, Outcome};
use bennett_ir::{Fault, ValidModule, load};

const MAIN: &str = r#"
func @main(i64 %budget) {
entry:
    %a = qalloc 2
    h %a[0]
    h %a[1]
    s %a[1]
    %out = qalloc 2
    %fault = qalloc 5
    x %out[1]
    x %fault[4]
    apply @f(%a) -> %out, %fault budget %budget
    ret
}
"#;

fn countdown(tail: &str, before_effect: &str) -> ValidModule {
    load(&format!(
        r#"
memory 4096
func @f(i2 %x) -> i2 {{
entry:
    br head(%x)
head(i2 %n):
    %more = icmp ne i2 %n, 0
    br %more, stop, done
stop:
    %next = lshr i2 %n, 1
    {before_effect}
    effect 65
    {tail}
done:
    br count(%n)
count(i2 %k):
    %again = icmp ne i2 %k, 0
    br %again, down, retzero
down:
    %prev = sub i2 %k, 1
    br count(%prev)
retzero:
    ret %k
}}
{MAIN}
"#
    ))
    .expect("valid countdown")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cost {
    operations: usize,
    toffoli: u128,
    peak_qubits: usize,
}

fn compare(module: &ValidModule, budget: u64, expected: impl Fn(u64) -> (u64, u64)) -> (Construction, Cost) {
    let main = module.function_named("main").expect("entry");
    let options = Options { machine_meaning: true, ..Options::default() };
    let mut direct = Executor::new(module, main, &[budget], options).expect("direct run");
    let mut emitted = Executor::new(module, main, &[budget], options).expect("emitted run");
    let mut measurement = None;
    let mut compiler = |input: ApplyInput<'_>| {
        let mut device = CountingDevice::default();
        let [Argument::Register(argument)] = input.args else { panic!("one register argument") };
        device.allocate(argument.clone()).expect("allocate argument");
        device.allocate(input.out.clone()).expect("allocate result");
        device.allocate(input.fault.clone()).expect("allocate fault");
        let compiled = compile_apply_with(input, ApplyMode::Automatic)?;
        compiled.send(&mut device).expect("count circuit");
        let counts = device.counts();
        let cost =
            Cost { operations: compiled.block.ops().len(), toffoli: counts.toffoli, peak_qubits: counts.peak_qubits };
        assert!(measurement.replace((compiled.construction, cost)).is_none());
        Ok(compiled.block)
    };

    assert_eq!(direct.run(), Outcome::Returned(None), "B={budget}");
    assert_eq!(emitted.run_emitted(&mut compiler), Outcome::Returned(None), "B={budget}");
    let snapshot = direct.snapshot();
    assert_eq!(emitted.snapshot(), snapshot, "B={budget}");
    assert_eq!(emitted.stats().nonzero_release_weight, 0.0, "B={budget}");
    assert!(snapshot.classical.tape.is_empty(), "B={budget}");
    assert_eq!(snapshot.quantum.amplitudes.len(), 4, "B={budget}");
    for (basis, _) in &snapshot.quantum.amplitudes {
        let x = basis[0];
        let (result, fault) = expected(x);
        assert_eq!(*basis, [x, 2 ^ result, 16 ^ fault], "B={budget}, x={x}");
    }
    measurement.expect("one compiled apply")
}

#[test]
fn a_countdown_after_a_guarded_effect_has_budget_independent_cost() {
    let mut enough_budget = None;
    for tail in ["br head(%next)", "ret 0"] {
        let module = countdown(tail, "");
        // Nonzero inputs reach effect on attempt five. Zero returns on attempt seven.
        for budget in [4, 5, 6, 7, 8, 18, 64, 256] {
            let (construction, cost) = compare(&module, budget, |x| {
                let fault = match x {
                    0 if budget >= 7 => 0,
                    1..=3 if budget >= 5 => Fault::EffectInRegion.code(),
                    _ => Fault::Budget.code(),
                };
                (0, fault)
            });
            println!("{tail}, B={budget}: {construction:?}, {cost:?}");
            if budget >= 6 {
                assert_eq!(construction, Construction::Controlled { proved_passes: 0 }, "{tail}, B={budget}");
            }
            if budget >= 7 {
                assert_eq!(*enough_budget.get_or_insert(cost), cost, "{tail}, B={budget}");
            }
        }
    }
}

#[test]
fn a_live_back_edge_keeps_the_proved_shift_passes() {
    let mut enough_budget = None;
    for tail in ["ret 0", "br head(%next)"] {
        let module = countdown(
            tail,
            r#"
    %stop_now = icmp eq i2 %n, 2
    br %stop_now, faulting, advance
advance:
    br head(%next)
faulting:
"#,
        );
        for budget in [6, 7, 8, 12, 13, 14, 18, 19, 20, 64, 256] {
            let (construction, cost) = compare(&module, budget, |x| {
                let fault = match x {
                    0 | 2 if budget < 7 => Fault::Budget.code(),
                    1 if budget < 13 => Fault::Budget.code(),
                    3 if budget < 19 => Fault::Budget.code(),
                    2 => Fault::EffectInRegion.code(),
                    _ => 0,
                };
                (0, fault)
            });
            println!("live edge, {tail}, B={budget}: {construction:?}, {cost:?}");
            if budget >= 19 {
                assert_eq!(construction, Construction::Controlled { proved_passes: 2 }, "{tail}, B={budget}");
                assert_eq!(*enough_budget.get_or_insert(cost), cost, "{tail}, B={budget}");
            }
        }
    }
}

#[test]
fn equality_refines_the_equal_target_arguments_and_keeps_other_inputs() {
    let module = load(&format!(
        r#"
memory 4096
func @f(i2 %x) -> i2 {{
entry:
    %at_two = icmp eq i2 2, %x
    br %at_two, count(%x), other
count(i2 %k):
    %again = icmp ne i2 %k, 2
    br %again, down, done
down:
    %prev = sub i2 %k, 1
    br count(%prev)
done:
    br join(%k)
other:
    br join(i2 1)
join(i2 %result):
    %mixed = xor i2 %result, %x
    ret %mixed
}}
{MAIN}
"#
    ))
    .expect("valid conditional and join");
    for budget in [7, 64] {
        let (construction, _) = compare(&module, budget, |x| (if x == 2 { 0 } else { x ^ 1 }, 0));
        assert_eq!(construction, Construction::Controlled { proved_passes: 0 }, "B={budget}");
    }
}

#[test]
fn a_join_preserves_equal_arguments_to_skip_an_unreachable_load() {
    let module = load(
        r#"
memory 131072
func @f(i16 %x) -> i2 {
entry:
    %zero = icmp eq i16 %x, 0
    br %zero, join(%x, i2 1), join(%x, i2 2)
join(i16 %k, i2 %tag):
    %diff = icmp ne i16 %k, %x
    br %diff, read, done
read:
    %addr = zext i16 %k to i64
    %value = load i2 %addr
    ret %value
done:
    ret %tag
}
func @main(i64 %budget) {
entry:
    %a = qalloc 16
    h %a[0]
    h %a[15]
    s %a[15]
    %out = qalloc 2
    %fault = qalloc 5
    x %out[1]
    x %fault[4]
    apply @f(%a) -> %out, %fault budget %budget
    ret
}
"#,
    )
    .expect("valid unreachable load");
    let (construction, _) = compare(&module, 18, |x| (if x == 0 { 1 } else { 2 }, 0));
    assert_eq!(construction, Construction::Controlled { proved_passes: 0 });
}

#[test]
fn a_join_keeps_a_countdown_unreachable_at_every_budget() {
    for (condition, targets, equal_arm) in [
        ("%test = icmp eq i2 %x, 0", "join(%x), join(%x)", ""),
        ("%flipped = xor i2 %x, 1\n    %test = icmp ne i2 %flipped, 1", "join(%x), join(i2 0)", ""),
        ("%test = icmp ne i2 %x, 0", "join(%x), equal", "equal:\n    %same = xor i2 %x, 0\n    br join(%same)"),
    ] {
        let module = load(&format!(
            r#"
memory 4096
func @f(i2 %x) -> i2 {{
entry:
    %other = xor i2 %x, 0
    {condition}
    br %test, {targets}
{equal_arm}
join(i2 %k):
    %diff = icmp ne i2 %k, %other
    br %diff, count(%k), done
count(i2 %n):
    %more = icmp ne i2 %n, 0
    br %more, step, done
step:
    %previous = sub i2 %n, 1
    br count(%previous)
done:
    ret %x
}}
{MAIN}
"#
        ))
        .expect("valid unreachable countdown");
        for budget in [18, 64, 256] {
            let (construction, cost) = compare(&module, budget, |x| (x, 0));
            println!("{targets}, B={budget}: {construction:?}, {cost:?}");
            assert_eq!(construction, Construction::Controlled { proved_passes: 0 }, "{targets}, B={budget}");
            assert_eq!(cost.operations, 14, "{targets}, B={budget}");
        }
    }
}

#[test]
fn a_reassigned_block_parameter_uses_its_new_value_at_the_join() {
    let module = load(&format!(
        r#"
memory 4096
func @f(i2 %x) -> i2 {{
entry:
    %zero = icmp eq i2 %x, 0
    br %zero, reset, other
reset:
    br local(%x, i1 0)
local(i2 %word, i1 %reassigned):
    br %reassigned, leave, advance
advance:
    br local(i2 3, i1 1)
leave:
    br join(%word)
other:
    br join(%x)
join(i2 %result):
    ret %result
}}
{MAIN}
"#
    ))
    .expect("valid reassignment and join");
    let (construction, _) = compare(&module, 18, |x| (if x == 0 { 3 } else { x }, 0));
    assert_eq!(construction, Construction::Controlled { proved_passes: 0 });
}
