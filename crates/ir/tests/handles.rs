//! Handles as integers in the text form (`docs/SYNTAX.md`, "Handles as integers"): `qalloc i64 WIDTH`, an
//! `i64` handle wherever a gate, a control or `qfree` names a register, and run-time positions `%r[%i]`,
//! beside the literal forms, which read as before.

use bennett_ir::{
    Expected, Inst, LoadError, Operand, Place, Problem, Qubit, Qubits, Slot, Type, Width, load, read, write,
};

const TEXT: &str = "memory 64
func @main(i64 %w, i64 %i, i32 %narrow) {
entry:
    %q = qalloc 2
    %h = qalloc i64 %w
    %k = qalloc i64 3
    x %h[%i] ctrl %q[%i], %k[2], [8:1]
    h %h[5] ctrl %q
    rz(%w) %k[%i]
    qfree %h
    qfree %q
    ret
}
";

#[test]
fn the_handle_forms_read_write_and_keep_the_literal_forms() {
    let module = read(TEXT).expect("reads");
    let insts = &module.functions[0].blocks[0].insts;
    let (w, i, q, h, k) = (Slot(0), Slot(1), Slot(3), Slot(4), Slot(5));
    assert_eq!(insts[0], Inst::Qalloc { dst: q, width: Qubits::new(2).expect("a width") });
    assert_eq!(insts[1], Inst::QallocBy { dst: h, width: Operand::Slot(w) });
    assert!(matches!(insts[2], Inst::QallocBy { dst, width: Operand::Const(c) } if dst == k && c.value() == 3));
    assert_eq!(
        insts[3],
        Inst::GateAt {
            gate: bennett_ir::Gate::Fixed(bennett_ir::FixedGate::X),
            target: Place::Indexed { register: h, index: i },
            controls: vec![
                Place::Indexed { register: q, index: i },
                Place::Register(Qubit { register: k, bit: 2 }),
                Place::Memory { address: Operand::Const(bennett_ir::Const::new(Width::WORD, 8).expect("8")), bit: 1 },
            ],
        }
    );
    // A literal position on an `i64` handle is an ordinary register qubit, `Gate`.
    assert!(matches!(insts[4], Inst::Gate { target: Qubit { register, bit: 5 }, .. } if register == h));
    assert!(matches!(insts[5], Inst::RotateBy { target: Place::Indexed { register, index }, .. }
        if register == k && index == i));
    let written = write(&module);
    for line in ["    %h = qalloc i64 %w\n", "    %k = qalloc i64 3\n", "    x %h[%i] ctrl %q[%i], %k[2], [8:1]\n"] {
        assert!(written.contains(line), "{line}{written}");
    }
    assert_eq!(read(&written), Ok(module));
    let loaded = load(TEXT).expect("valid");
    assert_eq!(loaded.slot_type(loaded.function_named("main").expect("main"), h), Type::Int(Width::WORD));
}

fn problems(text: &str) -> Vec<Problem> {
    match load(text) {
        Err(LoadError::Invalid(errors)) => errors.0.into_iter().map(|error| error.problem).collect(),
        other => panic!("expected validation errors, got {other:?}"),
    }
}

#[test]
fn a_handle_is_an_i64_and_a_position_an_i64_slot() {
    let not_a_handle = TEXT.replace("qfree %h", "qfree %narrow");
    assert_eq!(
        problems(&not_a_handle),
        [Problem::TypeMismatch {
            operand: "`%narrow`".into(),
            expected: Expected::RegisterOrHandle,
            found: Type::Int(Width::JUMP_VALUE),
        }]
    );
    let narrow_width = TEXT.replace("qalloc i64 %w", "qalloc i64 %narrow");
    assert!(matches!(problems(&narrow_width)[..], [Problem::TypeMismatch { .. }]));
    let narrow_index = TEXT.replace("rz(%w) %k[%i]", "rz(%w) %k[%narrow]");
    assert!(matches!(problems(&narrow_index)[..], [Problem::TypeMismatch { .. }]));
    let register_index = TEXT.replace("rz(%w) %k[%i]", "rz(%w) %k[%q]");
    assert!(matches!(problems(&register_index)[..], [Problem::TypeMismatch { .. }]));
    // A `qN` register is not an integer: it is no width, and an `i64` handle is not measured whole.
    let register_width = TEXT.replace("qalloc i64 %w", "qalloc i64 %q");
    assert!(matches!(problems(&register_width)[..], [Problem::TypeMismatch { .. }]));
    let measured = TEXT.replace("qfree %h", "%m = measure %h");
    assert!(matches!(problems(&measured)[..], [Problem::TypeMismatch { expected: Expected::AnyRegister, .. }]));
    // A literal position on a `qN` register is still checked by the validator; on an `i64` handle it cannot be.
    let outside = TEXT.replace("h %h[5] ctrl %q", "h %h[5] ctrl %q[2]");
    assert_eq!(problems(&outside), [Problem::QubitOutOfRange { bit: 2, bits: 2 }]);
}
