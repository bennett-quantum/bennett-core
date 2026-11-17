//! Admission witnesses for guarded calls, including both definitions and their use positions.

use bennett_ir::{LoadError, Problem, load, read, write};

const CALLS: &str = "memory 512
func @identity(i8 %n) -> i8 {
entry:
    ret %n
}
func @nothing() {
entry:
    ret
}
func @main(i64 %budget, i8 %input) -> i8 {
entry:
    %before = add i8 %input, 3
    %value, %code = call @identity(%before) budget %budget
    %code8 = zext i5 %code to i8
    %only = call @nothing() budget 7
    %only8 = zext i5 %only to i8
    %sum = add i8 %value, %code8
    %result = add i8 %sum, %only8
    ret %result
}
";

#[test]
fn guarded_calls_define_a_value_and_a_five_bit_code_or_just_the_code() {
    load(CALLS).expect("both returned definitions are available with their documented types");
    let parsed = read(CALLS).expect("read calls");
    let printed = write(&parsed);
    for instruction in ["%value, %code = call @identity(%before) budget %budget", "%only = call @nothing() budget 7"] {
        assert!(printed.contains(instruction), "{printed}");
    }
    assert_eq!(read(&printed), Ok(parsed));
    for bad in [
        CALLS.replace("zext i5 %code", "zext i8 %code"),
        CALLS.replace("zext i5 %only", "zext i8 %only"),
        CALLS.replace("i64 %budget", "i8 %budget"),
        CALLS
            .replace("call @nothing() budget 7", "call @identity(%before) budget 7")
            .replace("zext i5 %only", "zext i8 %only"),
    ] {
        let Err(LoadError::Invalid(errors)) = load(&bad) else { panic!("expected type refusal: {bad}") };
        assert!(errors.0.iter().any(|error| matches!(error.problem, Problem::TypeMismatch { .. })));
    }
}

#[test]
fn a_call_cannot_read_the_fault_code_it_is_about_to_define() {
    // At both the first and a later instruction, the definition is after its own operand uses.
    for before in ["", "%earlier = add i8 1, 2\n"] {
        let text = format!(
            "memory 512
func @identity(i5 %n) -> i5 {{
entry:
    ret %n
}}
func @main() -> i5 {{
entry:
    {before}%value, %code = call @identity(%code) budget 10
    ret %value
}}"
        );
        let Err(LoadError::Invalid(errors)) = load(&text) else { panic!("self-use must be refused: {text}") };
        assert_eq!(errors.0.iter().map(|e| &e.problem).collect::<Vec<_>>(), [&Problem::NotDominated("code".into())]);
    }
}

#[test]
fn a_pair_of_results_requires_a_call_with_a_budget_and_an_equals_sign() {
    for bad in [
        CALLS.replace("%value, %code = call", "%value, %code call"),
        CALLS.replace("%value, %code = call", "%value, = call"),
        CALLS.replace("call @identity(%before) budget %budget", "call @identity(%before)"),
        CALLS.replace("%value, %code = call @identity(%before) budget %budget", "%value, %code = add i8 1, 2"),
    ] {
        assert!(read(&bad).is_err(), "{bad}");
    }
}
