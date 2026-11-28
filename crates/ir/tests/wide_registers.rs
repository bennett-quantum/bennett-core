//! Registers wider than 64 qubits in the text form and the validator (`docs/SYNTAX.md`, "Registers are not
//! integers"): `qN` and `qalloc N` from 1 to 2^32 - 1, qubit indices below N; a whole-register `measure` and
//! the operands of `apply` are integers, so a register wider than 128 qubits is refused there by name.

use bennett_ir::{LoadError, Problem, ReadErrorKind, load, read, write};

fn problems(text: &str) -> Vec<Problem> {
    match load(text) {
        Err(LoadError::Invalid(errors)) => errors.0.into_iter().map(|error| error.problem).collect(),
        other => panic!("expected validation errors, got {other:?}"),
    }
}

#[test]
fn wide_registers_read_write_and_validate_at_each_width() {
    for width in [64u32, 65, 128, 1000, u32::MAX] {
        let top = width - 1;
        let text = format!(
            "memory 64
func @keep(q{width} %r) -> q{width} {{
entry:
    x %r[{top}] ctrl %r
    ret %r
}}
func @main() {{
entry:
    %q = qalloc {width}
    h %q[{top}]
    %k = call @keep(%q)
    qfree %k
    ret
}}
"
        );
        let module = load(&text).unwrap_or_else(|error| panic!("q{width}: {error}"));
        let written = write(&module);
        assert!(written.contains(&format!("qalloc {width}")) && written.contains(&format!("%r[{top}]")));
        assert_eq!(read(&written).as_ref(), Ok(&*module), "q{width}");
    }
}

#[test]
fn indices_and_widths_outside_the_register_are_refused() {
    let gate =
        |width: u32, bit: u64| format!("func @f() {{\nentry:\n    %q = qalloc {width}\n    x %q[{bit}]\n    ret\n}}\n");
    assert_eq!(problems(&gate(1000, 1000)), [Problem::QubitOutOfRange { bit: 1000, bits: 1000 }]);
    assert!(load(&gate(1000, 999)).is_ok());
    // A literal position from 2^32 reads (an `i64` handle may be named so) and is past every register.
    assert_eq!(problems(&gate(1000, 1 << 32)), [Problem::QubitOutOfRange { bit: 1 << 32, bits: 1000 }]);
    // Past 2^64 - 1 the position is no `i64` literal, as for any `i64` operand.
    let error = read(&gate(1000, 1 << 32).replace("4294967296", "18446744073709551616")).expect_err("past 2^64 - 1");
    assert!(matches!(error.kind, ReadErrorKind::OutOfRange { .. }), "{error:?}");
    let error = read("func @f(q4294967296 %q) {\nentry:\n    ret\n}\n").expect_err("too wide");
    assert!(matches!(error.kind, ReadErrorKind::BadType(_)), "{error:?}");
}

#[test]
fn measuring_a_register_wider_than_an_integer_is_refused_by_name() {
    let measure = |width: u32| {
        format!(
            "func @f() -> i8 {{\nentry:\n    %q = qalloc {width}\n    %m = measure %q\n    qfree %q\n    ret 0\n}}\n"
        )
    };
    assert!(load(&measure(128)).is_ok());
    assert_eq!(problems(&measure(129)), [Problem::MeasureTooWide(129)]);
    assert_eq!(problems(&measure(1000)), [Problem::MeasureTooWide(1000)]);
}

#[test]
fn apply_operands_wider_than_an_integer_are_refused_by_name() {
    let apply = |arg: u32, out: u32, fault: u32| {
        format!(
            "func @id(i128 %x) -> i128 {{
entry:
    ret %x
}}
func @main() {{
entry:
    %x = qalloc {arg}
    %o = qalloc {out}
    %f = qalloc {fault}
    apply @id(%x) -> %o, %f budget 4
    ret
}}
"
        )
    };
    assert!(load(&apply(128, 128, 5)).is_ok());
    assert!(load(&apply(128, 128, 1000)).is_ok(), "a fault register may be of any width from 5");
    assert_eq!(problems(&apply(129, 128, 5)), [Problem::RegionOperandTooWide("x".into(), 129)]);
    assert_eq!(problems(&apply(128, 1000, 5)), [Problem::RegionOperandTooWide("o".into(), 1000)]);
}
