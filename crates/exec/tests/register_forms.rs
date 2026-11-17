//! What `qalloc` and `qfree` do, written with the newer forms: qubits in memory (`docs/DESIGN.md`, "The forms
//! beside `apply`, `qalloc` and `qfree`"). `apply`, `qalloc` and `qfree` stay first-class forms (the
//! maintainer's decision of the evening of 2026-11-16); this shows the correspondence, not a migration. The
//! broad evidence is `memory_qubits.rs` (every register program of `worlds_oracle.rs` with its registers moved
//! into memory, and Grover); here, each register instruction beside its memory form, on one program.

mod common;

use bennett_exec::{Options, Outcome};
use common::{bounded, executor, load};

/// A Bell pair, read by a call whose result a rewind keeps, and measured, in a register (`%q = qalloc 2`,
/// cleared from its measured value and freed at the end) and in memory (the two qubits at bits 0 and 1 of a stack byte, promoted by `z`).
const REGISTER: &str = "memory 256
func @parity(i2 %v) -> i1 {
entry:
    %a = trunc i2 %v to i1
    %s = lshr i2 %v, 1
    %b = trunc i2 %s to i1
    %p = xor i1 %a, %b
    ret %p
}
func @main() -> i8 {
entry:
    %q = qalloc 2
    %o = qalloc 1
    h %q[0]
    x %q[1] ctrl %q[0]
    %m = mark budget 20
    %p = call @parity(%q)
    rewind %m keep (%p -> %o)
    %v = measure %q
    %w = measure %o
    x %q[1] ctrl %q[0]
    %set = icmp ne i2 %v, 0
    br %set, clear, free
clear:
    x %q[0]
    br free
free:
    qfree %q
    %v8 = zext i2 %v to i8
    %w8 = zext i1 %w to i8
    %w2 = shl i8 %w8, 2
    %r = or i8 %v8, %w2
    ret %r
}";

/// The same with `%q` in memory: `qalloc 2` is a stack byte promoted to qubits (`z` changes nothing else), a
/// qubit `%q[i]` is `[%a:i]`, the call's read of the register is a `load` of the byte, `measure %q` is
/// `measure i2 [%a]`, and the `qfree` of a register that is zero is nothing: a byte whose qubits are classical
/// reads as memory does, and stays a cell until the promotion is un-run.
const MEMORY: &str = "memory 256
func @parity(i2 %v) -> i1 {
entry:
    %a = trunc i2 %v to i1
    %s = lshr i2 %v, 1
    %b = trunc i2 %s to i1
    %p = xor i1 %a, %b
    ret %p
}
func @main() -> i8 {
entry:
    %a = alloca 1
    store i8 0, %a
    z [%a]
    %o = qalloc 1
    h [%a:0]
    x [%a:1] ctrl [%a:0]
    %m = mark budget 20
    %byte = load i8 %a
    %two = trunc i8 %byte to i2
    %p = call @parity(%two)
    rewind %m keep (%p -> %o)
    %v = measure i2 [%a]
    %w = measure %o
    %v8 = zext i2 %v to i8
    %w8 = zext i1 %w to i8
    %w2 = shl i8 %w8, 2
    %r = or i8 %v8, %w2
    ret %r
}";

/// Seed by seed both give the same result: 0 or 3 (the pair), and the parity 0 kept into `%o` (bit 2 clear).
#[test]
fn a_register_and_its_memory_form_agree_seed_by_seed() {
    let (register, memory) = (load(REGISTER), load(MEMORY));
    let mut seen = [0u32; 4];
    for seed in 0..32 {
        let options = Options { seed, ..bounded() };
        let a = common::run(&register, "main", &[], options).0;
        let b = common::run(&memory, "main", &[], options).0;
        assert_eq!(a, b, "seed {seed}");
        let Outcome::Returned(Some(r)) = a else { panic!("seed {seed}: {a:?}") };
        assert!(r == 0 || r == 3, "seed {seed}: {r}");
        seen[r as usize] += 1;
    }
    assert!(seen[0] > 0 && seen[3] > 0, "both outcomes of the pair occur: {seen:?}");
}

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
