//! Miscompilations found by testing, each kept as a C program (`front/tests/c`) and checked against the
//! natively compiled program where there is one, together with the rest of its class.

use std::process::Command;

use bennett_exec::{Executor, Options, Outcome, RunError};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, assert_agrees_with_native, c_dir, entry, is_fault_register, lower, lower_text,
    lower_text_with_budget, native_build, options, run, write_c,
};
use bennett_ir::{Fault, Inst, ValidModule};

/// Seeds per program, for outcomes that should not depend on the seed.
const SEEDS: u64 = 16;

fn program(name: &str, clang_flags: &[&str]) -> ValidModule {
    let tag = format!("{name}{}", if clang_flags == CLANG_FLAGS { "" } else { ".o2" });
    lower(&c_dir().join(format!("{name}.c")), &tag, clang_flags, &[])
}

/// The fault registers of the `apply`s in `@main`, in order.
fn fault_registers(module: &ValidModule) -> Vec<String> {
    let main = module.function(entry(module, "main"));
    let applies = main.blocks.iter().flat_map(|block| &block.insts);
    applies
        .filter_map(|inst| match inst {
            Inst::Apply { fault, .. } => Some(main.slot_names[fault.index()].clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn faults_of_a_compute_and_its_uncompute_do_not_cancel() {
    let source = c_dir().join("fault_cancel.c");
    let text =
        lower_text_with_budget(&source, "fault_cancel.budget1", CLANG_FLAGS, &[], 1).unwrap_or_else(|e| panic!("{e}"));
    let starved = bennett_ir::load(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let registers = fault_registers(&starved);
    assert!(registers.len() >= 2 && registers.iter().all(|r| is_fault_register(r)), "{registers:?}");
    let mut distinct = registers.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), registers.len(), "every apply has its own fault register: {registers:?}");

    for seed in 0..SEEDS {
        // The simulator sees the fault on both branches and stops the run there.
        let outcome = run(&starved, "main", &[], seed);
        let Outcome::Stopped(RunError::RegionFault { fault, branches, configurations, .. }) = outcome else {
            panic!("seed {seed}: {outcome:?}")
        };
        assert_eq!((fault, branches, configurations), (Fault::Budget, 2, 2), "seed {seed}");
        // Run on regardless, the program keeps the evidence to the end: releasing it faults.
        let options = Options { allow_region_faults: true, ..options(seed) };
        let mut executor = Executor::new(&starved, entry(&starved, "main"), &[], options).expect("starts");
        assert_eq!(executor.run(), Outcome::Faulted(Fault::DirtyFree), "seed {seed}");
        assert_eq!(executor.stats().region_faults, 2, "seed {seed}: both applications faulted");
    }

    // With budget enough the kickback works: q measures 1 on every run.
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = program("fault_cancel", flags);
        for seed in 0..SEEDS {
            assert_eq!(run(&module, "main", &[], seed), Outcome::Returned(Some(1)), "{flags:?}, seed {seed}");
        }
    }
}

/// Inside a loop each execution of an `apply` gets a fresh fault register, released right after it, so
/// nothing can write it in between. Each pass kicks a phase back onto q (H Z H = X) and flips q again.
#[test]
fn faults_inside_a_loop_are_kept_per_execution() {
    let source = write_c(
        "fault_in_loop",
        "#include \"bennett.h\"\nint loop(unsigned char n) {\n  _Bool q = 0, t = 0;\n  for (int i = 0; i < n; i++) {\n    \
         bennett_h(&q);\n    t ^= q;\n    bennett_z(&t);\n    t ^= q;\n    bennett_h(&q);\n    bennett_x(&q);\n  }\n  \
         return (int)bennett_measure(&q);\n}\n",
    );
    let module = lower(&source, "fault_in_loop", CLANG_FLAGS, &[]);
    let text =
        lower_text_with_budget(&source, "fault_in_loop.budget1", CLANG_FLAGS, &[], 1).unwrap_or_else(|e| panic!("{e}"));
    let starved = bennett_ir::load(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let body = starved.function(entry(&starved, "loop"));
    let in_loop = body.blocks.iter().find(|block| block.insts.iter().any(|inst| matches!(inst, Inst::Apply { .. })));
    let insts = &in_loop.expect("a block with an apply").insts;
    for (i, inst) in insts.iter().enumerate() {
        if let Inst::Apply { fault, .. } = inst {
            assert!(matches!(insts[i - 1], Inst::Qalloc { dst, .. } if dst == *fault), "allocated just before\n{text}");
            assert!(
                matches!(insts[i + 1], Inst::Qfree { register } if register == *fault),
                "released just after\n{text}"
            );
        }
    }
    for n in [0, 1, 2, 3] {
        for seed in 0..SEEDS {
            assert_eq!(run(&module, "loop", &[n], seed), Outcome::Returned(Some(0)), "n = {n}, seed {seed}");
            let starved_outcome = run(&starved, "loop", &[n], seed);
            if n == 0 {
                assert_eq!(starved_outcome, Outcome::Returned(Some(0)));
                continue;
            }
            assert!(matches!(starved_outcome, Outcome::Stopped(RunError::RegionFault { fault: Fault::Budget, .. })));
            let options = Options { allow_region_faults: true, ..options(seed) };
            let mut executor = Executor::new(&starved, entry(&starved, "loop"), &[n], options).expect("starts");
            assert_eq!(executor.run(), Outcome::Faulted(Fault::DirtyFree), "n = {n}, seed {seed}");
            assert_eq!(executor.stats().region_faults, 1, "the first faulting execution is caught at once");
        }
    }
}

/// Lowering `source` is refused by name, at a source location, with `expected` in the message.
fn assert_refused(source: &std::path::Path, name: &str, expected: &str) {
    match lower_text(source, name, CLANG_FLAGS, &[]) {
        Ok(program) => panic!("{name}: accepted\n{program}"),
        Err(message) => assert!(
            message.starts_with("not handled yet: ") && message.contains(expected) && message.contains(".c:"),
            "{name}: {message}"
        ),
    }
}

#[test]
fn a_write_to_memory_under_qubit_control_is_refused_wherever_it_hides() {
    assert_refused(&c_dir().join("qubit_controlled_store.c"), "qubit_controlled_store", "a store to memory controlled");
    let prelude = "#include \"bennett.h\"\n#include <string.h>\nint cells[4];\nvolatile int device;\n\
                   char from[64], to[64];\n__attribute__((noinline)) void put(int v) { cells[0] = v; }\n\
                   __attribute__((noinline)) int sum(int *p) { return p[0] + p[1]; }\n";
    let cases = [
        ("call", "if (q) put(7);", "a call to @put, which may write memory, controlled"),
        ("pointer", "int *p = &cells[n & 3]; if (q) *p = 7;", "a store to memory controlled"),
        ("memcpy", "if (q) memcpy(to, from + n, (unsigned)n * 8);", "a store to memory controlled"),
        ("local", "int buf[2] = {0, 0}; if (q) buf[1] = 7; r = sum(buf);", "a store to memory controlled"),
        ("volatile_load", "int y = 0; if (q) y = device; r = y;", "a volatile or atomic load controlled"),
        ("address", "cells[q] = 7;", "used by ordinary code"),
    ];
    for (name, body, expected) in cases {
        let text = format!(
            "{prelude}int main(int n, char **argv) {{\n  (void)argv;\n  int r = 0;\n  _Bool q = 0;\n  \
             bennett_h(&q);\n  {body}\n  bennett_measure(&q);\n  return r + cells[0];\n}}\n"
        );
        let source = write_c(&format!("write_{name}"), &text);
        assert_refused(&source, &format!("write_{name}"), expected);
    }
}

#[test]
fn a_struct_passed_by_value_is_a_private_copy() {
    assert_agrees_with_native("byval_alias", 0);
    assert_agrees_with_native("byval_copy", 24);
}

#[test]
fn allocations_honour_their_alignment() {
    assert_agrees_with_native("alloca_alignment", 0);
    assert_agrees_with_native("alignments", 0);
}

#[test]
fn initialisers_are_written_in_full() {
    assert_agrees_with_native("long_double_initialiser", 255);
    let source = c_dir().join("initialisers.c");
    let native = native_build(&source, "initialisers", &["-DNATIVE"]);
    let output = Command::new(&native).output().unwrap_or_else(|e| panic!("cannot run {native:?}: {e}"));
    assert!(output.status.success(), "native initialisers.c failed");
    let expected: Vec<u64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.parse().unwrap_or_else(|e| panic!("{line:?}: {e}")))
        .collect();
    assert!(expected.len() > 200, "the parts are listed: {} bytes", expected.len());
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = program("initialisers", flags);
        let device: Vec<u64> = (0..expected.len() as u64)
            .map(|i| match run(&module, "byte_at", &[i], 0) {
                Outcome::Returned(Some(value)) => value as u64,
                other => panic!("byte_at({i}): {other:?}"),
            })
            .collect();
        let differ: Vec<usize> = (0..expected.len()).filter(|&i| device[i] != expected[i]).collect();
        assert!(differ.is_empty(), "{flags:?}: bytes {differ:?} differ from the native build");
        assert_eq!(run(&module, "byte_at", &[expected.len() as u64], 0), Outcome::Returned(Some(256)));
    }
}

/// Floating-point arithmetic and the other things this version cannot lower are refused by name.
#[test]
fn floating_point_arithmetic_and_library_calls_are_refused() {
    let cases = [
        ("float_arithmetic", "volatile double a = 1.5;\nint main(void) { return (int)(a * 3.0); }\n", "double"),
        (
            "printf",
            "int printf(const char *, ...);\nint main(int n, char **v) { (void)v; return printf(\"%d\", n); }\n",
            "printf",
        ),
    ];
    for (name, text, expected) in cases {
        let source = write_c(name, text);
        assert_refused(&source, name, expected);
    }
}

#[test]
fn a_call_through_a_mismatched_prototype_is_refused() {
    let source = write_c(
        "mismatched_prototype",
        "long f(long x) { return x; }\nint main(void) { return ((int (*)(int))f)(5) - 5; }\n",
    );
    assert_refused(&source, "mismatched_prototype", "a call through a mismatched prototype of @f");
}

/// LLVM 18's dead-store elimination crashes on a module that defines `calloc` and a `malloc` it cannot inline.
/// The crash is LLVM's; the front must report it by name instead of crashing.
#[test]
fn a_crash_inside_llvms_optimiser_is_reported_by_name() {
    let source = write_c(
        "optimiser_crash",
        "typedef __SIZE_TYPE__ size_t;\nvoid *memset(void *, int, size_t);\nstatic char heap[256];\nsize_t used;\n\
         __attribute__((noinline)) void *malloc(size_t n) { used = 0; return heap + (n & 1); }\n\
         void *calloc(size_t n, size_t s) { (void)n; (void)s; return 0; }\n\
         int main(void) { char *p = malloc(94); memset(p, 0, 94); return p[3]; }\n",
    );
    match lower_text(&source, "optimiser_crash", CLANG_FLAGS, &[]) {
        // A later LLVM may fix its bug; then the program simply lowers.
        Ok(_) => {}
        Err(message) => assert!(message.contains("LLVM's optimiser crashed"), "{message}"),
    }
}

/// A `_Bool` has one bit. Optimised clang input says so on its loads, and a gate on another bit would make
/// LLVM's assumption false.
#[test]
fn a_gate_on_a_bit_the_type_does_not_have_is_refused() {
    let source = write_c(
        "bool_bit",
        "#include \"bennett.h\"\nint main(void) {\n  _Bool b = 0;\n  bennett_h(&b);\n  bennett_x(bennett_bit(&b, 3));\n  \
         _Bool c = 0;\n  c ^= b;\n  bennett_x(bennett_bit(&b, 3));\n  c ^= b;\n  bennett_h(&b);\n  \
         return (int)bennett_measure(&b) + (int)bennett_measure(&c);\n}\n",
    );
    let flags: Vec<&str> = CLANG_FLAGS.iter().copied().filter(|flag| *flag != "-O0").chain(["-O2"]).collect();
    match lower_text(&source, "bool_bit", &flags, &[]) {
        Ok(program) => panic!("accepted\n{program}"),
        Err(message) => assert!(message.contains("whose type does not have that bit"), "{message}"),
    }
}

/// Inline assembly that executes an instruction, or whose output would hold whatever its register held,
/// is refused by name.
#[test]
fn inline_assembly_with_an_effect_is_refused() {
    let cases = [
        ("asm_nop", "int main(void) { __asm__ volatile(\"nop\"); return 0; }\n"),
        ("asm_output", "int main(void) { long x; __asm__(\"\" : \"=r\"(x)); return (int)x; }\n"),
        ("asm_any_output", "int main(void) { long x = 3; __asm__ volatile(\"\" : \"=g\"(x)); return (int)x; }\n"),
    ];
    for (name, text) in cases {
        let source = write_c(name, text);
        assert_refused(&source, name, "inline assembly");
    }
}

/// When one register is left for a variadic `__int128`, the psABI passes it on the stack and leaves the
/// register to the next argument, which is where clang's inline `va_arg` reads it. LLVM's x86-64 backend
/// skips the register instead, so a native build gets this wrong at some optimisation levels; the device
/// program follows the callee, and gets the C answer.
#[test]
fn a_variadic_int128_that_misses_the_last_register_leaves_it_to_the_next_argument() {
    let source = write_c(
        "int128_last_register",
        "#include <stdarg.h>\ntypedef unsigned __int128 u128;\n\
         __attribute__((noinline)) u128 f(int n, ...) {\n  va_list ap;\n  va_start(ap, n);\n  u128 s = 0;\n  \
         for (int i = 0; i < n; i++) s = s * 10 + (unsigned long)va_arg(ap, long);\n  \
         s = s * 1000 + va_arg(ap, u128);\n  s = s * 10 + (unsigned long)va_arg(ap, long);\n  va_end(ap);\n  return s;\n}\n\
         int main(void) {\n  u128 r = f(4, 1L, 2L, 3L, 4L, (u128)99, 5L);\n  \
         return r == ((u128)1234 * 1000 + 99) * 10 + 5 ? 0 : 1;\n}\n",
    );
    for flags in [CLANG_FLAGS, CLANG_O2_FLAGS] {
        let module = lower(&source, "int128_last_register", flags, &[]);
        assert_eq!(run(&module, "main", &[], 0), Outcome::Returned(Some(0)), "{flags:?}");
    }
}
