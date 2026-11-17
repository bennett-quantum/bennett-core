//! Runs stopped at entry still have an outcome and a well-defined undo boundary.

use bennett_exec::{Executor, Options, Outcome, ReverseError, RunError, StartError};
use bennett_ir::{Fault, FuncId, ValidModule};

fn load(text: &str) -> ValidModule {
    bennett_ir::load(text).expect("valid fixture")
}

fn stopped_at_entry(module: &ValidModule, entry: &str, args: &[u64], options: Options, expected: Outcome) {
    let entry = module.function_named(entry).expect("entry");
    for reversible in [false, true] {
        for step_first in [false, true] {
            let mut exec = Executor::new(module, entry, args, Options { reversible, ..options }).expect("start");
            let initial = exec.snapshot();
            if reversible {
                assert_eq!(exec.step_back(), Ok(false));
                assert_eq!(exec.reverse(), Ok(0));
                assert_eq!(exec.snapshot(), initial);
            }
            if step_first {
                assert_eq!(exec.step(), Some(expected));
            } else {
                assert_eq!(exec.run(), expected);
            }
            assert_eq!(exec.outcome(), Some(expected));
            assert_eq!(exec.stats().steps, 0);
            assert_eq!(exec.tape(), b"");
            assert_eq!(exec.stats().journal_bytes, reversible.then_some(0));
            let mut stopped = initial.clone();
            stopped.classical.outcome = Some(expected);
            assert_eq!(exec.snapshot(), stopped, "a failed first step changes only the outcome");
            assert_eq!(exec.run(), expected, "run again while stopped");
            assert_eq!(exec.step(), Some(expected), "step again while stopped");
            assert_eq!(exec.snapshot(), stopped);
            if reversible {
                assert_eq!(exec.step_back(), Ok(false));
                assert_eq!(exec.snapshot(), initial);
                assert_eq!(exec.reverse(), Ok(0));
                assert_eq!(exec.snapshot(), initial);
                assert_eq!(exec.run(), expected, "retry after reversal");
                assert_eq!(exec.reverse(), Ok(0));
                assert_eq!(exec.snapshot(), initial);
            } else {
                assert_eq!(exec.step_back(), Err(ReverseError::NotReversible));
                assert_eq!(exec.reverse(), Err(ReverseError::NotReversible));
                assert_eq!(exec.snapshot(), stopped);
            }
        }
    }
}

#[test]
fn an_entry_frame_that_does_not_fit_returns_stack_overflow() {
    let module = load("memory 1\nfunc @main() -> i32 {\nentry:\n %v = add i32 1, 2\n ret %v\n}\n");
    stopped_at_entry(&module, "main", &[], Options::default(), Outcome::Faulted(Fault::StackOverflow));
    // Construction fails before the run's budget is considered.
    stopped_at_entry(
        &module,
        "main",
        &[],
        Options { budget: Some(0), ..Options::default() },
        Outcome::Faulted(Fault::StackOverflow),
    );
}

#[test]
fn static_data_leaves_too_little_room_for_the_entry_parameters() {
    let module = load("memory 8\ndata 0 = i8 91\nfunc @main(i8 %v) -> i8 {\nentry:\n ret %v\n}\n");
    stopped_at_entry(&module, "main", &[7], Options::default(), Outcome::Faulted(Fault::StackOverflow));
}

#[test]
fn a_zero_budget_stops_before_the_first_instruction_or_terminator() {
    let module = load(
        "memory 64\nfunc @main() -> i8 {\nentry:\n %v = add i8 1, 2\n ret %v\n}\nfunc @empty() {\nentry:\n ret\n}\nfunc @fault() {\nentry:\n fault unreachable\n}",
    );
    for entry in ["main", "empty", "fault"] {
        stopped_at_entry(
            &module,
            entry,
            &[],
            Options { budget: Some(0), ..Options::default() },
            Outcome::Faulted(Fault::Budget),
        );
    }
}

#[test]
fn every_explicit_fault_can_stop_at_the_entry_terminator() {
    for &fault in Fault::ALL {
        let module = load(&format!("func @main() {{\nentry:\n fault {fault}\n}}\n"));
        stopped_at_entry(&module, "main", &[], Options::default(), Outcome::Faulted(fault));
    }
}

#[test]
fn first_instruction_faults_leave_no_history_to_undo() {
    let cases = [
        ("%v = udiv i8 1, 0", Fault::DivideByZero),
        ("%v = sdiv i8 1, 0", Fault::DivideByZero),
        ("%v = urem i8 1, 0", Fault::DivideByZero),
        ("%v = srem i8 1, 0", Fault::DivideByZero),
        ("%v = load i8 64", Fault::OutOfBounds),
        ("store i8 1, 64", Fault::OutOfBounds),
        ("%v = alloca 65", Fault::StackOverflow),
        ("stackrestore 65", Fault::BadStackRestore),
        ("call_indirect fn() 0()", Fault::BadIndirect),
        ("%v = setjmp 64", Fault::OutOfBounds),
        ("h [64]", Fault::OutOfBounds),
    ];
    for (instruction, fault) in cases {
        let module = load(&format!("memory 64\nfunc @main() {{\nentry:\n {instruction}\n ret\n}}\n"));
        stopped_at_entry(&module, "main", &[], Options::default(), Outcome::Faulted(fault));
    }
}

#[test]
fn aliased_memory_qubits_fault_on_the_first_gate() {
    let module = load("memory 64\nfunc @main(i64 %a, i64 %b) {\nentry:\n h [%a] ctrl [%b]\n ret\n}\n");
    stopped_at_entry(&module, "main", &[0, 0], Options::default(), Outcome::Faulted(Fault::QubitAlias));
}

#[test]
fn a_configuration_limit_can_stop_the_first_memory_gate() {
    let module = load("memory 1\nfunc @main() {\nentry:\n h [0]\n ret\n}\n");
    stopped_at_entry(
        &module,
        "main",
        &[],
        Options { max_configurations: Some(1), ..Options::default() },
        Outcome::Stopped(RunError::ConfigurationLimit { limit: 1 }),
    );
}

#[test]
fn a_first_call_can_overflow_without_entering_the_callee() {
    for call in ["call @large(i64 0)", "call_indirect fn(i64) 2(i64 0)"] {
        let module = load(&format!(
            "memory 0\nfunc @main() {{\nentry:\n {call}\n ret\n}}\nfunc @large(i64 %v) {{\nentry:\n ret\n}}\n"
        ));
        stopped_at_entry(&module, "main", &[], Options::default(), Outcome::Faulted(Fault::StackOverflow));
    }
}

#[test]
fn invalid_first_terminators_stop_without_a_successful_step() {
    for (term, fault) in [
        ("longjmp 64, 1", Fault::OutOfBounds),
        ("longjmp 0, 1", Fault::BadLongjmp),
        ("exit i32 1", Fault::ExitTypeMismatch),
    ] {
        let module = load(&format!("memory 64\nfunc @main() {{\nentry:\n {term}\n}}\n"));
        stopped_at_entry(&module, "main", &[], Options::default(), Outcome::Faulted(fault));
    }
}

#[test]
fn a_terminator_only_entry_with_no_slots_can_return_and_reverse() {
    for (term, result, expected) in [("ret", "", None), ("ret 7", " -> i8", Some(7)), ("exit i8 7", " -> i8", Some(7))]
    {
        let module = load(&format!("func @main(){result} {{\nentry:\n {term}\n}}\n"));
        for step_first in [false, true] {
            let mut exec = Executor::new(&module, FuncId(0), &[], Options { reversible: true, ..Options::default() })
                .expect("start");
            let initial = exec.snapshot();
            assert_eq!(exec.step_back(), Ok(false));
            assert_eq!(exec.reverse(), Ok(0));
            assert_eq!(exec.snapshot(), initial);
            let expected = Outcome::Returned(expected);
            if step_first {
                assert_eq!(exec.step(), Some(expected));
            }
            assert_eq!(exec.run(), expected);
            assert_eq!(exec.step(), Some(expected));
            assert_eq!(exec.stats().steps, 1);
            assert_eq!(exec.reverse(), Ok(1));
            assert_eq!(exec.snapshot(), initial);
        }
    }
}

#[test]
fn an_empty_module_has_no_entry() {
    let module = load("");
    assert!(matches!(Executor::new(&module, FuncId(0), &[], Options::default()), Err(StartError::InvalidEntry(0))));
}

#[test]
fn an_out_of_range_entry_is_a_start_error() {
    let module = load("func @main() {\nentry:\n ret\n}\n");
    for index in [1, u32::MAX] {
        assert!(
            matches!(Executor::new(&module, FuncId(index), &[], Options::default()), Err(StartError::InvalidEntry(found)) if found == index)
        );
    }
}

#[test]
fn an_entry_without_blocks_is_rejected_before_execution() {
    let error = bennett_ir::load("func @main() {\n}\n").expect_err("invalid entry");
    assert!(error.to_string().contains("a function needs at least one block"), "{error}");
}

#[test]
fn an_entry_without_a_terminator_is_rejected_before_execution() {
    let error = bennett_ir::load("func @main() {\nentry:\n}\n").expect_err("invalid entry");
    assert!(error.to_string().contains("terminator"), "{error}");
}

#[test]
fn invalid_entry_arguments_are_start_errors() {
    let module = load("func @main(i8 %v) {\nentry:\n ret\n}\nfunc @register(q1 %q) {\nentry:\n ret\n}\n");
    assert!(matches!(
        Executor::new(&module, FuncId(0), &[], Options::default()),
        Err(StartError::ArgumentCount { expected: 1, found: 0, .. })
    ));
    assert!(matches!(
        Executor::new(&module, FuncId(0), &[256], Options::default()),
        Err(StartError::ArgumentRange { index: 0, value: 256, .. })
    ));
    assert!(matches!(
        Executor::new(&module, FuncId(1), &[0], Options::default()),
        Err(StartError::RegisterParameter { index: 0, .. })
    ));
}

#[test]
fn an_unallocatable_memory_image_is_a_start_error() {
    let module = load("memory 18446744073709551615\nfunc @main() {\nentry:\n ret\n}\n");
    assert!(matches!(
        Executor::new(&module, FuncId(0), &[], Options::default()),
        Err(StartError::MemoryTooLarge(u64::MAX))
    ));
}
