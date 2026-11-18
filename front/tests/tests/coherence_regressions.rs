//! Native truth tables and pre-readout states for short coherent C fragments.

use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, lower_text_with_diagnostics, native_build, run_with_stats, write_c,
};

#[path = "qubit_differential/state.rs"]
mod state;

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];

const PRELUDE: &str = r#"
#include <stdint.h>
typedef unsigned __int128 packet;
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
    packet value = entry(strtoul(argv[1], 0, 0), strtoul(argv[2], 0, 0), 0);
    printf("%016llx%016llx\n", (unsigned long long)(value >> 64), (unsigned long long)value);
}
#endif
"#;

struct Program<'a> {
    name: &'a str,
    declarations: &'a str,
    body: &'a str,
    qubits: u32,
}

impl Program<'_> {
    fn source(&self) -> std::path::PathBuf {
        // prep=0 prepares equal amplitudes. prep=1+q prepares the exact definite input q.
        let mut prepare = String::from("    uint8_t q = 0;\n    if (prep) {\n");
        for bit in 0..self.qubits {
            prepare.push_str(&format!("        if ((prep - 1) & (1u << {bit})) XBIT(q, {bit});\n"));
        }
        prepare.push_str("    } else {\n");
        for bit in 0..self.qubits {
            prepare.push_str(&format!("        HBIT(q, {bit});\n"));
        }
        prepare.push_str("    }\n");
        write_c(
            self.name,
            &format!(
                "{PRELUDE}{}\npacket entry(unsigned prep, unsigned n, unsigned column) {{\n{prepare}{}\n}}\n{DRIVER}",
                self.declarations, self.body
            ),
        )
    }

    fn native_table(&self, source: &std::path::Path, n: u64) -> Vec<u128> {
        let executable = native_build(source, self.name, &["-DNATIVE"]);
        (1..=1 << self.qubits)
            .map(|prep| {
                let output =
                    Command::new(&executable).args([prep.to_string(), n.to_string()]).output().expect("run native C");
                assert!(output.status.success(), "{} prep={prep}: {output:?}", self.name);
                let stdout = std::str::from_utf8(&output.stdout).expect("native stdout");
                u128::from_str_radix(stdout.trim(), 16).expect("native packet")
            })
            .collect()
    }

    fn refused(&self, n: u64, native: &[u128], reason: &str) {
        let source = self.source();
        assert_eq!(self.native_table(&source, n), native, "{}", self.name);
        for (mode, clang, flags) in MODES {
            let name = format!("{}-{mode}", self.name);
            let refusal = lower_text_with_diagnostics(&source, &name, clang, flags).expect_err("named refusal");
            println!("{name}: {refusal}");
            assert!(refusal.starts_with(reason), "{name}: {refusal}");
        }
    }

    fn agrees(&self, n: u64, native: &[u128], readout: &state::Check, retained_bits: u32) {
        let source = self.source();
        let table = self.native_table(&source, n);
        assert_eq!(table, native, "{}", self.name);
        println!("{} native: {table:?}", self.name);
        let mut failures = Vec::new();
        for (mode, clang, flags) in MODES {
            let name = format!("{}-{mode}", self.name);
            let (text, diagnostics) = lower_text_with_diagnostics(&source, &name, clang, flags).expect("lowers");
            let module = bennett_ir::load(&text).expect("valid device program");
            for (input, &wanted) in table.iter().enumerate() {
                let (found, stats) = run_with_stats(&module, "entry", &[input as u64 + 1, n, 0], 0);
                let released_nonzero = readout.retained.as_ref().is_some_and(|old| old[input] != 0);
                let release = (u64::from(released_nonzero), if released_nonzero { retained_bits } else { 0 });
                println!(
                    "{name} q={input}: {found:?}, releases={}, kept={}",
                    stats.nonzero_releases, stats.leftover_qubits
                );
                if found != Outcome::Returned(Some(wanted))
                    || (stats.nonzero_releases, stats.leftover_qubits) != release
                {
                    failures
                        .push(format!("{name} q={input}: native={wanted:#x}, device={found:?}, releases={stats:?}"));
                }
            }
            let result = state::compare(
                readout,
                &module,
                std::slice::from_ref(&table),
                n,
                retained_bits,
                &mut state::Counts::default(),
            );
            println!("{name} state: {result:?}");
            if let Err(error) = result {
                failures.push(format!("{name} state: {error}"));
            }
            if retained_bits != 0 && !diagnostics.contains(&format!("keeps the old value in {retained_bits} qubits")) {
                failures.push(format!("{name} missing retention warning: {diagnostics}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

fn readout(shifts: &[u32]) -> state::Check {
    state::Check::new(shifts.iter().map(|&shift| state::Field { packet: 0, shift }).collect())
}

const ACCUMULATOR: &str = r#"
    w = START;
    SEPARATOR
    GUARD {
        #pragma clang loop unroll(disable)
        for (unsigned t = COUNT; t; --t)
            w ^= ((unsigned char *)&w)[OFFSET];
    }
    packet key = READ(q);
    return key | ((packet)READ_BYTES(w, 2) << 8);
"#;
const SEPARATOR: &str = "uint8_t z = 0; ZBIT(z, 0);";

fn accumulator(start: &str, separator: &str, guard: &str, count: &str, offset: &str) -> String {
    ACCUMULATOR
        .replace("START", start)
        .replace("SEPARATOR", separator)
        .replace("GUARD", guard)
        .replace("COUNT", count)
        .replace("OFFSET", offset)
}

#[test]
fn variable_offset_into_held_accumulator_is_refused() {
    for (name, separator, guard, count, n) in [
        ("accumulator-variable", SEPARATOR, "", "q", 0),
        ("accumulator-no-separator", "", "", "q", 0),
        ("accumulator-guarded", SEPARATOR, "if (q)", "q", 0),
        ("accumulator-ordinary-count", SEPARATOR, "if (q)", "n", 1),
    ] {
        Program {
            name,
            declarations: "uint16_t w;",
            body: &accumulator("0x100u | q", separator, guard, count, "t & 1u"),
            qubits: 1,
        }
        .refused(
            n,
            &[65536, 65537],
            "not handled yet: an access at a position that is not constant into 2 bytes at @w+0, which hold qubits, outside code under a qubit applied whole",
        );
    }
}

#[test]
fn variable_offset_into_scalar_qubit_accumulator_is_refused() {
    let body = accumulator("0x100u | q", SEPARATOR, "if (q)", "q", "t & 1u")
        .replace("w = ", "uint16_t w = ")
        .replace("READ_BYTES(w, 2)", "READ(w)");
    Program { name: "accumulator-local", declarations: "", body: &body, qubits: 1 }.refused(
        0,
        &[65536, 65537],
        "not handled yet: the address of qubit variable 'w' is used by ordinary code",
    );
}

#[test]
fn fixed_offset_accumulator_has_the_native_result_and_state() {
    Program {
        name: "accumulator-fixed",
        declarations: "uint16_t w;",
        body: &accumulator("0x100u | q", SEPARATOR, "", "q", "1"),
        qubits: 1,
    }
    .agrees(0, &[65536, 65537], &readout(&[0, 8]), 0);
}

#[test]
fn variable_offset_accumulator_with_known_start_has_the_native_result_and_state() {
    Program {
        name: "accumulator-known",
        declarations: "uint16_t w;",
        body: &accumulator("0x101u", SEPARATOR, "if (q)", "q", "t & 1u"),
        qubits: 1,
    }
    .agrees(0, &[65792, 65537], &readout(&[0, 8]), 0);
}

fn mixed_store(name: &str, separator: &str, op: &str, words: [u128; 4], retained: bool) {
    let read_x = if retained { "READ(x)" } else { "READ_BYTES(x, 1)" };
    let body = format!(
        "uint8_t x = 0; if (q & 2) x = 2; {separator} if (q & 1) x {op} q;\n    \
         packet key = READ(q); return key | ((packet){read_x} << 8);"
    );
    let native: Vec<_> = words.into_iter().enumerate().map(|(q, x)| q as u128 | (x << 8)).collect();
    let mut check = readout(&[0, 8]);
    check.retained = retained.then(|| vec![0, 0, 2, 2]);
    Program { name, declarations: "", body: &body, qubits: 2 }.agrees(0, &native, &check, if retained { 8 } else { 0 });
}

#[test]
fn mixed_assignment_preserves_the_coherences() {
    mixed_store("mixed-assign", "", "=", [0, 1, 2, 3], false);
}

#[test]
fn mixed_addition_preserves_the_coherences() {
    mixed_store("mixed-add", "", "+=", [0, 1, 2, 5], false);
}

#[test]
fn mixed_subtraction_preserves_the_coherences() {
    mixed_store("mixed-subtract", "", "-=", [0, 255, 2, 255], false);
}

#[test]
fn mixed_assignment_after_phase_gate_traces_the_old_word() {
    mixed_store("mixed-retained", "ZBIT(x, 0);", "=", [0, 1, 2, 3], true);
}

const LOAD_LOOP: &str = r#"
    uint8_t y = 0;
    INITIALIZE
    if (q) {
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i)
            if (held[i]) XBIT(y, 0);
    }
    packet key = READ(q);
    return key | ((packet)READ(y) << 8);
"#;

#[test]
fn gate_loop_with_a_held_load_in_the_same_function_is_refused() {
    Program {
        name: "gate-load-same-function",
        declarations: "uint8_t held[2];",
        body: &LOAD_LOOP.replace("INITIALIZE", "held[0] = q + 1u;"),
        qubits: 1,
    }
    .refused(
        1,
        &[0, 257],
        "not handled yet: an access of memory that holds qubits in 1 byte at @held+0 only in part",
    );
}

#[test]
fn moved_gate_loop_with_a_held_load_is_refused() {
    Program {
        name: "gate-load-callee",
        declarations: r#"
uint8_t held[2] = {0, 1};
__attribute__((noinline)) void fill(void) {
    uint8_t r = 0;
    XBIT(r, 0);
    held[0] = r;
    XBIT(r, 0);
}
"#,
        body: r#"
    uint8_t y = 0;
    fill();
    ZBIT(q, 1);
    if (q) {
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i)
            if (held[i + column + (prep != 1)]) XBIT(y, 0);
    }
    packet key = READ(q);
    packet target = READ(y);
    return key | (target << 8) | ((packet)READ_BYTES(held[0], 1) << 16);
"#,
        qubits: 1,
    }
    .refused(
        1,
        &[65536, 65793],
        "not handled yet: a moved gate loop would execute ordinary `load` even where its gates are disabled, without proof that it is safe and has no visible effect (route: compute the ordinary part inside one applied function for the table of control bits)",
    );
}

#[test]
fn moved_gate_loop_with_an_unproved_divisor_is_refused() {
    Program {
        name: "gate-divide",
        declarations: "",
        body: r#"
    uint8_t y = 0;
    ZBIT(q, 1);
    if (q) {
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i)
            if (((i + 1u) / (column + (prep != 1))) & 1) XBIT(y, 0);
    }
    packet key = READ(q);
    return key | ((packet)READ(y) << 8);
"#,
        qubits: 1,
    }
    .refused(
        1,
        &[0, 257],
        "not handled yet: a moved gate loop would execute ordinary `udiv` even where its gates are disabled, without proof that it is safe and has no visible effect (route: compute the ordinary part inside one applied function for the table of control bits)",
    );
}

#[test]
fn moved_gate_loop_with_a_provably_nonzero_divisor_stays_coherent() {
    Program {
        name: "gate-proved-divide",
        declarations: "",
        body: r#"
    uint8_t y = 0;
    if (q) {
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i)
            if (((i + 1u) / (column | 1u)) & 1) XBIT(y, 0);
    }
    packet key = READ(q);
    return key | ((packet)READ(y) << 8);
"#,
        qubits: 1,
    }
    .agrees(1, &[0, 257], &readout(&[0, 8]), 0);
}

#[test]
fn moved_gate_loop_with_unproved_termination_is_refused() {
    Program {
        name: "gate-unproved-termination",
        declarations: "",
        body: r#"
    uint8_t y = 0;
    if (q) {
        #pragma clang loop unroll(disable)
        for (unsigned i = 1; i != n; i *= 2)
            XBIT(y, 0);
    }
    packet key = READ(q);
    return key | ((packet)READ(y) << 8);
"#,
        qubits: 1,
    }
    .refused(
        8,
        &[0, 257],
        "not handled yet: a moved gate loop whose ordinary passes are not proved to terminate without counter wrap where its gates are disabled",
    );
}

const RETADDR_HELPER: &str = r#"
__attribute__((noinline)) uintptr_t f(uint8_t q) {
    return q ? (uintptr_t)__builtin_return_address(0) : 0;
}
"#;
const RETADDR_USE: &str = r#"
    uint8_t y = 1;
    QUALIFIER uint64_t x = f(q);
    if (x) ZBIT(y, 0);
    x = 0;
    packet key = READ(q);
    packet target = READ(y);
    return key | (target << 8) | ((packet)READ(x) << 16);
"#;

fn return_address_readout() -> state::Check {
    let mut check = readout(&[0, 8, 16]);
    // The phase on the nonzero input is negative because the target is one.
    check.preparation.z = 1;
    check
}

const CALLER_REPLAY_REFUSAL: &str = "not handled yet: replaying a computation that can observe its caller's position through a return address (route: keep the old value and release it, or pass the caller's identity as an argument)";

#[test]
fn clearing_a_predicate_by_replaying_a_return_address_is_refused() {
    Program {
        name: "return-address-predicate",
        declarations: RETADDR_HELPER,
        body: &RETADDR_USE.replace("QUALIFIER", ""),
        qubits: 1,
    }
    .refused(0, &[256, 257], CALLER_REPLAY_REFUSAL);
}

#[test]
fn clearing_a_word_by_replaying_a_return_address_is_refused() {
    Program {
        name: "return-address-word",
        declarations: RETADDR_HELPER,
        body: &RETADDR_USE.replace("QUALIFIER", "volatile"),
        qubits: 1,
    }
    .refused(0, &[256, 257], CALLER_REPLAY_REFUSAL);
}

#[test]
fn clearing_a_word_by_replaying_a_callee_that_reads_a_return_address_is_refused() {
    Program {
        name: "return-address-transitive",
        declarations: r#"
__attribute__((noinline)) uintptr_t observe(void) {
    return (uintptr_t)__builtin_return_address(0);
}
__attribute__((noinline)) uintptr_t f(uint8_t q) {
    return q ? observe() : 0;
}
"#,
        body: &RETADDR_USE.replace("QUALIFIER", "volatile"),
        qubits: 1,
    }
    .refused(0, &[256, 257], CALLER_REPLAY_REFUSAL);
}

#[test]
fn replay_of_a_caller_independent_helper_clears_the_word() {
    Program {
        name: "caller-independent-word",
        declarations: "__attribute__((noinline)) uintptr_t f(uint8_t q) { return 3u * q; }",
        body: &RETADDR_USE.replace("QUALIFIER", "volatile"),
        qubits: 1,
    }
    .agrees(0, &[256, 257], &return_address_readout(), 0);
}

#[test]
fn inverting_an_update_that_replays_a_return_address_is_refused() {
    Program {
        name: "return-address-inverse",
        declarations: RETADDR_HELPER,
        body: r#"
    uint64_t x = 4;
    ZBIT(x, 0);
    x += f(q);
    packet key = READ(q);
    return key | ((packet)(READ(x) != 0) << 8);
"#,
        qubits: 1,
    }
    .refused(0, &[256, 257], CALLER_REPLAY_REFUSAL);
}

#[test]
fn clearing_a_gate_loop_condition_by_replaying_a_return_address_is_refused() {
    Program {
        name: "return-address-loop-predicate",
        declarations: RETADDR_HELPER,
        body: r#"
    uint8_t y = 0;
    if (f(q)) {
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i) XBIT(y, 0);
    }
    packet key = READ(q);
    return key | ((packet)READ(y) << 8);
"#,
        qubits: 1,
    }
    .refused(1, &[0, 257], CALLER_REPLAY_REFUSAL);
}

#[test]
fn clearing_an_exchanged_word_by_replaying_a_return_address_is_refused() {
    Program {
        name: "return-address-exchanged-word",
        declarations: &format!("{RETADDR_HELPER}\nuint64_t x;"),
        body: r#"
    x = f(q);
    uint8_t fence = 0;
    ZBIT(fence, 0);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t; --t)
        x /= 3u;
    packet key = READ(q);
    return key | ((packet)(READ_BYTES(x, 8) != 0) << 8);
"#,
        qubits: 1,
    }
    .refused(0, &[0, 257], CALLER_REPLAY_REFUSAL);
}

#[test]
fn repeating_a_region_that_reads_a_return_address_for_multiple_results_is_refused() {
    Program {
        name: "return-address-multiple-results",
        declarations: &format!("{RETADDR_HELPER}\nuint64_t x; uint8_t y;"),
        body: r#"
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t; --t) {
        x += f(t);
        y += t;
    }
    packet key = READ(q);
    packet first = READ_BYTES(x, 8) != 0;
    return key | (first << 8) | ((packet)READ_BYTES(y, 1) << 16);
"#,
        qubits: 1,
    }
    .refused(0, &[0, 65793], CALLER_REPLAY_REFUSAL);
}

#[test]
fn overwriting_a_return_address_after_a_gate_traces_it() {
    let mut check = readout(&[0, 8]);
    // The nonzero value is an opaque device address. Only its equality to zero is needed.
    check.retained = Some(vec![0, 1]);
    Program {
        name: "return-address-retained",
        declarations: RETADDR_HELPER,
        body: r#"
    uint64_t x = f(q);
    ZBIT(x, 63);
    x = 0;
    packet key = READ(q);
    return key | ((packet)READ(x) << 8);
"#,
        qubits: 1,
    }
    .agrees(0, &[0, 1], &check, 64);
}

#[test]
fn moved_gate_loop_cannot_wrap_a_counter_past_its_exit() {
    Program {
        name: "gate-counter-wrap",
        declarations: "",
        body: r#"
    ZBIT(q, 1);
    uint8_t y = 0;
    if (q) {
        #pragma clang loop unroll(disable)
        for (uint64_t i = prep == 1 ? 2 : 0; i != n; ++i) XBIT(y, 0);
    }
    return READ(y);
"#,
        qubits: 1,
    }
    .refused(
        1,
        &[0, 1],
        "not handled yet: a moved gate loop whose ordinary passes are not proved to terminate without counter wrap",
    );
}
