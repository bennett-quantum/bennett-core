//! Ordinary C that the device program has no instruction for, lowered by the front end: each program
//! (`front/tests/c`) checks its own results and returns 0 or the number of the first check that failed,
//! natively and on the device, from unoptimised and from optimised clang output.

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, assert_agrees_with_native, lower, run, write_c};
use bennett_ir::Fault;

#[test]
fn variadic_functions_read_their_arguments_where_a_native_call_puts_them() {
    assert_agrees_with_native("variadic", 0);
}

#[test]
fn empty_inline_assembly_is_an_optimisation_barrier_and_nothing_else() {
    assert_agrees_with_native("empty_asm", 0);
}

#[test]
fn structs_returned_in_registers_come_back_field_by_field() {
    assert_agrees_with_native("struct_results", 0);
}

#[test]
fn intrinsics_compute_what_the_native_build_computes() {
    assert_agrees_with_native("intrinsics", 0);
}

/// `__builtin_trap()` stops the program with the named fault `unreachable`.
#[test]
fn a_trap_is_a_named_fault() {
    let source =
        write_c("trap", "volatile int n = 3;\nint main(void) {\n  if (n == 3) __builtin_trap();\n  return n;\n}\n");
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = lower(&source, "trap", flags, &[]);
        assert_eq!(run(&module, "main", &[], 0), Outcome::Faulted(Fault::Unreachable), "{flags:?}");
    }
}

#[test]
fn computed_goto_jumps_through_tables_of_label_addresses() {
    assert_agrees_with_native("computed_goto", 0);
}

#[test]
fn integers_wider_than_sixty_four_bits_compute_what_the_native_build_computes() {
    assert_agrees_with_native("wide_integers", 0);
}

#[test]
fn vectors_compute_element_by_element_what_the_native_build_computes() {
    assert_agrees_with_native("vectors", 0);
}

#[test]
fn constant_expressions_on_addresses_are_evaluated_where_the_globals_are() {
    assert_agrees_with_native("constant_expressions", 0);
}

#[test]
fn variable_length_arrays_in_loops_free_their_stack_each_pass() {
    assert_agrees_with_native("vla_loop", 0);
}
