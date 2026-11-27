//! Compare the documented outer device language with the executor on worked and generated programs.

#[path = "device_reference/mod.rs"]
mod device_reference;
#[path = "device_reference/generated.rs"]
mod generated;
#[path = "device_reference/memory.rs"]
mod memory;
#[path = "device_reference/memory_generated.rs"]
mod memory_generated;
#[path = "device_reference/nonlocal_generated.rs"]
mod nonlocal_generated;
#[path = "device_reference/nonlocal_worked.rs"]
mod nonlocal_worked;
#[path = "device_reference/compare.rs"]
mod observe;
#[path = "device_reference/registers.rs"]
mod registers;
#[path = "device_reference/registers_generated.rs"]
mod registers_generated;
#[path = "device_reference/wide.rs"]
mod wide;
#[path = "device_reference/wide_generated.rs"]
mod wide_generated;

use std::f64::consts::FRAC_1_SQRT_2;
use std::fmt::Write as _;

use bennett_ir::{Fault, ValidModule};
use device_reference::{Basis, Density, Finish};
use num_complex::Complex64 as C;
use observe::{assert_density, run};

const BUDGET: u64 = 256;

fn module(text: &str) -> ValidModule {
    bennett_ir::load(text).unwrap_or_else(|error| panic!("{error}\n{text}"))
}

fn pure(amplitudes: &[(Vec<u64>, C)]) -> Density {
    let logical: Vec<_> = amplitudes
        .iter()
        .map(|(words, amplitude)| (basis(words.iter().copied().map(u128::from)), *amplitude))
        .collect();
    pure_basis(&logical)
}

fn basis(values: impl IntoIterator<Item = u128>) -> Basis {
    values.into_iter().map(Into::into).collect()
}

fn pure_basis(amplitudes: &[(Basis, C)]) -> Density {
    amplitudes
        .iter()
        .flat_map(|(row, left)| {
            amplitudes.iter().map(move |(col, right)| ((row.clone(), col.clone()), left * right.conj()))
        })
        .collect()
}

#[test]
fn every_gate_with_zero_to_three_controls_matches_worked_amplitudes() {
    // Ry(0.8)|0>, then S, prepares a|0> + ib|1>. Both coefficients are nonzero and distinct.
    let (b, a) = 0.4_f64.sin_cos();
    let one = C::new(a, 0.0);
    let two = C::new(0.0, b);
    let (s, c) = 0.1735_f64.sin_cos();
    let phase = C::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2);
    let cases = [
        ("x", [two, one]),
        ("h", [(one + two) * FRAC_1_SQRT_2, (one - two) * FRAC_1_SQRT_2]),
        ("z", [one, -two]),
        ("s", [one, C::new(-b, 0.0)]),
        ("sdg", [one, C::new(b, 0.0)]),
        ("t", [one, two * phase]),
        ("tdg", [one, two * phase.conj()]),
        ("rx(-0.347)", [C::new(c * a - s * b, 0.0), C::new(0.0, c * b + s * a)]),
        ("ry(0.347)", [C::new(c * a, -s * b), C::new(s * a, c * b)]),
        ("rz(-0.347)", [one * C::new(c, s), two * C::new(c, -s)]),
    ];
    for (gate, active) in cases {
        for count in 0..=3 {
            let mut program = String::from("memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\nry(0.8) %q\ns %q\n");
            if count > 0 {
                writeln!(program, "%controls = qalloc {count}").expect("write control allocation");
                for bit in 0..count {
                    writeln!(program, "h %controls[{bit}]").expect("write control preparation");
                }
            }
            write!(program, "{gate} %q").expect("write gate");
            for bit in 0..count {
                let separator = if bit == 0 { " ctrl " } else { ", " };
                write!(program, "{separator}%controls[{bit}]").expect("write control");
            }
            program.push_str("\nret\n}");
            let assignments = 1 << count;
            let scale = (assignments as f64).sqrt().recip();
            let amplitudes: Vec<_> = (0..assignments)
                .flat_map(|control| {
                    let coefficients = if control == assignments - 1 { active } else { [one, two] };
                    coefficients.into_iter().enumerate().map(move |(target, value)| {
                        let basis = if count == 0 { vec![target as u64] } else { vec![target as u64, control] };
                        (basis, value * scale)
                    })
                })
                .collect();

            let module = module(&program);
            let result = run(&module, "main", &[], 0, BUDGET);
            assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
            assert_density(&result.reference.quantum.density, &pure(&amplitudes), &format!("{gate}, {count} controls"));
        }
    }
}

#[test]
fn partial_trace_removes_bell_coherence_and_preserves_surviving_phase() {
    let bell = module(
        "memory 128
func @main() {
entry:
 %keep = qalloc 1
 %drop = qalloc 1
 h %keep
 x %drop ctrl %keep
 qfree %drop
 ret
}",
    );
    let result = run(&bell, "main", &[], 0, BUDGET);
    let half = C::new(0.5, 0.0);
    let expected = Density::from([((basis([0]), basis([0])), half), ((basis([1]), basis([1])), half)]);
    assert_density(&result.reference.quantum.density, &expected, "half of a Bell pair");

    let ghz = module(
        "memory 256
func @main() {
entry:
 %drop = qalloc 1
 %keep = qalloc 3
 h %keep[0]
 x %keep[1] ctrl %keep[0]
 x %drop ctrl %keep[0]
 h %keep[2]
 s %keep[2] ctrl %keep[1]
 qfree %drop
 ret
}",
    );
    // The discarded qubit tags |000> + |100> and |011> + i|111>. Only each pair interferes.
    let half_amplitude = C::new(0.5, 0.0);
    let mut expected = pure(&[(vec![0], half_amplitude), (vec![4], half_amplitude)]);
    expected.extend(pure(&[(vec![3], half_amplitude), (vec![7], C::I * 0.5)]));
    let result = run(&ghz, "main", &[], 0, BUDGET);
    assert_density(&result.reference.quantum.density, &expected, "GHZ with a surviving controlled phase");
}

#[test]
fn pure_and_mixed_measurements_project_and_drive_nested_control() {
    let program = module(
        "memory 256
data 0 = i8 0
func @main(i1 %release) -> i8 {
entry:
 %q = qalloc 1
 %env = qalloc 1
 h %q
 x %env ctrl %q
 br %release, mixed, pure
mixed:
 qfree %env
 br observe
pure:
 br observe
observe:
 %m = call @read(%q)
 br %m, one, zero
one:
 store i8 81, 0
 br check(i8 81)
zero:
 store i8 43, 0
 br check(i8 43)
check(i8 %byte):
 effect %byte
 %again = measure %q
 %good = icmp eq i1 %again, %m
 br %good, done, fail
done:
 ret %byte
fail:
 fault unreachable
}
func @read(q1 %r) -> i1 {
entry:
 %m = measure %r
 ret %m
}",
    );
    for mixed in 0..=1 {
        let mut ones = 0;
        for seed in 0..128 {
            let result = run(&program, "main", &[mixed], seed, BUDGET);
            let [(first, before), (second, after)] = result.measurements.as_slice() else {
                panic!("two measurements");
            };
            for value in [0, 1] {
                assert!((before[&value] - 0.5).abs() < 1e-11);
            }
            assert!((after[first] - 1.0).abs() < 1e-11);
            assert_eq!(first, second);
            let bit = *first;
            let expected = if bit == 1 { 81 } else { 43 };
            assert_eq!(result.reference.tape, [expected]);
            assert_eq!(result.reference.memory[0], expected);
            assert_eq!(result.reference.finish, Some(Finish::Returned(Some(u128::from(expected)))));
            let basis = if mixed == 1 { vec![bit as u64] } else { vec![bit as u64; 2] };
            assert_density(&result.reference.quantum.density, &pure(&[(basis, C::ONE)]), "projected pair");
            ones += bit;
        }
        assert!((40..=88).contains(&ones), "seeded half-probability count: {ones}");
    }
}

#[test]
fn multibit_measurement_preserves_little_endian_order_and_returned_handles() {
    let program = module(
        "memory 128
func @fresh() -> q64 {
entry:
 %r = qalloc 64
 x %r[63]
 x %r[7]
 x %r[0]
 ret %r
}
func @main() -> i64 {
entry:
 %r = call @fresh()
 %value = measure %r
 ret %value
}",
    );
    let expected = (1 << 63) | (1 << 7) | 1;
    let result = run(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(Some(expected))));
    assert_eq!(result.measurements[0].0, expected);
    assert_eq!(result.measurements[0].1[&expected], 1.0);
    let handle = run(&program, "fresh", &[], 0, BUDGET);
    assert_eq!(handle.reference.finish, Some(Finish::Returned(None)));
}

#[test]
fn stale_handles_fault_even_after_allocating_a_fresh_register() {
    for preparation in ["", "x %old", "h %old"] {
        for instruction in [
            "x %old",
            "x %fresh ctrl %old",
            "%m = measure %old",
            "qfree %old",
            "apply @copy(%old) -> %fresh, %flag budget 2",
            "apply @copy(%fresh) -> %old, %flag budget 2",
        ] {
            let program = module(&format!(
                "memory 256
func @main() {{
entry:
 %old = qalloc 1
 {preparation}
 qfree %old
 %fresh = qalloc 1
 %flag = qalloc 5
 {instruction}
 effect 33
 ret
}}
func @copy(i1 %n) -> i1 {{
entry:
 ret %n
}}"
            ));
            let result = run(&program, "main", &[], 0, BUDGET);
            assert_eq!(result.reference.finish, Some(Finish::Faulted(Fault::DanglingRegister)), "{instruction}");
            assert!(result.reference.tape.is_empty());
        }
    }
}

#[test]
fn dynamically_aliased_qubits_fault_before_acting() {
    for body in ["x %a ctrl %b", "x %c ctrl %a, %b", "apply @copy(%a) -> %b, %f budget 1"] {
        let program = module(&format!(
            "memory 256
func @main() {{
entry:
 %r = qalloc 1
 %s = qalloc 1
 %f = qalloc 5
 h %r
 call @alias(%r, %r, %s, %f)
 ret
}}
func @alias(q1 %a, q1 %b, q1 %c, q5 %f) {{
entry:
 {body}
 ret
}}
func @copy(i1 %n) -> i1 {{
entry:
 ret %n
}}"
        ));
        let result = run(&program, "main", &[], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Faulted(Fault::QubitAlias)), "{body}");
        assert_density(
            &result.reference.quantum.density,
            &pure(&[(vec![0, 0, 0], C::new(FRAC_1_SQRT_2, 0.0)), (vec![1, 0, 0], C::new(FRAC_1_SQRT_2, 0.0))]),
            "alias leaves the state as prepared",
        );
    }

    let program = module(
        "memory 256
func @main() {
entry:
 %flag = qalloc 5
 call @alias(%flag, %flag)
 ret
}
func @alias(q5 %a, q5 %b) {
entry:
 apply @constant() -> %a, %b budget 1
 ret
}
func @constant() -> i5 {
entry:
 ret 31
}",
    );
    assert_eq!(run(&program, "main", &[], 0, BUDGET).reference.finish, Some(Finish::Faulted(Fault::QubitAlias)));
}

#[test]
fn apply_reads_current_memory_and_faults_per_basis_before_release_and_measurement() {
    // The first address is below the static floor; the second exercises a caller write above it.
    for addr in [16, 64] {
        let program = module(&format!(
            "memory 256
data 16 = i8 4, zero 15
func @main() -> i8 {{
entry:
 store i8 7, {addr}
 %q = qalloc 1
 %out = qalloc 1
 %flag = qalloc 5
 h %q
 t %q
 x %out
 x %flag[4]
 apply @f(%q, %q, i64 {addr}) -> %out, %flag budget 8
 qfree %q
 %mflag = measure %flag
 %mout = measure %out
 %code = zext i5 %mflag to i8
 %bit = zext i1 %mout to i8
 %result = add i8 %code, %bit
 effect %result
 ret %result
}}
func @f(i1 %x, i1 %alias, i64 %addr) -> i1 {{
entry:
 %old = load i8 %addr
 store i8 0, %addr
 %valid = xor i1 %x, %alias
 br %valid, fail, choose
choose:
 br %x, fail, pass
pass:
 %v = trunc i8 %old to i1
 ret %v
fail:
 fault divide_by_zero
}}"
        ));
        for seed in 0..16 {
            let result = run(&program, "main", &[], seed, BUDGET);
            let flag = result.measurements[0].0;
            assert!(flag == 16 || flag == 18);
            let bit = u128::from(flag == 18);
            assert_eq!(result.measurements[1].0, bit);
            assert_eq!(result.reference.finish, Some(Finish::Returned(Some(flag + bit))));
            assert_eq!(result.reference.memory[addr], 7, "applied store is private");
            assert_eq!(result.reference.tape, [(flag + bit) as u8]);
        }
    }
}

#[test]
fn nested_exit_checks_entry_type_and_outer_budget() {
    let program = module(
        "memory 256
data 0 = i8 0
func @main(i1 %bad) -> i8 {
entry:
 effect 65
 %fp = faddr @leaf
 call_indirect fn(i1) %fp(%bad)
 effect 67
 ret 99
}
func @leaf(i1 %bad) {
entry:
 store i8 7, 0
 effect 66
 br %bad, wrong, right
wrong:
 exit i16 23
right:
 exit i8 23
}",
    );
    for (arg, budget, answer, tape, steps) in [
        (0, 0, Finish::Faulted(Fault::Budget), &b""[..], 0),
        (0, 6, Finish::Faulted(Fault::Budget), &b"AB"[..], 6),
        (0, 7, Finish::Returned(Some(23)), &b"AB"[..], 7),
        (1, 7, Finish::Faulted(Fault::ExitTypeMismatch), &b"AB"[..], 6),
    ] {
        let result = run(&program, "main", &[arg], 0, budget);
        assert_eq!(result.reference.finish, Some(answer));
        assert_eq!(result.reference.tape, tape);
        assert_eq!(result.reference.steps, steps);
        assert_eq!(result.reference.memory[0], if budget == 0 { 0 } else { 7 });
    }
}

#[test]
fn ordinary_faults_are_atomic_at_the_boundary() {
    let cases = [
        ("memory 1", "%slot = alloca 1\nret", Fault::StackOverflow, 0),
        ("memory 64\ndata 60 = zero 1", "call @callee()\nret", Fault::StackOverflow, 0),
        ("memory 64", "%slot = alloca 63\nret", Fault::StackOverflow, 0),
        ("memory 64", "stackrestore 65\nret", Fault::BadStackRestore, 0),
        ("memory 64", "call_indirect fn() -> i8 2()\nret", Fault::BadIndirect, 0),
        ("memory 64", "store i16 0xaabb, 63\nret", Fault::OutOfBounds, 0),
        ("memory 64", "effect 33\n%v = sdiv i8 -8, 0\nret", Fault::DivideByZero, 1),
        ("memory 64", "effect 33\nfault unreachable", Fault::Unreachable, 1),
    ];
    for (layout, body, fault, steps) in cases {
        let program = module(&format!(
            "{layout}
func @main() {{
entry:
 {body}
}}
func @callee() {{
entry:
 %value = add i8 1, 2
 ret
}}"
        ));
        let result = run(&program, "main", &[], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Faulted(fault)), "{body}");
        assert_eq!(result.reference.steps, steps, "{body}");
        assert_eq!(result.reference.tape, if steps == 0 { &b""[..] } else { &b"!"[..] });
        assert!(result.reference.memory.iter().all(|&byte| byte == 0));
    }
}

#[test]
fn narrow_integer_words_and_stack_pointers_have_worked_results() {
    let program = module(
        "memory 259
data 0 = i8 0, zero 7
func @main() -> i64 {
entry:
 %saved = stacksave
 %local = alloca 3
 store i13 0x1357, %local
 %read = load i13 %local
 %low = trunc i13 %read to i8
 effect %low
 stackrestore %saved
 %shift = ashr i8 -2, 8
 effect %shift
 %minimum = sdiv i8 -128, -1
 effect %minimum
 %remainder = srem i8 -13, 5
 effect %remainder
 %unsigned = udiv i8 243, 5
 effect %unsigned
 %modulus = urem i8 243, 5
 effect %modulus
 %left = shl i8 161, 3
 effect %left
 %right = lshr i8 161, 3
 effect %right
 %cmp = icmp slt i13 %read, 0
 %choose = select i8 %cmp, 42, 84
 effect %choose
 %again = stacksave
 ret %again
}",
    );
    let result = run(&program, "main", &[], 0, BUDGET);
    let expected_stack = 259 - 14 * 8;
    let local = (expected_stack - 3) & !7;
    assert_eq!(result.reference.finish, Some(Finish::Returned(Some(expected_stack as u128))));
    assert_eq!(result.reference.tape, [0x57, 0, 128, 253, 48, 3, 8, 20, 42]);
    assert_eq!(&result.reference.memory[local..local + 2], [0x57, 0x13]);
}
