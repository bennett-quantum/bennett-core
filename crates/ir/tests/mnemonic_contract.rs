//! Independent text-to-variant expectations: a reader/writer round trip alone hides a shared swap.

use bennett_ir::{
    BinOp, CastOp, Const, FixedGate, Gate, Inst, Operand, Predicate, Qubit, Rotation, Slot, Width, read, validate,
    write,
};

// These spellings come from SYNTAX, independently of the production mnemonic macro and its ALL lists.
const BINARIES: &[(BinOp, &str)] = &[
    (BinOp::Add, "add"),
    (BinOp::Sub, "sub"),
    (BinOp::Mul, "mul"),
    (BinOp::Udiv, "udiv"),
    (BinOp::Sdiv, "sdiv"),
    (BinOp::Urem, "urem"),
    (BinOp::Srem, "srem"),
    (BinOp::And, "and"),
    (BinOp::Or, "or"),
    (BinOp::Xor, "xor"),
    (BinOp::Shl, "shl"),
    (BinOp::Lshr, "lshr"),
    (BinOp::Ashr, "ashr"),
];
const PREDICATES: &[(Predicate, &str)] = &[
    (Predicate::Eq, "eq"),
    (Predicate::Ne, "ne"),
    (Predicate::Ult, "ult"),
    (Predicate::Ule, "ule"),
    (Predicate::Ugt, "ugt"),
    (Predicate::Uge, "uge"),
    (Predicate::Slt, "slt"),
    (Predicate::Sle, "sle"),
    (Predicate::Sgt, "sgt"),
    (Predicate::Sge, "sge"),
];
const CASTS: &[(CastOp, &str)] = &[(CastOp::Zext, "zext"), (CastOp::Sext, "sext"), (CastOp::Trunc, "trunc")];
const FIXED: &[(FixedGate, &str)] = &[
    (FixedGate::X, "x"),
    (FixedGate::H, "h"),
    (FixedGate::Z, "z"),
    (FixedGate::S, "s"),
    (FixedGate::Sdg, "sdg"),
    (FixedGate::T, "t"),
    (FixedGate::Tdg, "tdg"),
];
const ROTATIONS: &[(Rotation, &str)] = &[(Rotation::Rx, "rx"), (Rotation::Ry, "ry"), (Rotation::Rz, "rz")];

fn constant(bits: u32, value: u128) -> Operand {
    Operand::Const(Const::new(Width::new(bits).expect("width"), value).expect("constant fits"))
}

fn text(line: &str) -> String {
    format!("func @f(q3 %q) {{\nentry:\n {line}\n ret\n}}\n")
}

fn instruction_contract(line: &str, expected: Inst, has_result: bool) {
    let parsed = read(&text(line)).expect("documented instruction reads");
    assert_eq!(parsed.functions[0].blocks[0].insts.as_slice(), std::slice::from_ref(&expected), "reader: {line}");
    validate(parsed).expect("documented instruction validates");

    // Build the writer's input without reading the instruction under test, so the two cannot
    // compensate for each other's mistakes. Slot 0 is the skeleton's register parameter.
    let mut direct = read("func @f(q3 %q) {\nentry:\n ret\n}\n").expect("skeleton");
    if has_result {
        direct.functions[0].slot_names.push("r".into());
    }
    direct.functions[0].blocks[0].insts.push(expected);
    validate(direct.clone()).expect("independent instruction validates");
    let written = write(&direct);
    assert!(written.lines().any(|actual| actual.trim() == line), "writer: {line}\n{written}");
}

macro_rules! vocabulary {
    ($ty:ident, $table:ident) => {
        assert_eq!($ty::ALL, $table.iter().map(|&(variant, _)| variant).collect::<Vec<_>>());
        for &(variant, word) in $table {
            assert_eq!(variant.mnemonic(), word, "writer spelling for {variant:?}");
            assert_eq!($ty::from_mnemonic(word), Some(variant), "reader meaning of {word}");
            assert_eq!($ty::from_mnemonic(&word.to_ascii_uppercase()), None);
            assert_eq!($ty::from_mnemonic(&format!("{word}_")), None);
        }
        assert_eq!($ty::from_mnemonic(""), None);
        assert_eq!($ty::from_mnemonic("unknown"), None);
    };
}

#[test]
fn binary_words_name_the_documented_operations() {
    vocabulary!(BinOp, BINARIES);
    for &(op, word) in BINARIES {
        instruction_contract(
            &format!("%r = {word} i8 173, 19"),
            Inst::Binary { op, width: Width::BYTE, dst: Slot(1), lhs: constant(8, 173), rhs: constant(8, 19) },
            true,
        );
    }
}

#[test]
fn predicate_words_name_the_documented_comparisons() {
    vocabulary!(Predicate, PREDICATES);
    for &(pred, word) in PREDICATES {
        instruction_contract(
            &format!("%r = icmp {word} i8 173, 19"),
            Inst::Compare { pred, width: Width::BYTE, dst: Slot(1), lhs: constant(8, 173), rhs: constant(8, 19) },
            true,
        );
    }
}

#[test]
fn cast_words_name_the_documented_width_changes() {
    vocabulary!(CastOp, CASTS);
    for &(op, word) in CASTS {
        let (from, to) = if op == CastOp::Trunc { (16, 8) } else { (8, 16) };
        instruction_contract(
            &format!("%r = {word} i{from} 173 to i{to}"),
            Inst::Cast {
                op,
                from: Width::new(from).expect("source width"),
                to: Width::new(to).expect("destination width"),
                dst: Slot(1),
                value: constant(from, 173),
            },
            true,
        );
    }
}

fn gate_contract(line: &str, gate: Gate) {
    instruction_contract(
        line,
        Inst::Gate {
            gate,
            target: Qubit { register: Slot(0), bit: 1 },
            controls: vec![Qubit { register: Slot(0), bit: 0 }, Qubit { register: Slot(0), bit: 2 }],
        },
        false,
    );
}

#[test]
fn fixed_gate_words_name_the_documented_gates() {
    vocabulary!(FixedGate, FIXED);
    for &(gate, word) in FIXED {
        gate_contract(&format!("{word} %q[1] ctrl %q, %q[2]"), Gate::Fixed(gate));
    }
}

#[test]
fn rotation_words_name_the_documented_axes_and_preserve_angles() {
    vocabulary!(Rotation, ROTATIONS);
    for &(axis, word) in ROTATIONS {
        gate_contract(&format!("{word}(0.625) %q[1] ctrl %q, %q[2]"), Gate::Rotate(axis, 0.625));
    }
}

#[test]
fn macro_named_instructions_require_the_documented_operands() {
    for &(_, word) in BINARIES {
        for line in [format!("%r = {word} i8 173"), format!("%r = {word} i8 173, 19, 7")] {
            assert!(read(&text(&line)).is_err(), "accepted wrong binary arity: {line}");
        }
    }
    for &(_, word) in PREDICATES {
        assert!(read(&text(&format!("%r = icmp {word} i8 173"))).is_err());
    }
    for &(_, word) in CASTS {
        assert!(read(&text(&format!("%r = {word} i8 173"))).is_err());
    }
    for &(_, word) in FIXED {
        assert!(read(&text(word)).is_err(), "a gate needs its target");
    }
    for &(_, word) in ROTATIONS {
        assert!(read(&text(&format!("{word} %q"))).is_err(), "a rotation needs its angle");
        assert!(read(&text(&format!("{word}(0.625)"))).is_err(), "a rotation needs its target");
    }
}
