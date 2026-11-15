//! The reader, writer and validator on the examples and on deliberately broken programs.

use bennett_ir::{DataItem, LoadError, NameKind, Problem, ReadErrorKind, load, read, validate, write};

const EXAMPLES: [(&str, &str); 6] = [
    ("sum", include_str!("../../../examples/sum.bn")),
    ("calls", include_str!("../../../examples/calls.bn")),
    ("thesis", include_str!("../../../examples/thesis.bn")),
    ("fault", include_str!("../../../examples/fault.bn")),
    ("grover", include_str!("../../../examples/grover.bn")),
    ("history", include_str!("../../../examples/history.bn")),
];

#[test]
fn examples_are_valid_and_survive_a_round_trip_through_the_writer() {
    for (name, text) in EXAMPLES {
        let module = read(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let written = write(&module);
        assert_eq!(read(&written).as_ref(), Ok(&module), "{name}:\n{written}");
        validate(module).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

fn read_error(text: &str) -> (usize, usize, ReadErrorKind) {
    match read(text) {
        Ok(_) => panic!("expected a read error:\n{text}"),
        Err(error) => (error.line, error.column, error.kind),
    }
}

fn problems(text: &str) -> Vec<Problem> {
    match load(text) {
        Err(LoadError::Invalid(errors)) => errors.0.into_iter().map(|e| e.problem).collect(),
        other => panic!("expected validation errors, got {other:?}"),
    }
}

#[test]
fn undefined_names_are_reported_at_their_first_use() {
    let text = "func @f() -> i8 {\nentry:\n    %x = add i8 %y, 1\n    ret %x\n}\n";
    let undefined = ReadErrorKind::Undefined { kind: NameKind::Value, name: "y".into() };
    assert_eq!(read_error(text), (3, 17, undefined));
    let text = "func @f() {\nentry:\n    call @g()\n    ret\n}\n";
    let undefined = ReadErrorKind::Undefined { kind: NameKind::Function, name: "g".into() };
    assert_eq!(read_error(text), (3, 10, undefined));
    let text = "func @f() {\nentry:\n    br nowhere\n}\n";
    assert!(matches!(read_error(text).2, ReadErrorKind::Undefined { kind: NameKind::Block, .. }));
}

#[test]
fn malformed_statements_are_named() {
    let body = |line: &str| format!("func @f() -> i8 {{\nentry:\n    {line}\n    ret 0\n}}\n");
    assert!(matches!(read_error(&body("%x = frob i8 1, 2")).2, ReadErrorKind::UnknownInstruction(_)));
    assert!(matches!(read_error(&body("%x = add i8 300, 1")).2, ReadErrorKind::OutOfRange { .. }));
    assert!(matches!(read_error(&body("%x = add i129 1, 1")).2, ReadErrorKind::BadType(_)));
    assert!(matches!(read_error(&body("%x = qalloc 65")).2, ReadErrorKind::BadNumber(_)));
    assert!(matches!(read_error(&body("add i8 1, 1")).2, ReadErrorKind::ResultRequired("add")));
    assert!(matches!(read_error(&body("%x = store i8 1, 0")).2, ReadErrorKind::ResultNotAllowed("store")));
    assert!(matches!(read_error(&body("%x = add i8 1, 1 1")).2, ReadErrorKind::Unexpected { .. }));
    let missing = "func @f() {\nentry:\n    %x = add i8 1, 1\nnext:\n    ret\n}\n";
    assert_eq!(read_error(missing).2, ReadErrorKind::MissingTerminator("entry".into()));
    let twice = "func @f() {\nentry:\n    %x = add i8 1, 1\n    %x = add i8 1, 1\n    ret\n}\n";
    assert!(matches!(read_error(twice).2, ReadErrorKind::Redefined { .. }));
    assert_eq!(
        read_error("ret\n").2,
        ReadErrorKind::Unexpected { expected: "`memory`, `data` or `func`", found: "`ret`".into() }
    );
}

#[test]
fn operand_types_are_checked() {
    let text = "func @f(i8 %a, i16 %b) -> i8 {\nentry:\n    %x = add i8 %a, %b\n    ret %x\n}\n";
    assert!(matches!(problems(text).as_slice(), [Problem::TypeMismatch { .. }]));
    let text = "func @f() -> i8 {\nentry:\n    %x = zext i8 1 to i8\n    ret %x\n}\n";
    assert!(matches!(problems(text).as_slice(), [Problem::CastWidths { .. }]));
    let text = "func @f() {\nentry:\n    %q = qalloc 2\n    h %q[2]\n    ret\n}\n";
    assert!(matches!(problems(text).as_slice(), [Problem::QubitOutOfRange { bit: 2, bits: 2 }]));
    let text = "func @f() {\nentry:\n    %q = qalloc 2\n    x %q[1] ctrl %q[1]\n    ret\n}\n";
    assert!(matches!(problems(text).as_slice(), [Problem::RepeatedQubit(..)]));
    let text = "func @f() -> i8 {\nentry:\n    ret\n}\n";
    assert!(matches!(problems(text).as_slice(), [Problem::ReturnMismatch]));
}

#[test]
fn uses_must_be_dominated_by_their_definitions() {
    let text =
        "func @f(i1 %c) -> i8 {\nentry:\n    br %c, a, b\na:\n    %x = add i8 1, 1\n    br b\nb:\n    ret %x\n}\n";
    assert_eq!(problems(text), [Problem::NotDominated("x".into())]);
}

#[test]
fn apply_needs_an_integer_function_and_distinct_registers_of_the_right_widths() {
    let region = "func @g(i2 %x) -> i2 {\nentry:\n    ret %x\n}\n";
    let caller = |apply: &str, fault_bits: u32| {
        format!(
            "{region}func @f() {{\nentry:\n    %x = qalloc 2\n    %y = qalloc 2\n    %e = qalloc {fault_bits}\n    \
             {apply}\n    ret\n}}\n"
        )
    };
    assert!(load(&caller("apply @g(%x) -> %y, %e budget 10", 4)).is_ok());
    assert!(matches!(problems(&caller("apply @g(%x) -> %x, %e budget 10", 4)).as_slice(), [Problem::RegionAlias(_)]));
    assert!(matches!(
        problems(&caller("apply @g(%x) -> %y, %e budget 10", 3)).as_slice(),
        [Problem::TypeMismatch { .. }]
    ));
    assert!(matches!(problems(&caller("apply @g(%x) -> %e, %y budget 10", 4)).as_slice(), [_, _]));
}

#[test]
fn data_must_fit_memory_without_overlapping() {
    let problems = problems("memory 8\ndata 0 = i64 1\ndata 4 = i32 2\ndata 6 = \"abc\"\n");
    assert!(problems.contains(&Problem::DataOverlap(0)));
    assert!(problems.iter().any(|p| matches!(p, Problem::DataOutOfBounds { address: 6, len: 3, .. })));
}

#[test]
fn exit_is_a_typed_integer_terminator() {
    for statement in ["exit", "exit 7", "exit i8 256", "%x = exit i8 1", "exit q1 0", "exit i8 1, 2"] {
        let text = format!("func @f() {{\nentry:\n {statement}\n}}\n");
        assert!(read(&text).is_err(), "accepted {statement}");
    }
    let text = "func @f() {\nentry:\n exit i8 1\n ret\n}\n";
    assert!(read(text).is_err(), "exit must end its block");
    let text = "func @f() {\nentry:\n %q = qalloc 1\n exit %q\n}\n";
    assert_eq!(problems(text), vec![Problem::ExitOperand]);
    let text = "func @f() {\nentry:\n exit %missing\n}\n";
    assert!(matches!(read_error(text).2, ReadErrorKind::Undefined { kind: NameKind::Value, .. }));
    for text in [
        include_str!("../../../examples/exit.bn"),
        "func @f() {\nentry:\n exit i128 0xffffffffffffffffffffffffffffffff\n}\n",
        // Negative constants at several widths, a narrow slot, and exits on block parameters and
        // down switch arms: each writes back as the same module.
        "func @f() {\nentry:\n exit i8 -1\n}\n",
        "func @f() {\nentry:\n exit i8 -128\n}\n",
        "func @f() {\nentry:\n exit i1 -1\n}\n",
        "func @f() {\nentry:\n exit i128 -1\n}\n",
        "func @f(i7 %v) {\nentry:\n exit %v\n}\n",
        "func @f(i1 %c) {\nentry:\n br %c, a(i32 4), a(i32 9)\na(i32 %v):\n exit %v\n}\n",
        "func @f(i8 %c) {\nentry:\n switch i8 %c, 1: a, default: b\na:\n exit i32 1\nb:\n ret\n}\n",
    ] {
        let module = read(text).expect("reads");
        assert_eq!(read(&write(&module)).expect("rereads"), module);
        validate(module).expect("valid");
    }
    for statement in ["exit i8 -129", "exit i0 0", "exit i129 0"] {
        let text = format!("func @f() {{\nentry:\n {statement}\n}}\n");
        assert!(read(&text).is_err(), "accepted {statement}");
    }
    // A register reaching the exit through a block parameter is still the one problem.
    let text = "func @f(i1 %c) {\nentry:\n %q = qalloc 2\n br %c, a(%q), a(%q)\na(q2 %v):\n exit %v\n}\n";
    assert_eq!(problems(text), vec![Problem::ExitOperand]);
}

#[test]
fn effect_takes_exactly_one_byte() {
    assert!(read("func @f() {\nentry:\n effect 256\n ret\n}\n").is_err(), "a wide constant is not truncated");
    assert!(read("func @f() {\nentry:\n effect -129\n ret\n}\n").is_err());
    assert!(read("func @f() {\nentry:\n effect -1\n ret\n}\n").is_ok());
    let text = "func @f(i32 %v) {\nentry:\n effect %v\n ret\n}\n";
    assert!(matches!(problems(text).as_slice(), [Problem::TypeMismatch { .. }]), "a wide slot is not truncated");
    let text = "func @f() {\nentry:\n %q = qalloc 8\n effect %q\n ret\n}\n";
    assert!(!problems(text).is_empty(), "a register is not a byte");
}

#[test]
fn zero_items_read_write_and_count_towards_static_data() {
    let text = "memory 4096\ndata 16 = \"ab\", zero 0, zero 3, i8 7, zero 0x10\ndata 64 = zero 4000\n";
    let module = read(text).expect("reads");
    let items = &module.data[0].items;
    assert_eq!(items[1], DataItem::Zero(0));
    assert_eq!(items[2], DataItem::Zero(3));
    assert_eq!(items[4], DataItem::Zero(16));
    assert_eq!(module.data[0].len(), 2 + 3 + 1 + 16);
    assert_eq!(module.data[0].encode(), [b'a', b'b', 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(module.static_end(), 4064, "a zero extent is static data");
    let written = write(&module);
    assert_eq!(written, "memory 4096\ndata 16 = \"ab\", zero 0, zero 3, i8 7, zero 16\ndata 64 = zero 4000\n");
    assert_eq!(read(&written).as_ref(), Ok(&module));
    assert_eq!(write(&read(&written).expect("rereads")), written);
    validate(module).expect("valid");
}

#[test]
fn a_zero_extent_and_its_bytes_spelled_out_are_the_same_memory() {
    let zero = read("memory 64\ndata 16 = i8 1, zero 30, i8 2\n").expect("reads");
    let spelled = read(&format!("memory 64\ndata 16 = i8 1, \"{}\", i8 2\n", "\\0".repeat(30))).expect("reads");
    assert_eq!(zero.static_end(), 48);
    assert_eq!(zero.static_end(), spelled.static_end());
    assert_eq!(zero.data[0].encode(), spelled.data[0].encode());
}

#[test]
fn malformed_zero_items_are_named() {
    let data = |items: &str| read_error(&format!("memory 64\ndata 16 = {items}\n")).2;
    for count in ["-1", "-0", "18446744073709551616", "0x10000000000000000", "1.5", "1e3"] {
        assert_eq!(data(&format!("zero {count}")), ReadErrorKind::BadNumber(count.into()), "zero {count}");
    }
    assert!(matches!(data("zero"), ReadErrorKind::Unexpected { .. }));
    assert!(matches!(data("zero i8"), ReadErrorKind::Unexpected { .. }));
    assert!(matches!(data("zero 1 2"), ReadErrorKind::Unexpected { .. }));
    assert!(matches!(data("i8 zero"), ReadErrorKind::Unexpected { .. }));
    assert!(matches!(data("zeros 1"), ReadErrorKind::BadType(_)));
    assert!(matches!(data("zero16"), ReadErrorKind::BadType(_)));
    for count in ["0x", "0X10"] {
        assert!(matches!(data(&format!("zero {count}")), ReadErrorKind::Unexpected { .. }), "the number 0, a word");
    }
    assert!(matches!(read_error("memory 64\ndata 16 = zero\n1\n").2, ReadErrorKind::Unexpected { .. }));
    assert!(matches!(data("ZERO 1"), ReadErrorKind::BadType(_)));
    assert!(read("memory 64\ndata 16 = zero 18446744073709551615\n").is_ok(), "the largest count reads");
    let module = read("memory 64\ndata 16 = zero 0xFf, zero 007, zero 0x0\n").expect("hex and leading zeros");
    assert_eq!(module.data[0].items, [DataItem::Zero(255), DataItem::Zero(7), DataItem::Zero(0)]);
}

#[test]
fn zero_extents_must_fit_memory_without_overlapping() {
    assert!(load("memory 64\ndata 0 = zero 64\n").is_ok());
    assert!(load("memory 0\ndata 0 = zero 0\n").is_ok());
    assert_eq!(
        problems("memory 64\ndata 1 = zero 64\n"),
        [Problem::DataOutOfBounds { address: 1, len: 64, memory_size: 64 }]
    );
    let max = u64::MAX;
    assert_eq!(
        problems(&format!("memory {max}\ndata 1 = zero {max}\n")),
        [Problem::DataOutOfBounds { address: 1, len: max, memory_size: max }],
        "the end address overflows"
    );
    assert_eq!(problems(&format!("memory {max}\ndata 0 = zero {max}, zero 1\n")), [Problem::DataTooLong]);
    assert_eq!(problems(&format!("memory {max}\ndata 0 = \"a\", zero {max}\n")), [Problem::DataTooLong]);
    assert_eq!(problems("memory 64\ndata 0 = zero 20\ndata 16 = i8 1\n"), [Problem::DataOverlap(0)]);
    assert!(load("memory 64\ndata 0 = zero 16\ndata 16 = i8 1\n").is_ok(), "adjacent extents do not overlap");
    assert!(load("memory 64\ndata 16 = zero 0\ndata 16 = i8 1\n").is_ok(), "an empty extent overlaps nothing");
}

#[test]
fn mark_and_rewind_read_write_and_validate() {
    let text = "func @f(q8 %r, q2 %s, i8 %v) {\nentry:\n %h = mark\n %g = mark\n rewind %g\n \
                rewind %h keep (%v -> %r, i2 -1 -> %s, i8 0 -> %r)\n ret\n}\n";
    let module = read(text).expect("reads");
    let written = write(&module);
    assert!(written.contains("%h = mark\n"), "{written}");
    assert!(written.contains("rewind %g\n"), "{written}");
    assert!(written.contains("rewind %h keep (%v -> %r, i2 3 -> %s, i8 0 -> %r)\n"), "{written}");
    assert_eq!(read(&written).as_ref(), Ok(&module));
    validate(module).expect("valid");
    for statement in [
        "mark",
        "%h = mark 3",
        "%x = rewind %h",
        "rewind",
        "rewind %h keep",
        "rewind %h keep ()",
        "rewind %h keep (%v %r)",
        "rewind %h keep (%v -> %r",
        "rewind %h keep (3 -> %r)",
        "rewind %h keep (%v -> 3)",
        "rewind %h keep (%v -> %r,)",
    ] {
        let text = format!("func @f(q8 %r, i8 %v) {{\nentry:\n %h = mark\n {statement}\n ret\n}}\n");
        assert!(read(&text).is_err(), "accepted {statement}");
    }
    let body = |statements: &str| format!("func @f(q8 %r, q4 %s, i8 %v, i64 %n) {{\nentry:\n{statements}\n ret\n}}\n");
    assert_eq!(problems(&body(" rewind %n")), vec![Problem::NotAMark("n".into())]);
    assert_eq!(problems(&body(" %h = add i64 %n, 1\n rewind %h")), vec![Problem::NotAMark("h".into())]);
    assert!(matches!(
        problems(&body(" %h = mark\n rewind %h keep (%v -> %s)")).as_slice(),
        [Problem::TypeMismatch { .. }]
    ));
    assert!(matches!(
        problems(&body(" %h = mark\n rewind %h keep (%v -> %v)")).as_slice(),
        [Problem::TypeMismatch { .. }]
    ));
    assert!(matches!(
        problems(&body(" %h = mark\n rewind %h keep (%s -> %s)")).as_slice(),
        [Problem::TypeMismatch { .. }]
    ));
    assert!(matches!(problems(&body(" rewind %h\n %h = mark")).as_slice(), [Problem::NotDominated(_)]));
    // A handle is an i64 like any other value: it may be compared, but only a mark's own slot rewinds.
    assert!(load(&body(" %h = mark\n %same = icmp eq i64 %h, %n\n rewind %h")).is_ok());
    let text = "func @f() {\nentry:\n %h = mark\n br next(%h)\nnext(i64 %k):\n rewind %k\n ret\n}\n";
    assert_eq!(problems(text), vec![Problem::NotAMark("k".into())]);
}
