//! Handles as integers (`docs/SYNTAX.md`, "Handles as integers"): the register made by the `k`-th successful
//! allocation of a run is the integer `k`, from 1, never given again; `qalloc i64 %w` takes its width at run
//! time; an `i64` handle names a register wherever a gate, a control or `qfree` names one; a position may be
//! computed at run time (`%r[%i]`). Checked: the numbering, the faults `bad_width`, `dangling_register` and
//! `bad_range` before anything changes, the alias check through run-time positions, forward then reverse
//! back to every earlier state, a register of 2^32 - 1 qubits on a device, and `quantum_in_region`.

mod common;

use std::ops::Range;

use bennett_exec::{Device, Executor, GateOp, Outcome, RunError, Wire};
use bennett_ir::Fault;
use common::{assert_reverses, bounded, executor, run};

fn returned(module: &str, args: &[u64]) -> Outcome {
    run(&common::load(module), "main", args, bounded()).0
}

/// The run's allocations are numbered 1, 2, 3, ... whatever their form, released ones included.
#[test]
fn handles_number_the_allocations_from_one_and_are_never_given_again() {
    let module = "memory 256
func @main(i64 %w) -> i64 {
entry:
    %a = qalloc 2
    %b = qalloc i64 %w
    qfree %b
    qfree %a
    %c = qalloc i64 1
    %d = qalloc i64 %w
    %e = mul i64 %c, 16
    %f = add i64 %e, %d
    ret %f
}";
    // Handles 3 and 4: neither 1 nor 2, released before, is given again.
    assert_eq!(returned(module, &[5]), Outcome::Returned(Some(3 * 16 + 4)));
}

/// An `i64` handle names its register as a `qN` value does: here the gates through `%h` act on the register
/// `%q` names, which is handle 1, and a copy into `%m` reads it.
#[test]
fn an_i64_handle_names_its_register_in_gates_controls_and_qfree() {
    let module = "memory 256
func @main() -> i3 {
entry:
    %q = qalloc 3
    %h = add i64 0, 1
    x %h[2]
    %m = qalloc 3
    x %m[0] ctrl %h[2]
    %k = qalloc i64 3
    x %k[1] ctrl %q[2]
    x %m[1] ctrl %k[1], %h[2]
    qfree %k
    %r = measure %m
    ret %r
}";
    assert_eq!(returned(module, &[]), Outcome::Returned(Some(0b011)));
}

/// `%r[%i]`: the position is an `i64` value; any position at or past the width is `bad_range`, before the
/// gate acts, whatever its high bits (2^32, -1).
#[test]
fn a_run_time_position_selects_the_qubit_and_is_checked_against_the_width() {
    let module = common::load(
        "memory 256
func @main(i64 %i) -> i4 {
entry:
    %q = qalloc 4
    %h = qalloc i64 4
    x %h[%i]
    x %q[%i] ctrl %h[%i]
    qfree %h
    %m = measure %q
    ret %m
}",
    );
    for i in 0..4u64 {
        assert_eq!(run(&module, "main", &[i], bounded()).0, Outcome::Returned(Some(1 << i)), "{i}");
    }
    for i in [4, 5, 1 << 32, (1 << 32) + 1, u64::MAX] {
        let mut executor = executor(&module, "main", &[i], bounded());
        for _ in 0..2 {
            executor.step();
        }
        let before = executor.snapshot();
        assert_eq!(executor.run(), Outcome::Faulted(Fault::BadRange), "{i}");
        let after = executor.snapshot();
        assert_eq!((after.quantum, after.classical.frames), (before.quantum, before.classical.frames), "{i}");
    }
    assert_eq!(Fault::BadRange.code(), 22);
}

/// A literal position on an `i64` handle is checked when the gate runs: the validator cannot see the width.
#[test]
fn a_literal_position_past_an_i64_handles_width_is_bad_range() {
    let module = "memory 256
func @main() {
entry:
    %h = qalloc i64 2
    h %h[1]
    x %h[2] ctrl %h[1]
    ret
}";
    assert_eq!(returned(module, &[]), Outcome::Faulted(Fault::BadRange));
}

/// A literal position from 2^32 on an `i64` handle is `bad_range` when the gate runs, with the state as
/// before, as the same position given in a slot is.
#[test]
fn a_literal_position_from_two_to_the_32_on_an_i64_handle_is_bad_range() {
    for position in ["4294967296", "18446744073709551615"] {
        let module = common::load(&format!(
            "memory 256
func @main() {{
entry:
    %h = qalloc i64 1
    h %h
    x %h[{position}]
    ret
}}"
        ));
        let mut executor = executor(&module, "main", &[], bounded());
        for _ in 0..2 {
            executor.step();
        }
        let before = executor.snapshot();
        assert_eq!(executor.run(), Outcome::Faulted(Fault::BadRange), "{position}");
        assert_eq!(executor.snapshot().quantum, before.quantum, "{position}");
    }
}

/// The same `i64` handle slot twice in one gate: its handle is checked first, so a value naming no register
/// is `dangling_register`; for a live register the repetition is `qubit_alias`.
#[test]
fn a_repeated_i64_handle_is_checked_for_its_handle_before_the_repetition() {
    let module = "memory 256
func @main(i64 %h) {
entry:
    %q = qalloc i64 1
    x %h[0] ctrl %h[0]
    ret
}";
    assert_eq!(returned(module, &[0]), Outcome::Faulted(Fault::DanglingRegister));
    assert_eq!(returned(module, &[2]), Outcome::Faulted(Fault::DanglingRegister));
    assert_eq!(returned(module, &[1]), Outcome::Faulted(Fault::QubitAlias));
}

/// Widths outside 1 to 2^32 - 1 are `bad_width` before anything is allocated.
#[test]
fn a_width_outside_one_to_two_to_the_32_minus_one_is_bad_width() {
    let module = common::load(
        "memory 256
func @main(i64 %w) {
entry:
    %q = qalloc 1
    %h = qalloc i64 %w
    ret
}",
    );
    for w in [0, 1 << 32, (1 << 32) + 1, u64::MAX] {
        let mut executor = executor(&module, "main", &[w], bounded());
        executor.step();
        let before = executor.snapshot();
        assert_eq!(executor.run(), Outcome::Faulted(Fault::BadWidth), "{w}");
        let after = executor.snapshot();
        assert_eq!(after.quantum, before.quantum, "{w}");
        assert_eq!(after.classical.handles, before.classical.handles, "{w}: no handle given");
    }
    assert_eq!((Fault::BadWidth.code(), Fault::OutOfHandles.code()), (20, 21));
}

/// A handle that names no register: released, never given, or 0.
#[test]
fn a_handle_that_names_no_register_is_dangling_register() {
    for body in [
        "%h = qalloc i64 2\n    qfree %h\n    qfree %h",
        "%h = qalloc i64 2\n    qfree %h\n    x %h[0]",
        "%h = qalloc i64 2\n    %g = add i64 %h, 1\n    x %h[0] ctrl %g[0]",
        "%g = add i64 0, 0\n    qfree %g",
        "%q = qalloc 1\n    %g = add i64 0, 2\n    x %q ctrl %g[0]",
    ] {
        let module = format!("memory 256\nfunc @main() {{\nentry:\n    {body}\n    ret\n}}");
        assert_eq!(returned(&module, &[]), Outcome::Faulted(Fault::DanglingRegister), "{body}");
    }
}

/// The target among the controls is `qubit_alias`, also when the positions are values.
#[test]
fn a_target_among_its_controls_through_run_time_positions_is_qubit_alias() {
    let module = common::load(
        "memory 256
func @main(i64 %i, i64 %j) {
entry:
    %h = qalloc i64 3
    %g = add i64 %h, 0
    x %h[%i] ctrl %g[%j]
    ret
}",
    );
    assert_eq!(run(&module, "main", &[1, 2], bounded()).0, Outcome::Returned(None));
    assert_eq!(run(&module, "main", &[2, 2], bounded()).0, Outcome::Faulted(Fault::QubitAlias));
}

/// Allocations, releases of a zero and of a nonzero register, and gates through handles and run-time
/// positions, undone one step at a time: each earlier state comes back, the handle table included.
#[test]
fn undoing_each_step_restores_every_earlier_state_and_the_handle_table() {
    let module = common::load(
        "memory 256
func @main(i64 %w, i64 %i) {
entry:
    %q = qalloc 2
    %h = qalloc i64 %w
    h %h[%i]
    x %q[1] ctrl %h[%i]
    %z = qalloc i64 1
    qfree %z
    qfree %h
    %k = qalloc i64 %w
    x %k[0] ctrl %q[1]
    qfree %k
    ret
}",
    );
    for (w, i) in [(3, 2), (1, 0), (70, 69)] {
        assert_eq!(assert_reverses(&module, "main", &[w, i], bounded()), Outcome::Returned(None), "{w}");
        let mut executor = executor(&module, "main", &[w, i], bennett_exec::Options { reversible: true, ..bounded() });
        let mut before = vec![executor.snapshot()];
        while executor.step().is_none() {
            before.push(executor.snapshot());
        }
        let (count, live) = executor.snapshot().classical.handles;
        assert_eq!((count, live.iter().map(|&(handle, _, width)| (handle, width)).collect()), (4, vec![(1, 2)]), "{w}");
        while let Some(expected) = before.pop() {
            assert!(executor.step_back().expect("reversible"), "{w}");
            common::assert_same_state(&executor.snapshot(), &expected, &format!("{w}: step {}", before.len()));
        }
    }
}

/// Inside an applied function `qalloc i64` and a gate at a run-time position are quantum instructions:
/// `quantum_in_region`, before any operand is looked at (here a width of 0 and an unallocated handle).
#[test]
fn the_new_forms_inside_an_applied_function_fault_quantum_in_region() {
    for body in ["%h = qalloc i64 %a", "x %a[%a]", "qfree %a", "%w = qwidth %a"] {
        let module = common::load(&format!(
            "memory 64
func @f(i64 %a) -> i1 {{
entry:
    {body}
    ret 0
}}
func @main(i64 %a) -> i5 {{
entry:
    %o = qalloc 1
    %f = qalloc 5
    apply @f(%a) -> %o, %f budget 10
    %m = measure %f
    ret %m
}}"
        ));
        let (outcome, _) = run(&module, "main", &[0], common::allowing_region_faults());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(Fault::QuantumInRegion.code()))), "{body}");
    }
}

/// A device that records allocations and gates, holding no state.
#[derive(Default)]
struct Wires {
    allocated: Vec<Range<Wire>>,
    targets: Vec<Wire>,
}

impl Device for Wires {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.allocated.push(wires);
        Ok(())
    }
    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        self.targets.push(gate.target);
        Ok(())
    }
    fn measure(&mut self, _wire: Wire) -> Result<bool, RunError> {
        Ok(false)
    }
    fn discard(&mut self, _wires: Range<Wire>) -> Result<(), RunError> {
        Ok(())
    }
    fn release_zero(&mut self, _wires: Range<Wire>) -> Result<(), RunError> {
        Ok(())
    }
}

/// The widest register, 2^32 - 1 qubits, by a run-time width, on a device that stores nothing: one range of
/// wires, its last qubit named by a run-time position.
#[test]
fn the_widest_run_time_width_reaches_a_device_as_one_range() {
    let module = common::load(
        "memory 256
func @main(i64 %w) {
entry:
    %h = qalloc i64 %w
    %last = sub i64 %w, 1
    x %h[%last]
    qfree %h
    ret
}",
    );
    let width = u64::from(u32::MAX);
    let mut device = Wires::default();
    let entry = module.function_named("main").expect("entry");
    let run = Executor::run_on(&module, entry, &[width], None, &mut device).expect("starts");
    assert_eq!(run.outcome, Outcome::Returned(None));
    assert_eq!(device.allocated.len(), 1);
    let wires = &device.allocated[0];
    assert_eq!(wires.end - wires.start, width);
    assert_eq!(device.targets, [wires.end - 1]);
}

/// A hexadecimal position names the same qubit after a named handle as after a constant one.
#[test]
fn a_hexadecimal_position_names_the_same_qubit_after_every_handle_form() {
    for place in ["%q[0x1]", "%h[0x1]", "1[0x1]"] {
        let module = common::load(&format!(
            "memory 64
func @main() -> i2 {{
entry:
    %q = qalloc 2
    %h = add i64 0, 1
    x {place}
    %m = measure %q
    ret %m
}}"
        ));
        assert_eq!(run(&module, "main", &[], bounded()).0, Outcome::Returned(Some(2)), "{place}");
    }
}

/// `qwidth` gives the width the allocation was given, for a `qN` register and an `i64` handle alike, and
/// does nothing to the qubits; a handle that names no register is `dangling_register`.
#[test]
fn qwidth_reads_the_width_of_the_allocation() {
    let module = common::load(
        "memory 256
func @main(i64 %w, i1 %late) -> i64 {
entry:
    %q = qalloc 3
    h %q
    %h = qalloc i64 %w
    %a = qwidth %q
    %b = qwidth %h
    %s = shl i64 %a, 32
    %r = or i64 %s, %b
    br %late, freed, done
freed:
    qfree %h
    %c = qwidth %h
    ret %c
done:
    ret %r
}",
    );
    for w in [1, 64, 65, 200] {
        let (outcome, executor) = run(&module, "main", &[w, 0], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(u128::from(3u64 << 32 | w))), "{w}");
        assert_eq!(executor.snapshot().quantum.amplitudes.len(), 2, "{w}: the superposition of %q is kept");
        assert_eq!(run(&module, "main", &[w, 1], bounded()).0, Outcome::Faulted(Fault::DanglingRegister), "{w}");
        assert_eq!(
            assert_reverses(&module, "main", &[w, 0], bounded()),
            Outcome::Returned(Some(u128::from(3u64 << 32 | w)))
        );
    }
}
