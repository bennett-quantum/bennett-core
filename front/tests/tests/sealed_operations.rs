//! Instructions the language's rules depend on are classified before LLVM's optimiser and sealed for every pass
//! that could move, merge, copy or remove them (`front/src/Classify.h`, `front/src/Passes.h`).

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, lower, lower_text_with_diagnostics, native_build, root, run_with_stats, write_c,
};

/// LLVM's pass machinery and its simplifier utilities, by name. A front source other than `Passes.cpp` that
/// names one could run a simplifier without sealing the classified instructions.
fn names_a_simplifier(identifier: &str) -> bool {
    const UTILITIES: &[&str] = &[
        "simplifyInstruction",
        "SimplifyInstructionsInBlock",
        "simplifyCFG",
        "ConstantFoldInstruction",
        "ConstantFoldTerminator",
        "FoldBranchToCommonDest",
        "FoldSingleEntryPHINodes",
        "MergeBlockIntoPredecessor",
        "TryToSimplifyUncondBranchFromEmptyBlock",
        "promoteLoopAccessesToScalars",
        "hoistRegion",
        "sinkRegion",
    ];
    identifier.ends_with("Pass")
        || identifier.ends_with("PassManager")
        || identifier.ends_with("Pipeline")
        || identifier.contains("PassBuilder")
        || identifier == "addPass"
        || identifier == "registerPass"
        || UTILITIES.contains(&identifier)
}

/// The identifiers of C++ `text` outside `//` comments that name a simplifier.
fn simplifier_names(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in text.lines() {
        let code = line.split("//").next().unwrap_or("");
        for identifier in code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if !identifier.is_empty() && names_a_simplifier(identifier) {
                found.push(identifier.to_string());
            }
        }
    }
    found
}

#[test]
fn the_scan_finds_a_simplifier_run() {
    let witness = "  FunctionPassManager passes; // InstCombinePass in a comment\n  passes.addPass(GVNPass());\n";
    assert_eq!(simplifier_names(witness), ["FunctionPassManager", "addPass", "GVNPass"]);
    assert!(simplifier_names("  simplifyDeep(*copy); // through Passes.h\n").is_empty());
}

#[test]
fn only_the_passes_module_runs_llvm_passes() {
    let sources = root().join("front/src");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&sources).expect("front/src") {
        let path = entry.expect("source entry").path();
        let name = path.file_name().and_then(|name| name.to_str()).expect("file name").to_string();
        if !(name.ends_with(".cpp") || name.ends_with(".h") || name.ends_with(".def")) || name == "Passes.cpp" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read source");
        for identifier in simplifier_names(&text) {
            offenders.push(format!("{name}: {identifier}"));
        }
    }
    assert!(offenders.is_empty(), "run LLVM passes through front/src/Passes.h:\n{}", offenders.join("\n"));
}

const PRELUDE: &str = r#"
#include <stdint.h>
#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>
#define HBIT(q, b) ((void)0)
#define ZBIT(q, b) ((void)0)
#define XBIT(q, b) ((q) ^= 1U << (b))
#define READ(q) (q)
#define READ_BYTES(q, n) (q)
#else
#include "bennett.h"
#define HBIT(q, b) bennett_h(bennett_bit(&(q), (b)))
#define ZBIT(q, b) bennett_z(bennett_bit(&(q), (b)))
#define XBIT(q, b) bennett_x(bennett_bit(&(q), (b)))
#define READ(q) bennett_measure((const void *)&(q))
#define READ_BYTES(q, n) bennett_measure_bytes(&(q), (n))
#endif
"#;

const DRIVER: &str = r#"
#ifdef NATIVE
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    printf("%llu\n", (unsigned long long)entry(strtoul(argv[1], 0, 0), strtoul(argv[2], 0, 0), 0));
}
#endif
"#;

/// The device's result for `entry(prep, n, 0)` in the project's modes (the meaning is promised for C compiled
/// with the project's flags, so not after clang -O2), against native C's.
fn agrees(name: &str, program: &str, inputs: &[(u64, u64)]) {
    let source = write_c(name, &format!("{PRELUDE}{program}{DRIVER}"));
    let executable = native_build(&source, name, &["-DNATIVE"]);
    let mut failures = Vec::new();
    for (mode, flags) in [("o0", &[][..]), ("taint", &["--taint-all"][..])] {
        let module = lower(&source, &format!("{name}-{mode}"), CLANG_FLAGS, flags);
        for &(prep, n) in inputs {
            let output = std::process::Command::new(&executable)
                .args([prep.to_string(), n.to_string()])
                .output()
                .expect("run native C");
            let native: u64 = std::str::from_utf8(&output.stdout).expect("stdout").trim().parse().expect("number");
            let (found, _) = run_with_stats(&module, "entry", &[prep, n, 0], 0);
            if found != Outcome::Returned(Some(u128::from(native))) {
                failures.push(format!("{name}-{mode} prep={prep} n={n}: native {native}, device {found:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `held[0]` holds qubits after `fill`, which runs when `prep` is 1; then `q` is 0. A read of `held[0]` that C
/// guards by `q` must stay under that guard: read first, as the optimiser would, it is a fault on the device
/// where C reads nothing.
const FILL: &str = r#"
uint8_t held[2] = {0, 1};
__attribute__((noinline)) void fill(void) {
    uint8_t r = 0;
    XBIT(r, 0);
    held[0] = r;
    XBIT(r, 0);
}
"#;

fn guarded_read(name: &str, body: &str, result: &str) {
    let program = format!(
        "{FILL}
unsigned long long entry(unsigned prep, unsigned n, unsigned column) {{
    uint8_t q = 0, y = 0, z = 0;
    if (prep == 2) XBIT(q, 0); else if (prep == 0) HBIT(q, 0);
    ZBIT(q, 1);
    if (prep == 1) fill();
    ZBIT(q, 1);
{body}
    unsigned long long key = {result};
    return key | ((unsigned long long)READ(y) << 8) | ((unsigned long long)READ_BYTES(held[0], 1) << 16);
}}
"
    );
    agrees(name, &program, &[(1, 1), (2, 1), (3, 1)]);
}

#[test]
fn a_held_read_under_a_qubit_test_before_a_gate_loop_stays_guarded() {
    guarded_read(
        "guarded-read-before-loop",
        "    if (q && held[0]) { ZBIT(z, 0); } else if (!column) goto done;
#pragma clang loop unroll(disable)
    for (unsigned i = 0; i < 4; ++i) XBIT(y, 0);
done:;",
        "READ(q)",
    );
}

#[test]
fn a_held_read_whose_result_controls_a_gate_loop_stays_guarded() {
    guarded_read(
        "guarded-read-between",
        "    if (q && held[0]) XBIT(z, 0);
    ZBIT(z, 1);
    if (z) {
#pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i) XBIT(y, 0);
    }",
        "READ(q)",
    );
}

#[test]
fn a_held_read_after_a_gate_loop_under_the_same_qubit_stays_guarded() {
    guarded_read(
        "guarded-read-after-loop",
        "    if (q) {
#pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i) XBIT(y, 0);
    }
    if (q && held[0]) XBIT(z, 0);",
        "READ(q)",
    );
}

#[test]
fn a_held_read_guarding_one_gate_stays_guarded() {
    guarded_read(
        "guarded-read-one-gate",
        "    if (q && held[0]) XBIT(z, 0);",
        "READ(q) | ((unsigned long long)READ(z) << 32)",
    );
}

#[test]
fn an_ordinary_guard_keeps_a_product_with_a_held_byte_unread() {
    agrees(
        "guarded-product",
        &format!(
            "{FILL}
__attribute__((noinline)) unsigned masked(unsigned n) {{ if (n) return n * held[0]; return 0; }}
unsigned entry(unsigned prep, unsigned n, unsigned column) {{
    fill();
    unsigned v = masked(n);
    return v + READ_BYTES(held[0], 1);
}}
"
        ),
        &[(1, 0)],
    );
}

#[test]
fn a_call_with_an_argument_that_depends_on_a_qubit_stays_applied() {
    agrees(
        "applied-call-kept",
        r#"
unsigned a, b;
__attribute__((noinline)) static unsigned first(unsigned x) { a = 1; return x; }
__attribute__((noinline)) unsigned second(unsigned x) { b = 1; return x + 1; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    y ^= first(q);
    y ^= second(q);
    unsigned value = READ_BYTES(y, 1);
    return (a == b) * 16u + value;
}
"#,
        &[(1, 1)],
    );
}

#[test]
fn calls_merged_at_the_function_entry_are_refused_by_name() {
    let source = write_c(
        "merged-calls-at-entry",
        &format!(
            "{PRELUDE}
unsigned recorded;
__attribute__((noinline)) unsigned record_call(unsigned x) {{ recorded = 7; return x + 1; }}
unsigned entry(unsigned prep, unsigned n, unsigned column) {{
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    unsigned result;
    if (n) result = record_call(q);
    else result = record_call(2);
    y ^= result;
    return READ_BYTES(y, 1) + 16u * recorded;
}}
"
        ),
    );
    let error = lower_text_with_diagnostics(&source, "merged-calls-at-entry", CLANG_FLAGS, &[])
        .expect_err("the merge at the entry block is not outlined");
    assert!(
        error.contains("a value that depends on a qubit, merged by control flow that does not form a region"),
        "{error}"
    );
}

/// The front's refusal of `program` in the project's modes; it must name `reason`.
fn refused(name: &str, program: &str, reason: &str) {
    let source = write_c(name, &format!("{PRELUDE}{program}"));
    for (mode, flags) in [("o0", &[][..]), ("taint", &["--taint-all"][..])] {
        let error = lower_text_with_diagnostics(&source, &format!("{name}-{mode}"), CLANG_FLAGS, flags)
            .expect_err("refused by name");
        assert!(error.contains(reason), "{name}-{mode}: {error}");
    }
}

/// Calls through a pointer whose argument depends on a qubit come through the optimiser as calls, each with its
/// argument, and the front then refuses them by name (an applied call needs a known callee).
#[test]
fn calls_through_a_pointer_with_an_argument_that_depends_on_a_qubit_are_refused_by_name() {
    refused(
        "applied-indirect-calls",
        r#"
unsigned a, b;
__attribute__((noinline)) static unsigned first(unsigned x) { a = 1; return x; }
__attribute__((noinline)) unsigned second(unsigned x) { b = 1; return x + 1; }
unsigned (*volatile table[2])(unsigned) = {first, second};
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, y = 0;
    unsigned (*f)(unsigned) = table[0], (*g)(unsigned) = table[1];
    XBIT(q, 0);
    y ^= f(q);
    y ^= g(q);
    return READ_BYTES(y, 1) + (a == b) * 16u;
}
"#,
        "a call that depends on a qubit, to a function that performs quantum operations, is not defined, or is called \
         through a pointer",
    );
}

/// Atomic updates of bytes that may hold qubits are classified, sealed for the optimiser and made again; the
/// device program has no atomic update yet, which the front refuses by name.
#[test]
fn atomic_updates_of_bytes_that_may_hold_qubits_come_back_and_are_refused_by_name() {
    refused(
        "atomic-updates",
        &format!(
            "{FILL}
unsigned entry(unsigned prep, unsigned n, unsigned column) {{
    fill();
    unsigned measured = READ_BYTES(held[0], 1);
    __atomic_fetch_add(&held[0], 5, __ATOMIC_SEQ_CST);
    uint8_t expected = 6;
    _Bool swapped = __atomic_compare_exchange_n(&held[0], &expected, 9, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
    return measured + 16u * held[0] + 4096u * swapped;
}}
"
        ),
        "the LLVM instruction `atomicrmw`",
    );
}
