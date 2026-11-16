//! Admission boundary table: the validator must check unreachable functions too.
use bennett_ir::{LoadError, Problem, load, read, write};

fn program(body: &str) -> String {
    format!("memory 256\nfunc @subject() -> i8 {{\nentry:\n{body}\n}}\n")
}

#[test]
fn well_formed_programs_are_accepted() {
    for body in [
        "ret 0",
        "ret 255",
        "ret -128",
        "%v = add i8 0xff, 1\nret %v",
        "%v = add i128 340282366920938463463374607431768211455, 0\nret 0",
        "%q = qalloc 1\nqfree %q\nret 0",
        "%q = qalloc 64\nqfree %q\nret 0",
        "br done(i8 0)\ndone(i8 %x):\nret %x",
        "exit i8 0",
        "%m = mark\nrewind %m\nret 0",
    ] {
        let text = program(body);
        load(&text).unwrap_or_else(|e| panic!("{body}: {e}"));
        let module = read(&text).expect("accepted text");
        assert_eq!(read(&write(&module)), Ok(module), "{body}");
    }
}

#[test]
fn malformed_programs_have_the_expected_validation_problem() {
    for (body, problem) in [
        ("ret", Problem::ReturnMismatch),
        ("br done()\ndone(i8 %x):\nret %x", Problem::ArgumentCount { expected: 1, found: 0 }),
        ("%v = zext i8 1 to i8\nret %v", Problem::CastWidths { op: "zext", from: 8, to: 8 }),
        ("%q = qalloc 1\nh %q[1]\nret 0", Problem::QubitOutOfRange { bit: 1, bits: 1 }),
        ("%q = qalloc 1\nx %q[0] ctrl %q[0]\nret 0", Problem::RepeatedQubit("q".into(), 0)),
        ("%v = add i64 0, 0\nrewind %v\nret 0", Problem::NotAMark("v".into())),
    ] {
        for prefix in ["", "func @main() -> i8 {\nentry:\nret 0\n}\n"] {
            let text = format!("{prefix}{}", program(body));
            match load(&text) {
                Err(LoadError::Invalid(errors)) => {
                    assert_eq!(errors.0.iter().map(|e| &e.problem).collect::<Vec<_>>(), [&problem], "{text}");
                }
                result => panic!("{text}: expected {problem:?}, got {result:?}"),
            }
        }
    }
}
