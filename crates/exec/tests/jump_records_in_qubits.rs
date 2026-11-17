//! A jump record held in qubits. Ordinary code that reads or writes a byte holding qubits faults
//! `qubit_memory_access` (the maintainer's decision of 2026-11-17), whatever the qubits hold: a record whose
//! qubits all agree faults as one in superposition does, in strict mode and on the machine alike. Inside
//! `apply` the applied function is ordinary code and faults the same way, for each argument value.

mod common;

use bennett_exec::{Options, Outcome, RunError};
use bennett_ir::Fault;
use common::{bounded, executor, load};

/// `@callee(%buf, 0)` reserves 16 bytes and jumps through the record at `%buf`. Each caller writes, with
/// ordinary stores (`@record`), a record naming the callee's own frame (depth 1, function 0) at its `setjmp`
/// (block 1, instruction 0), the frame's entry stack pointer and a saved stack pointer 48 below it, then gates
/// bit 3 of the saved stack pointer: `z` keeps it (a valid record), `x` flips it (another valid record, 8 bytes
/// higher, still within the frame's allocations), `h` holds both. Each valid record lands the jump at the
/// `setjmp`, and the callee returns 9. `@applied` returns `%y | %c << 8`.
const PROGRAM: &str = "memory 256

func @callee(i64 %buf, i1 %probe) -> i8 {
entry:
    br %probe, saved, jump
saved:
    %r = setjmp %buf
    %first = icmp eq i32 %r, 0
    br %first, fresh, landed
fresh:
    ret 2
landed:
    ret 9
jump:
    %pad = alloca 16
    longjmp %buf, 1
}

func @record(i64 %buf) {
entry:
    store i64 1, %buf
    %p1 = add i64 %buf, 8
    store i64 1, %p1
    %p2 = add i64 %buf, 16
    store i64 %buf, %p2
    %p3 = add i64 %buf, 24
    %s0 = sub i64 %buf, 48
    store i64 %s0, %p3
    ret
}

func @call(i8 %how) -> i16 {
entry:
    %buf = alloca 32
    call @record(%buf)
    %bit = add i64 %buf, 24
    switch i8 %how, 0: kept, 1: flipped, default: both
kept:
    z [%bit:3]
    br go
flipped:
    x [%bit:3]
    br go
both:
    h [%bit:3]
    br go
go:
    %y = call @callee(%buf, i1 0)
    %y16 = zext i8 %y to i16
    ret %y16
}

func @applied(i8 %how) -> i16 {
entry:
    %buf = alloca 32
    call @record(%buf)
    %bit = add i64 %buf, 24
    switch i8 %how, 0: kept, 1: flipped, default: both
kept:
    z [%bit:3]
    br go
flipped:
    x [%bit:3]
    br go
both:
    h [%bit:3]
    br go
go:
    %out = qalloc 8
    %fault = qalloc 5
    apply @callee(%buf, i1 0) -> %out, %fault budget 50
    %y = measure %out
    %c = measure %fault
    %c16 = zext i5 %c to i16
    %y16 = zext i8 %y to i16
    %shifted = shl i16 %c16, 8
    %packed = or i16 %shifted, %y16
    ret %packed
}";

fn machine() -> Options {
    Options { machine_meaning: true, ..bounded() }
}

/// Before the gate each record jumps (`@call` with the gate skipped: the callee returns 9). After it, the
/// `longjmp` faults whether the gate kept the record (`z`), flipped it (`x`) or superposed it (`h`).
#[test]
fn a_longjmp_through_a_record_holding_qubits_faults_whatever_they_hold() {
    let module = load(PROGRAM);
    let ungated = load(&PROGRAM.replace("    z [%bit:3]\n", "").replace("    x [%bit:3]\n", ""));
    for options in [bounded(), machine()] {
        for how in [0, 1] {
            assert_eq!(common::run(&ungated, "call", &[how], options).0, Outcome::Returned(Some(9)), "how {how}");
        }
        for how in [0, 1, 2] {
            let (outcome, e) = common::run(&module, "call", &[how], options);
            assert_eq!(outcome, Outcome::Faulted(Fault::QubitMemoryAccess), "how {how}");
            assert_eq!(e.stats().memory_qubit_bytes, 1, "how {how}: one byte holds qubits");
        }
    }
}

/// Under `apply` the applied function's `longjmp` reads the record from the bytes holding qubits: it faults
/// for every argument value, so strict mode stops (a region fault, the simulator's), and on the machine the
/// code 17 is XORed into the fault register and `out` stays 0: `out | fault << 8` is 17 << 8.
#[test]
fn an_applied_function_reading_the_record_faults_for_every_argument_value() {
    let module = load(PROGRAM);
    let applied = module.function_named("callee").expect("callee");
    for how in [0, 1, 2] {
        let mut e = executor(&module, "applied", &[how], machine());
        assert_eq!(e.run(), Outcome::Returned(Some(u128::from(Fault::QubitMemoryAccess.code()) << 8)), "how {how}");
        let (outcome, _) = common::run(&module, "applied", &[how], bounded());
        let expected = RunError::RegionFault {
            callee: applied,
            fault: Fault::QubitMemoryAccess,
            faulting: if how == 2 { 2 } else { 1 },
            configurations: if how == 2 { 2 } else { 1 },
        };
        assert_eq!(outcome, Outcome::Stopped(expected), "how {how}");
    }
}
