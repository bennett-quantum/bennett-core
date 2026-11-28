//! Gate kinds at run time in the text form (`docs/SYNTAX.md`, "Gate kinds at run time"): `gate(CODE)` and
//! `rotate(CODE, %angle)` read, write and validate; the code is an `i64` value or constant, the angle an
//! `i64` slot, the qubits as any gate's.

use bennett_ir::{GateKind, Inst, LoadError, Operand, Place, Problem, Qubit, Slot, load, read, write};

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
            if *code == k && *register == h && *index == Operand::Slot(i) && controls.len() == 2
    ));
    assert!(matches!(
        &insts[2],
        Inst::GateBy { kind: GateKind::Gate(Operand::Const(c)), target: Place::Register(Qubit { register, bit: 1 }), .. }
            if c.value() == 4 && *register == q
    ));
    assert!(matches!(
        &insts[3],
        Inst::GateBy { kind: GateKind::Rotation { code: Operand::Slot(code), angle }, .. } if *code == k && *angle == a
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
    assert!(read(&TEXT.replace("rotate(12, %a)", "rotate(12, 0.5)")).is_err(), "the angle is a slot");
    assert!(read(&TEXT.replace("gate(4)", "%x = gate(4)")).is_err(), "no result");
}
