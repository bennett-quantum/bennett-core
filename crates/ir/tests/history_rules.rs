//! The validator's rules for `mark` and `rewind` that the maintainer decided on 2026-11-16: a mark's handle
//! is opaque (`docs/SYNTAX.md`, "History: `mark` and `rewind`").

use bennett_ir::{LoadError, Problem, load};

fn problems(text: &str) -> Vec<Problem> {
    match load(text) {
        Err(LoadError::Invalid(errors)) => errors.0.into_iter().map(|e| e.problem).collect(),
        other => panic!("expected validation errors, got {other:?}\n{text}"),
    }
}

/// A function with a register, an integer, a mark `%h` and the statements given, between the mark and a
/// `ret` (or the terminator given at the end of the statements).
fn body(statements: &str) -> String {
    format!(
        "memory 256\nfunc @g(i64 %x) -> i64 {{\nentry:\n ret %x\n}}\n\
         func @f(q8 %r, i8 %v, i64 %n) -> i64 {{\nentry:\n %h = mark\n{statements}\n}}\n"
    )
}

/// Every way a handle is used but as the handle of a `rewind` is refused, naming the handle.
#[test]
fn a_handle_is_used_only_by_rewind() {
    for statements in [
        " %c = icmp eq i64 %h, %n\n ret 0",
        " %s = add i64 %h, 1\n ret %s",
        " %s = select i64 1, %h, %n\n ret %s",
        " %t = trunc i64 %h to i8\n ret 0",
        " %l = load i8 %h\n ret 0",
        " store i64 %h, 64\n ret 0",
        " store i8 %v, %h\n ret 0",
        " %a = alloca %h\n ret 0",
        " stackrestore %h\n ret 0",
        " %j = setjmp %h\n ret 0",
        " %y = call @g(%h)\n ret %y",
        " %f = faddr @g\n %y = call_indirect fn(i64) -> i64 %f(%h)\n ret %y",
        " x [%h:0]\n ret 0",
        " rewind %h keep (%h -> [64])\n ret 0",
        " rewind %h keep (i8 1 -> [%h])\n ret 0",
        " ret %h",
        " br next(%h)\nnext(i64 %k):\n ret 0",
        " %c = trunc i64 %h to i1\n br %c, a, b\na:\n ret 0\nb:\n ret 1",
        " switch i64 %h, 0: a, default: a\na:\n ret 0",
        " exit %h",
        " longjmp %h, 1",
    ] {
        let found = problems(&body(statements));
        assert!(found.contains(&Problem::HandleUse("h".into())), "{statements}: {found:?}");
        assert!(found.iter().all(|p| *p == Problem::HandleUse("h".into())), "{statements}: {found:?}");
    }
    // A handle passed through a block parameter is a use of the handle, and the parameter is no mark.
    let found = problems(&body(" br next(%h)\nnext(i64 %k):\n rewind %k\n ret 0"));
    assert_eq!(found, vec![Problem::NotAMark("k".into()), Problem::HandleUse("h".into())]);
    // As the keep value of a rewind of another mark too.
    let found = problems(&body(" %g = mark\n rewind %g keep (%h -> [64])\n ret 0"));
    assert_eq!(found, vec![Problem::HandleUse("h".into())]);
}

#[test]
fn a_handle_named_by_rewind_is_accepted() {
    for statements in [
        " rewind %h\n ret 0",
        " rewind %h keep (%v -> %r)\n ret 0",
        " rewind %h keep (%v -> %r, i8 3 -> [64])\n rewind %h\n ret 0",
        " %g = mark\n rewind %g\n rewind %h\n ret %n",
        " br next\nnext:\n rewind %h\n ret 0",
    ] {
        assert!(load(&body(statements)).is_ok(), "{statements}");
    }
}

/// The message names the handle and says what is allowed.
#[test]
fn the_refusal_names_the_handle() {
    let error = load(&body(" ret %h")).expect_err("a handle returned");
    let text = error.to_string();
    assert!(text.contains("%h") && text.contains("rewind"), "{text}");
}
