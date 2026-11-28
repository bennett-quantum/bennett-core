//! Gate kinds at run time in the text form (`docs/SYNTAX.md`, "Gate kinds at run time"): `gate(CODE)` and
//! `rotate(CODE, ANGLE)` read, write and validate; the code is an `i64` value or constant, the angle an `i64`
//! value or an integer constant holding its binary64 bits, the qubits as any gate's.

use bennett_ir::{Controls, GateKind, Inst, LoadError, Operand, Place, Problem, Qubit, Slot, load, read, write};

const TEXT: &str = "memory 64
func @main(i64 %k, i64 %a, i64 %h, i64 %i) {
entry:
    %q = qalloc 2
    gate(%k) %h[%i] ctrl %q, [8:3]
    gate(4) %q[1]
    rotate(%k, %a) %q ctrl %h[1]
    rotate(12, %a) %h[%i]
    ret
}
";

fn problems(text: &str) -> Vec<Problem> {
    match load(text) {
        Err(LoadError::Invalid(errors)) => errors.0.into_iter().map(|error| error.problem).collect(),
        other => panic!("expected validation errors, got {other:?}"),
    }
}

#[test]
fn the_run_time_kinds_read_write_and_validate() {
    let module = read(TEXT).expect("reads");
    let insts = &module.functions[0].blocks[0].insts;
    let (k, a, h, i, q) = (Slot(0), Slot(1), Slot(2), Slot(3), Slot(4));
    assert!(matches!(
        &insts[1],
        Inst::GateBy { kind: GateKind::Gate(Operand::Slot(code)), target: Place::Indexed { register, index }, controls }
            if *code == k && *register == Operand::Slot(h) && *index == Operand::Slot(i) && matches!(controls, Controls::Written(c) if c.len() == 2)
    ));
    assert!(matches!(
        &insts[2],
        Inst::GateBy { kind: GateKind::Gate(Operand::Const(c)), target: Place::Register(Qubit { register, bit: 1 }), .. }
            if c.value() == 4 && *register == q
    ));
    assert!(matches!(
        &insts[3],
        Inst::GateBy { kind: GateKind::Rotation { code: Operand::Slot(code), angle }, .. }
            if *code == k && *angle == Operand::Slot(a)
    ));
    assert!(matches!(
        &insts[4],
        Inst::GateBy { kind: GateKind::Rotation { code: Operand::Const(c), .. }, .. } if c.value() == 12
    ));
    let written = write(&module);
    assert!(
        written.contains("gate(%k) %h[%i] ctrl %q, [8:3]") && written.contains("rotate(12, %a) %h[%i]"),
        "{written}"
    );
    assert_eq!(read(&written).expect("reads back"), module);
    load(TEXT).expect("validates");
}

#[test]
fn the_code_and_the_angle_are_i64_and_the_qubits_are_checked_as_for_any_gate() {
    let narrow = TEXT.replace("(i64 %k,", "(i32 %k,");
    assert!(problems(&narrow).iter().all(|problem| matches!(problem, Problem::TypeMismatch { .. })));
    assert_eq!(problems(&narrow).len(), 2, "both uses of the code");
    let angle = TEXT.replace("i64 %a,", "i8 %a,");
    assert!(matches!(problems(&angle)[..], [Problem::TypeMismatch { .. }, Problem::TypeMismatch { .. }]));
    let outside = TEXT.replace("gate(4) %q[1]", "gate(4) %q[2]");
    assert_eq!(problems(&outside), [Problem::QubitOutOfRange { bit: 2, bits: 2 }]);
    let repeated = TEXT.replace("gate(4) %q[1]", "gate(4) %q[1] ctrl %q[1]");
    assert!(matches!(problems(&repeated)[..], [Problem::RepeatedQubit(..)]));
    assert!(read(&TEXT.replace("rotate(12, %a)", "rotate(12)")).is_err(), "a rotation needs its angle");
    assert!(
        read(&TEXT.replace("rotate(12, %a)", "rotate(12, 0.5)")).is_err(),
        "the angle is an integer, its bits, not a decimal"
    );
    assert!(read(&TEXT.replace("gate(4)", "%x = gate(4)")).is_err(), "no result");
}

/// A constant handle (`1[0]`, `2[%i]`), a constant angle's bits and `qfree` of a constant handle read,
/// write, read back and validate: every operand of these forms is a constant or a slot.
#[test]
fn constant_handles_and_angles_read_write_and_validate() {
    let text = "memory 64
func @main(i64 %i) {
entry:
    %q = qalloc 2
    gate(4) 1[0] ctrl 2[%i]
    rotate(9, 4602678819172646912) 1[%i]
    x 1[3] ctrl 1[4294967296]
    qfree 1
    ret
}
";
    let module = read(text).expect("reads");
    let insts = &module.functions[0].blocks[0].insts;
    let constant = |value: u128| Operand::Const(bennett_ir::Const::new(bennett_ir::Width::WORD, value).expect("i64"));
    assert!(matches!(
        &insts[1],
        Inst::GateBy { target: Place::Indexed { register, index }, .. }
            if *register == constant(1) && *index == constant(0)
    ));
    assert!(matches!(
        &insts[2],
        Inst::GateBy { kind: GateKind::Rotation { angle, .. }, .. } if *angle == constant(0.5f64.to_bits().into())
    ));
    assert!(
        matches!(&insts[3], Inst::GateAt { target: Place::Indexed { register, .. }, .. } if *register == constant(1))
    );
    assert_eq!(insts[4], Inst::Qfree { register: constant(1) });
    let written = write(&module);
    for line in [
        "gate(4) 1[0] ctrl 2[%i]\n",
        "rotate(9, 4602678819172646912) 1[%i]\n",
        "x 1[3] ctrl 1[4294967296]\n",
        "qfree 1\n",
    ] {
        assert!(written.contains(line), "{line}{written}");
    }
    assert_eq!(read(&written).expect("reads back"), module);
    load(text).expect("validates");
    assert!(read(&text.replace("qfree 1", "qfree 1[0]")).is_err(), "qfree takes a handle, not a qubit");
}

/// `ctrl list(ADDRESS, COUNT)`: the controls of `gate` and `rotate` in memory, both operands `i64` values or
/// constants; it reads, writes and reads back, and is the only control of its instruction.
#[test]
fn a_control_list_reads_writes_and_takes_i64_operands() {
    let text = "memory 64
func @main(i64 %k, i64 %a, i64 %h, i64 %p, i64 %n, i32 %narrow) {
entry:
    %q = qalloc 2
    gate(%k) %h[%p] ctrl list(%p, %n)
    rotate(12, %a) %q ctrl list(64, 0)
    ret
}
";
    let module = read(text).expect("reads");
    let insts = &module.functions[0].blocks[0].insts;
    let (p, n) = (Slot(3), Slot(4));
    assert!(matches!(
        &insts[1],
        Inst::GateBy { controls: Controls::List { address: Operand::Slot(a), count: Operand::Slot(c) }, .. }
            if *a == p && *c == n
    ));
    assert!(matches!(
        &insts[2],
        Inst::GateBy { controls: Controls::List { address: Operand::Const(a), count: Operand::Const(c) }, .. }
            if a.value() == 64 && c.value() == 0
    ));
    let written = write(&module);
    assert!(written.contains("gate(%k) %h[%p] ctrl list(%p, %n)\n"), "{written}");
    assert!(written.contains("rotate(12, %a) %q ctrl list(64, 0)\n"), "{written}");
    assert_eq!(read(&written).expect("reads back"), module);
    load(text).expect("validates");
    for narrow in ["ctrl list(%narrow, %n)", "ctrl list(%p, %narrow)"] {
        let bad = text.replace("ctrl list(%p, %n)", narrow);
        assert!(matches!(problems(&bad)[..], [Problem::TypeMismatch { .. }]), "{narrow}");
    }
    assert!(read(&text.replace("ctrl list(64, 0)", "ctrl list(64, 0), %q[1]")).is_err(), "a list and written controls");
    assert!(
        read(&text.replace("rotate(12, %a) %q ctrl list(64, 0)", "x %q ctrl list(64, 0)")).is_err(),
        "a literal gate"
    );
}
