//! Worked observations of bytes that hold qubits and of adjacent ordinary memory.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::FRAC_1_SQRT_2;

use bennett_ir::Fault;
use num_complex::Complex64 as C;

use super::{BUDGET, Finish, assert_density, module, pure, run};

#[test]
fn mixed_controls_promote_nonzero_bytes_and_memory_rotations_match_amplitudes() {
    let program = module(
        "memory 256
data 16 = i8 0xa5, i8 0x57, i8 0x80, i8 0x69
func @main(i64 %p, i64 %r) {
entry:
 %q = qalloc 1
 h %q
 x [%p:1] ctrl %q[0], [%r:7]
 rx(0.7) [%p:0]
 ry(-0.4) [%p:0]
 rz(0.6) [%p:0]
 ret
}",
    );
    let result = run(&program, "main", &[16, 18], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
    assert_eq!(&result.reference.memory[16..20], [0, 0x57, 0, 0x69]);
    assert_eq!(result.reference.quantum.cells, BTreeMap::from([(16, 1), (18, 2)]));

    // On bit 0, Rx acts on |1>, then Ry mixes both amplitudes and Rz adds opposite phases.
    let (sx, cx) = 0.35_f64.sin_cos();
    let (sy, cy) = (-0.2_f64).sin_cos();
    let zero = C::from_polar(FRAC_1_SQRT_2, -0.3) * C::new(-sy * cx, -cy * sx);
    let one = C::from_polar(FRAC_1_SQRT_2, 0.3) * C::new(cy * cx, -sy * sx);
    let expected = pure(&[
        (vec![0, 0xa4, 0x80], zero),
        (vec![0, 0xa5, 0x80], one),
        (vec![1, 0xa6, 0x80], zero),
        (vec![1, 0xa7, 0x80], one),
    ]);
    assert_density(&result.reference.quantum.density, &expected, "mixed controls and rotations");
}

#[test]
fn partial_measurement_preserves_unmeasured_entanglement_and_demotes_only_full_bytes() {
    for (last, fault) in [
        ("ret %seen", None),
        ("%bad = load i9 32\n ret %seen", Some(Fault::QubitMemoryAccess)),
        ("store i9 0x1bc, 32\n ret %seen", Some(Fault::QubitMemoryAccess)),
    ] {
        let program = module(&format!(
            "memory 256
data 31 = i8 0xe1, i8 0x25, i8 0x42, i8 0x9a
func @main() -> i12 {{
entry:
 %q = qalloc 2
 h %q[0]
 h %q[1]
 x [32:0] ctrl %q[0]
 x [32:9] ctrl %q[0]
 x [33:7] ctrl %q[1]
 s [33:7] ctrl [33:6]
 %seen = measure i12 [32]
 %loaded = load i8 32
 effect %loaded
 store i8 0xc7, 32
 {last}
}}"
        ));
        let mut outcomes = BTreeSet::new();
        for seed in 0..12 {
            let result = run(&program, "main", &[], seed, BUDGET);
            let (seen, probabilities) = &result.measurements[0];
            outcomes.insert(*seen);
            assert_eq!(probabilities.len(), 2, "two measured bytes share their low-bit correlation");
            for value in [0x225, 0x024] {
                assert!((probabilities[&value] - 0.5).abs() < 1e-11);
            }
            let low = u64::from(*seen == 0x024);
            let cell = 0x42 ^ (low << 1);
            let expected = pure(&[
                (vec![low, cell], C::new(FRAC_1_SQRT_2, 0.0)),
                (vec![low + 2, cell | 0x80], C::new(0.0, FRAC_1_SQRT_2)),
            ]);
            assert_density(&result.reference.quantum.density, &expected, "unmeasured high-bit entanglement");
            assert_eq!(result.reference.quantum.cells, BTreeMap::from([(33, 1)]));
            assert_eq!(&result.reference.memory[31..35], [0xe1, 0xc7, 0, 0x9a]);
            assert_eq!(result.reference.tape, [*seen as u8], "the demoted byte is immediately readable");
            assert_eq!(result.reference.finish, Some(fault.map_or(Finish::Returned(Some(*seen)), Finish::Faulted)),);
        }
        assert_eq!(outcomes.len(), 2);
    }
}

#[test]
fn ordinary_measurement_masks_high_bits_and_mixed_measurement_assembles_a_wide_word() {
    let program = module(
        "memory 256
data 16 = i128 0x80fedcba987654321122334455667788
func @main() -> i128 {
entry:
 %ordinary = measure i77 [16]
 x [16:127]
 h [16:126]
 %mixed = measure i128 [16]
 %after = load i128 16
 ret %after
}",
    );
    let original = 0x80fe_dcba_9876_5432_1122_3344_5566_7788_u128;
    let ordinary = original & ((1 << 77) - 1);
    let base = original ^ (1 << 127);
    let high = base | (1 << 126);
    for seed in 0..12 {
        let result = run(&program, "main", &[], seed, BUDGET);
        let [(before, ordinary_probabilities), (after, mixed_probabilities)] = result.measurements.as_slice() else {
            panic!("two measurements");
        };
        assert_eq!(*before, ordinary);
        assert_eq!(ordinary_probabilities, &BTreeMap::from([(ordinary, 1.0)]));
        assert_eq!(mixed_probabilities.len(), 2);
        for value in [base, high] {
            assert!((mixed_probabilities[&value] - 0.5).abs() < 1e-11);
        }
        assert_eq!(result.reference.finish, Some(Finish::Returned(Some(*after))));
        assert_eq!(&result.reference.memory[16..32], &after.to_le_bytes());
        assert!(result.reference.quantum.cells.is_empty());
    }
}

#[test]
fn gate_faults_admit_no_new_cells_and_leave_the_held_state_untouched() {
    for (instruction, fault) in [
        ("x [%p:8] ctrl [%r:0]", Fault::QubitAlias),
        ("x [16:0] ctrl [%p:8], [%r:0]", Fault::QubitAlias),
        ("call @alias(%live, %live)", Fault::QubitAlias),
        ("x [16:0] ctrl [%p:8], [%r:0], [256]", Fault::OutOfBounds),
        ("x [16:0] ctrl [%p:8], [%r:0], %old", Fault::DanglingRegister),
        ("x [16:0] ctrl [0xffffffffffffffff:8]", Fault::OutOfBounds),
        ("x [255:8] ctrl [16:0]", Fault::OutOfBounds),
        ("x [24:0] ctrl [17:7], %old", Fault::DanglingRegister),
        ("x %old ctrl [16:0]", Fault::DanglingRegister),
    ] {
        let program = module(&format!(
            "memory 256
data 16 = i8 0x25, i8 0xf0
func @main(i64 %p, i64 %r) {{
entry:
 %old = qalloc 1
 qfree %old
 %live = qalloc 1
 h %live
 x [24:0] ctrl %live
 {instruction}
 ret
}}
func @alias(q1 %a, q1 %b) {{
entry:
 x [16:0] ctrl %a, %b
 ret
}}"
        ));
        let result = run(&program, "main", &[16, 17], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Faulted(fault)), "{instruction}");
        assert_eq!(&result.reference.memory[16..18], [0x25, 0xf0]);
        assert_eq!(result.reference.quantum.cells, BTreeMap::from([(24, 1)]));
        let amplitude = C::new(FRAC_1_SQRT_2, 0.0);
        let expected = pure(&[(vec![0, 0], amplitude), (vec![1, 1], amplitude)]);
        assert_density(&result.reference.quantum.density, &expected, instruction);
    }
}

#[test]
fn a_partially_measured_definite_byte_stays_held_and_range_faults_precede_cell_checks() {
    for (instruction, fault) in [
        ("%read = load i8 255", Fault::QubitMemoryAccess),
        ("store i1 0, 255", Fault::QubitMemoryAccess),
        ("%read = load i9 255", Fault::OutOfBounds),
        ("store i9 0x123, 255", Fault::OutOfBounds),
        ("%read = measure i9 [255]", Fault::OutOfBounds),
        ("%read = measure i16 [0xffffffffffffffff]", Fault::OutOfBounds),
    ] {
        let program = module(&format!(
            "memory 256
func @main() {{
entry:
 %q = qalloc 1
 x [255] ctrl %q
 %low = measure i5 [255]
 {instruction}
 ret
}}"
        ));
        let result = run(&program, "main", &[], 0, BUDGET);
        assert_eq!(result.reference.finish, Some(Finish::Faulted(fault)), "{instruction}");
        assert_eq!(result.measurements, [(0, BTreeMap::from([(0, 1.0)]))]);
        assert_eq!(result.reference.quantum.cells, BTreeMap::from([(255, 1)]));
        assert_density(&result.reference.quantum.density, &pure(&[(vec![0, 0], C::ONE)]), instruction);
    }
}

#[test]
fn apply_uses_ordinary_neighbors_and_checks_each_definite_access_before_writing() {
    for (width, address, fault) in [
        (8, 255, Fault::QubitMemoryAccess),
        (9, 254, Fault::QubitMemoryAccess),
        (9, 255, Fault::OutOfBounds),
        (16, u64::MAX, Fault::OutOfBounds),
    ] {
        for access in [format!("%bad = load i{width} %addr"), format!("store i{width} 0xbd, %addr")] {
            let program = module(&format!(
                "memory 256
func @main(i64 %addr) {{
entry:
 store i8 0x41, 254
 %q = qalloc 1
 h %q
 x [255] ctrl %q
 %out = qalloc 1
 %flag = qalloc 5
 x %out
 x %flag[3]
 apply @access(%q, %addr) -> %out, %flag budget 8
 ret
}}
func @access(i1 %choose, i64 %addr) -> i1 {{
entry:
 br %choose, hazard, ordinary
ordinary:
 %old = load i8 254
 store i8 0xee, 254
 %readback = load i8 254
 %wrote = icmp eq i8 %readback, 0xee
 %bit = trunc i8 %old to i1
 %result = and i1 %bit, %wrote
 ret %result
hazard:
 store i8 0, 254
 {access}
 ret 1
}}"
            ));
            let result = run(&program, "main", &[address], 0, BUDGET);
            assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
            assert_eq!(&result.reference.memory[254..], [0x41, 0]);
            assert_eq!(result.reference.quantum.cells, BTreeMap::from([(255, 3)]));
            let amplitude = C::new(FRAC_1_SQRT_2, 0.0);
            let expected = pure(&[(vec![0, 0, 8, 0], amplitude), (vec![1, 1, 8 ^ fault.code(), 1], amplitude)]);
            assert_density(&result.reference.quantum.density, &expected, &access);
        }
    }
}
