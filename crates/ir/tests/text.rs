//! The reader, writer and validator on the examples and on deliberately broken programs.

use bennett_ir::{LoadError, NameKind, Problem, ReadErrorKind, load, read, validate, write};

const EXAMPLES: [(&str, &str); 5] = [
    ("sum", include_str!("../../../examples/sum.bn")),
    ("calls", include_str!("../../../examples/calls.bn")),
    ("thesis", include_str!("../../../examples/thesis.bn")),
    ("fault", include_str!("../../../examples/fault.bn")),
    ("grover", include_str!("../../../examples/grover.bn")),
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
    assert!(matches!(read_error(&body("%x = add i65 1, 1")).2, ReadErrorKind::BadType(_)));
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
