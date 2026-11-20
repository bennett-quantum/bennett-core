//! First-outcome checks for instructions that cannot succeed inside an applied function.

use super::{
    ApplyInput, ApplyMode, Argument, Construction, CountingDevice, Device, Executor, Fault, Options, Outcome,
    compile_apply_with, load,
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

fn compare(
    module: &ValidModule,
    mode: ApplyMode,
    budget: u64,
    expected: impl Fn(u64, u64) -> (u64, u64),
) -> (Counts, Construction) {
    let main = module.function_named("main").expect("entry");
    let options = Options { machine_meaning: true, ..Options::default() };
    let mut direct = Executor::new(module, main, &[budget], options).expect("direct run");
    let mut emitted = Executor::new(module, main, &[budget], options).expect("emitted run");
    let mut device = CountingDevice::default();
    let mut construction = None;
    let mut compile = |input: ApplyInput<'_>| {
        for arg in input.args {
            let Argument::Register(wires) = arg else { panic!("register input") };
            device.allocate(wires.clone()).expect("allocate argument");
        }
        device.allocate(input.out.clone()).expect("allocate result");
        device.allocate(input.fault.clone()).expect("allocate fault");
        let compiled = compile_apply_with(input, mode)?;
        assert!(construction.replace(compiled.construction).is_none());
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
    (device.counts(), construction.expect("one compiled apply"))
}

fn compare_guarded_stop(stop: &str, fault: Fault) {
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
            let (counts, construction) = compare(&module, mode, budget, |a, b| {
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
            assert_eq!(counts.toffoli, (2 * predicate) as u128, "{stop}, {mode:?}, B={budget}");
            assert_eq!(counts.peak_qubits, INTERFACE_WIDTH + predicate);
            if mode == ApplyMode::Automatic && budget >= 3 {
                assert_eq!(construction, Construction::Controlled { proved_passes: 0 }, "{stop}, B={budget}");
            }
        }
    }
}

#[test]
fn every_named_fault_and_region_stop_matches_the_executor() {
    for &fault in Fault::ALL {
        compare_guarded_stop(&format!("fault {fault}"), fault);
    }
    compare_guarded_stop("effect 65\n    fault unreachable", Fault::EffectInRegion);
    compare_guarded_stop("exit i2 1", Fault::ExitInRegion);
}

#[test]
fn quantum_instructions_inside_apply_give_quantum_in_region() {
    for stop in [
        // An addressed instruction must fault before checking the address or touching any byte.
        "x [0xffffffffffffffff]\n    fault unreachable",
        "%m = measure i8 [0xffffffffffffffff]\n    fault unreachable",
        // A valid applied function can name register handles only after a producer.
        // These operations appear in the copy, and the first allocation faults before them.
        "%q = qalloc 1\n    x %q\n    %m = measure %q\n    qfree %q\n    fault unreachable",
        "%q = qalloc 2\n    %error = qalloc 5\n    apply @f(%a, %b) -> %q, %error budget 7\n    fault unreachable",
    ] {
        compare_guarded_stop(stop, Fault::QuantumInRegion);
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

#[test]
fn an_exit_in_a_copied_callee_keeps_its_fault_code() {
    let module = load(&format!(
        r#"
memory 512
func @f(i1 %a, i1 %b) -> i2 {{
entry:
    %both = and i1 %a, %b
    br %both, stop, done
stop:
    call @leaf()
    ret 3
done:
    ret 1
}}
func @leaf() {{
entry:
    exit i8 23
}}
{MAIN}
"#
    ))
    .expect("valid region with copied exit");
    for budget in 0..=5 {
        for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
            compare(&module, mode, budget, |a, b| {
                if a & b == 1 {
                    let fault = if budget >= 4 { Fault::ExitInRegion } else { Fault::Budget };
                    (0, fault.code())
                } else if budget >= 3 {
                    (1, 0)
                } else {
                    (0, Fault::Budget.code())
                }
            });
        }
    }
}

#[test]
fn a_dead_back_edge_after_effect_preserves_the_proved_loop_count() {
    let mut expected_counts = None;
    for tail in ["ret 0", "br loop(%left)"] {
        let module = load(&format!(
            r#"
memory 512
func @f(i1 %a, i1 %b) -> i2 {{
entry:
    %lo = zext i1 %a to i2
    %hi = zext i1 %b to i2
    %high = shl i2 %hi, 1
    %arg = or i2 %lo, %high
    br loop(%arg)
loop(i2 %left):
    %empty = icmp eq i2 %left, 0
    br %empty, done, body
body:
    %stop = icmp eq i2 %left, 2
    br %stop, faulting, advance
advance:
    %next = lshr i2 %left, 1
    br loop(%next)
faulting:
    effect 65
    {tail}
done:
    ret 3
}}
{MAIN}
"#
        ))
        .expect("valid shift loop with effect");
        for budget in [32, 256] {
            let (counts, construction) = compare(&module, ApplyMode::Automatic, budget, |a, b| {
                if (a, b) == (0, 1) { (0, Fault::EffectInRegion.code()) } else { (3, 0) }
            });
            assert_eq!(construction, Construction::Controlled { proved_passes: 2 }, "{tail}, B={budget}");
            assert_eq!(*expected_counts.get_or_insert(counts), counts, "{tail}, B={budget}");
        }
    }
}
