//! Admission and text probes independent of the implementation's dataflow.
use bennett_ir::{LoadError, Problem, load, read, write};

fn problems(text: &str) -> Vec<Problem> {
    match load(text) {
        Err(LoadError::Invalid(errors)) => errors.0.into_iter().map(|e| e.problem).collect(),
        other => panic!("expected refusal: {other:?}\n{text}"),
    }
}

#[test]
fn handles_cannot_be_forged_loaded_or_transferred_between_activations() {
    for definition in ["%fake = add i64 1, 0", "%fake = load i64 16", "%fake = stacksave", "%fake = call @number()"] {
        let text = format!(
            "memory 512\nfunc @number() -> i64 {{\nentry:\n ret 1\n}}\nfunc @main() {{\nentry:\n {definition}\n rewind %fake\n ret\n}}"
        );
        assert_eq!(problems(&text), [Problem::NotAMark("fake".into())]);
    }
    let other_activation = "memory 512\nfunc @use(i64 %foreign) {\nentry:\n rewind %foreign\n ret\n}\nfunc @main() {\nentry:\n %h = mark\n call @use(%h)\n ret\n}";
    let found = problems(other_activation);
    assert!(found.contains(&Problem::NotAMark("foreign".into())));
    assert!(found.contains(&Problem::HandleUse("h".into())));
    // Storing any width of the handle, or using it as an address in the newly admitted form,
    // must fail even in an unreachable block and a function no entry calls (DESIGN, "Tests").
    for operation in ["store i64 %h, 16", "%m = measure i8 [%h]", "rewind %h keep (i8 1 -> [%h])"] {
        for dead in [false, true] {
            let branch = if dead { "ret\ndead:" } else { "br dead\ndead:" };
            let text = format!("memory 512\nfunc @unused() {{\nentry:\n %h = mark\n {branch}\n {operation}\n ret\n}}");
            assert_eq!(problems(&text), [Problem::HandleUse("h".into())]);
        }
    }
}

/// Independently enumerate the linear order, rather than invoking any analysis helper:
/// a definition is stale exactly when mark < definition < rewind < use. Definitions are unique.
#[test]
fn all_linear_orders_obey_the_rewound_use_rule() {
    let mut cases = 0;
    for mark in 0..4 {
        for definition in 0..4 {
            for rewind in 0..4 {
                for usage in 0..4 {
                    let positions = [mark, definition, rewind, usage];
                    if (0..4).any(|i| (i + 1..4).any(|j| positions[i] == positions[j]))
                        || mark >= rewind
                        || definition >= usage
                    {
                        continue;
                    }
                    let mut lines = [""; 4];
                    lines[mark] = "%h = mark";
                    lines[definition] = "%v = add i8 7, 11";
                    lines[rewind] = "rewind %h";
                    lines[usage] = "%used = add i8 %v, 1";
                    let text = format!("memory 512\nfunc @main() {{\nentry:\n{}\n ret\n}}", lines.join("\n"));
                    let invalid = mark < definition && definition < rewind && rewind < usage;
                    if invalid {
                        assert_eq!(problems(&text), [Problem::RewoundUse("v".into())]);
                    } else {
                        load(&text).unwrap_or_else(|e| panic!("{text}\n{e}"));
                    }
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 6);
}

/// Two paths with independently chosen rewinds. Block order must have no semantic effect.
#[test]
fn diamond_paths_and_block_orders_have_the_same_admission() {
    for reverse in [false, true] {
        for left_rewind in [false, true] {
            for right_rewind in [false, true] {
                for use_join in [false, true] {
                    let left = format!("left:\n {}\n br join", if left_rewind { "rewind %h" } else { "" });
                    let right = format!("right:\n {}\n br join", if right_rewind { "rewind %h" } else { "" });
                    let paths = if reverse { format!("{right}\n{left}") } else { format!("{left}\n{right}") };
                    let result = if use_join { "%v" } else { "0" };
                    let text = format!(
                        "memory 512\nfunc @main(i1 %c) -> i8 {{\nentry:\n %h = mark\n %v = add i8 7, 11\n br %c, left, right\n{paths}\njoin:\n ret {result}\n}}"
                    );
                    if use_join && (left_rewind || right_rewind) {
                        assert_eq!(problems(&text), [Problem::RewoundUse("v".into())]);
                    } else {
                        load(&text).unwrap_or_else(|e| panic!("{text}\n{e}"));
                    }
                }
            }
        }
    }
}

#[test]
fn stale_values_are_refused_in_every_new_address_operand() {
    for operation in
        ["%m = measure i128 [%p]", "z [%p:127]", "rewind %h keep (i1 1 -> [%p])", "%s = setjmp %p", "longjmp %p, 1"]
    {
        let end = if operation.starts_with("longjmp") { "" } else { "ret" };
        let text = format!(
            "memory 512\nfunc @main() {{\nentry:\n %h = mark\n %p = add i64 16, 0\n rewind %h\n {operation}\n {end}\n}}"
        );
        assert_eq!(problems(&text), [Problem::RewoundUse("p".into())]);
    }
}

#[test]
fn nonlocal_resumptions_check_uses_and_allow_new_definitions_after_the_jump() {
    let helpers = "func @throw() {\nentry:\n longjmp 16, 1\n}\n";
    for jump in
        ["longjmp 16, 1", "call @throw()\n ret 0", "%target = faddr @throw\n call_indirect fn() %target()\n ret 0"]
    {
        for redefine in [false, true] {
            // The buffer is literal here: it is the continuation's actual use of %v that matters.
            let use_value = if redefine { "%new = add i8 7, 11\n ret %new" } else { "ret %v" };
            let text = format!(
                "memory 512\n{helpers}func @main() -> i8 {{\nentry:\n %h = mark\n %v = add i8 7, 11\n %s = setjmp 16\n %first = icmp eq i32 %s, 0\n br %first, jump, done\njump:\n rewind %h\n {jump}\ndone:\n {use_value}\n}}"
            );
            if redefine {
                load(&text).unwrap_or_else(|e| panic!("{text}\n{e}"));
            } else {
                assert_eq!(problems(&text), [Problem::RewoundUse("v".into())]);
            }
        }
    }
}

#[test]
fn all_measurement_widths_and_extreme_addresses_round_trip() {
    for width in 1..=128 {
        for address in ["0", "17", "0xffffffffffffffff", "-1", "%p"] {
            let text = format!(
                "memory 512\nfunc @main(i64 %p) -> i{width} {{\nentry:\n %m = measure i{width} [{address}]\n ret %m\n}}"
            );
            let module = read(&text).expect("read");
            assert_eq!(read(&write(&module)), Ok(module));
            load(&text).unwrap_or_else(|e| panic!("{text}\n{e}"));
        }
    }
}

/// DESIGN, "History" defines non-local paths as resumptions *after* setjmp; "Non-local jumps"
/// says the jump sets the result and resumes there. It does not execute setjmp or read its buffer.
/// `%buf` is restored away, but no continuation instruction reads it. Rebuild the four-word record
/// with constants after the rewind so that the jump itself is valid too (SYNTAX, "Non-local jumps").
#[test]
#[ignore = "defect: setjmp resumption spuriously reads a buffer operand restored by rewind"]
fn resuming_after_setjmp_does_not_read_its_stale_buffer_operand() {
    let text = include_str!("fixtures/rewound_setjmp.bn");
    // Four slots reserve 32 bytes, so the saved SP is 480; setjmp is entry instruction 2.
    let parsed = read(text).expect("readable");
    assert_eq!(parsed.functions[0].slot_count(), 4);
    assert!(load(text).is_ok(), "a resumption starts after setjmp: {:?}", load(text).err());
}
