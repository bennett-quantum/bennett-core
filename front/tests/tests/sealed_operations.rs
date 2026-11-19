//! Instructions the language's rules depend on are classified before LLVM's optimiser and sealed for every pass
//! that could move, merge, copy or remove them (`front/src/Classify.h`, `front/src/Passes.h`).

use std::path::Path;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, front, lower, lower_text_with_diagnostics, native_build, out_dir, root, run_with_stats, write_c,
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

/// Every file under `dir`, recursively, that names a simplifier, except the files `allowed` (by file name).
fn simplifier_offenders(dir: &Path, allowed: &[&str]) -> Vec<String> {
    let mut offenders = Vec::new();
    let mut work = vec![dir.to_path_buf()];
    while let Some(next) = work.pop() {
        for entry in std::fs::read_dir(&next).unwrap_or_else(|e| panic!("{next:?}: {e}")) {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                work.push(path);
                continue;
            }
            let name = path.file_name().and_then(|name| name.to_str()).expect("file name").to_string();
            if allowed.contains(&name.as_str()) {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            for identifier in simplifier_names(&String::from_utf8_lossy(&bytes)) {
                offenders.push(format!("{}: {identifier}", path.display()));
            }
        }
    }
    offenders
}

/// The files that may name LLVM's pass machinery: the one module that seals the classified instructions.
const PASS_MODULE: &[&str] = &["Passes.cpp"];

#[test]
fn the_scan_reads_every_file_whatever_its_extension() {
    let dir = out_dir().join("simplifier-scan-witness");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("detail")).expect("witness directory");
    std::fs::write(dir.join("Passes.cpp"), "ModulePassManager passes;\n").expect("allowed file");
    std::fs::write(dir.join("detail/Cleanup.hpp"), "  ModulePassManager passes;\n").expect("header");
    std::fs::write(dir.join("Normalise.inc"), "  passes.addPass(NoOpModulePass());\n").expect("include");
    let mut found = simplifier_offenders(&dir, PASS_MODULE);
    found.sort();
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found.iter().any(|line| line.contains("Cleanup.hpp: ModulePassManager")), "{found:?}");
    assert!(found.iter().any(|line| line.contains("Normalise.inc: NoOpModulePass")), "{found:?}");
}

#[test]
fn only_the_passes_module_runs_llvm_passes() {
    let mut offenders = simplifier_offenders(&root().join("front/src"), PASS_MODULE);
    offenders.extend(simplifier_offenders(&root().join("front/include"), &[]));
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

/// The two writing helpers of `a_call_with_an_argument_that_depends_on_a_qubit_stays_applied`, with the first
/// argument `q & 0u`. It depends on a qubit by the text, so the call is applied; the optimiser folds the argument
/// to 0. Translated as an ordinary call, `first`'s write survived (native 18, device 2): the class decides, and
/// translating it as applied is not built.
#[test]
fn an_applied_call_whose_argument_the_optimiser_folds_is_refused_by_name() {
    refused(
        "applied-call-folded-argument",
        r#"
unsigned a, b;
__attribute__((noinline)) static unsigned first(unsigned x) { a = 1; return x; }
__attribute__((noinline)) unsigned second(unsigned x) { b = 1; return x + 1; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    y ^= first(q & 0u);
    y ^= second(q);
    return READ_BYTES(y, 1) + (a == b) * 16u;
}
"#,
        "a call with an argument that depends on a qubit before LLVM's optimiser, whose arguments are ordinary after it",
    );
}

/// A variadic argument that depends on a qubit: the front passes it in a caller-owned area, after which the call
/// carried only an ordinary address and was not applied, and the callee's write survived (native 16, device 0).
#[test]
fn a_variadic_argument_that_depends_on_a_qubit_is_refused_by_name() {
    refused(
        "applied-call-variadic-argument",
        r#"
unsigned a, b;
__attribute__((noinline)) static unsigned first(unsigned x, ...) { a = 1; return x; }
__attribute__((noinline)) unsigned second(unsigned x) { b = 1; return x + 1; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    y ^= first(2, q);
    y ^= second(q);
    return READ_BYTES(y, 1) + (a == b) * 16u;
}
"#,
        "a call with a variadic argument, which the front passes in memory, that depends on a qubit",
    );
}

/// A structure passed by value whose field depends on a qubit: the call passes the address of a copy.
#[test]
fn a_structure_passed_by_value_that_depends_on_a_qubit_is_refused_by_name() {
    refused(
        "applied-call-by-value",
        r#"
struct pair { unsigned long long x, y, z; };
unsigned a;
__attribute__((noinline)) static unsigned first(struct pair p) { a = 1; return (unsigned)(p.x + p.z); }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    struct pair p = {q, 2, 3};
    y ^= first(p);
    return READ_BYTES(y, 1) + 16u * a;
}
"#,
        "a call with an argument passed by value, which is a copy in memory, that depends on a qubit",
    );
}

/// `fill` writes a qubit's value through a `restrict` parameter into `held[0]`: the bytes hold qubits after the
/// call, whatever `restrict` says about the call itself. The guarded read in `masked` must stay guarded.
#[test]
fn a_write_through_a_restrict_parameter_keeps_later_guarded_reads_guarded() {
    agrees(
        "restrict-fill-guarded-product",
        r#"
uint8_t held[2] = {0, 1};
__attribute__((noinline)) void fill(uint8_t *restrict dest) {
    uint8_t r = 0;
    XBIT(r, 0);
    dest[0] = r;
    XBIT(r, 0);
}
__attribute__((noinline)) unsigned masked(unsigned n) { if (n) return n * held[0]; return 0; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    fill(held);
    unsigned v = masked(n - 1);
    return v + READ_BYTES(held[0], 1);
}
"#,
        &[(1, 1)],
    );
}

/// The bytes of a qubit variable copied into `held[0]` by memcpy or memmove: they hold qubits after the copy, so
/// a guarded read of them must stay guarded.
fn copied_qubit(name: &str, copy: &str, body: &str, inputs: &[(u64, u64)]) {
    agrees(
        name,
        &format!(
            "
uint8_t held[2] = {{0, 1}};
__attribute__((noinline)) void fill(void) {{
    uint8_t r = 0;
    XBIT(r, 0);
    {copy}(&held[0], &r, 1);
    XBIT(r, 0);
}}
{body}
"
        ),
        inputs,
    );
}

const COPIED_PRODUCT: &str = r#"
__attribute__((noinline)) unsigned masked(unsigned n) { if (n) return n * held[0]; return 0; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    fill();
    unsigned v = masked(n - 1);
    return v + READ_BYTES(held[0], 1);
}
"#;

const COPIED_GUARD: &str = r#"
unsigned long long entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, z = 0;
    if (prep == 2) XBIT(q, 0); else if (prep == 0) HBIT(q, 0);
    ZBIT(q, 1);
    if (prep == 1) fill();
    ZBIT(q, 1);
    if (q && held[0]) XBIT(z, 0);
    return (READ(q) | ((unsigned long long)READ(z) << 32)) | ((unsigned long long)READ_BYTES(held[0], 1) << 16);
}
"#;

#[test]
fn bytes_copied_from_a_qubit_variable_keep_a_guarded_product_guarded() {
    copied_qubit("memcpy-qubit-product", "__builtin_memcpy", COPIED_PRODUCT, &[(1, 1)]);
    copied_qubit("memmove-qubit-product", "__builtin_memmove", COPIED_PRODUCT, &[(1, 1)]);
}

#[test]
fn bytes_copied_from_a_qubit_variable_keep_a_qubit_guard_guarded() {
    copied_qubit("memcpy-qubit-guard", "__builtin_memcpy", COPIED_GUARD, &[(1, 1), (2, 1), (3, 1)]);
    copied_qubit("memmove-qubit-guard", "__builtin_memmove", COPIED_GUARD, &[(1, 1), (2, 1), (3, 1)]);
}

/// The device result of `entry()` in the LLVM IR `text`, translated as handed over (`clang -O0` keeps it).
fn run_ir(name: &str, text: &str) -> Outcome {
    let path = out_dir().join(format!("{name}.ll"));
    std::fs::write(&path, text).expect("write IR");
    let module = lower(&path, name, CLANG_FLAGS, &[]);
    run_with_stats(&module, "entry", &[], 0).0
}

/// A vector of two 24-bit elements takes six bytes in memory: element 1 starts at byte 3 (an i24 alone takes 4).
/// `fill`, never called, makes `@buf` memory that may hold qubits, so the vector access is classified and split.
const VECTOR24: &str = r#"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-linux-gnu"
@buf = global [8 x i8] c"\01\02\03\04\05\06\07\08", align 16
declare void @bennett_x(ptr)
define void @fill() {
  %q = alloca i8
  store i8 0, ptr %q
  call void @bennett_x(ptr %q)
  %x = load i8, ptr %q
  store i8 %x, ptr @buf
  ret void
}
"#;

#[test]
fn a_split_vector_read_finds_each_element_at_its_packed_offset() {
    let text = format!(
        "{VECTOR24}
define i32 @entry() {{
  %v = load <2 x i24>, ptr @buf, align 1
  %e = extractelement <2 x i24> %v, i32 1
  %r = zext i24 %e to i32
  ret i32 %r
}}
"
    );
    assert_eq!(run_ir("vector24-read", &text), Outcome::Returned(Some(0x06_05_04)));
}

#[test]
fn a_split_vector_write_puts_each_element_at_its_packed_offset() {
    let text = format!(
        "{VECTOR24}
define i32 @entry() {{
  store <2 x i24> <i24 657164, i24 855567>, ptr @buf, align 1
  %p = getelementptr inbounds i8, ptr @buf, i64 3
  %e = load i24, ptr %p, align 1
  %r = zext i24 %e to i32
  ret i32 %r
}}
"
    );
    assert_eq!(run_ir("vector24-write", &text), Outcome::Returned(Some(855_567)));
}

/// The lines of LLVM text `text` that define the module's lists of globals kept from deletion.
fn used_lists(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.starts_with("@llvm.used") || line.starts_with("@llvm.compiler.used"))
        .map(String::from)
        .collect()
}

/// A function the module keeps in `llvm.used` only, called applied: sealing the call adds the function to
/// `llvm.compiler.used` for the run and must take away only that entry, leaving `llvm.used` as it was.
#[test]
fn sealing_an_applied_call_leaves_the_used_list_as_it_was() {
    let source = write_c(
        "used-applied-callee",
        &format!(
            "{PRELUDE}
__attribute__((used, noinline)) static unsigned twice(unsigned x) {{ return 2 * x; }}
unsigned entry(unsigned prep, unsigned n, unsigned column) {{
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    y ^= twice(q);
    return READ_BYTES(y, 1);
}}
"
        ),
    );
    // clang puts a `used` function into llvm.compiler.used; the same module with llvm.used instead.
    let include = format!("-I{}", root().join("front/include").display());
    let text = std::process::Command::new(bennett_front_tests::clang())
        .args(CLANG_FLAGS)
        .arg(&include)
        .args(["-emit-llvm", "-S", "-o", "-"])
        .arg(&source)
        .output()
        .expect("clang");
    assert!(text.status.success(), "{}", String::from_utf8_lossy(&text.stderr));
    let text = String::from_utf8_lossy(&text.stdout).replace("@llvm.compiler.used", "@llvm.used");
    assert_eq!(used_lists(&text), ["@llvm.used = appending global [1 x ptr] [ptr @twice], section \"llvm.metadata\""]);
    let input = out_dir().join("used-applied-callee.ll");
    std::fs::write(&input, &text).expect("write IR");
    let emitted = out_dir().join("used-applied-callee.emitted.ll");
    let status = std::process::Command::new(front())
        .arg(&input)
        .arg("-o")
        .arg(out_dir().join("used-applied-callee.bn"))
        .arg("--stack-bytes=65536")
        .arg("--region-budget=100000")
        .arg("--emit-llvm")
        .arg(&emitted)
        .status()
        .expect("run the front");
    assert!(status.success());
    assert_eq!(used_lists(&std::fs::read_to_string(&emitted).expect("emitted LLVM")), used_lists(&text));
}

/// The two writing helpers, `first` and `second`, of the applied-call tests, and a helper that returns its
/// argument: a qubit variable's address that comes back from a call is no longer one the front follows.
const TWO_WRITERS: &str = r#"
unsigned a, b;
__attribute__((noinline)) static unsigned first(unsigned x, ...) { a = 1; return x; }
__attribute__((noinline)) unsigned second(unsigned x) { b = 1; return x + 1; }
__attribute__((noinline)) static uint8_t *id(uint8_t *p) { return p; }
"#;

#[test]
fn a_qubit_read_through_a_returned_address_is_a_variadic_argument_refused_by_name() {
    refused(
        "returned-address-variadic",
        &format!(
            "{TWO_WRITERS}
unsigned entry(unsigned prep, unsigned n, unsigned column) {{
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    y ^= first(2, *id(&q));
    y ^= second(q);
    return READ_BYTES(y, 1) + (a == b) * 16u;
}}
"
        ),
        "a call with a variadic argument, which the front passes in memory, that depends on a qubit",
    );
}

/// `held[0] = *id(&r)` stores the value of qubit variable `r` read through an address the front cannot follow:
/// the byte holds qubits after it, and the guarded read in `masked` stays guarded (native 1; fault 17 before).
#[test]
fn a_store_of_a_qubit_read_through_a_returned_address_keeps_a_guarded_read_guarded() {
    agrees(
        "returned-address-store",
        r#"
__attribute__((noinline)) static uint8_t *id(uint8_t *p) { return p; }
uint8_t held[2] = {0, 1};
__attribute__((noinline)) void fill(void) {
    uint8_t r = 0;
    XBIT(r, 0);
    uint8_t *src = id(&r);
    held[0] = *src;
    XBIT(r, 0);
}
__attribute__((noinline)) unsigned masked(unsigned n) { if (n) return n * held[0]; return 0; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    fill();
    unsigned v = masked(n - 1);
    return v + READ_BYTES(held[0], 1);
}
"#,
        &[(1, 1)],
    );
}

/// memcpy of one byte out of a qubit variable through a returned address: rewritten into a load and a store, as
/// InstCombine would rewrite it, both classified; the guarded read stays guarded (native 1; it faulted before).
#[test]
fn a_copy_out_of_a_qubit_variable_through_a_returned_address_keeps_a_guarded_read_guarded() {
    agrees(
        "returned-address-copy",
        r#"
__attribute__((noinline)) static uint8_t *id(uint8_t *p) { return p; }
uint8_t held[2] = {0, 1};
__attribute__((noinline)) void fill(void) {
    uint8_t r = 0;
    XBIT(r, 0);
    __builtin_memcpy(&held[0], id(&r), 1);
    XBIT(r, 0);
}
__attribute__((noinline)) unsigned masked(unsigned n) { if (n) return n * held[0]; return 0; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    fill();
    unsigned v = masked(n - 1);
    return v + READ_BYTES(held[0], 1);
}
"#,
        &[(1, 1)],
    );
}

/// `relay(&q)` returns `first(*p & 0u)`: the call in the helper is applied by its class (native 18; 2 before, an
/// ordinary call). The helper reads the qubit variable through its parameter, which the front refuses by name.
#[test]
fn an_applied_call_in_a_helper_given_a_qubit_variable_is_refused_by_name() {
    refused(
        "relay-folded-argument",
        r#"
unsigned a, b;
__attribute__((noinline)) static unsigned first(unsigned x) { a = 1; return x; }
__attribute__((noinline)) unsigned second(unsigned x) { b = 1; return x + 1; }
__attribute__((noinline)) static unsigned relay(uint8_t *p) { return first(*p & 0u); }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    y ^= relay(&q);
    y ^= second(q);
    return READ_BYTES(y, 1) + (a == b) * 16u;
}
"#,
        "qubit variable 'q' is passed to a function",
    );
}

/// A helper the control program calls as ordinary code, holding a call classified as applied: `held[0]` may hold
/// qubits (`fill`), so the argument read from it depends on a qubit by the classification, which does not follow
/// the measurement before the call (a debt, docs/DESIGN.md "Not yet"); the optimiser folds the argument to 0, and
/// the helper runs as ordinary code. Refused by name, never emitted as an ordinary call.
#[test]
fn an_applied_call_in_a_helper_run_as_ordinary_code_is_refused_by_name() {
    refused(
        "ordinary-helper-applied-call",
        &format!(
            "{FILL}
unsigned a;
__attribute__((noinline)) static unsigned first(unsigned x) {{ a = 1; return x; }}
__attribute__((noinline)) static unsigned relay(void) {{ return first(held[0] & 0u); }}
unsigned entry(unsigned prep, unsigned n, unsigned column) {{
    fill();
    unsigned measured = READ_BYTES(held[0], 1);
    unsigned r = relay();
    return measured + 2u * r + 16u * a;
}}
"
        ),
        "which runs as ordinary code",
    );
}

/// `if (q) t = n; else t = n;` then `first(t)`: by the text `t` depends on a qubit after the choice, so the call is
/// applied (native 19). SROA, before the classification, folded the join to `n` and the call was ordinary (3);
/// the join is kept for the classification now, and the optimiser's fold is refused by name.
#[test]
fn a_join_of_equal_values_under_a_qubit_keeps_the_call_applied_or_refused() {
    refused(
        "join-of-equal-values",
        r#"
unsigned a, b;
__attribute__((noinline)) static unsigned first(unsigned x) { a = 1; return x; }
__attribute__((noinline)) unsigned second(unsigned x) { b = 1; return x + 1; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    unsigned t;
    if (q) t = n; else t = n;
    y ^= first(t);
    y ^= second(q);
    return READ_BYTES(y, 1) + (a == b) * 16u;
}
"#,
        "a call with an argument that depends on a qubit before LLVM's optimiser, whose arguments are ordinary after it",
    );
}

/// A private global whose address never escapes is not reached by the pointer parameter of an exported writer
/// that is never called: the variadic argument read from it is ordinary (native 5; refused before).
#[test]
fn a_private_global_whose_address_stays_in_the_module_is_not_reached_by_an_unknown_pointer() {
    agrees(
        "private-global-variadic",
        r#"
static unsigned ordinary;
unsigned char held[1];
__attribute__((noinline)) void fill(unsigned char *restrict dst) {
    unsigned char r = 0;
    XBIT(r, 0);
    *dst = r;
    XBIT(r, 0);
}
__attribute__((noinline)) static unsigned first(unsigned x, ...) { return x + 2; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    ordinary = n;
    return first(3, ordinary);
}
"#,
        &[(1, 1)],
    );
}

/// A call classified as applied in code the optimiser proves dead (`n == 3 && n == 5`): the optimiser removes the
/// call and then the declaration that stood for it. The front refuses the removed instruction by name, every time;
/// it read the deleted declaration before (a crash in most runs), so the program is lowered several times.
#[test]
fn a_sealed_call_the_optimiser_removes_is_refused_by_name_every_time() {
    let program = r#"
unsigned a, b;
__attribute__((noinline)) static unsigned first(unsigned x) { a = 1; return x; }
__attribute__((noinline)) unsigned second(unsigned x) { b = 1; return x + 1; }
unsigned entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    if (n == 3 && n == 5) y ^= first(q);
    y ^= second(q);
    return READ_BYTES(y, 1) + (a == b) * 16u;
}
"#;
    const RUNS: usize = 8;
    for _ in 0..RUNS {
        refused(
            "removed-sealed-call",
            program,
            "LLVM's optimiser removed an instruction whose class the front decided",
        );
    }
}
