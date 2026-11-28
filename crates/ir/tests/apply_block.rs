//! `apply` to a block in the text form (`docs/SYNTAX.md`, "Applying to a block"): `applied func` marks an
//! apply target, and `apply F budget B, fault R, image(P, S), fields(Q, M), ctrl list(C, N)` takes every
//! operand as an `i64` value or constant.

use bennett_ir::{BlockApply, Inst, LoadError, Operand, Problem, load, read, write};

const TEXT: &str = "memory 256
applied func @f(i64 %b) {
entry:
    ret
}
func @main(i64 %p, i64 %n) {
entry:
    %e = qalloc i64 5
    %fa = faddr @f
    apply %fa budget 100, fault %e, image(%p, 16), fields(64, %n), ctrl list(%p, 0)
    apply 1 budget %n, fault 1, image(0, 1), fields(0, 0), ctrl list(0, 0)
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
fn an_apply_to_a_block_reads_writes_and_validates() {
    let module = read(TEXT).expect("reads");
    assert!(module.functions[0].applied && !module.functions[1].applied);
    let insts = &module.functions[1].blocks[0].insts;
    let Inst::ApplyBlock(first) = &insts[2] else { panic!("{:?}", insts[2]) };
    assert!(matches!(**first, BlockApply { fault: Operand::Slot(_), image: Operand::Slot(_), .. }));
    let Inst::ApplyBlock(second) = &insts[3] else { panic!("{:?}", insts[3]) };
    assert!(matches!(**second, BlockApply { function: Operand::Const(_), fault: Operand::Const(_), .. }));
    let written = write(&module);
    assert!(written.contains("applied func @f(i64 %b) {\n"), "{written}");
    assert!(
        written.contains("apply %fa budget 100, fault %e, image(%p, 16), fields(64, %n), ctrl list(%p, 0)\n"),
        "{written}"
    );
    assert_eq!(read(&written).expect("reads back"), module);
    load(TEXT).expect("validates");
}

#[test]
fn operands_are_i64_and_an_apply_target_takes_one_i64_and_returns_nothing() {
    let narrow = TEXT.replace("(i64 %p, i64 %n)", "(i64 %p, i32 %n)");
    assert!(problems(&narrow).iter().all(|problem| matches!(problem, Problem::TypeMismatch { .. })));
    for signature in ["applied func @f(i32 %b) {", "applied func @f(i64 %b, i64 %c) {", "applied func @f() {"] {
        let text = TEXT.replace("applied func @f(i64 %b) {", signature);
        assert_eq!(problems(&text), [Problem::AppliedSignature], "{signature}");
    }
    let result = TEXT
        .replace("applied func @f(i64 %b) {\nentry:\n    ret", "applied func @f(i64 %b) -> i1 {\nentry:\n    ret 0");
    assert_eq!(problems(&result), [Problem::AppliedSignature]);
    assert!(read(&TEXT.replace("applied func", "applied")).is_err(), "`applied` needs `func`");
}
