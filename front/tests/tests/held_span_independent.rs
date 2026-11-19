//! Independent checks of byte ownership, exchange at function exits, and overwrite coherence.

use std::collections::BTreeMap;
use std::process::Command;

use bennett_exec::{Executor, Outcome, Snapshot};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, bitcode, c_dir, clang, entry, front, lower, lower_text, options, out_dir, root, run,
    run_with_stats, state_before_first_measurement, write_c,
};
use bennett_ir::{Inst, ValidModule};
use num_complex::Complex64 as C;

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];
const S: f64 = std::f64::consts::FRAC_1_SQRT_2;

fn image_address(text: &str) -> u64 {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "; @image" {
            return lines
                .next()
                .expect("image data line")
                .split_whitespace()
                .nth(1)
                .expect("address")
                .parse()
                .expect("numeric image address");
        }
    }
    panic!("the device program has no @image");
}

fn read_image(state: &Snapshot, values: &[u64], address: u64) -> u64 {
    (0..7).fold(0, |word, i| {
        let at = address + i;
        let byte = state
            .quantum
            .cells
            .iter()
            .find(|(cell, _)| *cell == at)
            .map_or_else(|| u64::from(state.classical.memory[at as usize]), |(_, register)| values[*register]);
        word | byte << (8 * i)
    })
}

fn register_column(module: &ValidModule, state: &Snapshot, name: &str) -> usize {
    let frame = state.classical.frames.last().expect("live frame");
    let slot = module.function(frame.func).slot_names.iter().position(|s| s == name).expect("named register slot");
    state.classical.slots[frame.base + slot] as u32 as usize
}

fn measurement_count(module: &ValidModule) -> usize {
    module
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.insts)
        .filter(|inst| matches!(inst, Inst::Measure { .. }))
        .count()
}

#[test]
fn global_and_pointer_memops_reach_the_caller_with_native_bytes_and_exact_phases() {
    let source = c_dir().join("held_span_native.c");
    let native = out_dir().join("held-span-native");
    let output = Command::new(clang())
        .args(["-O0", "-DNATIVE"])
        .arg(&source)
        .arg("-o")
        .arg(&native)
        .output()
        .expect("run native clang");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    for (mode, clang_flags, flags) in MODES {
        let text = lower_text(&source, &format!("held-span-memops-{mode}"), clang_flags, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid program");
        assert_eq!(measurement_count(&module), 1, "{mode}: one explicit measurement in the caller");
        let address = image_address(&text);
        for pointer in 0..=1 {
            let definite = [0, 1].map(|prep| {
                let output = Command::new(&native)
                    .args([prep.to_string(), pointer.to_string()])
                    .output()
                    .expect("run native fixture");
                assert!(output.status.success());
                let expected: u64 =
                    std::str::from_utf8(&output.stdout).expect("native output").trim().parse().expect("numeric image");
                let mut executor = Executor::new(&module, entry(&module, "entry"), &[prep, pointer], options(0))
                    .expect("start device fixture");
                assert_eq!(
                    executor.run(),
                    Outcome::Returned(Some(expected.into())),
                    "{mode}, prep {prep}, pointer {pointer}"
                );
                assert_eq!(
                    executor.stats().nonzero_releases,
                    prep,
                    "{mode}, prep {prep}, pointer {pointer}: only the writer's input may be released nonzero"
                );
                let state = executor.snapshot();
                assert!(state.quantum.cells.is_empty(), "the full measurement gives the bytes back to ordinary memory");
                assert_eq!(read_image(&state, &[], address), expected);
                expected
            });
            let state = state_before_first_measurement(&module, "entry", &[2, pointer]);
            assert_eq!(state.classical.frames.len(), 1, "{mode}: the writer has returned");
            let held: Vec<_> = state.quantum.cells.iter().map(|(at, _)| *at).collect();
            assert_eq!(held, (address + 1..address + 6).collect::<Vec<_>>(), "{mode}, pointer {pointer}");
            let expected = BTreeMap::from([(definite[0], C::new(S, 0.0)), (definite[1], C::new(0.0, S))]);
            let mut observed = BTreeMap::new();
            for (values, amplitude) in &state.quantum.amplitudes {
                let memory = read_image(&state, values, address);
                assert!(observed.insert(memory, *amplitude).is_none(), "{mode}: duplicate image {memory:#x}");
            }
            assert_eq!(
                observed.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>(),
                "{mode}, pointer {pointer}"
            );
            for (key, amplitude) in observed {
                assert!(
                    (amplitude - expected[&key]).norm() < 1e-12,
                    "{mode}, pointer {pointer}, {key:#x}: {amplitude}"
                );
            }
        }
    }
}

#[test]
fn a_span_threaded_through_an_ordinary_condition_is_refused() {
    let source = write_c(
        "held-span-reborrow",
        r#"#include "bennett.h"
unsigned char a = 3;
unsigned entry(unsigned bit) {
    _Bool q = 0;
    if (bit) bennett_x(&q);
    if (q) a = 6;
    unsigned old = bennett_measure_bytes(&a, 1);
    a += 7;
    if (q) a += 2;
    unsigned now = bennett_measure_bytes(&a, 1);
    return (old << 16) | (now << 8) | a;
}
"#,
    );
    // LLVM threads `if (bit)` into `if (q)`: on the path with bit = 0 the store is skipped, and the exchange would
    // have to move above `if (bit)` for the paths to agree (main gives C's values: route: a store of every byte on
    // every path after the condition, or a measurement before the return, each with its proof; docs/DESIGN.md,
    // "Not yet").
    for (mode, clang_flags, flags) in MODES {
        let error = lower_text(&source, &format!("held-span-reborrow-{mode}"), clang_flags, flags).expect_err(mode);
        assert!(
            error.contains("whose exchange would have to move earlier than the store that needs it"),
            "{mode}: {error}"
        );
    }
}

#[test]
fn memory_overwrites_keep_only_the_unrecoverable_old_value() {
    let updates = [
        "if (q) a = 0xa2; bennett_s(&q);",
        "if (q) a += 0x39; bennett_s(&q); if (q) a -= 0x39;",
        "if (q) a = 0xa2; bennett_s(&q); bennett_h(&r); if (r) a = 0xc3;",
    ];
    for (case, update) in updates.into_iter().enumerate() {
        let source = write_c(
            &format!("held-span-overwrites-{case}"),
            &format!(
                r#"#include "bennett.h"
unsigned char a = 0x41;
unsigned entry(void) {{
    _Bool q = 0, r = 0;
    bennett_h(&q);
    {update}
    unsigned byte = bennett_measure_bytes(&a, 1);
    return byte | (unsigned)bennett_measure(&q) << 8 | (unsigned)bennett_measure(&r) << 9;
}}
"#
            ),
        );
        for (mode, clang_flags, flags) in MODES {
            let name = format!("held-span-overwrites-{case}-{mode}");
            let include = format!("-I{}", root().join("front/include").display());
            let clang_flags: Vec<_> = clang_flags.iter().copied().chain([include.as_str()]).collect();
            let bc = bitcode(&source, &name, &clang_flags);
            let output = Command::new(front())
                .args(["--stack-bytes=65536", "--region-budget=512"])
                .args(flags)
                .arg(bc)
                .output()
                .expect("run front");
            let diagnostics = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{mode}: {diagnostics}");
            let kept: Vec<_> = diagnostics.lines().filter(|line| line.contains("keeps the old value")).collect();
            assert_eq!(kept.len(), usize::from(case == 2), "{mode}, case {case}: {diagnostics}");
            if let Some(warning) = kept.first() {
                assert!(warning.contains("in 8 qubits each time it runs"), "{mode}: {warning}");
            }
            let module =
                bennett_ir::load(std::str::from_utf8(&output.stdout).expect("device text")).expect("valid program");
            assert_eq!(measurement_count(&module), 3);
            let state = state_before_first_measurement(&module, "entry", &[]);
            let cell = state.quantum.cells[0].1;
            let columns = ["q", "r"].map(|name| register_column(&module, &state, name));
            let mut seen = BTreeMap::new();
            for (values, amplitude) in &state.quantum.amplitudes {
                let [q, r] = columns.map(|i| values[i]);
                let expected = if q == 0 { C::new(1.0, 0.0) } else { C::i() } * if case == 2 { 0.5 } else { S };
                let byte = match (case, q, r) {
                    (2, _, 1) => 0xc3,
                    (1, _, _) | (_, 0, _) => 0x41,
                    _ => 0xa2,
                };
                assert_eq!(values[cell], byte, "{mode}, case {case}, q {q}, r {r}");
                assert!((*amplitude - expected).norm() < 1e-12, "{mode}, case {case}, q {q}, r {r}: {amplitude}");
                assert!(seen.insert((q, r), byte).is_none());
            }
            assert_eq!(seen.len(), if case == 2 { 4 } else { 2 });
            let (_, stats) = run_with_stats(&module, "entry", &[], 0);
            assert_eq!(stats.leftover_qubits, if case == 2 { 8 } else { 0 }, "{mode}, case {case}");
        }
    }
}

#[test]
fn partial_access_is_named_and_disjoint_bytes_stay_ordinary() {
    let source = write_c(
        "held-span-overlap",
        r#"#include "bennett.h"
union { unsigned short w; unsigned char b[2]; } a = {513};
unsigned entry(void) {
    _Bool q = 0; bennett_h(&q);
    if (q) a.b[0] = 5;
#ifdef PARTIAL
    return a.w;
#else
    return a.b[1] | (unsigned)bennett_measure_bytes(a.b, 1) << 8;
#endif
}
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-disjoint-{mode}"), clang_flags, flags);
        assert!(
            matches!(run_with_stats(&module, "entry", &[], 0).0, Outcome::Returned(Some(0x0102 | 0x0502))),
            "{mode}"
        );
        let partial: Vec<_> = clang_flags.iter().copied().chain(["-DPARTIAL"]).collect();
        let error =
            lower_text(&source, &format!("held-span-overlap-{mode}"), &partial, flags).expect_err("partial overlap");
        assert!(error.starts_with("not handled yet:") && error.contains("only in part"), "{mode}: {error}");
    }
}

#[test]
fn a_partially_measured_span_allows_ordinary_reads_of_the_measured_byte() {
    let source = write_c(
        "held-span-partial-read",
        r#"#include "bennett.h"
unsigned char a[2] = {1, 2};
unsigned entry(void) {
    _Bool q = 0; bennett_h(&q);
    if (q) { a[0] = 5; a[1] = 7; }
    bennett_measure_bytes(a, 1);
    return a[0];
}
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-partial-read-{mode}"), clang_flags, flags);
        // q is left as the measurement found it and released with the other byte's qubits: not zero when q = 1.
        for seed in 0..2 {
            let (outcome, _) = run_with_stats(&module, "entry", &[], seed);
            assert!(matches!(outcome, Outcome::Returned(Some(1 | 5))), "{mode}: {outcome:?}");
        }
    }
}

#[test]
fn a_direct_callee_cannot_read_a_held_global_with_ordinary_code() {
    let source = write_c(
        "held-span-direct-global-read",
        r#"#include "bennett.h"
unsigned char a = 3;
__attribute__((noinline)) unsigned read_a(void) { return a; }
unsigned entry(void) {
    _Bool q = 0; bennett_h(&q);
    if (q) a = 5;
    return read_a();
}
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        match lower_text(&source, &format!("held-span-direct-global-read-{mode}"), clang_flags, flags) {
            Err(error) => assert!(error.contains("bennett_measure_bytes"), "{mode}: {error}"),
            Ok(text) => {
                let module = bennett_ir::load(&text).expect("valid program");
                let (outcome, _) = run_with_stats(&module, "entry", &[], 0);
                panic!("{mode}: front accepted an ordinary read of held bytes; executor returned {outcome:?}");
            }
        }
    }
}

#[test]
fn two_pointer_arguments_can_write_the_same_definite_byte() {
    // A function the host may call can be given the same byte twice: refused by name, with the route.
    let source = write_c(
        "held-span-alias",
        r#"#include "bennett.h"
unsigned char a = 3;
__attribute__((noinline)) void write_byte(unsigned char *p, unsigned char *r) {
    _Bool q = 0; bennett_x(&q);
    if (q) { *p = 5; *r = 7; }
}
unsigned entry(void) { write_byte(&a, &a); return bennett_measure_bytes(&a, 1); }
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let error = lower_text(&source, &format!("held-span-alias-{mode}"), clang_flags, flags).expect_err("aliasing");
        assert!(
            error.starts_with("not handled yet:")
                && error.contains("may reach the same bytes")
                && error.contains("route"),
            "{mode}: {error}"
        );
    }
    // A static function whose only call passes the same pointer twice: one object, one shadow, C's last write. One
    // whose calls pass distinct objects, or `restrict` pointers: one shadow each.
    let cases = [
        ("same", "static", "unsigned char *p, unsigned char *r", "write_byte(&a, &a)", 7, 3),
        ("distinct", "static", "unsigned char *p, unsigned char *r", "write_byte(&a, &b)", 5, 7),
        ("restrict", "", "unsigned char *restrict p, unsigned char *restrict r", "write_byte(&a, &b)", 5, 7),
    ];
    for (kind, linkage, params, call, first, second) in cases {
        let name = format!("held-span-alias-{kind}");
        let source = write_c(
            &name,
            &format!(
                r#"#include "bennett.h"
unsigned char a = 3, b = 3;
__attribute__((noinline)) {linkage} void write_byte({params}) {{
    _Bool q = 0; bennett_x(&q);
    if (q) {{ *p = 5; *r = 7; }}
}}
unsigned entry(void) {{
    {call};
    return bennett_measure_bytes(&a, 1) | bennett_measure_bytes(&b, 1) << 8;
}}
"#
            ),
        );
        for (mode, clang_flags, flags) in MODES {
            let module = lower(&source, &format!("{name}-{mode}"), clang_flags, flags);
            assert_eq!(
                run_with_stats(&module, "entry", &[], 0).0,
                Outcome::Returned(Some(first | second << 8)),
                "{kind}, {mode}"
            );
        }
    }
}

#[test]
fn an_overwrite_can_clear_old_bytes_computed_from_an_unchanged_register() {
    let source = write_c(
        "held-span-recomputable",
        r#"#include "bennett.h"
unsigned char a = 3;
unsigned entry(void) {
    _Bool q = 0, marker = 0;
    bennett_h(&q);
    if (q) a = 5;
    bennett_z(&marker);
    a = 3;
    bennett_h(&q);
    unsigned byte = bennett_measure_bytes(&a, 1);
    return byte | (unsigned)bennett_measure(&q) << 8;
}
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-recomputable-{mode}"), clang_flags, flags);
        let state = state_before_first_measurement(&module, "entry", &[]);
        let q = register_column(&module, &state, "q");
        let probability_one: f64 = state
            .quantum
            .amplitudes
            .iter()
            .filter(|(values, _)| values[q] == 1)
            .map(|(_, amplitude)| amplitude.norm_sqr())
            .sum();
        let (outcome, stats) = run_with_stats(&module, "entry", &[], 0);
        assert_eq!(outcome, Outcome::Returned(Some(3)), "{mode}");
        assert!(
            probability_one < 1e-12 && stats.leftover_qubits == 0,
            "{mode}: after the final H: P(q=1)={probability_one:.12}, retained qubits={}",
            stats.leftover_qubits
        );
    }
}

#[test]
fn measuring_one_held_byte_leaves_the_other_held_until_its_own_marker() {
    let source = write_c(
        "held-span-two-markers",
        r#"#include "bennett.h"
unsigned char image[7] = {1, 2, 3, 4, 5, 6, 7};
unsigned entry(void) {
    _Bool q = 0;
    bennett_h(&q);
    if (q) { image[2] = 51; image[3] = 74; }
    unsigned low = bennett_measure_bytes(image + 2, 1);
    unsigned high = bennett_measure_bytes(image + 3, 1);
    return low | high << 8;
}
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let text = lower_text(&source, &format!("held-span-two-markers-{mode}"), clang_flags, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid program");
        assert_eq!(measurement_count(&module), 2);
        let address = image_address(&text);
        for seed in 0..2 {
            let mut executor =
                Executor::new(&module, entry(&module, "entry"), &[], options(seed)).expect("start device fixture");
            loop {
                let state = executor.snapshot();
                let frame = state.classical.frames.last().expect("live frame");
                let inst = module.function(frame.func).block(frame.block).insts.get(frame.inst);
                if matches!(inst, Some(Inst::Measure { .. })) {
                    assert_eq!(
                        state.quantum.cells.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
                        [address + 2, address + 3]
                    );
                    assert_eq!(executor.step(), None);
                    break;
                }
                assert_eq!(executor.step(), None);
            }
            let state = executor.snapshot();
            assert_eq!(state.quantum.cells.len(), 1);
            assert_eq!(state.quantum.cells[0].0, address + 3);
            let old = state.classical.memory[address as usize + 2];
            let high = match old {
                3 => 4,
                51 => 74,
                value => panic!("{mode}: unexpected ordinary byte {value}"),
            };
            assert_eq!(executor.run(), Outcome::Returned(Some(u128::from(old) | high << 8)));
            let state = executor.snapshot();
            assert!(state.quantum.cells.is_empty());
            assert_eq!(u128::from(state.classical.memory[address as usize + 3]), high);
        }
    }
}

#[test]
fn unsupported_span_shapes_report_the_reason() {
    let cases = [
        ("wide", "p[0] = 9; p[8] = 7;", "more than one register holds"),
        ("unnamed", "*(choose ? p : r) = 9;", "object the front cannot name"),
        ("gate", "*p = 9; bennett_h(&y);", "in code that also has gates"),
        ("unmeasured", "*p = 9;", "measure the bytes first, with bennett_measure_bytes"),
    ];
    for (kind, write, reason) in cases {
        let name = format!("held-span-refuse-{kind}");
        let ret = if kind == "unmeasured" { "return *p;" } else { "return 0;" };
        let source = write_c(
            &name,
            &format!(
                r#"#include "bennett.h"
unsigned entry(unsigned char *p, unsigned char *r, unsigned choose) {{
    _Bool q = 0, y = 0; bennett_h(&q);
    if (q) {{ {write} }}
    {ret}
}}
"#
            ),
        );
        let error = lower_text(&source, &name, CLANG_FLAGS, &[]).expect_err(kind);
        assert!(error.starts_with("not handled yet:") && error.contains(reason), "{kind}: {error}");
    }
}

#[test]
fn an_ordinary_selector_can_end_the_span_at_a_marker_or_a_return() {
    let source = write_c(
        "held-span-marker-or-return",
        r#"#include "bennett.h"
unsigned char a = 3;
unsigned entry(unsigned measure) {
    _Bool q = 0; bennett_h(&q);
    if (q) a = 5;
    if (measure) {
        unsigned old = bennett_measure_bytes(&a, 1);
        return old + a;
    }
    return 17;
}
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-marker-or-return-{mode}"), clang_flags, flags);
        assert_eq!(run_with_stats(&module, "entry", &[0], 0).0, Outcome::Returned(Some(17)), "{mode}");
        for seed in 0..2 {
            let outcome = run_with_stats(&module, "entry", &[1], seed).0;
            assert!(matches!(outcome, Outcome::Returned(Some(6 | 10))), "{mode}: {outcome:?}");
        }
    }
}

#[test]
fn a_measured_local_leaves_no_qubits_in_the_stack_at_return() {
    let source = write_c(
        "held-span-local-return",
        r#"#include "bennett.h"
__attribute__((noinline)) unsigned local(void) {
    unsigned char a[2] = {3, 5};
    _Bool q = 0; bennett_h(&q);
    if (q) a[1] = 7;
    unsigned value = bennett_measure_bytes(a + 1, 1);
    return a[0] | (unsigned)a[1] << 8 | value << 16;
}
unsigned entry(void) { return local(); }
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-local-return-{mode}"), clang_flags, flags);
        assert_eq!(measurement_count(&module), 1);
        for seed in 0..2 {
            let mut executor =
                Executor::new(&module, entry(&module, "entry"), &[], options(seed)).expect("start device fixture");
            let outcome = executor.run();
            assert!(matches!(outcome, Outcome::Returned(Some(0x05_05_03 | 0x07_07_03))), "{mode}: {outcome:?}");
            assert!(executor.snapshot().quantum.cells.is_empty(), "{mode}: a byte still holds qubits after return");
        }
    }
}

#[test]
fn two_objects_written_in_one_region_get_every_bit_of_their_change() {
    // One region writing two objects returns one word of results; each field drives the bits its object may hold:
    // those of the old value its shadow takes, and those of a store into part of it at that part's bytes.
    let source = write_c(
        "held-span-two-objects",
        r#"#include "bennett.h"
unsigned char a[2] = {3, 3}, b = 3;
unsigned entry(unsigned bit) {
    _Bool q = 0;
    if (bit) bennett_x(&q);
    if (q) { a[0] = 1; a[1] = 0x45; b = 7; }
    return bennett_measure_bytes(a, 2) | bennett_measure_bytes(&b, 1) << 16;
}
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-two-objects-{mode}"), clang_flags, flags);
        assert_eq!(run(&module, "entry", &[0], 0), Outcome::Returned(Some(0x03_0303)), "{mode}");
        assert_eq!(run_with_stats(&module, "entry", &[1], 0).0, Outcome::Returned(Some(0x07_4501)), "{mode}");
    }
}

#[test]
fn an_ordinary_access_through_a_pointer_that_may_alias_held_bytes_is_refused() {
    // In `entry`, which the host may call with `r` equal to `p`, reading `*r` as ordinary memory while `p`'s byte
    // holds qubits could fault: refused by name. In the static `put`, whose only call passes `&a`, `p` never points
    // to `other`, which stays ordinary.
    let source = write_c(
        "held-span-may-alias",
        r#"#include "bennett.h"
unsigned char a = 3, other = 9;
__attribute__((noinline)) static unsigned put(unsigned char *p) {
    _Bool q = 0; bennett_x(&q);
    if (q) *p = 5;
    unsigned seen = other;
    return seen | (unsigned)bennett_measure_bytes(p, 1) << 8;
}
#ifdef ALIAS
unsigned entry(unsigned char *p, unsigned char *r) {
    _Bool q = 0; bennett_x(&q);
    if (q) *p = 5;
    unsigned seen = *r;
    return seen | (unsigned)bennett_measure_bytes(p, 1) << 8;
}
#else
unsigned entry(void) { return put(&a); }
#endif
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-may-alias-{mode}"), clang_flags, flags);
        assert_eq!(run_with_stats(&module, "entry", &[], 0).0, Outcome::Returned(Some(0x0509)), "{mode}");
        let alias: Vec<_> = clang_flags.iter().copied().chain(["-DALIAS"]).collect();
        let error =
            lower_text(&source, &format!("held-span-may-alias-r-{mode}"), &alias, flags).expect_err("may alias");
        assert!(
            error.starts_with("not handled yet:") && error.contains("may reach") && error.contains("route"),
            "{mode}: {error}"
        );
    }
}

#[test]
fn an_overwrite_after_its_source_register_changed_keeps_the_old_value() {
    // The old byte was computed from q, and q has changed since: it cannot be computed again, so it is retained.
    let source = write_c(
        "held-span-not-recomputable",
        r#"#include "bennett.h"
unsigned char a = 3;
unsigned entry(void) {
    _Bool q = 0;
    bennett_h(&q);
    if (q) a = 5;
    bennett_h(&q);
    a = 3;
    unsigned byte = bennett_measure_bytes(&a, 1);
    return byte | (unsigned)bennett_measure(&q) << 8;
}
"#,
    );
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-not-recomputable-{mode}"), clang_flags, flags);
        let (outcome, stats) = run_with_stats(&module, "entry", &[], 0);
        assert!(matches!(outcome, Outcome::Returned(Some(3 | 0x103))), "{mode}: {outcome:?}");
        assert_eq!(stats.leftover_qubits, 8, "{mode}");
    }
}

#[test]
fn a_controlled_overwrite_reading_a_recomputable_old_value_keeps_the_interference() {
    // `if (r) a = 7` stores `r ? a : 7`: the old byte, a function of the unchanged q, is cleared by computing it
    // again, and the read of it in the new value is that computation. Where r = 1 the byte is 7 for both values
    // of q, so the final H returns q to 0 there; where r = 0 q stays entangled with the byte.
    let source = write_c(
        "held-span-recomputable-controlled",
        r#"#include "bennett.h"
unsigned char a = 3;
unsigned entry(void) {
    _Bool q = 0, r = 0;
    bennett_h(&q);
    if (q) a = 5;
    bennett_h(&r);
    if (r) a = 7;
    bennett_h(&q);
    unsigned byte = bennett_measure_bytes(&a, 1);
    return byte | (unsigned)bennett_measure(&q) << 8 | (unsigned)bennett_measure(&r) << 9;
}
"#,
    );
    let quarter = 0.5 * S;
    for (mode, clang_flags, flags) in MODES {
        let module = lower(&source, &format!("held-span-recomputable-controlled-{mode}"), clang_flags, flags);
        let state = state_before_first_measurement(&module, "entry", &[]);
        let cell = state.quantum.cells[0].1;
        let [q, r] = ["q", "r"].map(|name| register_column(&module, &state, name));
        let expected = BTreeMap::from([
            ((0, 1, 7), C::new(S, 0.0)),
            ((0, 0, 3), C::new(quarter, 0.0)),
            ((1, 0, 3), C::new(quarter, 0.0)),
            ((0, 0, 5), C::new(quarter, 0.0)),
            ((1, 0, 5), C::new(-quarter, 0.0)),
        ]);
        // One amplitude per value of (q, r, byte): no other register distinguishes them.
        let mut observed = BTreeMap::new();
        for (values, amplitude) in &state.quantum.amplitudes {
            let key = (values[q], values[r], values[cell]);
            assert!(observed.insert(key, *amplitude).is_none(), "{mode}: {key:?} twice");
        }
        assert_eq!(observed.keys().collect::<Vec<_>>(), expected.keys().collect::<Vec<_>>(), "{mode}");
        for (key, amplitude) in observed {
            assert!((amplitude - expected[&key]).norm() < 1e-12, "{mode}, {key:?}: {amplitude}");
        }
        assert_eq!(run_with_stats(&module, "entry", &[], 0).1.leftover_qubits, 0, "{mode}");
    }
}
