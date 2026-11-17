//! Independent step arithmetic and state-vector checks for ordinary control and apply.
mod common;

use bennett_exec::{Options, Outcome, ReverseError};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};
use num_complex::Complex64;

// For n iterations of the outer loop, inner runs 0 + ... + (n - 1) times. Entry
// costs 2 steps, each outer visit 2, body/next/last inner visit 5 per iteration,
// each inner iteration 10 (including all five callee steps), and final return 1.
const NESTED: &str = r#"
memory 4096
data 0 = i8 91
func @bump(i4 %a) -> i4 {
entry:
    %m = load i8 0
    %m1 = add i8 %m, 1
    store i8 %m1, 0
    %v = add i4 %a, 3
    ret %v
}
func @f(i2 %n) -> i4 {
entry:
    store i8 7, 0
    br outer(i2 0, i4 0)
outer(i2 %i, i4 %sum):
    %more = icmp ult i2 %i, %n
    br %more, body, done
body:
    br inner(i2 0, %sum)
inner(i2 %j, i4 %acc):
    %again = icmp ult i2 %j, %i
    br %again, work, next
work:
    %v = call @bump(%acc)
    %j1 = add i2 %j, 1
    br inner(%j1, %v)
next:
    %i1 = add i2 %i, 1
    br outer(%i1, %acc)
done:
    ret %sum
}
func @main(i64 %budget) {
entry:
    %x = qalloc 2
    h %x[0]
    s %x[0]
    h %x[1]
    %o = qalloc 4
    x %o[3]
    h %o[0]
    t %o[0]
    x %o[1] ctrl %x[1]
    %e = qalloc 5
    x %e[4]
    h %e[0]
    s %e[0]
    %reference = qalloc 1
    h %reference
    x %reference ctrl %x[0]
    z %reference ctrl %o[0]
    apply @f(%x) -> %o, %e budget %budget
    ret
}
"#;

fn nested_steps(n: u64) -> u64 {
    5 + 7 * n + 10 * n * n.saturating_sub(1) / 2
}

#[test]
fn one_runtime_budget_covers_all_nested_loops_and_calls_and_preserves_complex_amplitudes() {
    let p = load(NESTED);
    // Every below/at/above boundary, including the return instruction and zero budget.
    for budget in [0, 1, 4, 5, 6, 11, 12, 13, 28, 29, 30, 55, 56, 57] {
        for reversible in [false, true] {
            let mut e = executor(&p, "main", &[budget], Options { reversible, allow_region_faults: true, ..bounded() });
            let initial = e.snapshot();
            assert_eq!(e.run(), Outcome::Returned(None));
            let state = e.snapshot();
            assert_eq!(state.classical.memory, initial.classical.memory, "private memory discarded");
            assert!(state.classical.tape.is_empty());
            assert_eq!(state.quantum.amplitudes.len(), 32);
            // Four independent H choices (x's two bits, output, fault) and one reference H.
            let scale = 1.0 / 32.0_f64.sqrt();
            for x in 0..4 {
                for out_bit in 0..2 {
                    for fault_bit in 0..2 {
                        for reference_before in 0..2 {
                            let mut out = 8 | out_bit | ((x >> 1) << 1);
                            let mut fault = 16 | fault_bit;
                            let reference = reference_before ^ (x & 1);
                            let angle = std::f64::consts::FRAC_PI_2 * ((x & 1) + fault_bit) as f64
                                + std::f64::consts::FRAC_PI_4 * out_bit as f64;
                            let sign = if reference * out_bit == 1 { -1.0 } else { 1.0 };
                            let expected = Complex64::from_polar(sign * scale, angle);
                            if budget >= nested_steps(x) {
                                out ^= 3 * x * x.saturating_sub(1) / 2 % 16;
                            } else {
                                fault ^= Fault::Budget.code();
                            }
                            let config = [x, out, fault, reference];
                            let actual = state.quantum.amplitude(&config);
                            assert!(
                                (actual - expected).norm() < 1e-12,
                                "B={budget}, {config:?}: {actual} != {expected}"
                            );
                        }
                    }
                }
            }
            // Counts belong to the simulator: each stored configuration is evaluated independently.
            let work: u64 = (0..4).map(|n| budget.min(nested_steps(n))).sum::<u64>() * 8;
            assert_eq!(e.stats().region_steps, work, "B={budget}");
            if reversible {
                e.reverse().expect("reverse apply and caller");
                assert_same_state(&e.snapshot(), &initial, "nested apply reverse");
            }
        }
    }
}

#[test]
fn faults_from_deep_callees_xor_only_fault_and_undo_private_memory() {
    for (instruction, code) in [
        ("effect 65\n    ret 0", Fault::EffectInRegion),
        ("exit i4 9", Fault::ExitInRegion),
        ("%z = udiv i4 %a, 0\n    ret %z", Fault::DivideByZero),
        ("fault unreachable", Fault::Unreachable),
    ] {
        let text = NESTED.replace("    %v = add i4 %a, 3\n    ret %v", instruction);
        let p = load(&text);
        let mut e = executor(&p, "main", &[100], Options { reversible: true, allow_region_faults: true, ..bounded() });
        let entry = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(None));
        let state = e.snapshot();
        assert_eq!(state.classical.memory, entry.classical.memory);
        assert!(state.classical.tape.is_empty(), "region's effect never escapes");
        for (c, _) in &state.quantum.amplitudes {
            assert_eq!(c[1] & !1, 8 | ((c[0] >> 1) << 1));
            let original_fault = if c[0] >= 2 { c[2] ^ code.code() } else { c[2] };
            assert!(original_fault == 16 || original_fault == 17, "{c:?}, {code:?}");
        }
        e.reverse().expect("reverse after region fault");
        assert_same_state(&e.snapshot(), &entry, "deep fault reverse");
    }
}

const ORDINARY: &str = r#"
memory 4096
data 17 = i8 165
func @f(i2 %x) -> i2 {
entry:
    %p = alloca 16
    store i2 %x, %p
    %y = add i2 %x, 1
    ret %y
}
func @driver(i64 %address) {
entry:
    %saved = stacksave
    %p = alloca 24
    store i64 1234567, %p
    h [%address:2]
    s [%address:2]
    ry(0.37) [%address:5] ctrl [%address:2]
    %q = qalloc 2
    h %q[0]
    s %q[0]
    h %q[1]
    %out = qalloc 2
    %fault = qalloc 5
    apply @f(%q) -> %out, %fault budget 4
    rz(0.23) [%address:7] ctrl %out[1]
    effect 90
    stackrestore %saved
    ret
}
func @main() {
entry:
    call @driver(i64 17)
    ret
}
"#;

#[test]
fn every_ordinary_gate_address_and_apply_step_matches_its_saved_predecessor() {
    let p = load(ORDINARY);
    let mut e = executor(&p, "main", &[], Options { reversible: true, ..bounded() });
    let mut states = vec![e.snapshot()];
    while e.step().is_none() {
        states.push(e.snapshot());
    }
    assert_eq!(e.outcome(), Some(Outcome::Returned(None)));
    let final_state = e.snapshot();
    for expected in states.iter().rev() {
        assert_eq!(e.step_back(), Ok(true));
        assert_same_state(&e.snapshot(), expected, "exact predecessor");
    }
    assert_eq!(e.step_back(), Ok(false));
    assert_eq!(e.run(), Outcome::Returned(None));
    assert_same_state(&e.snapshot(), &final_state, "forward after full reverse");
}

#[test]
fn measurement_is_an_atomic_reverse_barrier_for_registers_and_addresses() {
    for measure in ["%m = measure %q", "%m = measure i2 [17]"] {
        let text = ORDINARY.replace("    effect 90", &format!("    {measure}\n    effect 90"));
        let p = load(&text);
        let mut e = executor(&p, "main", &[], Options { reversible: true, ..bounded() });
        // Stop just after the measurement, then check refusal is state preserving.
        loop {
            let snap = e.snapshot();
            let frame = snap.classical.frames.last().expect("frame");
            let is_measure = matches!(
                p.function(frame.func).block(frame.block).insts.get(frame.inst),
                Some(bennett_ir::Inst::Measure { .. })
            );
            assert_eq!(e.step(), None);
            if is_measure {
                break;
            }
        }
        let barrier = e.snapshot();
        assert_eq!(e.step_back(), Err(ReverseError::PastMeasurement));
        assert_same_state(&e.snapshot(), &barrier, "barrier unchanged");
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_eq!(e.reverse(), Err(ReverseError::PastMeasurement));
        assert_same_state(&e.snapshot(), &barrier, "reverse stops immediately after measurement");
    }
}

#[test]
fn plain_address_gates_and_measurement_have_the_expected_value_around_apply() {
    let p = load(
        r#"
memory 1024
data 17 = i8 165
func @f(i2 %x) -> i2 {
entry:
    %v = add i2 %x, 1
    ret %v
}
func @main(i64 %p) -> i8 {
entry:
    %x = qalloc 2
    x %x[0]
    %o = qalloc 2
    %e = qalloc 5
    apply @f(%x) -> %o, %e budget 2
    x [%p:7] ctrl %o[1]
    x [%p:2] ctrl [%p:0]
    z [%p:0]
    apply @f(%x) -> %o, %e budget 2
    %m = measure i8 [%p]
    ret %m
}
"#,
    );
    for seed in 0..16 {
        let mut e = executor(&p, "main", &[17], Options { seed, ..bounded() });
        assert_eq!(e.run(), Outcome::Returned(Some(165 ^ 128 ^ 4)));
        let state = e.snapshot();
        assert_eq!(state.quantum.cells.len(), 1);
        assert_eq!(state.quantum.amplitudes.len(), 1);
        assert_eq!(state.quantum.amplitudes[0].0, [1, 0, 0, 33]);
        assert!((state.quantum.amplitudes[0].1 - Complex64::new(-1.0, 0.0)).norm() < 1e-12);
    }
}
