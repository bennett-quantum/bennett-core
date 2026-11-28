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
            target: Place::Indexed { register: h, index: Operand::Slot(i) },
            controls: vec![
                Place::Indexed { register: q, index: Operand::Slot(i) },
                Place::Register(Qubit { register: k, bit: 2 }),
                Place::Memory { address: Operand::Const(bennett_ir::Const::new(Width::WORD, 8).expect("8")), bit: 1 },
            ],
        }
    );
    // A literal position on an `i64` handle is an ordinary register qubit, `Gate`.
    assert!(matches!(insts[4], Inst::Gate { target: Qubit { register, bit: 5 }, .. } if register == h));
    assert!(matches!(insts[5], Inst::RotateBy { target: Place::Indexed { register, index }, .. }
        if register == k && index == Operand::Slot(i)));
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

/// The same `i64` handle slot twice at one literal position is left to the run: there its handle is checked
/// first (`dangling_register` for a value naming no register) and the repetition after (`qubit_alias`). On a
/// `qN` register the validator still reports it.
#[test]
fn a_repeated_qubit_of_an_i64_handle_is_checked_when_the_gate_runs() {
    let text = |register: &str| {
        format!("func @main(i64 %h) {{\nentry:\n %q = qalloc 2\n x {register}[0] ctrl {register}[0]\n ret\n}}\n")
    };
    load(&text("%h")).expect("a repeated i64 handle validates");
    assert!(matches!(problems(&text("%q"))[..], [Problem::RepeatedQubit(..)]));
}

/// A literal position from 2^32 reads as a position by an `i64` constant: on an `i64` handle it is checked
/// when the gate runs (`bad_range`), on a `qN` register the validator reports it. Written and read back, it
/// is the same instruction; a position below 2^32 stays a register qubit.
#[test]
fn a_literal_position_from_two_to_the_32_reads_and_is_checked_by_range() {
    let text = |position: &str| {
        format!("func @main(i64 %h) {{\nentry:\n %q = qalloc 2\n x %h[{position}] ctrl %q\n ret\n}}\n")
    };
    for position in ["4294967296", "18446744073709551615"] {
        let module = read(&text(position)).expect("reads");
        let Inst::GateAt { target: Place::Indexed { index: Operand::Const(c), .. }, .. } =
            module.functions[0].blocks[0].insts[1]
        else {
            panic!("{position}: {:?}", module.functions[0].blocks[0].insts[1]);
        };
        assert_eq!(c.value().to_string(), position);
        assert_eq!(read(&write(&module)).expect("reads back"), module, "{position}");
        load(&text(position)).expect("validates on an i64 handle");
        let on_register = text(position).replace("x %h[", "x %q[").replace("ctrl %q", "ctrl %q[1]");
        assert_eq!(problems(&on_register), [Problem::QubitOutOfRange { bit: position.parse().expect("u64"), bits: 2 }]);
    }
    let below = read(&text("4294967295")).expect("reads");
    assert!(matches!(below.functions[0].blocks[0].insts[1], Inst::Gate { .. }));
    assert!(read(&text("18446744073709551616")).is_err(), "past 2^64 - 1");
}

/// A module built through the API with a position as a constant: the validator accepts it only in the form
/// the reader gives (a constant from 2^32, an `i64`), so a module that validates is read back from its text
/// as itself and validates again. A constant below 2^32 is refused by name (it is a register qubit), and a
/// constant that is not an `i64` gives the type error alone.
#[test]
fn a_built_constant_position_validates_only_in_the_form_the_reader_gives() {
    use bennett_ir::{Const, ValidationErrors, validate};
    let base = |register: &str| {
        let text = format!(
            "func @main(i64 %h, i64 %i) {{\nentry:\n %q = qalloc 2\n x {register}[%i] ctrl %q[1]\n x {register}[%i]\n ret\n}}\n"
        );
        read(&text).expect("reads")
    };
    let problems_of = |module: bennett_ir::Module| match validate(module) {
        Ok(_) => Vec::new(),
        Err(ValidationErrors(errors)) => errors.into_iter().map(|error| error.problem).collect::<Vec<_>>(),
    };
    for register in ["%q", "%h"] {
        for position in [0u128, 1, u128::from(u32::MAX), 1 << 32, u128::from(u64::MAX)] {
            for inst in [1, 2] {
                let mut module = base(register);
                let Inst::GateAt { target: Place::Indexed { index, .. }, .. } =
                    &mut module.functions[0].blocks[0].insts[inst]
                else {
                    panic!("an indexed gate")
                };
                *index = Operand::Const(Const::new(Width::WORD, position).expect("a 64-bit position"));
                let context = format!("{register}[{position}], instruction {inst}");
                match validate(module.clone()) {
                    Ok(_) => {
                        let back = read(&write(&module)).expect("reads back");
                        assert_eq!(back, module, "{context}");
                        assert!(validate(back).is_ok(), "{context}");
                        assert!(position >> 32 != 0 && register == "%h", "{context}: accepted");
                    }
                    Err(ValidationErrors(errors)) => {
                        let found: Vec<Problem> = errors.into_iter().map(|error| error.problem).collect();
                        let expected = if position >> 32 == 0 {
                            Problem::SmallConstantPosition(position as u32)
                        } else {
                            Problem::QubitOutOfRange { bit: position as u64, bits: 2 }
                        };
                        assert_eq!(found, [expected], "{context}");
                    }
                }
            }
        }
    }
    let mut wide = base("%h");
    let Inst::GateAt { target: Place::Indexed { index, .. }, .. } = &mut wide.functions[0].blocks[0].insts[2] else {
        panic!("an indexed gate")
    };
    *index = Operand::Const(Const::new(Width::new(128).expect("i128"), 1 << 64).expect("2^64"));
    assert!(matches!(problems_of(wide)[..], [Problem::TypeMismatch { .. }]), "the type error alone");
}
