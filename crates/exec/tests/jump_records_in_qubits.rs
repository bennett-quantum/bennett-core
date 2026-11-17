//! A jump record held in qubits. Outside `apply`, ordinary code reading bytes that hold qubits whose
//! configurations disagree is not built: the simulator stops (`RunError::SuperposedMemoryRead`), in strict
//! mode and on the machine alike, which no program can read. Bytes whose configurations agree are read as
//! memory. Inside `apply` each configuration's run reads its own definite record from its private memory.

mod common;

use bennett_exec::{Options, Outcome, RunError};
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

#[test]
fn a_longjmp_over_disagreeing_qubits_stops_the_simulator() {
    let module = load(PROGRAM);
    for options in [bounded(), machine()] {
        // Either record alone jumps: the callee returns 9.
        for how in [0, 1] {
            assert_eq!(common::run(&module, "call", &[how], options).0, Outcome::Returned(Some(9)), "how {how}");
        }
        // Both at once: the simulator stops, with the state as before the `longjmp`.
        assert_eq!(
            common::run(&module, "call", &[2], options).0,
            Outcome::Stopped(RunError::SuperposedMemoryRead),
            "machine_meaning {}",
            options.machine_meaning
        );
    }
}

/// Under `apply` each configuration runs on private memory holding its own definite record, so each run
/// jumps: no fault, the result XORed in as usual.
#[test]
fn an_apply_runs_each_configuration_on_its_own_record() {
    let module = load(PROGRAM);
    for how in [0, 1, 2] {
        let mut e = executor(&module, "applied", &[how], machine());
        assert_eq!(e.run(), Outcome::Returned(Some(9)), "how {how}");
    }
}
