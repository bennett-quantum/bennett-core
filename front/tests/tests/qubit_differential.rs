//! Compare short C programs with native execution at every measured input value.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, lower_text, native_build, run_with_stats, write_c};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const SEED: u64 = 0x8719_6caa_251d_0f43;
const SHORT_CASES: usize = 48;
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
    clean: bool,
}

impl Case {
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
    failed: usize,
    declined: BTreeMap<String, usize>,
}

impl Results {
    fn print(&self, seed: u64, elapsed: Duration) {
        let declined: usize = self.declined.values().sum();
        println!(
            "seed={seed:#018x} generated={} accepted={} declined={declined} compared={} clean={} failed={} elapsed={:.2}s",
            self.generated,
            self.accepted,
            self.compared,
            self.clean,
            self.failed,
            elapsed.as_secs_f64()
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
            if case.clean && stats.nonzero_releases != 0 {
                return Err(format!(
                    "packet {column}, measurement seed {seed}, register={input}: {} nonzero releases (weight {})",
                    stats.nonzero_releases, stats.nonzero_release_weight
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
    let text = match lower_text(&source, name, cflags, flags) {
        Ok(text) => text,
        Err(error)
            if error.starts_with("not handled yet: ") || (error.starts_with("error: ") && error.contains(", at ")) =>
        {
            let reason = error.trim_end().rsplit_once(", at ").map_or(error.trim_end(), |(reason, _)| reason);
            *results.declined.entry(reason.to_owned()).or_default() += 1;
            return Ok(());
        }
        Err(error) => return Err(format!("front failed without a refusal: {error:?}")),
    };
    results.accepted += 1;
    results.clean += usize::from(case.clean);
    let module = bennett_ir::load(&text).map_err(|error| format!("front emitted an invalid program: {error}"))?;
    let native = native_build(&source, name, &["-DNATIVE"]);
    compare(case, &module, &case.native_table(&native), &mut results.compared)
}

fn prepare(qubits: u32) -> String {
    let mut out = String::from("    unsigned char q = PREPARE(prep);\n");
    for i in 0..qubits {
        writeln!(out, "    HBIT(q, {i});").expect("write gate");
    }
    out
}

// Each packet carries the measured q in its lowest byte and up to 15 bytes of object data.
fn measure_objects(body: &mut String, objects: &[(String, u32)]) -> u64 {
    body.push_str("    packet key = bennett_measure(&q);\n");
    for (i, (object, bytes)) in objects.iter().enumerate() {
        writeln!(body, "    packet v{i} = bennett_measure_bytes(&{object}, {bytes});").expect("write measurement");
    }
    let mut packets = vec![String::from("key")];
    let mut shift = 8;
    for (i, (_, bytes)) in objects.iter().enumerate() {
        if shift + 8 * bytes > 128 {
            packets.push(String::from("key"));
            shift = 8;
        }
        write!(packets.last_mut().expect("packet"), " | (v{i} << {shift})").expect("pack measurement");
        shift += 8 * bytes;
    }
    for (column, packet) in packets.iter().take(packets.len() - 1).enumerate() {
        writeln!(body, "    if (column == {column}) return {packet};").expect("select packet");
    }
    writeln!(body, "    return {};\n}}", packets.last().expect("last packet")).expect("write return");
    packets.len() as u64
}

fn condition(random: &mut ChaCha8Rng, qubits: u32) -> String {
    let mask = random.random_range(1..1_u32 << qubits);
    format!("(q & {mask}U) {} 0", if random.random_bool(0.25) { "==" } else { "!=" })
}

fn generate(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let mut body = String::new();
    let mut locals = String::new();
    let mut objects = Vec::new();
    for i in 0..random.random_range(1..=3) {
        let (bits, initial) = match random.random_range(0..4) {
            0 => (8, u64::from(random.random::<u8>())),
            1 => (16, u64::from(random.random::<u16>())),
            2 => (32, u64::from(random.random::<u32>())),
            _ => (64, random.random()),
        };
        let name = format!("cell{i}");
        let storage = random.random_range(0..3);
        let object = if storage == 2 {
            let index = random.random_range(0..2);
            let values = if index == 0 { format!("{initial}ULL, 11") } else { format!("11, {initial}ULL") };
            writeln!(body, "static uint{bits}_t {name}[2] = {{{values}}};").expect("write array");
            format!("{name}[{index}]")
        } else {
            let decl = if storage == 0 { &mut locals } else { &mut body };
            writeln!(decl, "    uint{bits}_t {name} = {initial}ULL;").expect("write object");
            name
        };
        objects.push((object, bits / 8));
    }

    let call = random.random_bool(0.3);
    if call {
        let expression = format!("((uint64_t)x * {}ULL) ^ {}ULL", random.random::<u32>(), random.random::<u64>());
        writeln!(body, "__attribute__((noinline)) static uint64_t helper(unsigned x) {{").expect("write helper");
        if random.random_bool(0.5) {
            writeln!(body, "    volatile uint64_t scratch = {expression}; scratch ^= 9; return scratch;")
                .expect("write helper");
        } else {
            writeln!(body, "    return {expression};").expect("write helper");
        }
        body.push_str("}\n");
    }
    body.push_str("packet entry(unsigned prep, unsigned n, unsigned column) {\n");
    body.push_str(&locals);
    body.push_str(&prepare(qubits));

    // A straight write before the first region brings that object into the region already held.
    let mut written = vec![false; objects.len()];
    let mut clean = true;
    if random.random_bool(0.45) {
        let i = random.random_range(0..objects.len());
        written[i] = true;
        let expression = if call { "helper(q)" } else { "17ULL * q + 3" };
        writeln!(body, "    {} = {expression};", objects[i].0).expect("write straight store");
    } else if call {
        let i = random.random_range(0..objects.len());
        written[i] = true;
        writeln!(body, "    {} = helper(q);", objects[i].0).expect("write applied call");
    }

    let regions = 1 + usize::from(random.random_bool(0.45));
    for _ in 0..regions {
        writeln!(body, "    if ({}) {{", condition(random, qubits)).expect("write condition");
        let mut region_writes = Vec::new();
        for _ in 0..random.random_range(1..=objects.len()) {
            let i = random.random_range(0..objects.len());
            // The clean check covers known starting values, including multiple writes in one
            // region. Repeated writes across separate regions can require unbuilt cleanup.
            clean &= !written[i];
            region_writes.push(i);
            let op = ["=", "=", "+=", "^=", "-="][random.random_range(0..5)];
            let value = random.random::<u64>();
            let expression =
                if random.random_bool(0.6) { format!("{value}ULL + 3ULL * q") } else { format!("{value}ULL") };
            writeln!(body, "        {} {op} {expression};", objects[i].0).expect("write update");
            if random.random_bool(0.28) {
                let bound = ["3U", "n", "(q & 3U)"][random.random_range(0..3)];
                body.push_str("        #pragma clang loop unroll(disable)\n");
                writeln!(
                    body,
                    "        for (unsigned j = 0; j < {bound}; ++j) {} ^= (j + 1U) * (q + 3U);",
                    objects[i].0
                )
                .expect("write loop");
            }
        }
        for i in region_writes {
            written[i] = true;
        }
        body.push_str("    }\n");
    }
    let packets = measure_objects(&mut body, &objects);
    Case { body, qubits, trips: random.random_range(0..=3), packets, clean }
}

// This fixed prefix precedes the known wrong results from a later conditional in-place
// update and from conditional stores merged by LLVM. The ignored witnesses exercise both.
#[test]
fn short_randomized_differential() {
    campaign("qubit-differential-short", SEED, Some(SHORT_CASES), None);
}

// Build the front with front/test.sh, then run:
// cargo test -p bennett-front-tests --test qubit_differential long_randomized_differential -- --ignored --exact --nocapture
// BENNETT_RANDOM_CASES caps the case count. BENNETT_RANDOM_SEED accepts a decimal or 0x-prefixed seed.
#[test]
#[ignore = "runs the differential generator for up to fifteen minutes"]
fn long_randomized_differential() {
    let limit = std::env::var("BENNETT_RANDOM_CASES").ok().map(|value| value.parse().expect("case count"));
    let seed = std::env::var("BENNETT_RANDOM_SEED").ok().map_or(SEED, |value| {
        value.strip_prefix("0x").map_or_else(|| value.parse(), |hex| u64::from_str_radix(hex, 16)).expect("seed")
    });
    campaign("qubit-differential-long", seed, limit, Some(LONG_TIME));
}

fn campaign(name: &str, seed: u64, limit: Option<usize>, time: Option<Duration>) {
    let mut random = ChaCha8Rng::seed_from_u64(seed);
    let start = Instant::now();
    let mut results = Results::default();
    println!("seed={seed:#018x} case limit={limit:?} time limit={time:?}");
    while limit.is_none_or(|limit| results.generated < limit) && time.is_none_or(|time| start.elapsed() < time) {
        let index = results.generated;
        let case = generate(&mut random);
        let mode = index % MODES.len();
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
    Case { body, qubits: 2, trips, packets, clean }
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
        let packets = measure_objects(&mut body, &[("a".into(), 2), ("b".into(), 1), ("c[1]".into(), 8)]);
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

fn check_witness(name: &str, qubits: u32, body: &str, refusal: Option<&str>) {
    let case = Case { body: body.to_owned(), qubits, trips: 0, packets: 1, clean: true };
    let source = write_c(name, &case.source());
    let text = match lower_text(&source, name, CLANG_FLAGS, &[]) {
        Ok(text) => text,
        Err(error) => {
            assert!(refusal.is_some_and(|reason| error.contains(reason)), "{name}: {error}");
            return;
        }
    };
    let module = bennett_ir::load(&text).expect("valid program");
    let native = native_build(&source, name, &["-DNATIVE"]);
    compare(&case, &module, &case.native_table(&native), &mut 0).unwrap_or_else(|error| panic!("{name}: {error}"));
}

#[test]
#[ignore = "the second region can add to the value from a write that did not run"]
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
#[ignore = "the front loses the value selected by the two conditional stores"]
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
#[ignore = "the front reports a memory change for a helper that reads only its own stack"]
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

#[test]
#[ignore = "the front reports two applied calls when only its outlined region and one C helper run"]
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
#[ignore = "the front asks for a measurement when the dead result of a private-stack helper is overwritten"]
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
