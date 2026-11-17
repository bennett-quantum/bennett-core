//! What `qfree` does beside qubits in memory (`docs/DESIGN.md`, "Qubits in memory"): a dirty `qfree` on
//! the machine and a memory byte holding qubits both keep the qubits.

mod common;

use bennett_exec::{Options, Outcome};
use common::{bounded, executor, load};

/// A `qfree` of a register that is not zero, on the machine, leaves its qubits over, kept (strict mode: the
/// diagnostic `dirty_free`); a byte of memory holding qubits is never released either: its qubits stay where
/// they are. Both keep the pair, so a measurement afterwards finds it.
#[test]
fn a_dirty_free_and_a_cell_left_alone_both_keep_the_qubits() {
    let register = load(
        "memory 64
func @main() -> i2 {
entry:
    %q = qalloc 2
    h %q[0]
    x %q[1] ctrl %q[0]
    qfree %q
    ret 0
}",
    );
    let machine = Options { machine_meaning: true, ..bounded() };
    let mut e = executor(&register, "main", &[], machine);
    assert_eq!(e.run(), Outcome::Returned(Some(0)));
    assert_eq!(e.stats().leftover_qubits, 2);
    let pair = |snapshot: bennett_exec::Snapshot| -> Vec<u64> {
        snapshot.quantum.amplitudes.iter().map(|(config, _)| config[0] & 3).collect()
    };
    assert_eq!(pair(e.snapshot()), [0, 3]);
    let memory = load(
        "memory 64
func @main() -> i2 {
entry:
    z [16]
    h [16:0]
    x [16:1] ctrl [16:0]
    ret 0
}",
    );
    let mut e = executor(&memory, "main", &[], machine);
    assert_eq!(e.run(), Outcome::Returned(Some(0)));
    assert_eq!(pair(e.snapshot()), [0, 3]);
}
