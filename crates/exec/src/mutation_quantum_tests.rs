//! Allocation identities used by mark checks and leftover replay, not exposed by public snapshots.

use bennett_ir::Width;

use crate::quantum::QuantumState;

#[test]
fn leaving_an_allocation_undoes_its_number_and_reclaiming_restores_it() {
    let mut state = QuantumState::default();
    state.allocate(Width::new(3).expect("width"));
    let handle = state.allocate(Width::new(2).expect("width"));
    state.xor_register(1, 3);
    assert_eq!(state.allocations(), 2);
    assert_eq!(state.allocation_number(1), 2);
    state.leave_allocation(1);
    assert_eq!(state.allocations(), 1, "the undone allocation gives its number back");
    assert_eq!(state.leftover_qubits(), 2);
    assert_eq!(state.reclaim_allocation(1), handle);
    assert_eq!(state.allocations(), 2);
    assert_eq!(state.allocation_number(1), 2);
    assert_eq!(state.leftover_qubits(), 0);
}

#[test]
fn a_leftover_read_wire_consumes_no_program_allocation_number() {
    let mut state = QuantumState::default();
    state.allocate(Width::new(3).expect("width"));
    assert_eq!(state.allocations(), 1);
    let wire = state.allocate_wire(Width::new(5).expect("width"));
    assert_eq!(wire, 1);
    assert_eq!(state.allocations(), 1);
    state.leave(wire);
    state.reclaim(wire);
    state.discard_wire(wire);
    assert_eq!(state.allocations(), 1);
    state.allocate(Width::new(2).expect("width"));
    assert_eq!(state.allocations(), 2);
    assert_eq!(state.allocation_number(0), 1);
    assert_eq!(state.allocation_number(1), 2);
}

#[test]
fn equal_amplitudes_with_different_register_widths_are_different_register_tables() {
    let mut three_bits = QuantumState::default();
    three_bits.allocate(Width::new(3).expect("width"));
    let mut five_bits = QuantumState::default();
    five_bits.allocate(Width::new(5).expect("width"));
    assert!(three_bits.same_registers(&three_bits.clone()));
    assert!(!three_bits.same_registers(&five_bits));
    assert!(!five_bits.same_registers(&three_bits));
}
