//! Worked hand-computed fixtures for the remaining handle forms: Pauli `y`, run-time gate and
//! rotation codes (`gate(%k)`, `rotate(%k, %a)`), constants as interface operands, `qwidth`,
//! slice measurement (`measure i64 %r[%low, %bits]`), control lists from memory, `apply` step
//! reports (D16), and block-form `apply` (D10–D14).

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2, FRAC_PI_4, PI};
use std::fmt::Write as _;

use bennett_exec::{ApplyReport, Executor, Options, Outcome, ReverseError};
use bennett_ir::Fault;
use num_complex::Complex64 as C;

use super::device_reference::{Density, Finish};
use super::observe::{run_and_reverse, run_partial_undo_and_redo, run_strict};
use super::{BUDGET, assert_density, basis, module, pure, pure_basis, run};

#[test]
fn pauli_y_and_runtime_gate_codes_match_worked_amplitudes_and_undo() {
    // Prepare |psi> = S Ry(0.8) |0> = a|0> + i*b|1>, with a = cos(0.4), b = sin(0.4).
    let (b, a) = 0.4_f64.sin_cos();
    let one = C::new(a, 0.0);
    let two = C::new(0.0, b);
    let theta = 0.347_f64;
    let (s, c) = (theta / 2.0).sin_cos();
    let t_phase = C::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2);

    let gate_cases: [(u64, [C; 2]); 8] = [
        (1, [two, one]),                                                 // x
        (2, [C::new(b, 0.0), C::new(0.0, a)]),                           // y = [[0, -i], [i, 0]]
        (3, [one, -two]),                                                // z
        (4, [(one + two) * FRAC_1_SQRT_2, (one - two) * FRAC_1_SQRT_2]), // h
        (5, [one, C::new(-b, 0.0)]),                                     // s
        (6, [one, C::new(b, 0.0)]),                                      // sdg
        (7, [one, two * t_phase]),                                       // t
        (8, [one, two * t_phase.conj()]),                                // tdg
    ];
    for (code, active) in gate_cases {
        for use_const in [false, true] {
            let inst =
                if use_const { format!("gate({code}) %q[0] ctrl %c[0]") } else { "gate(%k) %q[0] ctrl %c[0]".into() };
            let program = module(&format!(
                "memory 256
func @main(i64 %k) {{
entry:
 %q = qalloc 1
 %c = qalloc 1
 ry(0.8) %q[0]
 s %q[0]
 h %c[0]
 {inst}
 ret
}}"
            ));
            let expected = pure(&[
                (vec![0, 0], one * FRAC_1_SQRT_2),
                (vec![1, 0], two * FRAC_1_SQRT_2),
                (vec![0, 1], active[0] * FRAC_1_SQRT_2),
                (vec![1, 1], active[1] * FRAC_1_SQRT_2),
            ]);
            let result = run_and_reverse(&program, "main", &[code], 0, BUDGET);
            assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
            assert_density(
                &result.reference.quantum.density,
                &expected,
                &format!("gate code {code}, const={use_const}"),
            );
        }
    }

    let rot_cases: [(u64, [C; 2]); 4] = [
        (9, [C::new(c * a + s * b, 0.0), C::new(0.0, c * b - s * a)]), // rx(theta)
        (10, [C::new(c * a, -s * b), C::new(s * a, c * b)]),           // ry(theta)
        (11, [one * C::new(c, -s), two * C::new(c, s)]),               // rz(theta)
        (12, [one, two * C::from_polar(1.0, theta)]),                  // p(theta)
    ];
    let theta_bits = theta.to_bits();
    for (code, active) in rot_cases {
        for use_const in [false, true] {
            let inst = if use_const {
                format!("rotate({code}, {theta_bits}) %q[0] ctrl %c[0]")
            } else {
                "rotate(%k, %a) %q[0] ctrl %c[0]".into()
            };
            let program = module(&format!(
                "memory 256
func @main(i64 %k, i64 %a) {{
entry:
 %q = qalloc 1
 %c = qalloc 1
 ry(0.8) %q[0]
 s %q[0]
 h %c[0]
 {inst}
 ret
}}"
            ));
            let expected = pure(&[
                (vec![0, 0], one * FRAC_1_SQRT_2),
                (vec![1, 0], two * FRAC_1_SQRT_2),
                (vec![0, 1], active[0] * FRAC_1_SQRT_2),
                (vec![1, 1], active[1] * FRAC_1_SQRT_2),
            ]);
            let result = run_and_reverse(&program, "main", &[code, theta_bits], 0, BUDGET);
            assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
            assert_density(
                &result.reference.quantum.density,
                &expected,
                &format!("rotation code {code}, const={use_const}"),
            );
        }
    }

    // Controlled Y differs from controlled Ry(pi) by relative phase i on |11>.
    let cy_vs_cry = module(&format!(
        "memory 256
func @main(i64 %use_y) {{
entry:
 %c = qalloc 1
 %t = qalloc 1
 h %c[0]
 %pick = icmp ne i64 %use_y, 0
 br %pick, do_y, do_ry
do_y:
 y %t[0] ctrl %c[0]
 br done
do_ry:
 ry({PI}) %t[0] ctrl %c[0]
 br done
done:
 ret
}}"
    ));
    let cy = run_and_reverse(&cy_vs_cry, "main", &[1], 0, BUDGET);
    assert_density(
        &cy.reference.quantum.density,
        &pure(&[(vec![0, 0], C::new(FRAC_1_SQRT_2, 0.0)), (vec![1, 1], C::new(0.0, FRAC_1_SQRT_2))]),
        "controlled Y gives i|11>",
    );
    let cry = run_and_reverse(&cy_vs_cry, "main", &[0], 0, BUDGET);
    assert_density(
        &cry.reference.quantum.density,
        &pure(&[(vec![0, 0], C::new(FRAC_1_SQRT_2, 0.0)), (vec![1, 1], C::new(FRAC_1_SQRT_2, 0.0))]),
        "controlled Ry(pi) gives +|11>",
    );
}

#[test]
fn constants_as_interface_operands_and_qwidth_match_worked_state_and_undo() {
    let pi_half_bits = FRAC_PI_2.to_bits();
    let program = module(&format!(
        "memory 256
func @main() -> i64 {{
entry:
 %q = qalloc 3
 %h = qalloc i64 70
 %tmp = qalloc 2
 %w_const = qwidth 1
 %w_i64 = qwidth %h
 %w_qreg = qwidth %q
 h 1[0]
 y 2[69] ctrl 1[0]
 gate(4) 3[0]
 rotate(12, {pi_half_bits}) 2[69] ctrl 3[0]
 qfree 3
 %s1 = mul i64 %w_const, 1000
 %s2 = add i64 %s1, %w_i64
 %s3 = mul i64 %s2, 10
 %res = add i64 %s3, %w_qreg
 ret %res
}}"
    ));

    // Before qfree 3:
    //   tmp=0: (|0, 0> + i|1, 2^69>) / 2
    //   tmp=1: (|0, 0> - |1, 2^69>) / 2  (since p(pi/2) multiplies i by i = -1)
    // Partial trace over handle 3 leaves off-diagonal (-1 - i)/4 and (-1 + i)/4.
    let b0 = basis([0, 0]);
    let mut b1 = basis([1, 0]);
    b1[1].flip(69);
    let expected = Density::from([
        ((b0.clone(), b0.clone()), C::new(0.5, 0.0)),
        ((b1.clone(), b1.clone()), C::new(0.5, 0.0)),
        ((b0.clone(), b1.clone()), C::new(-0.25, -0.25)),
        ((b1.clone(), b0.clone()), C::new(-0.25, 0.25)),
    ]);

    let result = run_partial_undo_and_redo(&program, "main", &[], 0, BUDGET, 6);
    assert_eq!(result.reference.finish, Some(Finish::Returned(Some(30703))));
    assert_eq!(result.reference.quantum.allocation_count, 3);
    assert_density(&result.reference.quantum.density, &expected, "constant operands and qwidth partial trace");

    // qwidth and qfree on constant/slot zero, released, and future handles fault dangling_register.
    for inst in
        ["%w = qwidth 0", "%w = qwidth 1", "%w = qwidth 99", "%w = qwidth %dead", "qfree 0", "qfree 1", "qfree 99"]
    {
        let bad = module(&format!(
            "memory 128
func @main() {{
entry:
 %dead = qalloc 2
 qfree 1
 {inst}
 ret
}}"
        ));
        let res = run_and_reverse(&bad, "main", &[], 0, BUDGET);
        assert_eq!(res.reference.finish, Some(Finish::Faulted(Fault::DanglingRegister)), "{inst}");
        assert_eq!(res.reference.steps, 2, "{inst}");
    }
}

#[test]
fn slice_measurement_projects_narrow_wide_and_64bit_slices_preserving_coherence() {
    // 1. Measuring bit 0 of a 3-qubit entangled state leaves bits 1..3 coherent and keeps handle 1 live.
    //    State before measure: (|000> + |100> + |011> + i|111>) / 2  (bit 0 is LSB: 0, 4, 3, 7).
    let narrow = module(
        "memory 256
func @main() -> i64 {
entry:
 %q = qalloc 3
 h %q[0]
 x %q[1] ctrl %q[0]
 h %q[2]
 s %q[2] ctrl %q[1]
 %m = measure i64 1[0, 1]
 %w = qwidth 1
 %packed = mul i64 %w, 10
 %ans = add i64 %packed, %m
 ret %ans
}",
    );
    let mut seen_zero = false;
    let mut seen_one = false;
    for seed in 0..32 {
        let res = run(&narrow, "main", &[], seed, BUDGET);
        let outcome = res.measurements[0].0;
        assert!((res.measurements[0].1[&0] - 0.5).abs() < 1e-11);
        assert!((res.measurements[0].1[&1] - 0.5).abs() < 1e-11);
        assert_eq!(res.reference.quantum.allocation_count, 1, "slice measure takes no handle");
        assert_eq!(res.reference.quantum.live.len(), 1, "register stays allocated");
        assert_eq!(res.reference.finish, Some(Finish::Returned(Some(30 + outcome))));
        let expected = if outcome == 0 {
            seen_zero = true;
            pure(&[(vec![0], C::new(FRAC_1_SQRT_2, 0.0)), (vec![4], C::new(FRAC_1_SQRT_2, 0.0))])
        } else {
            seen_one = true;
            pure(&[(vec![3], C::new(FRAC_1_SQRT_2, 0.0)), (vec![7], C::new(0.0, FRAC_1_SQRT_2))])
        };
        assert_density(&res.reference.quantum.density, &expected, &format!("slice measure outcome {outcome}"));
    }
    assert!(seen_zero && seen_one);

    // 2. Wide 130-qubit register: slice crossing the 64-bit word boundary and a 64-bit slice at bit 66..130.
    let wide = module(
        "memory 256
func @main() -> i64 {
entry:
 %w = qalloc i64 130
 x %w[62]
 h %w[63]
 x %w[64]
 x %w[65]
 x %w[129] ctrl %w[63]
 %mid = measure i64 %w[61, 6]
 %top = measure i64 %w[66, 64]
 %res = xor i64 %mid, %top
 ret %res
}",
    );
    let mut saw_low = false;
    let mut saw_high = false;
    for seed in 0..32 {
        let res = run(&wide, "main", &[], seed, BUDGET);
        let (mid, ref mid_probs) = res.measurements[0];
        let (top, ref top_probs) = res.measurements[1];
        assert!((mid_probs[&26] - 0.5).abs() < 1e-11);
        assert!((mid_probs[&30] - 0.5).abs() < 1e-11);
        assert!((top_probs[&top] - 1.0).abs() < 1e-11);
        if mid == 26 {
            saw_low = true;
            assert_eq!(top, 0);
        } else {
            saw_high = true;
            assert_eq!(mid, 30);
            assert_eq!(top, 1u128 << 63);
        }
        assert_eq!(res.reference.finish, Some(Finish::Returned(Some(mid ^ top))));
    }
    assert!(saw_low && saw_high);

    // 3. Reversal past slice measurement is refused with ReverseError::PastMeasurement.
    let mut executor = Executor::new(
        &narrow,
        narrow.function_named("main").expect("main"),
        &[],
        Options { reversible: true, budget: Some(BUDGET), ..Options::default() },
    )
    .expect("executor starts");
    assert!(matches!(executor.run(), Outcome::Returned(Some(_))));
    // Undo ret, add, mul, qwidth -> 4 steps back to right after measure i64.
    for _ in 0..4 {
        assert_eq!(executor.step_back(), Ok(true));
    }
    assert_eq!(executor.step_back(), Err(ReverseError::PastMeasurement));

    // 4. Fault boundaries: dangling_register beats bad_range; low + bits never wraps.
    let fault_cases = [
        ("measure i64 0[0, 0]", Fault::DanglingRegister),
        ("measure i64 %dead[0, 65]", Fault::DanglingRegister),
        ("measure i64 99[0, 1]", Fault::DanglingRegister),
        ("measure i64 %q[0, 0]", Fault::BadRange),
        ("measure i64 %q[0, 65]", Fault::BadRange),
        ("measure i64 %q[0, %max]", Fault::BadRange),
        ("measure i64 %q[70, 1]", Fault::BadRange),
        ("measure i64 %q[69, 2]", Fault::BadRange),
        ("measure i64 %q[%max, 1]", Fault::BadRange),
        ("measure i64 %q[%near_max, 5]", Fault::BadRange),
    ];
    for (inst, fault) in fault_cases {
        let program = module(&format!(
            "memory 256
func @main(i64 %max, i64 %near_max) -> i64 {{
entry:
 %dead = qalloc 2
 qfree %dead
 %q = qalloc i64 70
 %m = {inst}
 ret %m
}}"
        ));
        let res = run_and_reverse(&program, "main", &[u64::MAX, u64::MAX - 3], 0, BUDGET);
        assert_eq!(res.reference.finish, Some(Finish::Faulted(fault)), "{inst}");
        assert_eq!(res.reference.steps, 3, "{inst}");
    }
}

#[test]
fn control_lists_repeated_entries_fault_order_and_self_promoting_undo() {
    // 1. Empty list (n=0) ignores invalid/OOB/qubit-cell pointer; repeated entries act as one control.
    let list_prog = module(
        "memory 512
; Control list at 64: (handle 2, bit 0), (handle 2, bit 0) -- repeated control
data 64 = i64 2, i64 0, i64 2, i64 0
; Control list at 128: (handle 1, bit 0), (handle 2, bit 1), (handle 1, bit 0) -- 3 entries, 2 distinct controls
data 128 = i64 1, i64 0, i64 2, i64 1, i64 1, i64 0
func @main(i64 %angle) {
entry:
 %a = qalloc 2
 %b = qalloc 2
 x [32:0]
 ; n=0 with OOB address 4096 and with qubit-cell address 32 both succeed.
 gate(4) %a[0] ctrl list(4096, 0)
 gate(4) %b[0] ctrl list(32, 0)
 ; Repeated control on b[0] applies X to b[1].
 gate(1) %b[1] ctrl list(64, 2)
 ; Repeated control (a[0], b[1], a[0]) applies P(angle) to a[1] after X sets a[1]=1.
 x %a[1]
 rotate(12, %angle) %a[1] ctrl list(128, 3)
 ret
}",
    );
    let theta = 1.25_f64;
    let phase = C::from_polar(0.5, theta);
    let half = C::new(0.5, 0.0);
    // Live columns: %a (width 2), %b (width 2), cell 32 (value 1).
    // a[1]=1 always (bit 1 -> +2); a[0] in |+>; b[0] in |+> with b[1]=b[0] -> b in {0, 3}.
    // Phase e^{i*theta} applies only when a[0]=1 AND b[1]=1 (i.e. a=3, b=3).
    let expected = pure(&[(vec![2, 0, 1], half), (vec![3, 0, 1], half), (vec![2, 3, 1], half), (vec![3, 3, 1], phase)]);
    let res = run_partial_undo_and_redo(&list_prog, "main", &[theta.to_bits()], 0, BUDGET, 4);
    assert_eq!(res.reference.finish, Some(Finish::Returned(None)));
    assert_density(&res.reference.quantum.density, &expected, "control list with repeated entries");

    // 2. Self-promoting target: target is a memory byte inside its own control list, then undone!
    let self_promote = module(
        "memory 256
; Control entry at 64: handle 1, bit 0 -> byte 64 is initially 0x01.
data 64 = i64 1, i64 0
func @main() {
entry:
 %c = qalloc 1
 h %c[0]
 ; Target [64:1] flips bit 1 of byte 64 (0x01 -> 0x03) where c[0]=1, promoting byte 64.
 gate(1) [64:1] ctrl list(64, 1)
 ret
}",
    );
    let expected_self = pure(&[(vec![0, 1], C::new(FRAC_1_SQRT_2, 0.0)), (vec![1, 3], C::new(FRAC_1_SQRT_2, 0.0))]);
    let res_self = run_partial_undo_and_redo(&self_promote, "main", &[], 0, BUDGET, 2);
    assert_eq!(res_self.reference.finish, Some(Finish::Returned(None)));
    assert_density(&res_self.reference.quantum.density, &expected_self, "self-promoting control list");

    // 3. Exact fault precedence for GateBy (written and list controls).
    let cases = [
        ("gate(0) %dead[99] ctrl list(4096, 1)", Fault::BadGate),
        ("gate(9) %dead[99] ctrl list(4096, 1)", Fault::BadGate),
        ("rotate(1, %nan) %dead[99] ctrl list(4096, 1)", Fault::BadGate),
        ("rotate(13, %nan) %dead[99] ctrl list(4096, 1)", Fault::BadGate),
        ("rotate(12, %nan) %dead[99] ctrl list(4096, 1)", Fault::BadAngle),
        ("gate(1) %dead[99] ctrl list(4096, 1)", Fault::DanglingRegister),
        ("gate(1) %live[99] ctrl list(4096, 1)", Fault::BadRange),
        ("gate(1) [4096:0] ctrl list(4096, 1)", Fault::OutOfBounds),
        ("gate(1) %live[0] ctrl list(64, %overflow_n)", Fault::BadArgument),
        ("gate(1) %live[0] ctrl list(504, 1)", Fault::BadArgument),
        ("gate(1) %live[0] ctrl list(32, 1)", Fault::BadArgument), // byte 32 holds qubits
        ("gate(1) %live[0] ctrl list(160, 2)", Fault::DanglingRegister), // entry 0 aliases target, entry 1 dangling -> 9 beats 10
        ("gate(1) %live[0] ctrl list(192, 2)", Fault::BadRange), // entry 0 aliases target, entry 1 out-of-range -> 22 beats 10
        ("gate(1) %live[0] ctrl list(224, 2)", Fault::QubitAlias), // entry 0 ok, entry 1 aliases target -> 10
    ];
    let overflow_n = (u64::MAX / 16) + 1;
    let a0 = C::new(FRAC_1_SQRT_2, 0.0);
    let a1 = C::from_polar(FRAC_1_SQRT_2, FRAC_PI_4);
    let expected_unchanged = pure(&[(vec![0, 0], a0), (vec![1, 1], a1)]);
    for (inst, fault) in cases {
        let program = module(&format!(
            "memory 512
; 160: entry 0 = (live=2, bit 0), entry 1 = (dead=1, bit 0)
data 160 = i64 2, i64 0, i64 1, i64 0
; 192: entry 0 = (live=2, bit 0), entry 1 = (live=2, bit 99)
data 192 = i64 2, i64 0, i64 2, i64 99
; 224: entry 0 = (live=2, bit 1), entry 1 = (live=2, bit 0)
data 224 = i64 2, i64 1, i64 2, i64 0
func @main(i64 %nan, i64 %overflow_n) {{
entry:
 %dead = qalloc i64 2
 qfree %dead
 %live = qalloc i64 2
 h %live[0]
 t %live[0]
 x [32:0] ctrl %live[0]
 {inst}
 effect 33
 ret
}}"
        ));
        let res = run_and_reverse(&program, "main", &[f64::NAN.to_bits(), overflow_n], 0, BUDGET);
        assert_eq!(res.reference.finish, Some(Finish::Faulted(fault)), "{inst}");
        assert_eq!(res.reference.steps, 6, "{inst}");
        assert!(res.reference.tape.is_empty(), "{inst}");
        assert_density(&res.reference.quantum.density, &expected_unchanged, inst);
    }
}

#[test]
fn block_apply_superposed_inputs_controls_faults_strict_mode_and_undo() {
    // Superposed control c in |+>, superposed 2-bit input x in (|0>+|1>+|2>+|3>)/2,
    // 4-bit out initialized to 5 (0b0101), 6-bit fault initialized to 32 (bit 5 set),
    // plus an uninvolved qubit u in |+> to prove ApplyBlock deduplicates inputs in ApplyReport.
    //
    // Image at 64 (size 16):
    //   byte 0: 0xf0 (in field x has 2 qubits at offset 0 -> high 6 bits zeroed to 0x00..0x03!)
    //   bytes 1..4: 0x11, 0x22, 0x33 (must survive untouched next to 2-bit in field!)
    //   byte 4: 0x10 (constant seed read by @mixed)
    //   byte 8: 0x00 (out field: 4 qubits)
    //
    // @mixed(block):
    //   also writes global byte 32 = 0x99 (must NOT survive apply!)
    //   x=0: writes (byte[4] + 3) = 0x13 to block+8 -> low 4 bits = 3 -> out ^= 3 (5^3 = 6)
    //   x=1: writes byte[1] (0x11) ^ 0x1b = 0x0a to block+8 -> low 4 bits = 10 -> out ^= 10 (5^10 = 15)
    //   x=2: udiv i8 1, 0 -> divide_by_zero (code 2) -> fault ^= 2 (32^2 = 34), out stays 5
    //   x=3: loops 20 times -> runs out of budget 12 (code 1) -> fault ^= 1 (32^1 = 33), out stays 5
    let program = module(
        "memory 512
data 32 = i8 0x42
data 64 = i8 0xf0, i8 0x11, i8 0x22, i8 0x33, i8 0x10, zero 11
; Fields at 96:
;   field 0: handle 2 (%x), low 0, bits 2, offset 0, dir 1 (IN)
;   field 1: handle 3 (%out), low 0, bits 4, offset 8, dir 2 (OUT)
data 96 = i64 2, i64 0, i64 2, i64 0, i64 1, i64 3, i64 0, i64 4, i64 8, i64 2
; Controls at 192: (handle 1 (%c), bit 0), repeated twice
data 192 = i64 1, i64 0, i64 1, i64 0

applied func @mixed(i64 %block) {
entry:
 store i8 0x99, 32
 %x = load i8 %block
 switch i8 %x, 0: case0, 1: case1, 2: case2, default: case3
case0:
 %p_seed = add i64 %block, 4
 %seed = load i8 %p_seed
 %v0 = add i8 %seed, 3
 %p_out0 = add i64 %block, 8
 store i8 %v0, %p_out0
 ret
case1:
 %p_b1 = add i64 %block, 1
 %b1 = load i8 %p_b1
 %v1 = xor i8 %b1, 0x1b
 %p_out1 = add i64 %block, 8
 store i8 %v1, %p_out1
 ret
case2:
 %boom = udiv i8 1, 0
 ret
case3:
 br loop(i64 20)
loop(i64 %n):
 %done = icmp eq i64 %n, 0
 br %done, finish, step
step:
 %next = sub i64 %n, 1
 br loop(%next)
finish:
 ret
}

func @main() -> i8 {
entry:
 %c = qalloc 1
 %x = qalloc 2
 %out = qalloc 4
 %fault = qalloc 6
 %u = qalloc 1
 h %c[0]
 h %x[0]
 h %x[1]
 x %out[0]
 x %out[2]
 x %fault[5]
 h %u[0]
 %fn = faddr @mixed
 apply %fn budget 12, fault %fault, image(64, 16), fields(96, 2), ctrl list(192, 2)
 ; Overwrite image, fields and controls after apply to test reversal order.
 store i64 0, 64
 store i64 0, 96
 store i64 0, 192
 %g = load i8 32
 ret %g
}",
    );

    // 16 basis states (2 controls * 4 inputs * 2 uninvolved u), each with amplitude 1/4.
    let amp = C::new(0.25, 0.0);
    let mut amps = Vec::new();
    for u in 0..=1u64 {
        for x in 0..4u64 {
            // c = 0: unchanged (out=5, fault=32).
            amps.push((vec![0, x, 5, 32, u], amp));
            // c = 1: mapped by @mixed.
            let (out, fault) = match x {
                0 => (5 ^ 3, 32),
                1 => (5 ^ 10, 32),
                2 => (5, 32 ^ 2),
                3 => (5, 32 ^ 1),
                _ => unreachable!(),
            };
            amps.push((vec![1, x, out, fault, u], amp));
        }
    }
    let expected = pure(&amps);
    let mixed_id = program.function_named("mixed").expect("mixed");

    // 1. Strict mode (allow_region_faults = false): ApplyBlock NEVER stops with RegionFault!
    let strict = run_strict(&program, "main", &[], 0, BUDGET);
    assert_eq!(strict.reference.finish, Some(Finish::Returned(Some(0x42))));
    assert_eq!(strict.reference.memory[32], 0x42, "private global write does not survive");
    assert_density(&strict.reference.quantum.density, &expected, "strict block apply");
    assert_eq!(
        strict.apply_reports,
        [ApplyReport { position: 1, callee: mixed_id, budget: 12, inputs: 4, largest: Some(9), at_budget: 1 }]
    );

    // 2. Full undo and partial-undo-and-redo across apply + caller overwrites of image/fields/controls.
    //    6 steps back undoes ret, load, 3 stores, AND the apply itself.
    let rev = run_partial_undo_and_redo(&program, "main", &[], 0, BUDGET, 6);
    assert_eq!(rev.reference.finish, Some(Finish::Returned(Some(0x42))));
    assert_density(&rev.reference.quantum.density, &expected, "reversible block apply");
    // Re-executing the apply after partial undo reports position 1 again.
    assert_eq!(rev.apply_reports.len(), 2);
    assert_eq!(rev.apply_reports[0], rev.apply_reports[1]);
}

#[test]
fn block_apply_shared_inputs_wide_fields_and_upper_fault_bits() {
    // 1. One qubit shared between two IN fields and a control, plus bits 5..8 of fault register
    //    used simultaneously as an OUT field while bits 0..5 receive faults (none here).
    let shared = module(
        "memory 512
data 64 = zero 16
; Fields at 96:
;   in0: handle 1 (%x), low 0, bits 1, offset 0, dir 1
;   in1: handle 1 (%x), low 0, bits 2, offset 4, dir 1 (shares qubit %x[0] with in0 AND control!)
;   out: handle 2 (%fault, width 8), low 5, bits 3, offset 8, dir 2 (bits 5..8 of fault reg!)
data 96 = i64 1, i64 0, i64 1, i64 0, i64 1, i64 1, i64 0, i64 2, i64 4, i64 1, i64 2, i64 5, i64 3, i64 8, i64 2
; Control at 256: handle 1 (%x), bit 0
data 256 = i64 1, i64 0

applied func @combine(i64 %b) {
entry:
 %a = load i8 %b
 %p1 = add i64 %b, 4
 %v = load i8 %p1
 %sum = add i8 %a, %v
 %p2 = add i64 %b, 8
 store i8 %sum, %p2
 ret
}

func @main() {
entry:
 %x = qalloc 2
 %fault = qalloc 8
 h %x[0]
 h %x[1]
 %fn = faddr @combine
 apply %fn budget 16, fault 2, image(64, 16), fields(96, 3), ctrl list(256, 1)
 ret
}",
    );
    // x in {0, 1, 2, 3} with amplitude 1/2 each:
    //   x=0, x=2 have x[0]=0 (control off) -> fault=0.
    //   x=1 (x[0]=1, x[1]=0): in0=1, in1=1 -> sum=2 -> XOR into fault bits 5..8 -> 2 << 5 = 64.
    //   x=3 (x[0]=1, x[1]=1): in0=1, in1=3 -> sum=4 -> XOR into fault bits 5..8 -> 4 << 5 = 128.
    let half = C::new(0.5, 0.0);
    let expected_shared = pure(&[(vec![0, 0], half), (vec![2, 0], half), (vec![1, 64], half), (vec![3, 128], half)]);
    let res = run_and_reverse(&shared, "main", &[], 0, BUDGET);
    assert_eq!(res.reference.finish, Some(Finish::Returned(None)));
    assert_density(&res.reference.quantum.density, &expected_shared, "shared IN fields, control, upper fault bits");

    // 2. Wide 130-qubit IN and OUT fields (17 bytes each).
    //    @wide_op copies byte 0 and byte 16, sets bits 130..136 in byte 16 to 1 (0xfc),
    //    which must be masked out to 130 bits so only bits 0..130 of out are XORed.
    let wide = module(
        "memory 512
data 64 = zero 40
; in: handle 1, low 0, bits 130, offset 0 (17 bytes: 0..17), dir 1
; out: handle 2, low 0, bits 130, offset 20 (17 bytes: 20..37), dir 2
data 128 = i64 1, i64 0, i64 130, i64 0, i64 1, i64 2, i64 0, i64 130, i64 20, i64 2

applied func @wide_op(i64 %b) {
entry:
 %lo = load i8 %b
 %p_hi = add i64 %b, 16
 %hi = load i8 %p_hi
 ; Flip bit 1 of byte 16 (qubit 129) and set unused high bits 2..8 to 1.
 %hi_out = xor i8 %hi, 0xfe
 %out_lo = add i64 %b, 20
 store i8 %lo, %out_lo
 %out_hi = add i64 %b, 36
 store i8 %hi_out, %out_hi
 ret
}

func @main() {
entry:
 %in = qalloc i64 130
 %out = qalloc i64 130
 %fault = qalloc 5
 h %in[0]
 x %in[129] ctrl %in[0]
 %fn = faddr @wide_op
 apply %fn budget 20, fault %fault, image(64, 40), fields(128, 2), ctrl list(0, 0)
 ret
}",
    );
    // Input 0: in = 0 -> lo=0, hi=0 -> hi_out = 0xfe -> low 2 bits = 0b10 (qubit 129 = 1, qubit 128 = 0).
    // Input 1: in = 1 | 2^129 -> lo=1, hi=2 -> hi_out = 0xfc -> low 2 bits = 0b00 (qubit 129 = 0), lo = 1.
    let mut b0 = basis([0, 0, 0]);
    b0[1].flip(129);
    let mut b1 = basis([1, 1, 0]);
    b1[0].flip(129);
    let expected_wide = pure_basis(&[(b0, C::new(FRAC_1_SQRT_2, 0.0)), (b1, C::new(FRAC_1_SQRT_2, 0.0))]);
    let res_wide = run_and_reverse(&wide, "main", &[], 0, BUDGET);
    assert_eq!(res_wide.reference.finish, Some(Finish::Returned(None)));
    assert_density(&res_wide.reference.quantum.density, &expected_wide, "130-qubit wide block apply");
}

#[test]
fn block_apply_placement_boundaries_and_setup_failures() {
    // 1. Exact placement formula: base = (stack - size) & !15, independent of budget.
    let placement = module(
        "memory 512
data 32 = zero 32
; out field: handle 1 (%out), low 0, bits 64, offset 0, dir 2
data 96 = i64 1, i64 0, i64 64, i64 0, i64 2

applied func @write_base(i64 %b) {
entry:
 store i64 %b, %b
 ret
}

func @main(i64 %size, i64 %pad, i64 %budget) -> i64 {
entry:
 %out = qalloc 64
 %fault = qalloc 5
 %buf = alloca %pad
 %sp = stacksave
 %fn = faddr @write_base
 apply %fn budget %budget, fault %fault, image(32, %size), fields(96, 1), ctrl list(0, 0)
 %base = measure i64 %out[0, 64]
 %diff = sub i64 %sp, %base
 ret %diff
}",
    );
    for (size, pad, budget_a, budget_b) in
        [(8u64, 0u64, 4u64, 50u64), (16, 8, 4, 20), (24, 16, 4, 100), (33, 24, 4, 40)]
    {
        let ra = run(&placement, "main", &[size, pad, budget_a], 0, BUDGET);
        let rb = run(&placement, "main", &[size, pad, budget_b], 0, BUDGET);
        assert_eq!(ra.reference.finish, rb.reference.finish, "placement independent of budget");
        let base_measured = ra.measurements[0].0 as u64;
        assert_eq!(base_measured % 16, 0, "16-byte aligned");
        // Verify exact formula from stack pointer in frame before apply:
        // main has 10 slots (80 bytes) -> entry stack = 512 - 80 = 432; alloca(pad) aligns down to 8.
        let sp = (432 - pad) & !7;
        assert_eq!(base_measured, (sp - size) & !15, "size={size}, pad={pad}");
    }

    // 2. Placement failure (base < static_end) -> out_of_memory (19) even with budget 0, report largest=Some(0).
    //    main has 3 slots (24 bytes): sp = 256 - 24 = 232.
    //    static_end = 208. For size = 32: (232 - 32) & !15 = 192 < 208 -> placement fails!
    let oom_floor = module(
        "memory 256
data 0 = zero 208
applied func @noop(i64 %b) {
entry:
 ret
}
func @main(i64 %budget) -> i64 {
entry:
 %fault = qalloc 5
 %fn = faddr @noop
 apply %fn budget %budget, fault %fault, image(0, 32), fields(0, 0), ctrl list(0, 0)
 %code = measure i64 %fault[0, 5]
 ret %code
}",
    );
    let noop_id = oom_floor.function_named("noop").expect("noop");
    for budget in [0u64, 10] {
        let res = run_strict(&oom_floor, "main", &[budget], 0, BUDGET);
        assert_eq!(res.reference.finish, Some(Finish::Returned(Some(u128::from(Fault::OutOfMemory.code())))));
        assert_eq!(
            res.apply_reports,
            [ApplyReport { position: 1, callee: noop_id, budget, inputs: 1, largest: Some(0), at_budget: 0 }]
        );
    }

    // 3. Placement failure due to held-qubit byte in [base, base + size) (does not shift base!),
    //    and reversible undo succeeds.
    //    main has 2 slots (16 bytes): sp = 256 - 16 = 240.
    //    size = 16 -> base = (240 - 16) & !15 = 224 -> block is [224, 240).
    //    Promote byte 227 -> inside [224, 240) -> placement fails with out_of_memory (19)!
    let oom_qubit = module(
        "memory 256
data 0 = zero 64
applied func @noop(i64 %b) {
entry:
 ret
}
func @main() {
entry:
 %fault = qalloc 5
 x [227:0]
 %fn = faddr @noop
 apply %fn budget 10, fault %fault, image(0, 16), fields(0, 0), ctrl list(0, 0)
 ret
}",
    );
    let noop_q = oom_qubit.function_named("noop").expect("noop");
    let res_q = run_and_reverse(&oom_qubit, "main", &[], 0, BUDGET);
    assert_eq!(res_q.reference.finish, Some(Finish::Returned(None)));
    assert_eq!(
        res_q.apply_reports,
        [ApplyReport { position: 1, callee: noop_q, budget: 10, inputs: 1, largest: Some(0), at_budget: 0 }]
    );
    assert_density(
        &res_q.reference.quantum.density,
        &pure(&[(vec![Fault::OutOfMemory.code(), 1], C::ONE)]),
        "placement blocked by qubit byte gives out_of_memory",
    );

    // 4. Placement succeeds right at static_end, but callee frame does not fit below base ->
    //    stack_overflow (6) even with budget 0, report largest=Some(0), at_budget=0.
    //    main has 3 slots (24 bytes): sp = 256 - 24 = 232.
    //    static_end = 208. size = 16 -> base = (232 - 16) & !15 = 208 >= 208 (placement succeeds!).
    //    @noop has 1 slot (8 bytes): 208 - 8 = 200 < 208 -> entry stack_overflow!
    let setup_so = module(
        "memory 256
data 0 = zero 208
applied func @noop(i64 %b) {
entry:
 ret
}
func @main(i64 %budget) -> i64 {
entry:
 %fault = qalloc 5
 %fn = faddr @noop
 apply %fn budget %budget, fault %fault, image(0, 16), fields(0, 0), ctrl list(0, 0)
 %code = measure i64 %fault[0, 5]
 ret %code
}",
    );
    let noop_so = setup_so.function_named("noop").expect("noop");
    for budget in [0u64, 10] {
        let res = run_strict(&setup_so, "main", &[budget], 0, BUDGET);
        assert_eq!(res.reference.finish, Some(Finish::Returned(Some(u128::from(Fault::StackOverflow.code())))));
        assert_eq!(
            res.apply_reports,
            [ApplyReport { position: 1, callee: noop_so, budget, inputs: 1, largest: Some(0), at_budget: 0 }]
        );
    }

    // 5. Placement & setup succeed with budget = 0 -> fault budget (1), largest = None, at_budget = 1.
    let zero_budget = module(
        "memory 256
data 0 = zero 64
applied func @noop(i64 %b) {
entry:
 ret
}
func @main() -> i64 {
entry:
 %fault = qalloc 5
 %fn = faddr @noop
 apply %fn budget 0, fault %fault, image(0, 16), fields(0, 0), ctrl list(0, 0)
 %code = measure i64 %fault[0, 5]
 ret %code
}",
    );
    let noop_zb = zero_budget.function_named("noop").expect("noop");
    let res_zb = run_strict(&zero_budget, "main", &[], 0, BUDGET);
    assert_eq!(res_zb.reference.finish, Some(Finish::Returned(Some(u128::from(Fault::Budget.code())))));
    assert_eq!(
        res_zb.apply_reports,
        [ApplyReport { position: 1, callee: noop_zb, budget: 0, inputs: 1, largest: None, at_budget: 1 }]
    );
}

#[test]
fn block_apply_check_order_and_in_region_precedence() {
    let overflow_fields = (u64::MAX / 40) + 1;
    let overflow_ctrls = (u64::MAX / 16) + 1;

    // Handles allocated in @main:
    //   1: %dead (width 5, freed)
    //   2: %narrow (width 4, too narrow for fault)
    //   3: %fault (width 6, valid fault register)
    //   4: %reg (width 8, valid data register)
    // Memory:
    //   byte 16: promoted to qubit cell
    //   32..48: ordinary image (16 bytes)
    //   64..104: invalid field (bad dir 3)
    //   112..152: field with dangling handle 1 (must give bad_argument 24, NOT 9!)
    //   160..200: field with bits = 0
    //   208..248: field with low = u64::MAX, bits = 1 (wrapping check)
    //   256..296: field with offset = 15, bits = 16 (2 bytes -> end 17 > size 16)
    //   304..384: two IN fields overlapping in block bytes (offset 0, 1 byte and offset 0, 1 byte)
    //   392..472: two OUT fields overlapping in qubits (reg 4 [0,2] and reg 4 [1,2])
    //   480..560: IN field and OUT field overlapping in qubits (reg 4 [2,2] and reg 4 [3,2])
    //   568..608: IN field overlapping fault[0..5] (handle 3, low 4, bits 2)
    //   616..656: OUT field on reg 4 [0, 2] (for control-overlap check)
    //   664..680: control entry (dead=1, bit 0) -> dangling_register (9)
    //   680..696: control entry (reg=4, bit 8) -> bad_range (22)
    //   696..712: control entry (reg=4, bit 1) -> overlaps OUT field at 616 -> bad_argument (24)
    //   712..728: control entry (fault=3, bit 4) -> overlaps fault[0..5] -> bad_argument (24)
    let cases = [
        // 1. Bad function (0, non-applied func) beats bad fault handle and bad size.
        ("apply 0 budget 10, fault 1, image(4096, 0), fields(4096, 1), ctrl list(4096, 1)", Fault::BadIndirect),
        ("apply %plain_fn budget 10, fault 1, image(4096, 0), fields(4096, 1), ctrl list(4096, 1)", Fault::BadIndirect),
        // 2. Fault register handle (9) then width < 5 (22), beating bad size/image.
        (
            "apply %app_fn budget 10, fault 1, image(4096, 0), fields(4096, 1), ctrl list(4096, 1)",
            Fault::DanglingRegister,
        ),
        ("apply %app_fn budget 10, fault 2, image(4096, 0), fields(4096, 1), ctrl list(4096, 1)", Fault::BadRange),
        // 3. Size == 0, size > memory_size, 40*M overflow, 16*N overflow -> bad_argument (24) beats dangling control.
        ("apply %app_fn budget 10, fault 3, image(32, 0), fields(0, 0), ctrl list(664, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 2048), fields(0, 0), ctrl list(664, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(0, %ovf_f), ctrl list(664, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(0, 0), ctrl list(664, %ovf_c)", Fault::BadArgument),
        // 4. Image/fields/controls readability (24), then control entries (9, 22) beating bad field entries (check 5)!
        ("apply %app_fn budget 10, fault 3, image(1020, 16), fields(0, 0), ctrl list(664, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(16, 4), fields(0, 0), ctrl list(664, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(1020, 1), ctrl list(664, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(16, 1), ctrl list(664, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(64, 1), ctrl list(1020, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(64, 1), ctrl list(16, 1)", Fault::BadArgument),
        // Control entry checks (check 4) beat malformed field entries (check 5)!
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(64, 1), ctrl list(664, 1)", Fault::DanglingRegister),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(64, 1), ctrl list(680, 1)", Fault::BadRange),
        // 5. Field entries in list order (all bad_argument 24, even dangling handle!).
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(64, 1), ctrl list(0, 0)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(112, 1), ctrl list(0, 0)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(160, 1), ctrl list(0, 0)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(208, 1), ctrl list(0, 0)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(256, 1), ctrl list(0, 0)", Fault::BadArgument),
        // 6. Disjointness checks (all bad_argument 24, never qubit_alias 10).
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(304, 2), ctrl list(0, 0)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(392, 2), ctrl list(0, 0)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(480, 2), ctrl list(0, 0)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(568, 1), ctrl list(0, 0)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(616, 1), ctrl list(696, 1)", Fault::BadArgument),
        ("apply %app_fn budget 10, fault 3, image(32, 16), fields(0, 0), ctrl list(712, 1)", Fault::BadArgument),
    ];

    let max = u64::MAX;
    for (inst, fault) in cases {
        let program = module(&format!(
            "memory 1024
data 32 = zero 16
data 64  = i64 4, i64 0, i64 1, i64 0, i64 3
data 112 = i64 1, i64 0, i64 1, i64 0, i64 1
data 160 = i64 4, i64 0, i64 0, i64 0, i64 1
data 208 = i64 4, i64 {max}, i64 1, i64 0, i64 1
data 256 = i64 4, i64 0, i64 16, i64 15, i64 1
data 304 = i64 4, i64 0, i64 1, i64 0, i64 1, i64 4, i64 1, i64 1, i64 0, i64 1
data 392 = i64 4, i64 0, i64 2, i64 0, i64 2, i64 4, i64 1, i64 2, i64 4, i64 2
data 480 = i64 4, i64 2, i64 2, i64 0, i64 1, i64 4, i64 3, i64 2, i64 4, i64 2
data 568 = i64 3, i64 4, i64 2, i64 0, i64 1
data 616 = i64 4, i64 0, i64 2, i64 0, i64 2
data 664 = i64 1, i64 0
data 680 = i64 4, i64 8
data 696 = i64 4, i64 1
data 712 = i64 3, i64 4

func @plain(i64 %b) {{
entry:
 ret
}}
applied func @app(i64 %b) {{
entry:
 ret
}}
func @main(i64 %ovf_f, i64 %ovf_c) {{
entry:
 %dead = qalloc 5
 qfree %dead
 %narrow = qalloc 4
 %fault = qalloc 6
 %reg = qalloc 8
 h %reg[0]
 x [16:0]
 %plain_fn = faddr @plain
 %app_fn = faddr @app
 {inst}
 effect 33
 ret
}}"
        ));
        let res = run_and_reverse(&program, "main", &[overflow_fields, overflow_ctrls], 0, BUDGET);
        assert_eq!(res.reference.finish, Some(Finish::Faulted(fault)), "{inst}");
        assert!(res.apply_reports.is_empty(), "outer fault emits no ApplyReport: {inst}");
        assert!(res.reference.tape.is_empty(), "{inst}");
    }

    // D14: every new form inside an applied function gives quantum_in_region (8) (or 7/13 for
    // effect/exit, or 19/24 for explicit faults) before any check, even with budget = 1!
    let in_region_cases = [
        ("%w = qwidth 0\n ret", Fault::QuantumInRegion),
        ("%m = measure i64 0[0, 0]\n ret", Fault::QuantumInRegion),
        ("gate(0) 0[99] ctrl list(4096, 1)\n ret", Fault::QuantumInRegion),
        ("rotate(0, 0) 0[99] ctrl list(4096, 1)\n ret", Fault::QuantumInRegion),
        ("y 0[99]\n ret", Fault::QuantumInRegion),
        ("qfree 0\n ret", Fault::QuantumInRegion),
        (
            "apply 0 budget 0, fault 0, image(4096, 0), fields(4096, 1), ctrl list(4096, 1)\n ret",
            Fault::QuantumInRegion,
        ),
        ("effect 65\n ret", Fault::EffectInRegion),
        ("exit i64 0", Fault::ExitInRegion),
        ("fault out_of_memory", Fault::OutOfMemory),
        ("fault bad_argument", Fault::BadArgument),
    ];
    for (body, expected_fault) in in_region_cases {
        let program = module(&format!(
            "memory 256
data 0 = zero 16
applied func @inner(i64 %b) {{
entry:
 {body}
}}
func @main() -> i64 {{
entry:
 %fault = qalloc 5
 %fn = faddr @inner
 apply %fn budget 1, fault %fault, image(0, 16), fields(0, 0), ctrl list(0, 0)
 %code = measure i64 %fault[0, 5]
 ret %code
}}"
        ));
        let inner_id = program.function_named("inner").expect("inner");
        let res = run_strict(&program, "main", &[], 0, BUDGET);
        assert_eq!(res.reference.finish, Some(Finish::Returned(Some(u128::from(expected_fault.code())))), "{body}");
        assert_eq!(
            res.apply_reports,
            [ApplyReport { position: 1, callee: inner_id, budget: 1, inputs: 1, largest: Some(1), at_budget: 0 }],
            "{body}"
        );
    }
}

#[test]
fn apply_step_report_measuring_replay_zero_inputs_and_undo_positions() {
    // Mix register-form apply and block-form apply, plus:
    //   - position 1: block apply with control=0 -> inputs=0, largest=None, at_budget=0
    //   - position 2: register apply on 2-bit superposition
    //   - position 3: block apply with explicit `fault budget` on one input -> largest=Some(k), at_budget=0
    //   - position 4: block apply with variable loop -> test replay at largest and largest - 1!
    let program = module(
        "memory 512
data 32 = zero 16
; in: handle 2 (%x), low 0, bits 2, offset 0, dir 1
; out: handle 3 (%out), low 0, bits 4, offset 8, dir 2
data 64 = i64 2, i64 0, i64 2, i64 0, i64 1, i64 3, i64 0, i64 4, i64 8, i64 2
; control: handle 1 (%c_off), bit 0
data 160 = i64 1, i64 0

func @reg_fn(i2 %v) -> i4 {
entry:
 %w = zext i2 %v to i4
 %r = add i4 %w, 1
 ret %r
}

applied func @explicit_budget(i64 %b) {
entry:
 %x = load i8 %b
 %hit = icmp eq i8 %x, 3
 br %hit, do_fault, ok
do_fault:
 fault budget
ok:
 ret
}

applied func @countdown(i64 %b) {
entry:
 %x = load i8 %b
 br loop(%x)
loop(i8 %n):
 %done = icmp eq i8 %n, 0
 br %done, out, next
next:
 %m = sub i8 %n, 1
 br loop(%m)
out:
 %p = add i64 %b, 8
 store i8 %x, %p
 ret
}

func @main(i64 %b4) {
entry:
 %c_off = qalloc 1
 %x = qalloc 2
 %out = qalloc 4
 %fault1 = qalloc 5
 %fault2 = qalloc 5
 h %x[0]
 h %x[1]
 %f_exp = faddr @explicit_budget
 %f_cnt = faddr @countdown
 ; Position 1: control is |0>, so 0 active inputs.
 apply %f_cnt budget 100, fault %fault1, image(32, 16), fields(64, 2), ctrl list(160, 1)
 ; Position 2: register-form apply.
 apply @reg_fn(%x) -> %out, %fault1 budget 100
 ; Position 3: explicit `fault budget` on x=3 (4 steps), return on x=0..2 (4 steps), at_budget=0!
 apply %f_exp budget 100, fault %fault1, image(32, 16), fields(64, 2), ctrl list(0, 0)
 ; Position 4: countdown with run-time budget %b4.
 apply %f_cnt budget %b4, fault %fault2, image(32, 16), fields(64, 2), ctrl list(0, 0)
 ret
}",
    );

    let reg_id = program.function_named("reg_fn").expect("reg_fn");
    let exp_id = program.function_named("explicit_budget").expect("explicit_budget");
    let cnt_id = program.function_named("countdown").expect("countdown");

    // Measuring run with ceiling 100, plus partial undo back past positions 3 and 4 and redo!
    let measured = run_partial_undo_and_redo(&program, "main", &[100], 0, BUDGET, 3);
    assert_eq!(measured.reference.finish, Some(Finish::Returned(None)));
    let cnt_largest = measured.apply_reports[3].largest.expect("countdown finished");
    assert_eq!(
        &measured.apply_reports[..4],
        &[
            ApplyReport { position: 1, callee: cnt_id, budget: 100, inputs: 0, largest: None, at_budget: 0 },
            ApplyReport { position: 2, callee: reg_id, budget: 100, inputs: 4, largest: Some(3), at_budget: 0 },
            ApplyReport { position: 3, callee: exp_id, budget: 100, inputs: 4, largest: Some(4), at_budget: 0 },
            ApplyReport {
                position: 4,
                callee: cnt_id,
                budget: 100,
                inputs: 4,
                largest: Some(cnt_largest),
                at_budget: 0
            },
        ]
    );
    // Partial undo of 3 steps stepped back over ret, apply #4, and apply #3, then replayed them
    // with rolled-back positions 3 and 4!
    assert_eq!(measured.apply_reports.len(), 6);
    assert_eq!(measured.apply_reports[4].position, 3);
    assert_eq!(measured.apply_reports[5].position, 4);

    // Replay with exact largest budget -> identical final density, 0 at_budget.
    let exact = run_and_reverse(&program, "main", &[cnt_largest], 0, BUDGET);
    assert_density(&exact.reference.quantum.density, &measured.reference.quantum.density, "replay at exact budget");
    assert_eq!(exact.apply_reports[3].at_budget, 0);

    // Replay with largest - 1 -> slowest input (x=3) hits budget!
    let short = run_and_reverse(&program, "main", &[cnt_largest - 1], 0, BUDGET);
    assert_eq!(short.apply_reports[3].at_budget, 1);

    // Without keep_apply_reports(), executor.apply_reports() stays empty.
    let mut silent = Executor::new(
        &program,
        program.function_named("main").expect("main"),
        &[100],
        Options { allow_region_faults: true, budget: Some(BUDGET), ..Options::default() },
    )
    .expect("executor starts");
    assert_eq!(silent.run(), Outcome::Returned(None));
    assert!(silent.apply_reports().is_empty());

    // Sanity check: formatting several apply instructions in one generated program doesn't lose position.
    let mut seq = String::from(
        "memory 256\ndata 0 = zero 16\napplied func @id(i64 %b) {\nentry:\n ret\n}\nfunc @main() {\nentry:\n %f = qalloc 5\n %fn = faddr @id\n",
    );
    for _ in 0..5 {
        writeln!(seq, " apply %fn budget 4, fault %f, image(0, 16), fields(0, 0), ctrl list(0, 0)").expect("write");
    }
    seq.push_str(" ret\n}");
    let seq_mod = module(&seq);
    let seq_run = run_and_reverse(&seq_mod, "main", &[], 0, BUDGET);
    let positions: Vec<u64> = seq_run.apply_reports.iter().map(|r| r.position).collect();
    assert_eq!(positions, [1, 2, 3, 4, 5]);
}
