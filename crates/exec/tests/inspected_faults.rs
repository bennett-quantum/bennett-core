//! A fault the simulator finds by looking at every configuration of the state at once is never a program's
//! fault code (the maintainer's decision of the afternoon of 2026-11-16: the simulator-only checks are never
//! delivered to a program). The one such fault today: a `longjmp` whose jump record is held in qubits whose
//! configurations disagree (`bad_longjmp`; that it faults at all is a debt, `docs/DESIGN.md`, "Qubits in
//! memory"). No call with a budget catches it and no `apply` XORs it into its fault register: it ends the
//! world, in strict mode and on the machine alike, as it does outside a call.

mod common;

use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;
use common::{bounded, executor, load};

/// `@callee(%buf, 0)` reserves 16 bytes and jumps through the record at `%buf`. Each caller writes, with
/// ordinary stores (`@record`), a record naming the callee's own frame (depth 1, function 0) at its `setjmp`
/// (block 1, instruction 0), the frame's entry stack pointer and a saved stack pointer 48 below it, then gates
/// bit 3 of the saved stack pointer: `z` keeps it (a valid record), `x` flips it (another valid record, 8 bytes
/// higher, still within the frame's allocations), `h` holds both. Each valid record lands the jump at the
/// `setjmp`, and the callee returns 9. The callers return `%y | %c << 8`.
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
    %y, %c = call @callee(%buf, i1 0) budget 50
    %c16 = zext i5 %c to i16
    %y16 = zext i8 %y to i16
    %shifted = shl i16 %c16, 8
    %packed = or i16 %shifted, %y16
    ret %packed
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
fn a_longjmp_over_disagreeing_qubits_is_never_a_call_fault_code() {
    let module = load(PROGRAM);
    for options in [bounded(), machine()] {
        // Either record alone jumps: the callee returns 9 and the code is 0 (`9 | 0 << 8`).
        for how in [0, 1] {
            assert_eq!(common::run(&module, "call", &[how], options).0, Outcome::Returned(Some(9)), "how {how}");
        }
        // Both at once: before 2026-11-16 the call caught `bad_longjmp` and the program returned 16 << 8,
        // telling with certainty that the byte was in superposition.
        // Now the world ends with the fault, as without the call.
        assert_eq!(
            common::run(&module, "call", &[2], options).0,
            Outcome::Faulted(Fault::BadLongjmp),
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
