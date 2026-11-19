//! Compare short C programs with native execution and check the simulated state before readout.
//!
//! Set `BENNETT_RANDOM_WRITING_CALLS` to include calls with qubit-dependent arguments
//! that write a global in the ignored long campaign. When any family switch is set,
//! that campaign runs only the selected families. Use the same switches to replay an index.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, lower_text, lower_text_with_diagnostics, native_build, run_with_stats, write_c,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

#[path = "qubit_differential/generator.rs"]
mod generator;
#[path = "qubit_differential/held_pass.rs"]
mod held_pass;
#[path = "qubit_differential/semantic_cases.rs"]
mod semantic_cases;
#[path = "qubit_differential/state.rs"]
mod state;

use state::{Check, Field, Preparation};

const SEED: u64 = 0x8719_6caa_251d_0f43;
const SHORT_CASES: usize = generator::SHORT_CASES;
const EXTRA_SEED_MASK: u64 = 0xe911_2825_44ba_3983;
const STATE_SEED_MASK: u64 = 0xdba2_4ce8_b7e9_8163;
const LOOP_SEED_MASK: u64 = 0x384e_2b7a_97dd_5c1f;
const LOOP_UPDATE: &str = " in a loop under the control of a value that depends on a qubit,";
const LONG_TIME: Duration = Duration::from_secs(15 * 60);
const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];

const PRELUDE: &str = r#"
#include <stdint.h>
typedef unsigned __int128 packet;
#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>
#define PREPARE(n) ((unsigned char)prep)
#define HBIT(q, b) ((void)0)
#define XBIT(q, b) ((q) ^= 1U << (b))
#define ZBIT(q, b) ((void)0)
#define SBIT(q, b) ((void)0)
#define bennett_measure(p) (*(p))
static uint64_t bennett_measure_bytes(const void *p, unsigned n) {
    const unsigned char *bytes = p;
    uint64_t result = 0;
    for (unsigned i = 0; i < n; ++i) result |= (uint64_t)bytes[i] << (8 * i);
    return result;
}
#else
#include "bennett.h"
#define PREPARE(n) 0
#define HBIT(q, b) bennett_h(bennett_bit(&(q), (b)))
#define XBIT(q, b) bennett_x(bennett_bit(&(q), (b)))
#define ZBIT(q, b) bennett_z(bennett_bit(&(q), (b)))
#define SBIT(q, b) bennett_s(bennett_bit(&(q), (b)))
#endif
"#;
const DRIVER: &str = r#"
#ifdef NATIVE
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    packet value = entry(strtoul(argv[1], 0, 0), strtoul(argv[2], 0, 0), strtoul(argv[3], 0, 0));
    printf("%016llx%016llx\n", (unsigned long long)(value >> 64), (unsigned long long)value);
    return 0;
}
#endif
"#;

struct Case {
    body: String,
    qubits: u32,
    trips: u64,
    packets: u64,
    expectation: Expectation,
    state: Option<Check>,
}

enum Expectation {
    Values { clean: bool },
    Clean,
    AppliedWrite(&'static str),
    ConditionalWrite,
    Retained(u32),
    LoopClean,
    LoopRetained { object: &'static str, bits: u32 },
}

impl Expectation {
    fn clean(&self) -> bool {
        matches!(self, Self::Values { clean: true } | Self::Clean | Self::AppliedWrite(_) | Self::LoopClean)
    }

    fn loop_overwrite(&self) -> bool {
        matches!(self, Self::LoopClean | Self::LoopRetained { .. })
    }

    fn retained_bits(&self) -> u32 {
        match self {
            Self::Retained(bits) | Self::LoopRetained { bits, .. } => *bits,
            _ => 0,
        }
    }

    fn expected_loop_decline(&self, reason: &str) -> bool {
        let Self::LoopRetained { object, .. } = self else { return false };
        reason.starts_with(&format!("not handled yet: an update of qubit variable '{object}'{LOOP_UPDATE}"))
            && reason.contains("whose value before the loop is neither known nor computable again")
            && reason.contains("its old value would have to be kept in the fresh register and released: not built yet")
    }

    fn check_diagnostics(&self, diagnostics: &str) -> Result<(), String> {
        let expected: [&str; 2] = match self {
            Self::AppliedWrite(object) => {
                ["warning: the call of @write_cell depends on a qubit and is applied whole", object]
            }
            Self::Retained(bits) => [
                "warning: the overwrite of qubit variable 'x'",
                &format!("keeps the old value in {bits} qubits each time it runs"),
            ],
            Self::LoopRetained { object, bits } => [
                &format!("warning: the overwrite of qubit variable '{object}'{LOOP_UPDATE}"),
                &format!("keeps the old value in {bits} qubits each time it runs"),
            ],
            Self::Values { .. } | Self::Clean | Self::ConditionalWrite | Self::LoopClean => return Ok(()),
        };
        if !diagnostics.lines().any(|line| expected.iter().all(|part| line.contains(part))) {
            return Err(format!("missing warning containing {expected:?}: {diagnostics:?}"));
        }
        Ok(())
    }
}

impl Case {
    fn values(body: String, qubits: u32, trips: u64, packets: u64, clean: bool) -> Self {
        Self { body, qubits, trips, packets, expectation: Expectation::Values { clean }, state: None }
    }

    fn native_table(&self, executable: &Path) -> Vec<Vec<u128>> {
        (0..self.packets)
            .map(|column| {
                (0..1 << self.qubits)
                    .map(|input| {
                        let output = Command::new(executable)
                            .args([input.to_string(), self.trips.to_string(), column.to_string()])
                            .output()
                            .expect("run native C");
                        assert!(output.status.success(), "native input {input}, packet {column}: {output:?}");
                        let stdout = std::str::from_utf8(&output.stdout).expect("native stdout");
                        u128::from_str_radix(stdout.trim(), 16).expect("native packet")
                    })
                    .collect()
            })
            .collect()
    }

    fn source(&self) -> String {
        format!("{PRELUDE}{}{DRIVER}", self.body)
    }
}

#[derive(Default)]
struct Results {
    generated: usize,
    accepted: usize,
    compared: usize,
    clean: usize,
    state: state::Counts,
    failed: usize,
    declined: BTreeMap<String, usize>,
    loop_accepted: usize,
    loop_state: usize,
    loop_declined: usize,
}

impl Results {
    fn print(&self, seed: u64, elapsed: Duration) {
        let declined: usize = self.declined.values().sum();
        println!(
            "seed={seed:#018x} generated={} accepted={} declined={declined} compared={} clean={} state={} phase={} return_to_zero={} retained={} failed={} elapsed={:.2}s loop_accepted={} loop_state={} loop_declined={}",
            self.generated,
            self.accepted,
            self.compared,
            self.clean,
            self.state.total,
            self.state.phase,
            self.state.return_to_zero,
            self.state.retained,
            self.failed,
            elapsed.as_secs_f64(),
            self.loop_accepted,
            self.loop_state,
            self.loop_declined,
        );
        for (reason, count) in &self.declined {
            println!("  {count:5} {reason}");
        }
    }
}

// The register is measured before the objects. Run through deterministic measurement seeds until
// every register value has occurred, comparing every result, including repeat observations.
fn compare(
    case: &Case,
    module: &bennett_ir::ValidModule,
    table: &[Vec<u128>],
    compared: &mut usize,
) -> Result<(), String> {
    for (column, expected) in table.iter().enumerate() {
        let mut seen = vec![false; expected.len()];
        for seed in 0..128 {
            let (outcome, stats) = run_with_stats(module, "entry", &[0, case.trips, column as u64], seed);
            let Outcome::Returned(Some(value)) = outcome else {
                return Err(format!("packet {column}, measurement seed {seed}: {outcome:?}"));
            };
            let input = (value & 0xff) as usize;
            if expected.get(input) != Some(&value) {
                return Err(format!(
                    "packet {column}, measurement seed {seed}, register={input}: device={value:#034x}, native={:?}",
                    expected.get(input).map(|word| format!("{word:#034x}"))
                ));
            }
            if case.expectation.clean() && stats.nonzero_releases != 0 {
                return Err(format!(
                    "packet {column}, measurement seed {seed}, register={input}: {} nonzero releases (weight {})",
                    stats.nonzero_releases, stats.nonzero_release_weight
                ));
            }
            let bits = case.expectation.retained_bits();
            if bits != 0 && (stats.nonzero_releases, stats.leftover_qubits) != (1, bits) {
                return Err(format!(
                    "packet {column}, measurement seed {seed}, register={input}: expected one release keeping {bits} qubits, got {} releases keeping {} qubits",
                    stats.nonzero_releases, stats.leftover_qubits
                ));
            }
            if !seen[input] {
                *compared += 1;
                seen[input] = true;
                if seen.iter().all(|&found| found) {
                    break;
                }
            }
        }
        if seen.contains(&false) {
            return Err(format!("packet {column}: missing measured register values, observed mask {seen:?}"));
        }
    }
    Ok(())
}

fn try_case(case: &Case, name: &str, mode: usize, results: &mut Results) -> Result<(), String> {
    let (_, cflags, flags) = MODES[mode];
    let source = write_c(name, &case.source());
    let (text, diagnostics) = match lower_text_with_diagnostics(&source, name, cflags, flags) {
        Ok(lowered) => lowered,
        Err(error)
            if error.starts_with("not handled yet: ") || (error.starts_with("error: ") && error.contains(", at ")) =>
        {
            let reason = error.trim_end().rsplit_once(", at ").map_or(error.trim_end(), |(reason, _)| reason);
            *results.declined.entry(reason.to_owned()).or_default() += 1;
            if reason.starts_with("not handled yet: an update of qubit variable ") && reason.contains(LOOP_UPDATE) {
                results.loop_declined += 1;
            }
            if matches!(case.expectation, Expectation::ConditionalWrite)
                && !reason.contains("a call to @write_cell, which may write memory,")
            {
                return Err(format!("expected the refusal of a conditional writing call: {reason}"));
            }
            if matches!(
                case.expectation,
                Expectation::Clean | Expectation::AppliedWrite(_) | Expectation::Retained(_) | Expectation::LoopClean
            ) || (matches!(case.expectation, Expectation::LoopRetained { .. })
                && !case.expectation.expected_loop_decline(reason))
            {
                return Err(format!("expected the front to lower the program: {reason}"));
            }
            return Ok(());
        }
        Err(error) => return Err(format!("front failed without a refusal: {error:?}")),
    };
    results.accepted += 1;
    results.clean += usize::from(case.expectation.clean());
    results.loop_accepted += usize::from(case.expectation.loop_overwrite());
    let module = bennett_ir::load(&text).map_err(|error| format!("front emitted an invalid program: {error}"))?;
    let native = native_build(&source, name, &["-DNATIVE"]);
    let table = case.native_table(&native);
    compare(case, &module, &table, &mut results.compared)?;
    if matches!(case.expectation, Expectation::ConditionalWrite) {
        return Err("accepted a call that may write memory under a qubit-dependent condition".to_owned());
    }
    case.expectation.check_diagnostics(&diagnostics)?;
    let retained_bits = case.expectation.retained_bits();
    if case.expectation.clean() || retained_bits > 0 {
        let check = case.state.as_ref().expect("generated clean or retained case has a state check");
        let before = results.state.total;
        let checked = state::compare(check, &module, &table, case.trips, retained_bits, &mut results.state);
        if case.expectation.loop_overwrite() {
            results.loop_state += results.state.total - before;
        }
        checked?;
    }
    Ok(())
}

fn hadamards(out: &mut String, qubits: u32) {
    for i in 0..qubits {
        writeln!(out, "    HBIT(q, {i});").expect("write gate");
    }
}

fn prepare(qubits: u32) -> String {
    let mut out = String::from("    unsigned char q = PREPARE(prep);\n");
    hadamards(&mut out, qubits);
    out
}

// Each packet carries the measured q in its lowest byte and up to 15 bytes of object data.
fn measure_objects(body: &mut String, objects: &[(String, u32)]) -> (u64, Vec<Field>) {
    body.push_str("    packet key = bennett_measure(&q);\n");
    for (i, (object, bytes)) in objects.iter().enumerate() {
        writeln!(body, "    packet v{i} = bennett_measure_bytes(&{object}, {bytes});").expect("write measurement");
    }
    let mut packets = vec![String::from("key")];
    let mut fields = vec![Field { packet: 0, shift: 0 }];
    let mut shift = 8;
    for (i, (_, bytes)) in objects.iter().enumerate() {
        if shift + 8 * bytes > 128 {
            packets.push(String::from("key"));
            shift = 8;
        }
        fields.push(Field { packet: packets.len() - 1, shift });
        write!(packets.last_mut().expect("packet"), " | (v{i} << {shift})").expect("pack measurement");
        shift += 8 * bytes;
    }
    for (column, packet) in packets.iter().take(packets.len() - 1).enumerate() {
        writeln!(body, "    if (column == {column}) return {packet};").expect("select packet");
    }
    writeln!(body, "    return {};\n}}", packets.last().expect("last packet")).expect("write return");
    (packets.len() as u64, fields)
}

// The original fixed prefix, then one case from each loop overwrite recipe.
#[test]
fn short_randomized_differential() {
    campaign("qubit-differential-short", SEED, Some(SHORT_CASES), None, None);
}

// Build the front with front/test.sh, then run:
// cargo test -p bennett-front-tests --test qubit_differential long_randomized_differential -- --ignored --exact --nocapture
// BENNETT_RANDOM_CASES caps the case count. BENNETT_RANDOM_INDEX replays one case.
// BENNETT_RANDOM_SEED accepts a decimal or 0x-prefixed seed.
#[test]
#[ignore = "runs the differential generator for up to fifteen minutes"]
fn long_randomized_differential() {
    let limit = std::env::var("BENNETT_RANDOM_CASES").ok().map(|value| value.parse().expect("case count"));
    let seed = std::env::var("BENNETT_RANDOM_SEED").ok().map_or(SEED, |value| {
        value.strip_prefix("0x").map_or_else(|| value.parse(), |hex| u64::from_str_radix(hex, 16)).expect("seed")
    });
    let replay = std::env::var("BENNETT_RANDOM_INDEX").ok().map(|value| value.parse().expect("case index"));
    campaign("qubit-differential-long", seed, limit, Some(LONG_TIME), replay);
}

fn campaign(name: &str, seed: u64, limit: Option<usize>, time: Option<Duration>, replay: Option<usize>) {
    let limit = if replay.is_some() { Some(1) } else { limit };
    let recipes = if time.is_some() { semantic_cases::enabled_recipes() } else { Vec::new() };
    let mut original = ChaCha8Rng::seed_from_u64(seed);
    let mut extra = ChaCha8Rng::seed_from_u64(seed ^ EXTRA_SEED_MASK);
    let mut state = ChaCha8Rng::seed_from_u64(seed ^ STATE_SEED_MASK);
    let mut loops = ChaCha8Rng::seed_from_u64(seed ^ LOOP_SEED_MASK);
    let start = Instant::now();
    let mut results = Results::default();
    println!("seed={seed:#018x} case limit={limit:?} time limit={time:?} replay={replay:?}");
    for index in 0.. {
        if limit.is_some_and(|limit| results.generated >= limit) || time.is_some_and(|time| start.elapsed() >= time) {
            break;
        }
        let (case, mode) = if recipes.is_empty() {
            (generator::generate(index, &mut original, &mut extra, &mut state, &mut loops), index % MODES.len())
        } else {
            let (generate, mode) = recipes[index % recipes.len()];
            (generate(&mut original), mode)
        };
        if replay.is_some_and(|selected| index < selected)
            || (time.is_none() && (generator::SHORT_PREFIX..generator::LOOP_START).contains(&index))
        {
            continue;
        }
        results.generated += 1;
        let case_name = format!("{name}-{index}");
        if let Err(error) = try_case(&case, &case_name, mode, &mut results) {
            results.failed += 1;
            eprintln!(
                "seed={seed:#018x} index={index} mode={} trips={}: {error}\n{}",
                MODES[mode].0,
                case.trips,
                case.source()
            );
            if time.is_none() {
                break;
            }
        }
        if results.generated % 1000 == 0 {
            results.print(seed, start.elapsed());
        }
    }
    results.print(seed, start.elapsed());
    assert_eq!(results.failed, 0, "failed generated programs");
}

fn targeted(body: String, trips: u64, packets: u64, clean: bool) -> Case {
    Case::values(body, 2, trips, packets, clean)
}

fn check_target(name: &str, case: &Case, refusal: Option<&str>) {
    let source = write_c(name, &case.source());
    let native = native_build(&source, name, &["-DNATIVE"]);
    let table = case.native_table(&native);
    for (mode, cflags, flags) in MODES {
        let name = format!("{name}-{mode}");
        match lower_text(&source, &name, cflags, flags) {
            Ok(text) => {
                let module = bennett_ir::load(&text).expect("valid program");
                compare(case, &module, &table, &mut 0).unwrap_or_else(|error| panic!("{name}: {error}"));
            }
            Err(error) => assert!(refusal.is_some_and(|reason| error.contains(reason)), "{name}: {error}"),
        }
    }
}

#[test]
fn straight_store_before_a_region_with_three_objects() {
    let declarations = "uint16_t a = 3; uint8_t b = 13; static uint64_t c[2] = {19, 21};\n";
    for (name, code, wrapped, refusal) in [
        ("straight-reordered", "a = 3 + 2 * q;\n    if (q & 2) { c[1] = 1ULL << 55; b = 9; a = 257; }", false, None),
        ("straight-static", "a = 3 + 2 * q;\n    if (q & 2) { b = 9; a = 257; c[1] = 1ULL << 55; }", true, None),
        (
            "straight-in-loop",
            "#pragma clang loop unroll(disable)\n    for (unsigned i = 0; i < n; ++i) { a = 3 + 2 * q + i; }\n    if (q & 2) { b = 9; a = 257; c[1] = 1ULL << 55; }",
            false,
            Some("that another write under a qubit holds again"),
        ),
    ] {
        let function = if wrapped { "work" } else { "entry" };
        let mut body = format!(
            "{declarations}__attribute__((noinline)) {}packet {function}(unsigned prep, unsigned n, unsigned column) {{\n{}    {code}\n",
            if wrapped { "static " } else { "" },
            prepare(2)
        );
        let (packets, _) = measure_objects(&mut body, &[("a".into(), 2), ("b".into(), 1), ("c[1]".into(), 8)]);
        if wrapped {
            body.push_str(
                "\npacket entry(unsigned prep, unsigned n, unsigned column) { return work(prep, n, column); }\n",
            );
        }
        check_target(name, &targeted(body, 2, packets, refusal.is_none()), refusal);
    }
}

#[test]
fn callee_measurement_of_an_offset_through_a_pointer() {
    for (name, measure_first, callee, trips) in [
        ("callee-measure-pointer", "", "return bennett_measure_bytes(p + 1, 4);", 0),
        (
            "callee-measure-loop",
            "",
            "#pragma clang loop unroll(disable)\n    for (unsigned i = 0; i < n; ++i) v = bennett_measure_bytes(p + 1, 4);\n    return v;",
            2,
        ),
        (
            "callee-measure-already-measured",
            "uint64_t first = bennett_measure_bytes(&a[1], 4);",
            "return bennett_measure_bytes(p + 1, 4);",
            0,
        ),
    ] {
        let value = if measure_first.is_empty() { "v" } else { "v | (first << 32)" };
        let body = format!(
            "uint32_t a[2] = {{77, 1234}};\n__attribute__((noinline)) static uint64_t snapshot(uint32_t *p, unsigned n) {{\n    uint64_t v = 0;\n    {callee}\n}}\n\
             packet entry(unsigned prep, unsigned n, unsigned column) {{\n{}    if (q & 1) a[1] = 99987;\n    {measure_first}\n    uint64_t v = snapshot(a, n);\n    return ((packet)({value}) << 8) | bennett_measure(&q);\n}}",
            prepare(2)
        );
        let refusal = measure_first.is_empty().then_some("a call that may read or write memory of @a");
        check_target(name, &targeted(body, trips, 1, true), refusal);
    }
}

#[test]
fn restrict_copy_to_an_offset_or_reloaded_in_a_loop() {
    for (name, ty, reads, trips) in [
        ("restrict-offset", "uint16_t", "v = *saved;", 0),
        (
            "restrict-loop",
            "uint64_t",
            "#pragma clang loop unroll(disable)\n    for (unsigned i = 0; i < n; ++i) v ^= *saved + i;",
            3,
        ),
    ] {
        let body = format!(
            "{ty} a[2] = {{11, 19}};\nstatic {ty} *volatile saved;\n\
             __attribute__((noinline)) static uint64_t peek(unsigned n) {{\n    uint64_t v = 0;\n    {reads}\n    return v;\n}}\n\
             __attribute__((noinline)) packet work({ty} *restrict p, unsigned prep, unsigned n) {{\n{}    saved = p + 1;\n    if (q & 2) p[1] = ({ty})0x8f07eeabac15ULL;\n    uint64_t v = peek(n);\n    return ((packet)v << 8) | bennett_measure(&q);\n}}\n\
             packet entry(unsigned prep, unsigned n, unsigned column) {{ return work(a, prep, n); }}",
            prepare(2)
        );
        check_target(name, &targeted(body, trips, 1, true), Some("a call that may read or write memory of %p"));
    }
}

// A value selected where control flow joins after code under a qubit, by the edge the code left on: constants,
// values from before the code, a merged store, and a later update that reads it, in all three modes. The
// objects are written before the gates, so that LLVM keeps the conditional stores as control flow.
#[test]
fn join_selects_the_value_of_the_path_taken() {
    for (name, globals, locals, code, objects, trips) in [
        ("join-later-add", "", "uint16_t a = 0;", "if (q & 2) a = 2;\n    if (q & 1) a += q;", vec![("a", 2)], 0),
        (
            "join-earlier-values",
            "",
            "uint16_t a = n + 5;",
            "if (q & 2) a = n;\n    if (q & 1) a += q;",
            vec![("a", 2)],
            3,
        ),
        (
            "join-merged-store",
            "",
            "uint8_t a = 1, b = 0;",
            "if (!(q & 1)) { a = 2; b += 3; }\n    if (q & 1) a = 4;",
            vec![("a", 1), ("b", 1)],
            0,
        ),
        (
            "join-three-ways",
            "uint32_t g;\n",
            "g = n;",
            "if (q & 1) g = 3; else if (q & 2) g = 5 * n;\n    if (q != 2) g += q;",
            vec![("g", 4)],
            2,
        ),
    ] {
        let mut body = format!(
            "{globals}packet entry(unsigned prep, unsigned n, unsigned column) {{\n    {locals}\n{}    {code}\n",
            prepare(2)
        );
        let objects: Vec<(String, u32)> =
            objects.into_iter().map(|(object, bytes)| (object.to_owned(), bytes)).collect();
        let (packets, _) = measure_objects(&mut body, &objects);
        check_target(name, &targeted(body, trips, packets, false), None);
    }
}

// A reduced witness, in all three modes.
fn check_witness(name: &str, qubits: u32, body: &str, refusal: Option<&str>) {
    check_target(name, &Case::values(body.to_owned(), qubits, 0, 1, true), refusal);
}

// A program the front must refuse with `reason` in all three modes.
fn check_refused(name: &str, body: &str, reason: &str) {
    let source = write_c(name, &format!("{PRELUDE}{body}{DRIVER}"));
    for (mode, cflags, flags) in MODES {
        match lower_text(&source, &format!("{name}-{mode}"), cflags, flags) {
            Ok(_) => panic!("{name}-{mode}: lowered, expected the refusal {reason:?}"),
            Err(error) => assert!(error.contains(reason), "{name}-{mode}: {error}"),
        }
    }
}

#[test]
fn later_region_adds_to_the_current_value() {
    check_witness(
        "later-region-add",
        2,
        r#"
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint16_t a = 0;
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    HBIT(q, 1);
    if (q & 2) a = 2;
    if (q & 1) a += q;
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 2);
    return key | (v << 8);
}
"#,
        None,
    );
}

#[test]
fn merged_store_keeps_both_conditional_values() {
    check_witness(
        "merged-conditional-store",
        1,
        r#"
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t a = 1, b = 0;
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    if (!q) { a = 2; b += 3; }
    if (q) a = 4;
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 1);
    packet other = bennett_measure_bytes(&b, 1);
    return key | (v << 8) | (other << 16);
}
"#,
        None,
    );
}

const PRIVATE_STACK_HELPER: &str = r#"
__attribute__((noinline)) static uint8_t f(uint8_t x) {
    volatile uint8_t t = x;
    return t;
}
"#;

#[test]
fn helper_with_private_stack_can_write_a_global_result() {
    check_witness(
        "helper-private-stack",
        1,
        &format!(
            "{PRIVATE_STACK_HELPER}{}",
            r#"
uint8_t a;
packet entry(unsigned prep, unsigned n, unsigned column) {
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    a = f(q);
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 1);
    return key | (v << 8);
}
"#
        ),
        None,
    );
}

// The frame of a helper written through a pointer by a helper it calls is still its own; a helper whose local's
// address escapes into a global may not be moved past a write.
#[test]
fn nested_private_frames_and_an_escaped_local() {
    let nested = r#"
__attribute__((noinline)) static void fill(volatile uint8_t *p, uint8_t x) { *p = x + 1; }
__attribute__((noinline)) static uint8_t f(uint8_t x) {
    volatile uint8_t t;
    fill(&t, x);
    return t;
}
uint8_t a;
packet entry(unsigned prep, unsigned n, unsigned column) {
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    a = f(q);
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 1);
    return key | (v << 8);
}
"#;
    check_witness("nested-private-frame", 1, nested, None);
    let escaped = r#"
volatile uint8_t *volatile where;
__attribute__((noinline)) static uint8_t f(uint8_t x) {
    volatile uint8_t t = x;
    where = &t;
    return t;
}
uint8_t a;
packet entry(unsigned prep, unsigned n, unsigned column) {
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    a = f(q);
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 1);
    return key | (v << 8);
}
"#;
    check_witness("escaped-local", 1, escaped, Some("reads memory that may change before its result is used"));
}

#[test]
fn private_stack_helper_can_feed_an_outlined_region() {
    check_witness(
        "helper-feeds-region",
        1,
        &format!(
            "{PRIVATE_STACK_HELPER}{}",
            r#"
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint16_t a = 0;
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    a = f(q);
    if (q) { a ^= q; a ^= 3*q; }
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 2);
    return key | (v << 8);
}
"#
        ),
        None,
    );
}

#[test]
fn dead_private_stack_call_needs_a_specific_diagnostic() {
    check_witness(
        "dead-private-stack-call",
        1,
        &format!(
            "{PRIVATE_STACK_HELPER}{}",
            r#"
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t a = 0;
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    a = f(q);
    if (q) a = 2;
    if (!q) a = 3;
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 1);
    return key | (v << 8);
}
"#
        ),
        Some("whose result is not used, to a function that may write memory"),
    );
}

// LLVM merges the stores to a at their join, so the value at the join is outlined and stored after the region,
// where the span of a starts. The control program computes the value where it is stored, after the exchange
// that moves a into its shadow; the region's load of a is made where the region is and passed in.
#[test]
fn merged_held_store_can_use_the_value_before_the_exchange() {
    check_witness(
        "merged-held-store-after-exchange",
        2,
        r#"
uint8_t a = 43, b = 9;
packet entry(unsigned prep, unsigned n, unsigned column) {
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    HBIT(q, 1);
    if (!q) a = 5;
    else { a ^= q; b += q; }
    packet key = bennett_measure(&q);
    packet first = bennett_measure_bytes(&a, 1);
    packet second = bennett_measure_bytes(&b, 1);
    return key | (first << 8) | (second << 16);
}
"#,
        None,
    );
}

// The nearest shape that must stay refused: the region reads a in a helper it calls, which is applied where
// the value is stored, after the exchange has changed a's bytes.
#[test]
fn merged_held_store_read_by_a_helper_after_the_exchange_is_refused() {
    check_refused(
        "merged-held-store-helper-read",
        r#"
uint8_t a = 43, b = 9;
__attribute__((noinline)) static uint8_t mix(uint8_t x) { return a ^ x; }
packet entry(unsigned prep, unsigned n, unsigned column) {
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    HBIT(q, 1);
    if (!q) a = 5;
    else { a = mix(q); b += q; }
    packet key = bennett_measure(&q);
    packet first = bennett_measure_bytes(&a, 1);
    packet second = bennett_measure_bytes(&b, 1);
    return key | (first << 8) | (second << 16);
}
"#,
        "reads memory that may change before its result is used",
    );
}

// Generated case 3960 of seed 0x87196caa251d0f43: LLVM merges the first region's stores to cell1 at its
// join and forwards that value to the second region's read of cell1, so the second region XORs into a value that
// is cell1's, not into a load of it. Clean: nothing kept, every spare register back at zero.
#[test]
fn later_region_xors_into_a_forwarded_merged_store() {
    check_witness(
        "forwarded-merged-store",
        3,
        r#"
static uint16_t cell0[2] = {21020ULL, 11};
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t cell1 = 40ULL;
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    HBIT(q, 1);
    HBIT(q, 2);
    if ((q & 2U) != 0) {
        cell0[0] = 11516135566429171499ULL;
        #pragma clang loop unroll(disable)
        for (unsigned j = 0; j < 3U; ++j) cell0[0] ^= (j + 1U) * (q + 3U);
        cell1 ^= 13776561470214499436ULL + 3ULL * q;
    }
    if ((q & 5U) != 0) {
        cell0[0] ^= 16469043345442188988ULL + 3ULL * q;
        cell1 ^= 7182442279589044698ULL + 3ULL * q;
    }
    packet key = bennett_measure(&q);
    packet v0 = bennett_measure_bytes(&cell0[0], 2);
    packet v1 = bennett_measure_bytes(&cell1, 1);
    return key | (v0 << 8) | (v1 << 24);
}
"#,
        None,
    );
}

// A program the front must lower in all three modes: its values equal native C for every definite value of the
// register, nothing is released that is not zero, and its state before measurement is native C's table with the
// preparation's amplitudes and phases, every spare register at zero.
fn check_state(name: &str, case: &Case) {
    let source = write_c(name, &case.source());
    let native = native_build(&source, name, &["-DNATIVE"]);
    let table = case.native_table(&native);
    let check = case.state.as_ref().expect("a state check");
    for (mode, cflags, flags) in MODES {
        let name = format!("{name}-{mode}");
        let text = lower_text(&source, &name, cflags, flags).unwrap_or_else(|error| panic!("{name}: {error}"));
        let module = bennett_ir::load(&text).expect("valid program");
        compare(case, &module, &table, &mut 0).unwrap_or_else(|error| panic!("{name}: {error}"));
        state::compare(check, &module, &table, case.trips, 0, &mut state::Counts::default())
            .unwrap_or_else(|error| panic!("{name}: state: {error}"));
    }
}

// An update in a loop whose trip count depends on the register, of a variable the loop reads and overwrites:
// its old value was written before the loop, computed from the register, which the loop does not change. A gate
// on another variable (`r`, left at 0) keeps the earlier writes out of the loop's code. The loop is one applied
// function; the variable's old value is exchanged into a fresh register and cleared by computing it again.
fn loop_overwrite(
    name: &str,
    qubits: u32,
    globals: &str,
    locals: &str,
    before: &str,
    code: &str,
    objects: &[(&str, u32)],
) {
    let mut body = format!(
        "{globals}packet entry(unsigned prep, unsigned n, unsigned column) {{\n    unsigned char r = 0;\n{locals}{}    \
         ZBIT(q, 1);\n{before}\n    ZBIT(r, 0);\n    #pragma clang loop unroll(disable)\n{code}\n",
        prepare(qubits)
    );
    let objects: Vec<(String, u32)> = objects.iter().map(|&(object, bytes)| (object.to_owned(), bytes)).collect();
    let (packets, fields) = measure_objects(&mut body, &objects);
    let mut check = Check::new(fields);
    check.preparation.z = 2;
    let case = Case { state: Some(check), ..Case::values(body, qubits, 0, packets, true) };
    check_state(name, &case);
}

#[test]
fn loop_overwrite_of_a_value_stored_before_the_loop() {
    loop_overwrite(
        "loop-overwrite-straight",
        3,
        "uint16_t y = 9;\n",
        "",
        "    y = 7U * q + 1U;",
        "    for (unsigned t = q; t != 0; --t) y = y * 3U + t;",
        &[("y", 2)],
    );
}

// LLVM keeps these variables in registers through the loop (scalar promotion) and stores them after it: the
// loop writes nothing, and each store after it reads the old value of a variable stored just before.
#[test]
fn loop_overwrite_promoted_to_values_beside_an_accumulator() {
    loop_overwrite(
        "loop-overwrite-two",
        3,
        "uint32_t a = 5;\n",
        "    uint8_t z = 3;\n    uint16_t w = 0;\n",
        "    a = 11U * q + 2U;\n    z = q ^ 6U;",
        "    for (unsigned t = q + 1U; t != 0; t >>= 1) { a = a * 5U + t; z = (uint8_t)(z * z + t); w ^= t; }",
        &[("a", 4), ("z", 1), ("w", 2)],
    );
}

#[test]
fn loop_overwrite_of_values_an_earlier_region_wrote() {
    loop_overwrite(
        "loop-overwrite-after-region",
        3,
        "uint16_t y = 4;\nuint8_t u = 1;\nuint16_t w = 0;\n",
        "",
        "    if (q & 1U) { y = 5U * q + 1U; u = q + 7U; }",
        "    for (unsigned t = q; t != 0; --t) { y = y * 3U + t; u = (uint8_t)(u * t + 1U); w ^= t << 3; }",
        &[("y", 2), ("u", 1), ("w", 2)],
    );
}

#[test]
fn loop_overwrite_of_a_value_an_applied_call_returned() {
    loop_overwrite(
        "loop-overwrite-after-call",
        3,
        "uint16_t y = 4;\n__attribute__((noinline)) static uint16_t spread(unsigned x) { return (uint16_t)(x * 4099U + 17U); }\n",
        "",
        "    y = spread(q);",
        "    for (unsigned t = q; t != 0; --t) y = (uint16_t)(y * y + t);",
        &[("y", 2)],
    );
}

// The old value is neither known nor computable again (a gate changed the variable `x`): the fresh register keeps
// it and is released (the partial trace), with the warning and its count. For each definite value of the register
// the values are native C's; the reduced state of the kept registers keeps coherence exactly between the values
// whose old values of `x` are equal. The other variables are locals, measured as registers.
fn loop_retained(
    name: &str,
    globals: &str,
    before: &str,
    code: &str,
    objects: &[(&str, u32)],
    old: Vec<u64>,
    bits: u32,
) {
    let mut body = format!(
        "{globals}packet entry(unsigned prep, unsigned n, unsigned column) {{\n    uint{bits}_t x = 9;\n{}    \
         ZBIT(q, 1);\n{before}\n    XBIT(x, 0);\n    #pragma clang loop unroll(disable)\n{code}\n",
        prepare(3)
    );
    body.push_str("    packet key = bennett_measure(&q);\n    packet kept = bennett_measure(&x);\n");
    let mut packet = String::from("key | (kept << 8)");
    let mut shifts = vec![0, 8];
    let mut shift = 8 + bits;
    for (i, (variable, bytes)) in objects.iter().enumerate() {
        writeln!(body, "    packet v{i} = bennett_measure(&{variable});").expect("write measurement");
        write!(packet, " | (v{i} << {shift})").expect("pack measurement");
        shifts.push(shift);
        shift += 8 * bytes;
    }
    writeln!(body, "    return {packet};\n}}").expect("write return");
    let mut check = Check::new(shifts.iter().map(|&shift| Field { packet: 0, shift }).collect());
    check.preparation.z = 2;
    check.retained = Some(old);
    let case =
        Case { body, qubits: 3, trips: 0, packets: 1, expectation: Expectation::Retained(bits), state: Some(check) };
    let source = write_c(name, &case.source());
    let native = native_build(&source, name, &["-DNATIVE"]);
    let table = case.native_table(&native);
    for (mode, cflags, flags) in MODES {
        let name = format!("{name}-{mode}");
        let (text, diagnostics) = lower_text_with_diagnostics(&source, &name, cflags, flags)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        case.expectation.check_diagnostics(&diagnostics).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(diagnostics.contains("in a loop under the control of a value that depends on a qubit"), "{name}");
        let module = bennett_ir::load(&text).expect("valid program");
        compare(&case, &module, &table, &mut 0).unwrap_or_else(|error| panic!("{name}: {error}"));
        let check = case.state.as_ref().expect("a state check");
        state::compare(check, &module, &table, 0, bits, &mut state::Counts::default())
            .unwrap_or_else(|error| panic!("{name}: state: {error}"));
    }
}

#[test]
fn loop_overwrite_of_a_value_a_gate_changed_keeps_it() {
    loop_retained(
        "loop-retained-distinct",
        "",
        "    x = 7U * q + 1U;",
        "    for (unsigned t = q; t != 0; --t) x = x * 3U + t;",
        &[],
        (0..8).map(|q| (7 * q + 1) ^ 1).collect(),
        16,
    );
}

#[test]
fn loop_overwrite_keeps_coherence_between_equal_old_values() {
    loop_retained(
        "loop-retained-equal",
        "",
        "    x = 5U + (q & 1U);",
        "    for (unsigned t = q; t != 0; --t) x = (uint8_t)(x * x + t);",
        &[],
        (0..8).map(|q| (5 + (q & 1)) ^ 1).collect(),
        8,
    );
}

#[test]
fn loop_overwrite_keeps_one_old_value_and_clears_the_other() {
    loop_retained(
        "loop-retained-beside-cleared",
        "",
        "    x = 3U * q + 2U;\n    uint16_t y = 11U * q + 5U;",
        "    for (unsigned t = q; t != 0; --t) { x = (uint8_t)(x * 5U + t); y = (uint16_t)(y * y + x); }",
        &[("y", 2)],
        (0..8).map(|q| (3 * q + 2) ^ 1).collect(),
        8,
    );
}

// Generated case 253 of seed 0x0b7e11a5d1ff0007 (O2): the first region exchanges cell0, whose old value was
// stored before it, so cell0 then holds the region's final value, computed from that old value. LLVM forwards
// the value to the second region, which overwrites cell0: clearing the value it held computes the old value
// again too. Clean: nothing kept.
#[test]
fn later_region_overwrites_an_exchanged_value() {
    check_witness(
        "overwrite-after-exchange",
        2,
        r#"
static uint32_t cell0[2] = {3186690542ULL, 11};
static uint32_t cell1[2] = {11, 2704361132ULL};
packet entry(unsigned prep, unsigned n, unsigned column) {
    unsigned char q = PREPARE(prep);
    HBIT(q, 0);
    HBIT(q, 1);
    cell0[0] = 17ULL * q + 3;
    if ((q & 1U) != 0) {
        cell0[0] ^= 9969435993843179127ULL + 3ULL * q;
        cell1[1] ^= 14202344268543918937ULL;
        #pragma clang loop unroll(disable)
        for (unsigned j = 0; j < 3U; ++j) cell1[1] ^= (j + 1U) * (q + 3U);
    }
    if ((q & 2U) != 0) {
        cell0[0] ^= 3756836512046373768ULL + 3ULL * q;
    }
    packet key = bennett_measure(&q);
    packet v0 = bennett_measure_bytes(&cell0[0], 4);
    packet v1 = bennett_measure_bytes(&cell1[1], 4);
    return key | (v0 << 8) | (v1 << 40);
}
"#,
        None,
    );
}
