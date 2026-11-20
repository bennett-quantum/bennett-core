//! The run-time angle form of a rotation (`docs/SYNTAX.md`): `rx(%a)`, `ry(%a)`, `rz(%a)` with an `i64`
//! value, on register or memory qubits, beside the literal form, which reads as before.

use bennett_ir::{
    Expected, Gate, Inst, LoadError, Place, Problem, Qubit, Rotation, Slot, Type, Width, load, read, write,
};

const TEXT: &str = "memory 64
func @main(i64 %a, i32 %narrow) {
entry:
    %q = qalloc 2
    rx(%a) %q[1]
    ry(%a) %q ctrl %q[1]
    rz(%a) [8:3] ctrl %q, [%a:9]
    rz(0.5) %q
    ret
}
";

#[test]
fn the_run_time_form_reads_writes_and_keeps_the_literal_form() {
    let module = read(TEXT).expect("reads");
    let insts = &module.functions[0].blocks[0].insts;
    let (a, q) = (Slot(0), Slot(2));
    assert_eq!(
        insts[1],
        Inst::RotateBy {
            axis: Rotation::Rx,
            angle: a,
            target: Place::Register(Qubit { register: q, bit: 1 }),
            controls: Vec::new(),
        }
    );
    assert!(
        matches!(&insts[3], Inst::RotateBy { axis: Rotation::Rz, target: Place::Memory { bit: 3, .. }, controls, .. }
        if controls.len() == 2)
    );
    assert!(matches!(insts[4], Inst::Gate { gate: Gate::Rotate(Rotation::Rz, 0.5), .. }));
    let written = write(&module);
    assert!(
        written.contains("    rx(%a) %q[1]\n") && written.contains("    rz(%a) [8:3] ctrl %q, [%a:9]\n"),
        "{written}"
    );
    assert_eq!(read(&written), Ok(module));
    assert!(load(TEXT).is_ok());
}

#[test]
fn the_angle_is_an_i64_and_the_qubits_are_checked_as_for_any_gate() {
    let problems = |text: &str| match load(text) {
        Err(LoadError::Invalid(errors)) => errors.0.into_iter().map(|error| error.problem).collect::<Vec<_>>(),
        other => panic!("expected validation errors, got {other:?}"),
    };
    let narrow = TEXT.replace("rx(%a) %q[1]", "rx(%narrow) %q[1]");
    assert_eq!(
        problems(&narrow),
        [Problem::TypeMismatch {
            operand: "`%narrow`".into(),
            expected: Expected::Type(Type::Int(Width::WORD)),
            found: Type::Int(Width::JUMP_VALUE),
        }]
    );
    let outside = TEXT.replace("rx(%a) %q[1]", "rx(%a) %q[2]");
    assert_eq!(problems(&outside), [Problem::QubitOutOfRange { bit: 2, bits: 2 }]);
    let repeated = TEXT.replace("ry(%a) %q ctrl %q[1]", "ry(%a) %q ctrl %q");
    assert_eq!(problems(&repeated), [Problem::RepeatedQubit("q".into(), 0)]);
    let register = TEXT.replace("rx(%a) %q[1]", "rx(%q) %q[1]");
    assert!(matches!(problems(&register)[..], [Problem::TypeMismatch { .. }]));
}
