//! The remaining language has ordinary calls and apply; obsolete forms are rejected.
use bennett_ir::{Expected, Problem, ReadErrorKind, Type, Width, load, read, validate, write};

#[test]
fn removed_instructions_are_rejected_even_in_unused_functions() {
    for body in [
        "%h = mark",
        "%h = mark budget 4",
        "%h = mark deadline 4",
        "rewind %h keep ()",
        "rewind %h keep (%x -> %q)",
        "%v, %c = call @f(i1 1) budget 4",
        "%v, %c = call @f(i1 1)",
        "%v = call @f(i1 1) budget 4",
    ] {
        let text = format!(
            "memory 512\nfunc @f(i1 %x) -> i1 {{\nentry:\nret %x\n}}\nfunc @unused() {{\nentry:\n{body}\nret\n}}\nfunc @main() {{\nentry:\nret\n}}\n"
        );
        let error = read(&text).expect_err("obsolete instruction syntax");
        // An undefined old handle must not hide a parser that still recognizes rewind.
        for keyword in ["mark", "rewind"] {
            if body.split_whitespace().any(|word| word == keyword) {
                assert_eq!(error.kind, ReadErrorKind::UnknownInstruction(keyword.into()), "{body}");
            }
        }
    }
}

#[test]
fn direct_and_indirect_ordinary_calls_cannot_consume_a_qubit_register() {
    for call in ["%v = call @f(%q)", "%p = faddr @f\n%v = call_indirect fn(i1) -> i1 %p(%q)"] {
        let text = format!(
            "memory 512\nfunc @f(i1 %x) -> i1 {{\nentry:\nret %x\n}}\nfunc @unused() {{\nentry:\n%q = qalloc 1\n{call}\nret\n}}\nfunc @main() {{\nentry:\nret\n}}\n"
        );
        let module = read(&text).expect("ordinary call syntax");
        let errors = validate(module).expect_err("qubits are not integer call arguments");
        assert!(
            errors.0.iter().any(|error| error.problem
                == Problem::TypeMismatch {
                    operand: "`%q`".into(),
                    expected: Expected::Type(Type::Int(Width::BIT)),
                    found: Type::Qreg(Width::BIT),
                }),
            "{call}: {errors}"
        );
    }
}

#[test]
fn writer_keeps_apply_and_plain_address_gates_without_obsolete_forms() {
    let text = "memory 512\nfunc @f(i1 %x) -> i1 {\nentry:\nret %x\n}\nfunc @main(i64 %p) {\nentry:\n%q = qalloc 1\n%e = qalloc 5\nh [%p:3]\napply @f(i1 1) -> %q, %e budget 4\ns [%p:3] ctrl %q\n%m = measure i8 [%p]\nret\n}\n";
    let module = read(text).expect("remaining forms");
    load(text).expect("valid remaining forms");
    let emitted = write(&module);
    assert_eq!(read(&emitted), Ok(module));
    for removed in ["mark", "rewind", "deadline", "world"] {
        assert!(!emitted.contains(removed), "{emitted}");
    }
}
