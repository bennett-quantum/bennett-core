//! The validator's rules for `mark` and `rewind` that the maintainer decided on 2026-11-16: a mark's handle
//! is opaque, and a value defined between a mark and its rewind is not used after the rewind
//! (`docs/SYNTAX.md`, "History: `mark` and `rewind`").

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

/// A function `@f(q8 %r, i8 %v, i1 %c)` with the given body, and `@jump(i64)` that jumps through a record.
fn program(blocks: &str) -> String {
    format!(
        "memory 256\nfunc @jump(i64 %buf) {{\nentry:\n longjmp %buf, 1\n}}\n\
         func @f(q8 %r, i8 %v, i1 %c) -> i8 {{\n{blocks}\n}}\n"
    )
}

fn rewound(name: &str) -> Vec<Problem> {
    vec![Problem::RewoundUse(name.into())]
}

/// A value defined between a mark and its rewind holds its mark-time value after the rewind (or none): any
/// use of it there, on some path, is refused, naming the value.
#[test]
fn a_value_defined_between_a_mark_and_its_rewind_is_not_used_after_it() {
    for (blocks, name) in [
        // Straight line: as a result, an operand, a branch argument, a condition, a keep of a later rewind.
        ("entry:\n %h = mark\n %y = add i8 %v, 1\n rewind %h\n ret %y", "y"),
        ("entry:\n %h = mark\n %y = add i8 %v, 1\n rewind %h keep (%y -> %r)\n %z = add i8 %y, 1\n ret %z", "y"),
        ("entry:\n %h = mark\n %y = add i8 %v, 1\n rewind %h\n br next(%y)\nnext(i8 %w):\n ret %w", "y"),
        ("entry:\n %h = mark\n %b = icmp eq i8 %v, 0\n rewind %h\n br %b, a, b\na:\n ret 0\nb:\n ret 1", "b"),
        ("entry:\n %h = mark\n %y = add i8 %v, 1\n rewind %h\n %g = mark\n rewind %g keep (%y -> %r)\n ret 0", "y"),
        // A register allocated since the mark is un-allocated by the rewind.
        ("entry:\n %h = mark\n %t = qalloc 1\n rewind %h\n x %t\n ret 0", "t"),
        // A block parameter defined since the mark.
        ("entry:\n %h = mark\n br next(%v)\nnext(i8 %p):\n rewind %h\n ret %p", "p"),
        // The use is reached on one path only.
        (
            "entry:\n %h = mark\n %y = add i8 %v, 1\n br %c, back, join\nback:\n rewind %h\n br join\njoin:\n ret %y",
            "y",
        ),
        // In a loop: defined after the mark, used after the loop that rewinds.
        (
            "entry:\n br loop(i8 0)\nloop(i8 %i):\n %h = mark\n %y = add i8 %i, 1\n rewind %h keep (%y -> %r)\n \
             %more = icmp ult i8 %i, 3\n %i.next = add i8 %i, 1\n br %more, loop(%i.next), done\ndone:\n ret %y",
            "y",
        ),
        // An inner mark's handle, after a rewind of the outer mark, names a mark that was un-run.
        ("entry:\n %h = mark\n %g = mark\n rewind %h\n rewind %g\n ret 0", "g"),
        // A definition in a loop between the mark and the rewind.
        (
            "entry:\n %h = mark\n br loop\nloop:\n %y = add i8 %v, 1\n rewind %h\n %z = add i8 %y, 2\n br %c, loop, done\n\
             done:\n ret %z",
            "y",
        ),
    ] {
        assert_eq!(problems(&program(blocks)), rewound(name), "{blocks}");
    }
}

/// Defined before the mark, after the rewind, or again on every path after it; used only on paths without
/// the rewind; or used by the rewind itself: accepted.
#[test]
fn uses_the_rewind_did_not_restore_are_accepted() {
    for blocks in [
        "entry:\n %x = add i8 %v, 1\n %h = mark\n rewind %h\n ret %x",
        "entry:\n %h = mark\n %y = add i8 %v, 1\n rewind %h keep (%y -> %r)\n ret %v",
        "entry:\n %h = mark\n rewind %h\n %y = add i8 %v, 1\n ret %y",
        "entry:\n %h = mark\n %y = add i8 %v, 1\n br %c, back, done\nback:\n rewind %h\n ret 0\ndone:\n ret %y",
        // Each pass defines %y again after the mark and uses it before the rewind.
        "entry:\n br loop(i8 0)\nloop(i8 %i):\n %h = mark\n %y = add i8 %i, 1\n rewind %h keep (%y -> %r)\n \
         %more = icmp ult i8 %i, 3\n %i.next = add i8 %i, 1\n br %more, loop(%i.next), done\ndone:\n ret %i",
        // The mark runs again after the definition: the rewind goes back only to there.
        "entry:\n br loop(i8 0)\nloop(i8 %i):\n %h = mark\n rewind %h\n %y = add i8 %i, 1\n br %c, loop(%y), done\n\
         done:\n ret %y",
        // Rewinding one mark twice: the handle itself is not restored away.
        "entry:\n %h = mark\n %y = add i8 %v, 1\n rewind %h keep (%y -> %r)\n rewind %h\n ret 0",
        // An inner rewind restores only what was defined since the inner mark.
        "entry:\n %h = mark\n %x = add i8 %v, 1\n %g = mark\n rewind %g\n rewind %h keep (%x -> %r)\n ret 0",
    ] {
        assert!(load(&program(blocks)).is_ok(), "{blocks}: {:?}", load(&program(blocks)).err());
    }
}

/// A `longjmp`, here or in a callee, resumes after a `setjmp` of the function: a use there is checked as if
/// the jump were a branch, so a value the rewind restored is refused there too. Without a rewind on the way
/// to the jump the same uses are accepted.
#[test]
fn a_jump_back_after_a_rewind_is_a_path_too() {
    let after_jump = |jump: &str| {
        format!(
            "entry:\n %buf = alloca 32\n %h = mark\n %y = add i8 %v, 1\n %s = setjmp %buf\n %first = icmp eq i32 %s, 0\n \
             br %first, go, use\ngo:\n{jump}\nuse:\n ret %y"
        )
    };
    for jump in [" rewind %h\n longjmp %buf, 1", " rewind %h\n call @jump(%buf)\n ret 0"] {
        assert_eq!(problems(&program(&after_jump(jump))), rewound("y"), "{jump}");
    }
    for jump in [" longjmp %buf, 1", " call @jump(%buf)\n ret 0", " rewind %h\n ret 0"] {
        assert!(load(&program(&after_jump(jump))).is_ok(), "{jump}");
    }
}

/// The refusal says what is wrong.
#[test]
fn the_rewound_use_message_names_the_value() {
    let error = load(&program("entry:\n %h = mark\n %y = add i8 %v, 1\n rewind %h\n ret %y")).expect_err("rewound");
    let text = error.to_string();
    assert!(text.contains("%y") && text.contains("rewind"), "{text}");
}
