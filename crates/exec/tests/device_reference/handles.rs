//! Worked cases for integer handles (`qalloc i64 W`, `i64` handles in gates, controls and `qfree`),
//! run-time qubit positions (`%r[%i]`), fault precedence, and step-by-step undo.

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4, PI};
use std::io::ErrorKind;
use std::process::Command;

use bennett_exec::{Executor, Options, Outcome, ReverseError, RunError};
use bennett_ir::Fault;
use num_complex::Complex64 as C;

use super::device_reference::{Density, Device, Finish};
use super::observe::{run_and_reverse, run_partial_undo_and_redo, run_until_stop_with_args};
use super::{BUDGET, assert_density, basis, module, pure, pure_basis, run};

#[test]
fn mixed_allocations_number_handles_from_one_without_reuse_and_match_worked_state() {
    // Handle numbering across both forms:
    //   1: %q = qalloc 2
    //   2: %h = qalloc i64 3   (freed via %h_copy loaded from memory)
    //   3: %r = qalloc 1       (freed via i64 handle 3)
    //   4: %k = qalloc i64 2   (never reuses 2 or 3)
    //
    // State preparation before releases:
    //   h %q[0] -> (|0> + |1>)/sqrt(2) on %q[0]
    //   x %h[2] ctrl %q[0]  -> entangles %h[2] with %q[0]
    //   x %r[0] ctrl %q[0]  -> entangles %r[0] with %q[0]
    //   h %k[0]             -> (|0> + |1>)/sqrt(2) on %k[0]
    //   p(pi/2) %k[0] ctrl %h[2] -> phase i when %q[0]=1 and %k[0]=1
    //   qfree %h_copy (2) and qfree %r_handle (3):
    //     both tag %q[0], so tracing them out leaves a mixture over %q[0]:
    //     with prob 1/2: %q=0, %k in (|0> + |1>)/sqrt(2)
    //     with prob 1/2: %q=1, %k in (|0> + i|1>)/sqrt(2)
    let program = module(
        "memory 256
func @main() -> i64 {
entry:
 %q = qalloc 2
 %h = qalloc i64 3
 store i64 %h, 16
 %r = qalloc 1
 %k = qalloc i64 2
 %q_handle = add i64 0, 1
 %r_handle = add i64 %h, 1
 h %q_handle[0]
 x %h[2] ctrl %q[0]
 x %r_handle[0] ctrl %q_handle[0]
 h %k[0]
 p(1.5707963267948966) %k[0] ctrl %h[2]
 %h_copy = load i64 16
 call @release_handle(%h_copy)
 qfree %r_handle
 %packed = mul i64 %h, 100
 %result = add i64 %packed, %k
 ret %result
}
func @release_handle(i64 %id) {
entry:
 qfree %id
 ret
}",
    );
    // Live registers at completion: handle 1 (%q, width 2) and handle 4 (%k, width 2).
    // Half mixture of (|0,0> + |0,1>)/sqrt(2) and (|1,0> + i|1,1>)/sqrt(2).
    let half = C::new(0.5, 0.0);
    let mut expected = pure(&[(vec![0, 0], half), (vec![0, 1], half)]);
    expected.extend(pure(&[(vec![1, 0], half), (vec![1, 1], C::I * 0.5)]));

    let result = run_and_reverse(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(Some(204))));
    assert_eq!(result.reference.quantum.allocation_count, 4);
    assert_density(&result.reference.quantum.density, &expected, "mixed handles and partial trace");
}

#[test]
fn run_time_positions_on_qreg_i64_and_memory_match_worked_amplitudes_and_undo() {
    // Prepare wide and narrow registers, mix literal and run-time positions, computed angles,
    // and memory qubits in one program; overwrite the loop index so undo must restore it.
    let theta = 0.6_f64;
    let phi = 1.1_f64;
    let program = module(
        "memory 256
data 24 = i8 0x05
func @main(i64 %w, i64 %theta_bits, i64 %phi_bits) {
entry:
 %q = qalloc 4
 %h = qalloc i64 %w
 %zero = add i64 0, 0
 %top = sub i64 %w, 1
 h %q[%zero]
 x %h[%top] ctrl %q[%zero]
 p(%phi_bits) %h[%top] ctrl [24:0]
 ry(%theta_bits) %q[3] ctrl %h[%top], [24:2]
 br loop(i64 1)
loop(i64 %i):
 x %q[%i] ctrl %h[%top]
 %next = add i64 %i, 1
 %more = icmp ult i64 %next, 3
 br %more, loop(%next), done
done:
 ret
}",
    );
    // Bit 0 and bit 2 of byte 24 (0x05 = 0b101) are both 1.
    // On %q[0]=0: %q = 0, %h = 0, mem[24] = 5, amplitude 1/sqrt(2).
    // On %q[0]=1: %h[top] flips to 1, p(phi) gives e^{i*phi}, ry(theta) on %q[3] gives
    //   cos(theta/2)|0> + sin(theta/2)|1>, loop flips %q[1] and %q[2] -> %q is 0b0111 (7) or 0b1111 (15).
    let (s, c) = (theta / 2.0).sin_cos();
    let phase = C::from_polar(FRAC_1_SQRT_2, phi);
    for width in [1_u64, 4, 70, 129] {
        let top = (width - 1) as u32;
        let b0 = basis([0, 0, 5]);
        let mut b1 = basis([7, 0, 5]);
        b1[1].flip(top);
        let mut b2 = basis([15, 0, 5]);
        b2[1].flip(top);
        let expected = pure_basis(&[(b0, C::new(FRAC_1_SQRT_2, 0.0)), (b1, phase * c), (b2, phase * s)]);

        let result = run_and_reverse(&program, "main", &[width, theta.to_bits(), phi.to_bits()], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Returned(None)), "width {width}");
        assert_density(&result.reference.quantum.density, &expected, &format!("run-time positions width {width}"));
    }
}

#[test]
fn bad_width_and_out_of_handles_boundaries_and_fault_order() {
    let program = module(
        "memory 256
func @main(i64 %w) -> i64 {
entry:
 %q = qalloc 2
 h %q[0]
 s %q[0]
 %h = qalloc i64 %w
 x %h[0]
 ret %h
}",
    );
    let expected_before = pure(&[(vec![0], C::new(FRAC_1_SQRT_2, 0.0)), (vec![1], C::new(0.0, FRAC_1_SQRT_2))]);

    // Invalid widths: 0, 2^32, 2^32 + 1, 2^64 - 1 -> bad_width (20) with state unchanged.
    for width in [0_u64, 1 << 32, (1 << 32) + 1, u64::MAX] {
        let result = run_and_reverse(&program, "main", &[width], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Faulted(Fault::BadWidth)), "width {width}");
        assert_eq!(result.reference.steps, 3, "width {width}");
        assert_eq!(result.reference.quantum.allocation_count, 1, "width {width}: no handle given");
        assert_density(&result.reference.quantum.density, &expected_before, &format!("bad_width {width}"));
    }
    assert_eq!((Fault::BadWidth.code(), Fault::OutOfHandles.code()), (20, 21));

    // Valid lower boundary: width 1 succeeds as handle 2.
    let one = run_and_reverse(&program, "main", &[1], 0, BUDGET);
    assert_eq!(one.reference.finish, Some(Finish::Returned(Some(2))));
    assert_eq!(one.reference.quantum.allocation_count, 2);

    // Reference-level checks for out_of_handles (21) and its precedence after bad_width (20).
    let alloc_prog = module(
        "memory 256
func @by_width(i64 %w) -> i64 {
entry:
 %h = qalloc i64 %w
 ret %h
}
func @literal() -> q1 {
entry:
 %q = qalloc 1
 ret %q
}",
    );
    let by_width = alloc_prog.function_named("by_width").expect("by_width");
    let literal = alloc_prog.function_named("literal").expect("literal");

    // At 2^64 - 2 allocations, the next succeeds with handle 2^64 - 1.
    let mut at_last = Device::new(&alloc_prog, by_width, &[2], BUDGET);
    at_last.set_allocation_count(u64::MAX - 1);
    at_last.step(None);
    at_last.step(None);
    assert_eq!(at_last.finish, Some(Finish::Returned(Some(u128::from(u64::MAX)))));
    assert_eq!(at_last.quantum.allocation_count, u64::MAX);

    // At 2^64 - 1 allocations, valid width gives out_of_handles (21); qalloc N also gives out_of_handles.
    let mut exhausted = Device::new(&alloc_prog, by_width, &[2], BUDGET);
    exhausted.set_allocation_count(u64::MAX);
    exhausted.step(None);
    assert_eq!(exhausted.finish, Some(Finish::Faulted(Fault::OutOfHandles)));
    assert_eq!(exhausted.quantum.allocation_count, u64::MAX);
    assert!(exhausted.quantum.live.is_empty());

    let mut literal_exhausted = Device::new(&alloc_prog, literal, &[], BUDGET);
    literal_exhausted.set_allocation_count(u64::MAX);
    literal_exhausted.step(None);
    assert_eq!(literal_exhausted.finish, Some(Finish::Faulted(Fault::OutOfHandles)));

    // Precedence: bad_width (20) is checked BEFORE out_of_handles (21).
    for bad in [0_u64, 1 << 32, u64::MAX] {
        let mut both_bad = Device::new(&alloc_prog, by_width, &[u128::from(bad)], BUDGET);
        both_bad.set_allocation_count(u64::MAX);
        both_bad.step(None);
        assert_eq!(both_bad.finish, Some(Finish::Faulted(Fault::BadWidth)), "bad_width before out_of_handles: {bad}");
    }
}

#[test]
fn fault_precedence_and_position_boundaries_match_specification_with_state_unchanged() {
    // Prepared state before the single tested instruction (at step 10):
    //   handle 1: %q = qalloc 2, h %q[0], t %q[0]  -> (|0> + e^{i*pi/4}|1>)/sqrt(2)
    //   handle 2: %dead = qalloc i64 2, qfree %dead -> released handle 2
    //   handle 3: %live = qalloc i64 2, x %live[1] ctrl %q[0]
    //   memory[16] = 0xa5 (stays ordinary), memory[24] promoted by x [24:0] ctrl %q[0]
    //   %zero = 0, %never = 99, %q_id = 1, %two = 2, %big = 1<<32, %max = u64::MAX, %nan = NaN
    let cases = [
        // 1. BadAngle (18) precedes target/control handle, range, memory bounds and alias checks.
        ("rz(%nan) %dead[99] ctrl [256:0]", Fault::BadAngle),
        ("p(%nan) %zero[4294967296] ctrl %never[%max]", Fault::BadAngle),
        ("rx(%nan) %live[2] ctrl %live[2]", Fault::BadAngle),
        // 2. Handle checked before position on target.
        ("x %zero[99]", Fault::DanglingRegister),
        ("x %zero[%big]", Fault::DanglingRegister),
        ("x %zero[4294967296]", Fault::DanglingRegister),
        ("x %dead[2]", Fault::DanglingRegister),
        ("x %dead[%max]", Fault::DanglingRegister),
        ("x %never[99]", Fault::DanglingRegister),
        ("x %zero[0] ctrl %zero[0]", Fault::DanglingRegister),
        ("x %dead[0] ctrl %dead[0]", Fault::DanglingRegister),
        // 3. Target checked before controls.
        ("x %live[2] ctrl %dead[0]", Fault::BadRange),
        ("x %live[%two] ctrl %zero[0]", Fault::BadRange),
        ("x %live[4294967296] ctrl %dead[0]", Fault::BadRange),
        ("x %live[%big] ctrl [256:0]", Fault::BadRange),
        ("x %q[%two] ctrl %dead[0]", Fault::BadRange),
        ("x %q[%big] ctrl %zero[0]", Fault::BadRange),
        ("x %dead[0] ctrl [256:0]", Fault::DanglingRegister),
        ("x [256:0] ctrl %dead[0]", Fault::OutOfBounds),
        // 4. Controls checked left-to-right, handle before position, all before alias.
        ("x %live[0] ctrl %dead[99], [256:0]", Fault::DanglingRegister),
        ("x %live[0] ctrl %live[2], %dead[0]", Fault::BadRange),
        ("x %live[0] ctrl %live[0], %dead[0]", Fault::DanglingRegister),
        ("x %live[0] ctrl %live[0], %live[2]", Fault::BadRange),
        ("x %live[0] ctrl %live[0], %live[%big]", Fault::BadRange),
        ("x %live[0] ctrl %live[0], [256:0]", Fault::OutOfBounds),
        ("x [16:0] ctrl %live[0], %live[2]", Fault::BadRange),
        ("x [16:0] ctrl %live[0], %dead[0]", Fault::DanglingRegister),
        ("x [16:0] ctrl [256:0], %dead[0]", Fault::OutOfBounds),
        // 5. Position boundaries on live registers: width, 2^32 (literal and slot), 2^64 - 1.
        ("x %live[2]", Fault::BadRange),
        ("x %live[4294967296]", Fault::BadRange),
        ("x %live[18446744073709551615]", Fault::BadRange),
        ("x %live[%two]", Fault::BadRange),
        ("x %live[%big]", Fault::BadRange),
        ("x %live[%max]", Fault::BadRange),
        ("x %q[%two]", Fault::BadRange),
        ("x %q[%big]", Fault::BadRange),
        ("x %q[%max]", Fault::BadRange),
        ("x %live[2] ctrl %live[2]", Fault::BadRange),
        // 6. Alias check after every target and control passes.
        ("x %live[0] ctrl %live[0]", Fault::QubitAlias),
        ("x %live[%zero] ctrl %live[%zero]", Fault::QubitAlias),
        ("x %q[0] ctrl %q_id[0]", Fault::QubitAlias),
        ("x %q[%zero] ctrl %q_id[%zero]", Fault::QubitAlias),
        ("x %live[0] ctrl %q[1], %q_id[1]", Fault::QubitAlias),
        ("x %live[0] ctrl %q[%zero], %q_id[%zero]", Fault::QubitAlias),
        // 7. qfree on zero, released, never-allocated handles.
        ("qfree %zero", Fault::DanglingRegister),
        ("qfree %dead", Fault::DanglingRegister),
        ("qfree %never", Fault::DanglingRegister),
        ("qfree %max", Fault::DanglingRegister),
    ];

    let a0 = C::new(FRAC_1_SQRT_2, 0.0);
    let a1 = C::from_polar(FRAC_1_SQRT_2, FRAC_PI_4);
    // Live columns: %q (id 1), %live (id 3), cell 24.
    let expected = pure(&[(vec![0, 0, 0], a0), (vec![1, 2, 1], a1)]);

    for (inst, fault) in cases {
        let program = module(&format!(
            "memory 256
data 16 = i8 0xa5
func @main(i64 %big, i64 %max, i64 %nan) {{
entry:
 %q = qalloc 2
 h %q[0]
 t %q[0]
 %dead = qalloc i64 2
 qfree %dead
 %live = qalloc i64 2
 x %live[1] ctrl %q[0]
 x [24:0] ctrl %q[0]
 %zero = add i64 0, 0
 %q_id = add i64 0, 1
 %two = add i64 0, 2
 %never = add i64 0, 99
 {inst}
 effect 33
 ret
}}"
        ));
        let result = run_and_reverse(&program, "main", &[1 << 32, u64::MAX, f64::NAN.to_bits()], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Faulted(fault)), "{inst}");
        assert_eq!(result.reference.steps, 12, "{inst}");
        assert_eq!(result.reference.memory[16], 0xa5, "{inst}: unpromoted byte intact");
        assert_eq!(result.reference.quantum.cells.keys().copied().collect::<Vec<_>>(), [24], "{inst}");
        assert!(result.reference.tape.is_empty(), "{inst}");
        assert_eq!(result.reference.quantum.allocation_count, 3, "{inst}");
        assert_density(&result.reference.quantum.density, &expected, inst);
    }
}

#[test]
fn handle_forms_inside_apply_fault_quantum_in_region_before_other_checks() {
    for body in [
        "%h = qalloc i64 0",
        "%h = qalloc i64 %arg",
        "qfree %arg",
        "x %arg[99]",
        "x %arg[%arg]",
        "x %arg[4294967296]",
        "rz(%nan) %arg[%arg]",
        "p(%nan) %arg[4294967296]",
    ] {
        let program = module(&format!(
            "memory 256
func @f(i64 %arg, i64 %nan) -> i1 {{
entry:
 {body}
 ret 1
}}
func @main(i64 %nan) -> i5 {{
entry:
 %zero = add i64 0, 0
 %out = qalloc 1
 %flag = qalloc 5
 apply @f(%zero, %nan) -> %out, %flag budget 10
 %code = measure %flag
 ret %code
}}"
        ));
        let result = run(&program, "main", &[f64::NAN.to_bits()], 0, BUDGET);
        assert_eq!(
            result.reference.finish,
            Some(Finish::Returned(Some(u128::from(Fault::QuantumInRegion.code())))),
            "{body}"
        );
    }
}

#[test]
fn partial_undo_and_redo_through_allocations_releases_and_gates_matches_reference() {
    // Steps:
    //  0: %q = qalloc 2
    //  1: %h = qalloc i64 %w
    //  2: h %h[%top]
    //  3: x %q[1] ctrl %h[%top]
    //  4: p(%angle) %q[1] ctrl %h[%top]
    //  5: %clean = qalloc i64 1
    //  6: qfree %clean          (zero release)
    //  7: qfree %h              (entangled/nonzero release)
    //  8: %fresh = qalloc i64 2 (handle 4)
    //  9: x %fresh[0] ctrl %q[1]
    // 10: qfree %fresh          (entangled/nonzero release)
    // 11: ret %fresh
    let program = module(
        "memory 256
func @main(i64 %w, i64 %top, i64 %angle) -> i64 {
entry:
 %q = qalloc 2
 %h = qalloc i64 %w
 h %h[%top]
 x %q[1] ctrl %h[%top]
 p(%angle) %q[1] ctrl %h[%top]
 %clean = qalloc i64 1
 qfree %clean
 qfree %h
 %fresh = qalloc i64 2
 x %fresh[0] ctrl %q[1]
 qfree %fresh
 ret %fresh
}",
    );
    // Final state: %q is in the equal mixture of |00> (0) and |10> (2); handle returned is 4.
    let half = C::new(0.5, 0.0);
    let expected = Density::from([((basis([0]), basis([0])), half), ((basis([2]), basis([2])), half)]);
    let angle = 0.75_f64.to_bits();

    for (width, top) in [(1_u64, 0_u64), (3, 2), (70, 69)] {
        for undo_steps in [1_usize, 3, 5, 7, 10, 12] {
            let result = run_partial_undo_and_redo(&program, "main", &[width, top, angle], 0, BUDGET, undo_steps);
            assert_eq!(result.reference.finish, Some(Finish::Returned(Some(4))), "width {width}, undo {undo_steps}");
            assert_eq!(result.reference.quantum.allocation_count, 4);
            assert_density(
                &result.reference.quantum.density,
                &expected,
                &format!("partial undo {undo_steps} width {width}"),
            );
        }
    }
}

#[test]
fn reversal_past_measurement_stays_refused_with_handle_operations_after_it() {
    let program = module(
        "memory 256
func @main() -> i64 {
entry:
 %q = qalloc 1
 h %q[0]
 %m = measure %q
 %w = add i64 2, 0
 %h = qalloc i64 %w
 x %h[1]
 qfree %h
 ret %h
}",
    );
    let mut executor = Executor::new(
        &program,
        program.function_named("main").expect("main"),
        &[],
        Options { reversible: true, budget: Some(BUDGET), ..Options::default() },
    )
    .expect("executor starts");
    assert_eq!(executor.run(), Outcome::Returned(Some(2)));
    // Undo ret, qfree %h, x %h[1], qalloc i64 %w, add -> 5 steps back to right after measure.
    for step in 0..5 {
        assert_eq!(executor.step_back(), Ok(true), "undo post-measure step {step}");
    }
    assert_eq!(executor.snapshot().classical.handles.0, 1, "post-measure handle allocation undone");
    assert_eq!(executor.step_back(), Err(ReverseError::PastMeasurement));
    assert_eq!(executor.step_back(), Err(ReverseError::PastMeasurement));
}

#[test]
fn large_run_time_allocation_stops_by_name_with_state_unchanged() {
    const CHILD: &str = "BENNETT_REFERENCE_HANDLE_WIDE_LIMIT";
    if std::env::var_os(CHILD).is_none() {
        let child = Command::new("prlimit")
            .args(["--as=201326592", "--core=0", "--"])
            .arg(std::env::current_exe().expect("test binary"))
            .args(["--exact", "handles::large_run_time_allocation_stops_by_name_with_state_unchanged"])
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
    let live = 3;
    for width in [(1 << 31) + 1, max - 10, max] {
        let top = width - 1;
        let program = module(
            "memory 256
func @main(i64 %w, i64 %top) {
entry:
 %q = qalloc 3
 h %q[2]
 %h = qalloc i64 %w
 x %h[%top] ctrl %q[2]
 ret
}",
        );
        let stop = if live + width > max {
            RunError::QubitCount { live, more: width }
        } else {
            RunError::QubitStorage { qubits: width }
        };
        let mut reference = run_until_stop_with_args(&program, "main", &[width, top], BUDGET, 2, stop);
        while reference.finish.is_none() {
            reference.step(None);
        }
        assert_eq!(reference.finish, Some(Finish::Returned(None)));
        assert_eq!(reference.quantum.allocation_count, 2);
        let zero = basis([0, 0]);
        let mut high = basis([4, 0]);
        high[1].flip(top as u32);
        let expected = pure_basis(&[(zero, C::new(FRAC_1_SQRT_2, 0.0)), (high, C::new(FRAC_1_SQRT_2, 0.0))]);
        assert_density(&reference.quantum.density, &expected, &format!("ideal qalloc i64 {width}"));
    }
}

#[test]
fn controlled_phase_through_handles_and_run_time_positions_differs_from_rz() {
    let program = module(
        "memory 256
func @main(i64 %use_p, i64 %angle) -> i2 {
entry:
 %q = qalloc 2
 %h = add i64 0, 1
 %i0 = add i64 0, 0
 %i1 = add i64 0, 1
 h %h[%i0]
 x %q[%i1]
 %is_p = icmp ne i64 %use_p, 0
 br %is_p, do_p, do_rz
do_p:
 p(%angle) %h[%i1] ctrl %q[%i0]
 br finish
do_rz:
 rz(%angle) %h[%i1] ctrl %q[%i0]
 br finish
finish:
 h %q[%i0]
 %m = measure %q
 ret %m
}",
    );
    // At theta = pi: controlled p(pi) is CZ so %q[0] flips to 1 deterministically -> %q reads 3.
    // Controlled rz(pi) gives relative phase i so %q reads 2 or 3 with probability 1/2 each.
    let angle = PI.to_bits();
    let p_run = run(&program, "main", &[1, angle], 0, BUDGET);
    assert_eq!(p_run.reference.finish, Some(Finish::Returned(Some(3))));
    assert!((p_run.measurements[0].1[&3] - 1.0).abs() < 1e-11);

    let rz_run = run(&program, "main", &[0, angle], 0, BUDGET);
    assert!((rz_run.measurements[0].1[&2] - 0.5).abs() < 1e-11);
    assert!((rz_run.measurements[0].1[&3] - 0.5).abs() < 1e-11);
}
