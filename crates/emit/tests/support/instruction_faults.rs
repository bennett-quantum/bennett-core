//! First-outcome checks for instructions that cannot succeed inside an applied function.

use super::{
    ApplyInput, ApplyMode, Argument, CountingDevice, Device, Executor, Fault, Options, Outcome, compile_apply_with,
    load,
};
use bennett_emit::Counts;
use bennett_ir::ValidModule;

const MAIN: &str = r#"
func @main(i64 %budget) {
entry:
    %a = qalloc 1
    %b = qalloc 1
    h %a
    h %b
    s %b
    %out = qalloc 2
    %fault = qalloc 5
    x %out[1]
    x %fault[4]
    apply @f(%a, %b) -> %out, %fault budget %budget
    ret
}
"#;
const INTERFACE_WIDTH: usize = 1 + 1 + 2 + 5;

fn compare(module: &ValidModule, mode: ApplyMode, budget: u64, expected: impl Fn(u64, u64) -> (u64, u64)) -> Counts {
    let main = module.function_named("main").expect("entry");
    let options = Options { machine_meaning: true, ..Options::default() };
    let mut direct = Executor::new(module, main, &[budget], options).expect("direct run");
    let mut emitted = Executor::new(module, main, &[budget], options).expect("emitted run");
    let mut device = CountingDevice::default();
    let mut compile = |input: ApplyInput<'_>| {
        for arg in input.args {
            let Argument::Register(wires) = arg else { panic!("register input") };
            device.allocate(wires.clone()).expect("allocate argument");
        }
        device.allocate(input.out.clone()).expect("allocate result");
        device.allocate(input.fault.clone()).expect("allocate fault");
        let compiled = compile_apply_with(input, mode)?;
        compiled.send(&mut device).expect("count emitted gates");
        Ok(compiled.block)
    };
    let outcome = direct.run();
    assert_eq!(outcome, Outcome::Returned(None));
    assert_eq!(emitted.run_emitted(&mut compile), outcome, "{mode:?}, B={budget}");
    assert_eq!(emitted.snapshot(), direct.snapshot(), "{mode:?}, B={budget}");
    assert_eq!(emitted.stats().nonzero_release_weight, 0.0);
    let snapshot = direct.snapshot();
    assert!(snapshot.classical.tape.is_empty());
    assert_eq!(snapshot.quantum.amplitudes.len(), 4);
    for (basis, _) in &snapshot.quantum.amplitudes {
        let (a, b) = (basis[0], basis[1]);
        let (out, fault) = expected(a, b);
        assert_eq!(*basis, [a, b, 2 ^ out, 16 ^ fault], "{mode:?}, B={budget}");
    }
    device.counts()
}

#[test]
fn every_named_fault_and_region_stop_matches_the_executor() {
    let mut cases: Vec<_> = Fault::ALL.iter().map(|fault| (format!("fault {fault}"), *fault)).collect();
    cases.extend([
        ("effect 65\n    fault unreachable".into(), Fault::EffectInRegion),
        ("exit i2 1".into(), Fault::ExitInRegion),
    ]);
    for (stop, fault) in cases {
        let module = load(&format!(
            r#"
memory 512
func @f(i1 %a, i1 %b) -> i2 {{
entry:
    %both = and i1 %a, %b
    br %both, stop, done
stop:
    {stop}
done:
    ret 1
}}
{MAIN}
"#
        ))
        .expect("valid region stop");
        for budget in 0..=4 {
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let counts = compare(&module, mode, budget, |a, b| {
                    if budget < 3 {
                        (0, Fault::Budget.code())
                    } else if a & b == 1 {
                        (0, fault.code())
                    } else {
                        (1, 0)
                    }
                });
                // The predicate uses one scratch bit and one Toffoli per direction.
                // A constant result or fault code uses at most singly controlled copies.
                let predicate = usize::from(budget >= 1);
                assert_eq!(counts.toffoli, (2 * predicate) as u128, "{fault}, {mode:?}, B={budget}");
                assert_eq!(counts.peak_qubits, INTERFACE_WIDTH + predicate);
            }
        }
    }
}

#[test]
fn a_region_stop_cannot_replace_an_earlier_fault_or_plan_a_dead_call() {
    let module = load(&format!(
        r#"
memory 512
func @f(i1 %a, i1 %b) -> i2 {{
entry:
    %divisor = zext i1 %a to i2
    %quotient = udiv i2 2, %divisor
    effect 65
    %target = zext i1 %b to i64
    %unused = call_indirect fn() -> i2 %target()
    br dead
dead:
    %recursive = call @f(%a, %b)
    ret %recursive
}}
{MAIN}
"#
    ))
    .expect("valid region with dead calls");
    for budget in 0..=4 {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare(&module, mode, budget, |a, _| {
                let fault = match (budget, a) {
                    (0..=1, _) | (2, 1) => Fault::Budget,
                    (_, 0) => Fault::DivideByZero,
                    (_, _) => Fault::EffectInRegion,
                };
                (0, fault.code())
            });
        }
    }
}
