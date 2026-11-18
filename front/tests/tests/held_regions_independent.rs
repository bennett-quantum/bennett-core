//! Native and phase-sensitive checks of held objects written under qubit control.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use bennett_exec::{Outcome, Snapshot};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, lower, lower_text, native_build, run_with_stats, state_before_first_measurement,
    write_c,
};
use bennett_ir::{Inst, ValidModule};
use num_complex::{Complex, Complex64};

#[allow(dead_code, reason = "this suite uses the swap construction from the shared test reference")]
#[path = "../../../crates/exec/tests/compute_copy_reference/mod.rs"]
mod compute_copy_reference;
use compute_copy_reference::swap_in::{StoreVector, apply_to_spare, clear_known, exchange, release};

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];
const PHASE: [Complex<i64>; 4] = [Complex::new(1, 0), Complex::new(0, 1), Complex::new(-1, 0), Complex::new(0, -1)];

const C_PREPARATION: &str = r#"
#include <stdint.h>
#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>
#define XBIT(v, b) ((v) ^= (1U << (b)))
#define HBIT(v, b) ((void)0)
#define SBIT(v, b) ((void)0)
#define ZBIT(v, b) ((void)0)
#define bennett_measure(p) (*(p))
static uint64_t bennett_measure_bytes(const void *p, unsigned n) {
    const unsigned char *bytes = p;
    uint64_t result = 0;
    for (unsigned i = 0; i < n; ++i) result |= (uint64_t)bytes[i] << (8 * i);
    return result;
}
#else
#include "bennett.h"
#define XBIT(v, b) bennett_x(bennett_bit(&(v), (b)))
#define HBIT(v, b) bennett_h(bennett_bit(&(v), (b)))
#define SBIT(v, b) bennett_s(bennett_bit(&(v), (b)))
#define ZBIT(v, b) bennett_z(bennett_bit(&(v), (b)))
#endif
"#;
const C_DRIVER: &str = r#"
#ifdef NATIVE
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    printf("%llu\n", (unsigned long long)entry(strtoul(argv[1], 0, 0), strtoul(argv[2], 0, 0)));
    return 0;
}
#endif
"#;

fn fixture(name: &str, body: &str) -> PathBuf {
    write_c(name, &format!("{C_PREPARATION}{body}{C_DRIVER}"))
}

fn native_value(exe: &Path, input: u64, column: u64) -> u64 {
    let output = Command::new(exe).args([input.to_string(), column.to_string()]).output().expect("run native C");
    assert!(output.status.success(), "native {input}, {column}: {}", String::from_utf8_lossy(&output.stderr));
    std::str::from_utf8(&output.stdout).expect("native stdout").trim().parse().expect("native integer")
}

fn measured(module: &ValidModule, input: u64, column: u64) -> Outcome {
    run_with_stats(module, "entry", &[input, column], 0).0
}

fn register_column(module: &ValidModule, state: &Snapshot, name: &str) -> usize {
    let frame = state.classical.frames.last().expect("live frame");
    let slot = module.function(frame.func).slot_names.iter().position(|s| s == name).expect("named register");
    state.classical.slots[frame.base + slot] as u32 as usize
}

fn global_address(text: &str, name: &str) -> u64 {
    let mut lines = text.lines().skip_while(|line| *line != format!("; @{name}"));
    lines.next().expect("global name");
    lines.next().expect("global data").split_whitespace().nth(1).expect("address").parse().expect("numeric address")
}

fn held_word(state: &Snapshot, basis: &[u64], address: u64, bytes: u64) -> u64 {
    (0..bytes).fold(0, |word, byte| {
        let (_, column) = state.quantum.cells.iter().find(|(at, _)| *at == address + byte).expect("held byte");
        word | basis[*column] << (8 * byte)
    })
}

// The native truth table supplies the ordinary function's values. The reference independently copies
// them into a spare, exchanges it with the target and clears the known old value.
fn reference_copy() -> ValidModule {
    bennett_ir::load("memory 16\nfunc @copy(i64 %v) -> i64 {\nentry:\n    ret %v\n}\n").expect("reference IR")
}

fn check_complex<K: Ord + std::fmt::Debug>(
    actual: &BTreeMap<K, Complex64>,
    expected: &BTreeMap<K, Complex64>,
    mode: &str,
) {
    assert_eq!(actual.keys().collect::<Vec<_>>(), expected.keys().collect::<Vec<_>>(), "{mode}: basis values");
    for (key, value) in expected {
        assert!((actual[key] - value).norm() < 1e-12, "{mode}, {key:?}: {} != {value}", actual[key]);
    }
}

fn density(
    state: &Snapshot,
    kept: impl Fn(usize) -> bool,
    values: impl Fn(&[u64]) -> [u64; 2],
) -> BTreeMap<([u64; 2], [u64; 2]), Complex64> {
    let mut result = BTreeMap::<_, Complex64>::new();
    for (row, alpha) in &state.quantum.amplitudes {
        for (col, beta) in &state.quantum.amplitudes {
            if (0..row.len()).all(|i| kept(i) || row[i] == col[i]) {
                *result.entry((values(row), values(col))).or_default() += alpha * beta.conj();
            }
        }
    }
    result.retain(|_, amp| amp.norm() > 1e-12);
    result
}

#[test]
fn unequal_width_held_objects_match_native_and_the_exchange_reference() {
    let name = "held-regions-wide";
    let source = fixture(
        name,
        r#"
uint64_t wide = 0x8000000000000013ULL;
uint32_t bytes = 0xc3b2a193U;
uint64_t entry(unsigned prep, unsigned which) {
    unsigned char q = 0;
    if (prep < 4) {
        if (prep & 1) XBIT(q, 0);
        if (prep & 2) XBIT(q, 1);
    } else {
        HBIT(q, 0); HBIT(q, 1); SBIT(q, 0); ZBIT(q, 1);
    }
    if (q & 1) {
        unsigned char *p = (unsigned char *)&bytes;
        wide = 0x4000000000000004ULL + (q & 2);
        p[0] = 0x21;
        p[3] = 0x54 + (q & 2);
    }
    uint64_t a = bennett_measure_bytes(&wide, 8);
    uint64_t b = bennett_measure_bytes(&bytes, 4);
    return which ? b : a;
}
"#,
    );
    let native = native_build(&source, name, &["-DNATIVE"]);
    let table = [0, 1, 2, 3].map(|input| [0, 1].map(|which| native_value(&native, input, which)));
    let reference = reference_copy();
    let expected_words = [64, 32].map(|width| {
        let column = usize::from(width == 32);
        let old = table[0][column];
        let input: StoreVector = PHASE.iter().enumerate().map(|(q, &amp)| ([q as u64, old, 0, 0], amp)).collect();
        let copy = apply_to_spare(&reference, "copy", 1, input, |basis| [table[basis[0] as usize][column]]);
        let clean = clear_known(exchange(copy, width), old);
        assert!(clean.keys().all(|basis| basis[2..] == [0, 0]));
        clean
    });
    let expected: BTreeMap<_, _> = expected_words[0]
        .iter()
        .zip(&expected_words[1])
        .map(|((&[q, wide, ..], &phase), (&[other_q, bytes, ..], &other_phase))| {
            assert_eq!((q, phase), (other_q, other_phase));
            ((q, wide, bytes), Complex64::new(phase.re as f64, phase.im as f64) / 2.0)
        })
        .collect();

    for (mode, cflags, flags) in MODES {
        let text = lower_text(&source, &format!("{name}-{mode}"), cflags, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid program");
        let measure_count = module
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter(|inst| matches!(inst, Inst::Measure { .. }))
            .count();
        assert_eq!(measure_count, 2, "{mode}: only the explicit measurements");
        for (input, expected) in table.iter().enumerate() {
            for (column, &word) in expected.iter().enumerate() {
                assert_eq!(
                    measured(&module, input as u64, column as u64),
                    Outcome::Returned(Some(word.into())),
                    "{mode}, input {input}, column {column}"
                );
            }
        }
        let state = state_before_first_measurement(&module, "entry", &[4, 0]);
        let q = register_column(&module, &state, "q");
        let wide = global_address(&text, "wide");
        // The first marker has ended wide's span. The second object is still in its shadow.
        let bytes = register_column(&module, &state, "bytes.held");
        let mut observed = BTreeMap::new();
        for (basis, amp) in &state.quantum.amplitudes {
            let key = (basis[q], held_word(&state, basis, wide, 8), basis[bytes]);
            assert!(observed.insert(key, *amp).is_none(), "{mode}: extra register distinguishes {key:?}");
            let kept_columns = state.quantum.cells.iter().map(|(_, column)| *column);
            assert!(
                basis.iter().enumerate().all(|(i, &value)| {
                    i == q || i == bytes || value == 0 || kept_columns.clone().any(|column| i == column)
                }),
                "{mode}: nonzero spare"
            );
        }
        check_complex(&observed, &expected, mode);
    }
}

#[test]
fn recomputation_distinguishes_gate_targets_from_gate_controls() {
    let source = fixture(
        "held-regions-recompute",
        r#"
unsigned char a = 3, b = 9;
uint64_t entry(unsigned prep, unsigned which) {
    unsigned char q = 0, t = 0;
#if KIND == 3
    unsigned char stable = 0;
    if (prep == 2) HBIT(stable, 0);
    else if (prep) XBIT(stable, 0);
#endif
    XBIT(t, 0);
    if (prep == 2) HBIT(q, 0);
    else if (prep) XBIT(q, 0);
    a = 3 + 2 * q;
#if KIND == 3
    b = 9 + 2 * stable;
#else
    b = 9 + 2 * q;
#endif
#if KIND == 0
    XBIT(t, 0);
#elif KIND == 1
    if (q) ZBIT(t, 0);
#else
    XBIT(q, 0);
#endif
    a = 3;
    b = 9;
    if (prep == 2) HBIT(q, 0);
#if KIND == 3
    if (prep == 2) HBIT(stable, 0);
#endif
    unsigned first = bennett_measure_bytes(&a, 1);
    unsigned second = bennett_measure_bytes(&b, 1);
    return which ? bennett_measure(&q) : first | (second << 8);
}
"#,
    );
    let reference = reference_copy();
    for (kind, define) in ["-DKIND=0", "-DKIND=1", "-DKIND=2", "-DKIND=3"].into_iter().enumerate() {
        let name = format!("held-regions-recompute-{kind}");
        let native = native_build(&source, &name, &["-DNATIVE", define]);
        for (mode, cflags, flags) in MODES {
            let cflags: Vec<_> = cflags.iter().copied().chain([define]).collect();
            let text = lower_text(&source, &format!("{name}-{mode}"), &cflags, flags).expect("lowers");
            let module = bennett_ir::load(&text).expect("valid program");
            for prep in 0..2 {
                for column in 0..2 {
                    assert_eq!(
                        measured(&module, prep, column),
                        Outcome::Returned(Some(native_value(&native, prep, column).into())),
                        "{mode}, kind {kind}, prep {prep}, column {column}"
                    );
                }
            }

            // For both bytes, the old value varies with the original q. The reference uses a packed
            // target and spare of 16 bits. The other gate either changes q, adds a phase or does neither.
            let original: StoreVector = (0..2)
                .map(|old_q| {
                    let q = old_q ^ u64::from(kind >= 2);
                    let old = (3 + 2 * old_q) | ((9 + 2 * old_q) << 8);
                    let sign = if kind == 1 && old_q == 1 { -1 } else { 1 };
                    ([q, old, 0, 0], Complex::new(sign, 0))
                })
                .collect();
            let copy = apply_to_spare(&reference, "copy", 1, original, |_| [0x0903]);
            let swapped = exchange(copy, 16);
            let after_h: StoreVector = swapped.into_iter().fold(BTreeMap::new(), |mut result, (basis, amp)| {
                for q in 0..=1 {
                    let sign = if basis[0] & q == 1 { -1 } else { 1 };
                    *result
                        .entry([
                            q,
                            basis[1],
                            match kind {
                                2 => basis[2],
                                3 => basis[2] & 0xff,
                                _ => 0,
                            },
                            basis[3],
                        ])
                        .or_default() += amp * sign;
                }
                result
            });
            let expected: BTreeMap<_, _> = release(&after_h)
                .into_iter()
                .map(|((row, col), amp)| ((row, col), Complex64::new(amp.re as f64, amp.im as f64) / 4.0))
                .collect();

            let state = state_before_first_measurement(&module, "entry", &[2, 0]);
            let q = register_column(&module, &state, "q");
            let a = global_address(&text, "a");
            let b = register_column(&module, &state, "b.held");
            let word = |basis: &[u64]| held_word(&state, basis, a, 1) | (basis[b] << 8);
            let kept = |i| i == q || i == b || state.quantum.cells.iter().any(|(_, column)| i == *column);
            let observed = density(&state, kept, |basis| [basis[q], word(basis)]);
            check_complex(&observed, &expected, &format!("{mode}, kind {kind}"));
            let t = register_column(&module, &state, "t");
            let stable = (kind == 3).then(|| register_column(&module, &state, "stable"));
            if let Some(stable) = stable {
                assert!(
                    state.quantum.amplitudes.iter().all(|(basis, _)| basis[stable] == 0),
                    "{mode}: the unchanged source of the second byte must interfere to zero"
                );
            }
            let released = (0..state.quantum.amplitudes[0].0.len())
                .filter(|&i| {
                    i != q
                        && i != b
                        && i != t
                        && Some(i) != stable
                        && !state.quantum.cells.iter().any(|(_, column)| i == *column)
                        && state.quantum.amplitudes.iter().any(|(basis, _)| basis[i] != 0)
                })
                .count();
            assert_eq!(
                released,
                match kind {
                    2 => 2,
                    3 => 1,
                    _ => 0,
                },
                "{mode}, kind {kind}: nonzero spares"
            );
        }
    }
}

#[test]
fn pointer_pairs_distinguish_equal_disjoint_and_ambiguous_calls() {
    let source = fixture(
        "held-regions-pairs",
        r#"
unsigned char a = 0x93, b = 0xc3;
#if KIND == 2
#define LINKAGE
#define RESTRICT restrict
#elif KIND == 4
#define LINKAGE
#define RESTRICT
#else
#define LINKAGE static
#define RESTRICT
#endif
__attribute__((noinline)) LINKAGE void put(unsigned prep, unsigned char *RESTRICT p, unsigned char *RESTRICT r) {
    unsigned char q = 0;
    if (prep & 1) XBIT(q, 0);
    if (prep & 2) XBIT(q, 1);
    if (q & 1) {
        *p = 5 + (q & 2);
        *r = 9 + (q & 2);
    }
}
uint64_t entry(unsigned prep, unsigned which) {
#if KIND == 0
    if (which) put(prep, &a, &a);
    else put(prep, &b, &b);
#elif KIND == 3
    if (which) put(prep, &a, &a);
    else put(prep, &a, &b);
#else
    put(prep, &a, &b);
#endif
    unsigned lo = bennett_measure_bytes(&a, 1);
    unsigned hi = bennett_measure_bytes(&b, 1);
    return lo | (hi << 8);
}
"#,
    );
    for (kind, define) in ["-DKIND=0", "-DKIND=1", "-DKIND=2", "-DKIND=3", "-DKIND=4"].into_iter().enumerate() {
        let name = format!("held-regions-pairs-{kind}");
        let native = (kind < 3).then(|| native_build(&source, &name, &["-DNATIVE", define]));
        for (mode, cflags, flags) in MODES {
            let cflags: Vec<_> = cflags.iter().copied().chain([define]).collect();
            let lowered = lower_text(&source, &format!("{name}-{mode}"), &cflags, flags);
            if let Some(native) = &native {
                let text = lowered.unwrap_or_else(|error| panic!("{mode}, kind {kind}: {error}"));
                let module = bennett_ir::load(&text).expect("valid program");
                for prep in 0..4 {
                    for which in 0..=u64::from(kind == 0) {
                        assert_eq!(
                            measured(&module, prep, which),
                            Outcome::Returned(Some(native_value(native, prep, which).into())),
                            "{mode}, kind {kind}, prep {prep}, which {which}"
                        );
                    }
                }
            } else {
                let error = lowered.expect_err("ambiguous pointers require a named refusal");
                assert!(
                    error.contains("may reach the same bytes") && error.contains("route"),
                    "{mode}, kind {kind}: {error}"
                );
            }
        }
    }
}

#[test]
fn call_summaries_follow_global_reads_through_two_callees() {
    let source = fixture(
        "held-regions-call-summary",
        r#"
unsigned char a = 3, other = 19;
__attribute__((noinline)) static unsigned read_global(void) {
#ifdef ALIAS
    return a;
#else
    return other;
#endif
}
__attribute__((noinline)) static unsigned relay(void) { return 1 + read_global(); }
uint64_t entry(unsigned prep, unsigned unused) {
    unsigned char q = 0;
    XBIT(q, 0);
    if (prep == 0) XBIT(q, 0);
    if (q) a = 5;
    unsigned value = relay();
    return value | (bennett_measure_bytes(&a, 1) << 8);
}
"#,
    );
    let native = native_build(&source, "held-regions-call-summary", &["-DNATIVE"]);
    for (mode, cflags, flags) in MODES {
        let module = lower(&source, &format!("held-regions-disjoint-call-{mode}"), cflags, flags);
        for prep in 0..2 {
            assert_eq!(
                measured(&module, prep, 0),
                Outcome::Returned(Some(native_value(&native, prep, 0).into())),
                "{mode}"
            );
        }
        let cflags: Vec<_> = cflags.iter().copied().chain(["-DALIAS"]).collect();
        let error = lower_text(&source, &format!("held-regions-aliased-call-{mode}"), &cflags, flags)
            .expect_err("the transitive access requires an explicit measurement in the caller");
        assert!(
            error.contains("call that may read or write memory of @a") && error.contains("bennett_measure_bytes"),
            "{mode}: {error}"
        );
    }
}

#[test]
fn straight_store_then_two_object_region_preserves_the_last_write() {
    let name = "held-regions-overlapping-spans";
    let source = fixture(
        name,
        r#"
unsigned char a = 3, b = 13;
uint64_t entry(unsigned unused, unsigned also_unused) {
    unsigned char q = 0, r = 0;
    XBIT(q, 0);
    a = 3 + 2 * q;
    XBIT(r, 0);
    if (r) { a = 7; b = 9; }
    return bennett_measure_bytes(&a, 1) | bennett_measure_bytes(&b, 1) << 8;
}
"#,
    );
    let native = native_build(&source, name, &["-DNATIVE"]);
    for (mode, cflags, flags) in MODES {
        let module = lower(&source, &format!("{name}-{mode}"), cflags, flags);
        let expected = Outcome::Returned(Some(native_value(&native, 0, 0).into()));
        assert_eq!(measured(&module, 0, 0), expected, "{mode}, entry(0, 0), packed bytes b:a");
    }
}

fn assert_call_is_safe(name: &str, source: &Path) {
    let native = native_build(source, name, &["-DNATIVE"]);
    let expected = Outcome::Returned(Some(native_value(&native, 0, 0).into()));
    for (mode, cflags, flags) in MODES {
        match lower_text(source, &format!("{name}-{mode}"), cflags, flags) {
            Ok(text) => {
                let module = bennett_ir::load(&text).expect("valid program");
                assert_eq!(measured(&module, 0, 0), expected, "{mode}, entry(0, 0)");
            }
            Err(error) => assert!(
                error.contains("call that may read or write memory of") && error.contains("bennett_measure_bytes"),
                "{mode}: {error}"
            ),
        }
    }
}

#[test]
fn a_callee_cannot_measure_the_cleared_storage_of_a_live_shadow() {
    let name = "held-regions-callee-marker";
    let source = fixture(
        name,
        r#"
unsigned char a = 3;
__attribute__((noinline)) static unsigned snapshot(void) {
    return bennett_measure_bytes(&a, 1);
}
uint64_t entry(unsigned unused, unsigned also_unused) {
    unsigned char q = 0;
    XBIT(q, 0);
    if (q) a = 5;
    return snapshot();
}
"#,
    );
    assert_call_is_safe(name, &source);
}

#[test]
fn a_saved_copy_of_a_restrict_pointer_can_reach_its_held_bytes() {
    let name = "held-regions-restrict-escape";
    let source = fixture(
        name,
        r#"
unsigned char a = 3;
static unsigned char *volatile saved;
__attribute__((noinline)) static unsigned peek(void) { return *saved; }
__attribute__((noinline)) unsigned writer(unsigned char *restrict p) {
    saved = p;
    unsigned char q = 0;
    XBIT(q, 0);
    if (q) *p = 5;
    return peek();
}
uint64_t entry(unsigned unused, unsigned also_unused) { return writer(&a); }
"#,
    );
    assert_call_is_safe(name, &source);
}

#[test]
fn seeded_short_loops_match_native_for_every_definite_input() {
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;
    use std::fmt::Write as _;

    const SEED: u64 = 0x4206_b58a_eba4_927d;
    const SAMPLES: usize = 9;
    println!("seed={SEED:#018x}, samples={SAMPLES}, definite inputs=8");
    let mut random = ChaCha8Rng::seed_from_u64(SEED);
    for sample in 0..SAMPLES {
        let objects = 1 + sample % 3;
        let mut declarations = String::new();
        let mut stores = String::new();
        let mut reads = String::new();
        for object in 0..objects {
            let initial: u8 = random.random();
            let bias: u8 = random.random();
            let shift = 8 * object;
            writeln!(declarations, "unsigned char cell{object} = {initial};").expect("write global");
            writeln!(stores, "    cell{object} = (acc >> {shift}) + {bias};").expect("write store");
            writeln!(reads, "    result |= bennett_measure_bytes(&cell{object}, 1) << {shift};").expect("write read");
        }
        let factor = u32::from(random.random::<u8>()) | 1;
        let initial: u32 = random.random();
        let name = format!("held-regions-seeded-{sample}");
        let source = fixture(
            &name,
            &format!(
                r#"
{declarations}
uint64_t entry(unsigned prep, unsigned unused) {{
    unsigned char q = 0;
    if (prep & 1) XBIT(q, 0);
    if (prep & 2) XBIT(q, 1);
    if (prep & 4) XBIT(q, 2);
    if (q & 4) {{
        uint32_t acc = {initial}U;
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i <= (q & 3U); ++i)
            acc = (acc ^ (acc >> 7)) * {factor}U + i;
{stores}
    }}
    uint64_t result = 0;
{reads}
    return result;
}}
"#
            ),
        );
        let native = native_build(&source, &name, &["-DNATIVE"]);
        let table: Vec<_> = (0..8).map(|input| native_value(&native, input, 0)).collect();
        for (mode, cflags, flags) in MODES {
            let module = lower(&source, &format!("{name}-{mode}"), cflags, flags);
            for (input, expected) in table.iter().copied().enumerate() {
                assert_eq!(
                    measured(&module, input as u64, 0),
                    Outcome::Returned(Some(expected.into())),
                    "seed={SEED:#018x}, {name}, {mode}, input={input}"
                );
            }
        }
    }
}

#[test]
fn a_two_object_region_handles_recomputable_and_retained_inputs() {
    let source = fixture(
        "held-regions-two-stage",
        r#"
unsigned char a = 3, b = 9;
uint64_t entry(unsigned prep, unsigned unused) {
    unsigned char q = 0, s = 0, r = 0;
    if (prep == 8) {
        HBIT(q, 0); HBIT(s, 0); HBIT(r, 0); SBIT(q, 0); ZBIT(s, 0);
    } else {
        XBIT(q, 0); if (!(prep & 1)) XBIT(q, 0);
        XBIT(s, 0); if (!(prep & 2)) XBIT(s, 0);
        XBIT(r, 0); if (!(prep & 4)) XBIT(r, 0);
    }
    if (q) a = 5;
    if (s) b = 11;
#if CHANGE_Q
    XBIT(q, 0);
#endif
    if (r) { a = 7; b = 13; }
    unsigned lo = bennett_measure_bytes(&a, 1);
    unsigned hi = bennett_measure_bytes(&b, 1);
    return lo | (hi << 8);
}
"#,
    );
    let reference = reference_copy();
    for (changed, define) in [(false, "-DCHANGE_Q=0"), (true, "-DCHANGE_Q=1")] {
        let name = format!("held-regions-two-stage-{changed}");
        let native = native_build(&source, &name, &["-DNATIVE", define]);
        let table: Vec<_> = (0..8).map(|prep| native_value(&native, prep, 0)).collect();
        let old_word = |input| (3 + 2 * (input & 1)) | ((9 + (input & 2)) << 8);
        let input: StoreVector = (0..8)
            .map(|prep| {
                let current = prep ^ u64::from(changed);
                ([current, old_word(prep), 0, 0], PHASE[(prep & 3) as usize])
            })
            .collect();
        let copy =
            apply_to_spare(&reference, "copy", 1, input, |basis| [table[(basis[0] ^ u64::from(changed)) as usize]]);
        let exchanged = exchange(copy, 16);
        let cleared = apply_to_spare(&reference, "copy", 1, exchanged, |basis| {
            // b's old value can always be recovered from s. Recompute a's only if q is unchanged.
            [old_word(basis[0]) & if changed { 0xff00 } else { 0xffff }]
        });
        let expected: BTreeMap<_, _> = release(&cleared)
            .into_iter()
            .map(|(basis, amp)| (basis, Complex64::new(amp.re as f64, amp.im as f64) / 8.0))
            .collect();

        for (mode, cflags, flags) in MODES {
            let cflags: Vec<_> = cflags.iter().copied().chain([define]).collect();
            let text = lower_text(&source, &format!("{name}-{mode}"), &cflags, flags).expect("lowers");
            let module = bennett_ir::load(&text).expect("valid program");
            for (prep, &value) in table.iter().enumerate() {
                assert_eq!(
                    measured(&module, prep as u64, 0),
                    Outcome::Returned(Some(value.into())),
                    "{mode}, changed={changed}, prep={prep}"
                );
            }
            let state = state_before_first_measurement(&module, "entry", &[8, 0]);
            let [q, s, r, b] = ["q", "s", "r", "b.held"].map(|name| register_column(&module, &state, name));
            let a = global_address(&text, "a");
            let kept = |i| [q, s, r, b].contains(&i) || state.quantum.cells.iter().any(|(_, column)| i == *column);
            let observed = density(&state, kept, |basis| {
                [basis[q] | (basis[s] << 1) | (basis[r] << 2), held_word(&state, basis, a, 1) | (basis[b] << 8)]
            });
            check_complex(&observed, &expected, &format!("{mode}, changed={changed}"));
        }
    }
}
