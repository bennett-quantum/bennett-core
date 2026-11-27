//! Worked cases for wide logical registers and binary64 angles supplied by ordinary code.

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_1_SQRT_2, PI, TAU};
use std::io::ErrorKind;
use std::process::Command;

use bennett_exec::RunError;
use bennett_ir::Fault;
use num_complex::Complex64 as C;

use super::device_reference::Finish;
use super::observe::run_until_stop;
use super::{BUDGET, assert_density, basis, module, pure_basis, run};

#[test]
fn a_wide_measurement_reads_both_halves_as_one_little_endian_word() {
    let program = module(
        "memory 256
func @main() -> i128 {
entry:
 %q = qalloc 128
 x %q[64]
 h %q[127]
 x %q[63] ctrl %q[127]
 x %q[0] ctrl %q[127]
 %first = measure %q
 %again = measure %q
 ret %again
}",
    );
    let zero = 1 << 64;
    let one = (1 << 127) | (1 << 64) | (1 << 63) | 1;
    for seed in 0..8 {
        let result = run(&program, "main", &[], seed, BUDGET);
        let [(first, probabilities), (again, projected)] = result.measurements.as_slice() else {
            panic!("two measurements");
        };
        assert_eq!(probabilities.len(), 2);
        for word in [zero, one] {
            assert!((probabilities[&word] - 0.5).abs() < 1e-11, "probability for {word:#x}");
        }
        assert_eq!(first, again);
        assert_eq!(*projected, BTreeMap::from([(*first, 1.0)]));
        assert_eq!(result.reference.finish, Some(Finish::Returned(Some(*first))));
    }
}

#[test]
fn wide_apply_xors_upper_result_bits_and_preserves_upper_fault_bits() {
    let addend = (1_u128 << 64) | (1 << 63);
    let program = module(&format!(
        "memory 512
func @main() {{
entry:
 %arg = qalloc 128
 x %arg[64]
 h %arg[127]
 s %arg[127]
 %out = qalloc 128
 x %out[63]
 x %out[127]
 %flag = qalloc 257
 x %flag[0]
 x %flag[256]
 apply @add(%arg) -> %out, %flag budget 12
 ret
}}
func @add(i128 %value) -> i128 {{
entry:
 %upper = lshr i128 %value, 127
 %ok = icmp eq i128 %upper, 0
 br %ok, add, fail
add:
 %sum = add i128 %value, {addend}
 ret %sum
fail:
 fault divide_by_zero
}}"
    ));
    let mut success = basis([1 << 64, (1 << 127) | (1 << 65), 1]);
    let code = u128::from(Fault::DivideByZero.code());
    let mut failure = basis([(1 << 127) | (1 << 64), (1 << 127) | (1 << 63), 1 ^ code]);
    success[2].flip(256);
    failure[2].flip(256);
    let expected = pure_basis(&[(success, C::ONE * FRAC_1_SQRT_2), (failure, C::I * FRAC_1_SQRT_2)]);
    let result = run(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
    assert_density(&result.reference.quantum.density, &expected, "upper result and fault bits");
}

#[test]
fn tracing_a_wide_register_separates_terms_with_different_high_bits() {
    let program = module(
        "memory 256
func @main() {
entry:
 %keep = qalloc 128
 %discard = qalloc 129
 h %keep[127]
 x %discard[128] ctrl %keep[127]
 h %keep[63]
 s %keep[63] ctrl %discard[128]
 qfree %discard
 %clean = qalloc 129
 x %clean[128] ctrl %keep[63]
 x %clean[128] ctrl %keep[63]
 qfree %clean
 ret
}",
    );
    // Bit 128 of the released register tags two pairs of amplitudes. Trace it out,
    // retaining the phase inside each pair and dropping cross terms between the pairs.
    let a = C::new(0.5, 0.0);
    let mut expected = pure_basis(&[(basis([0]), a), (basis([1 << 63]), a)]);
    expected.extend(pure_basis(&[(basis([1 << 127]), a), (basis([(1 << 127) | (1 << 63)]), C::I * 0.5)]));
    let result = run(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
    assert_density(&result.reference.quantum.density, &expected, "partial trace of bit 128");
}

#[test]
fn computed_wide_rotations_match_worked_controlled_amplitudes() {
    for (gate, theta, stay, flip) in [
        ("rx", PI, C::ZERO, -C::I),
        ("ry", -PI, C::ZERO, -C::ONE),
        ("rz", PI, -C::I, C::ZERO),
        ("rx", TAU, -C::ONE, C::ZERO),
        ("ry", -0.0, C::ONE, C::ZERO),
        ("rz", f64::MAX, C::from_polar(1.0, -f64::MAX / 2.0), C::ZERO),
    ] {
        let program = module(&format!(
            "memory 256
data 8 = i8 0xa4
func @main(i64 %masked) {{
entry:
 %a = qalloc 65
 %b = qalloc 129
 h %a[64]
 %angle = xor i64 %masked, 0x55aa
 {gate}(%angle) %b[128] ctrl %a[64], [8:2]
 ret
}}"
        ));
        let control = 1 << 64;
        let mut target_set = basis([control, 0, 0xa4]);
        target_set[1].flip(128);
        // Half the amplitude weight bypasses the gate. The other half sees the first column
        // of the rotation matrix, so even a full turn can change a relative phase.
        let expected = pure_basis(&[
            (basis([0, 0, 0xa4]), C::ONE * FRAC_1_SQRT_2),
            (basis([control, 0, 0xa4]), stay * FRAC_1_SQRT_2),
            (target_set, flip * FRAC_1_SQRT_2),
        ]);
        let result = run(&program, "main", &[theta.to_bits() ^ 0x55aa], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
        assert_density(&result.reference.quantum.density, &expected, &format!("{gate}({theta:e})"));
    }
}

#[test]
fn nonfinite_computed_rotations_fault_before_promoting_memory() {
    let program = module(
        "memory 256
data 8 = i8 0xa4
func @main(i64 %bits) {
entry:
 %q = qalloc 129
 h %q[128]
 %angle = add i64 %bits, 1
 rz(%angle) [8:2] ctrl %q[128]
 effect 33
 ret
}",
    );
    let zero = basis([0]);
    let mut high = zero.clone();
    high[0].flip(128);
    let a = C::new(FRAC_1_SQRT_2, 0.0);
    let expected = pure_basis(&[(zero, a), (high, a)]);
    for bits in [f64::INFINITY.to_bits(), f64::NEG_INFINITY.to_bits(), 0x7ff0_0000_0000_0001, f64::NAN.to_bits()] {
        let result = run(&program, "main", &[bits - 1], 0, BUDGET);
        let reference = result.reference;
        assert_eq!(reference.finish, Some(Finish::Faulted(Fault::BadAngle)));
        assert_eq!(reference.steps, 3);
        assert_eq!(reference.memory[8], 0xa4);
        assert!(reference.quantum.cells.is_empty());
        assert!(reference.tape.is_empty());
        assert_density(&reference.quantum.density, &expected, "state at the fault");
    }
}

#[test]
fn applied_computed_rotation_faults_before_looking_at_the_angle() {
    let program = module(
        "memory 256
func @main(i64 %bits) -> i128 {
entry:
 %out = qalloc 1
 %flag = qalloc 128
 x %flag[127]
 apply @rotate(%bits) -> %out, %flag budget 8
 %code = measure %flag
 ret %code
}
func @rotate(i64 %bits) -> i1 {
entry:
 %angle = or i64 %bits, 1
 rx(%angle) [8:0]
 ret 1
}",
    );
    for bits in [0, f64::INFINITY.to_bits()] {
        let result = run(&program, "main", &[bits], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Returned(Some((1 << 127) | 8))));
        assert!(result.reference.quantum.cells.is_empty());
    }
}

#[test]
fn large_allocations_stop_by_name_with_the_state_unchanged() {
    const CHILD: &str = "BENNETT_REFERENCE_WIDE_LIMIT";
    if std::env::var_os(CHILD).is_none() {
        // 192 MiB fits the test process and its small initial state, but none of these allocations.
        let child = Command::new("prlimit")
            .args(["--as=201326592", "--core=0", "--"])
            .arg(std::env::current_exe().expect("test binary"))
            .args(["--exact", "registers::large_allocations_stop_by_name_with_the_state_unchanged"])
            .env(CHILD, "1")
            .output();
        match child {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(
                    output.status.success() && stdout.contains("1 passed"),
                    "limited child: {:?}\n{stdout}\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(error) if error.kind() == ErrorKind::NotFound => eprintln!("prlimit unavailable; skipping wide stops"),
            Err(error) => panic!("prlimit: {error}"),
        }
        return;
    }
    let max = u64::from(u32::MAX);
    let live = 129 + 8;
    for width in [(1 << 31) + 1, max - 512, max - 120, max] {
        let top = width - 1;
        let program = module(&format!(
            "memory 256
data 8 = i8 0x94
func @main() {{
entry:
 %q = qalloc 129
 h %q[128]
 s %q[128]
 x [8:0] ctrl %q[128]
 effect 65
 %large = qalloc {width}
 x %large[{top}] ctrl %q[128]
 ret
}}"
        ));
        let stop = if live + width > max {
            RunError::QubitCount { live, more: width }
        } else {
            RunError::QubitStorage { qubits: width }
        };
        let mut reference = run_until_stop(&program, BUDGET, 5, stop);
        while reference.finish.is_none() {
            reference.step(None);
        }
        assert_eq!(reference.finish, Some(Finish::Returned(None)));
        // The ideal instruction allocates successfully. Only the reference completes this part.
        let zero = basis([0, 0, 0x94]);
        let mut high = basis([0, 0, 0x95]);
        high[0].flip(128);
        high[1].flip(top.try_into().expect("top qubit"));
        let expected = pure_basis(&[(zero, C::ONE * FRAC_1_SQRT_2), (high, C::I * FRAC_1_SQRT_2)]);
        assert_density(&reference.quantum.density, &expected, &format!("ideal q{width}"));
    }
}
