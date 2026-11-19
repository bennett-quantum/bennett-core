//! Native truth tables and pre-readout states for short coherent C fragments.

use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, lower, lower_text_with_diagnostics, native_build, run_with_stats, write_c,
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
        // Level-one return addresses need the native caller's frame to survive optimisation.
        let flags =
            ["-DNATIVE", "-fno-omit-frame-pointer", "-mno-omit-leaf-frame-pointer", "-fno-optimize-sibling-calls"];
        let executable = native_build(source, self.name, &flags);
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

// The ordinary code of a gate loop under a qubit-dependent condition runs in the control program for every
// input, also where C skips the loop. What it may contain is an allow-list (front/src/Unasked.h).
const UNASKED: &str = "not handled yet: in a loop with gates under the control of a value that depends on a qubit, whose ordinary code the control program runs for every input, also where C does not enter the loop: ";

fn unasked(what: &str) -> String {
    format!("{UNASKED}{what}")
}

// `PREFIX` runs under the qubit-dependent condition before the loop; `TEST` decides each pass's gate.
const GATE_LOOP: &str = r#"
    uint8_t y = 0;
    ZBIT(q, 1);
    if (q) {
        PREFIX
        #pragma clang loop unroll(disable)
        for (COUNTER)
            if (TEST) XBIT(y, 0);
    }
    packet key = READ(q);
    return key | ((packet)READ(y) << 8);
"#;

fn gate_loop(prefix: &str, counter: &str, test: &str) -> String {
    GATE_LOOP.replace("PREFIX", prefix).replace("COUNTER", counter).replace("TEST", test)
}

fn counted_gate_loop(test: &str) -> String {
    gate_loop("", "unsigned i = 0; i < n; ++i", test)
}

const UNCOUNTED: &str = "LLVM's loop analysis finds no number of passes for this loop";

fn lowering_diagnostics(program: &Program<'_>) -> Vec<String> {
    let source = program.source();
    MODES
        .iter()
        .map(|(mode, clang, flags)| {
            let name = format!("{}-{mode}-diagnostics", program.name);
            lower_text_with_diagnostics(&source, &name, clang, flags).expect("lowers").1
        })
        .collect()
}

#[test]
fn disabled_gate_loop_never_loads_a_held_byte() {
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
    .refused(1, &[65536, 65793], &unasked("a read of memory, which can fault there"));
}

#[test]
fn disabled_gate_loop_never_divides_by_zero() {
    let divisor = "(column + (prep != 1))";
    for (name, body, native, opcode) in [
        ("gate-divide", counted_gate_loop(&format!("((i + 1u) / {divisor}) & 1")), [0, 257], "udiv"),
        ("gate-remainder", counted_gate_loop(&format!("(i + 17u) % {divisor}")), [0, 1], "urem"),
        (
            "gate-divide-before-loop",
            gate_loop(&format!("unsigned m = n / {divisor};"), "unsigned i = 0; i < m; ++i", "1"),
            [0, 257],
            "udiv",
        ),
    ] {
        Program { name, declarations: "", body: &body, qubits: 1 }.refused(
            1,
            &native,
            &unasked(&format!("`{opcode}` whose divisor is not proved nonzero")),
        );
    }
}

#[test]
fn disabled_gate_loop_divides_by_a_proved_divisor() {
    for (name, test, n) in [
        ("gate-divide-constant", String::from("((i + 1u) / 3u) & 1"), 3),
        ("gate-divide-tested", String::from("d && ((i + 1u) / d) & 1"), 1),
    ] {
        let body = gate_loop("unsigned d = column + (prep != 1);", "unsigned i = 0; i < n; ++i", &test);
        Program { name, declarations: "", body: &body, qubits: 1 }.agrees(n, &[0, 257], &readout(&[0, 8]), 0);
    }
    // A divisor whose range excludes 0 (1 to 256), and one an ordinary test outside the qubit's condition guards.
    let counted = "unsigned i = 0; i < 3 + column; ++i";
    let test = "((i + 17u) / d) & 1";
    let ranged = gate_loop("", counted, test).replace("if (q) {", "unsigned d = (n & 255u) + 1u;\n    if (q) {");
    Program { name: "gate-divide-ranged", declarations: "", body: &ranged, qubits: 1 }.agrees(
        2,
        &[0, 257],
        &readout(&[0, 8]),
        0,
    );
    let guarded = gate_loop("", counted, test).replace("if (q) {", "unsigned d = n;\n    if (n) if (q) {");
    Program { name: "gate-divide-guarded", declarations: "", body: &guarded, qubits: 1 }.agrees(
        3,
        &[0, 257],
        &readout(&[0, 8]),
        0,
    );
}

#[test]
fn disabled_gate_loop_never_divides_by_a_shift_the_device_may_make_zero() {
    // LLVM's range of `-3 >> n` is -3 to -1 (a shift by 32 or more is poison to LLVM); the device gives 0 there.
    let body = gate_loop("int d = -3 >> n;", "unsigned i = 0; i < 3 + column; ++i", "(((int)i + 17) / d) & 1");
    Program { name: "gate-divide-shifted", declarations: "", body: &body, qubits: 1 }.refused(
        0,
        &[0, 257],
        &unasked("`sdiv` whose divisor is not proved nonzero"),
    );
}

#[test]
fn disabled_gate_loop_never_divides_after_a_test_whose_paths_rejoin() {
    // Both edges of the divisor's test reach the division's own test: the test does not guard the division.
    let body = r#"
    uint8_t y = 0;
    ZBIT(q, 1);
    if (q) {
        unsigned d = column + (prep != 1);
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i) {
            if (d) XBIT(y, 1);
            if ((i ^ prep) & 1) {
                if (((i + 17u) / d) & 1) XBIT(y, 0);
            }
        }
    }
    packet key = READ(q);
    return key | ((packet)READ(y) << 8);
"#;
    Program { name: "gate-divide-rejoined", declarations: "", body, qubits: 1 }.refused(
        1,
        &[0, 513],
        &unasked("`udiv` whose divisor is not proved nonzero"),
    );
}

#[test]
fn disabled_gate_loop_never_divides_after_a_test_on_another_path() {
    // Odd passes test the divisor, even passes divide by it: the test is not on the path to the division.
    let body = gate_loop("", "unsigned i = 0; i < n; ++i", "(i & 1) ? d != 0 : ((i + 17u) / d) & 1")
        .replace("if (q) {", "unsigned d = column + (prep != 1);\n    if (q) {");
    Program { name: "gate-divide-wrong-edge", declarations: "", body: &body, qubits: 1 }.refused(
        1,
        &[0, 257],
        &unasked("`udiv` whose divisor is not proved nonzero"),
    );
}

#[test]
fn disabled_gate_loop_never_calls() {
    let body = counted_gate_loop("pick(i, column + (prep != 1)) & 1");
    Program {
        name: "gate-call",
        declarations: "__attribute__((noinline)) unsigned pick(unsigned i, unsigned d) { return (i + 1u) / d; }",
        body: &body,
        qubits: 1,
    }
    .refused(1, &[0, 257], &unasked("a call to @pick"));
}

#[test]
fn disabled_gate_loop_never_traps() {
    let body = counted_gate_loop("i > 1000 ? (__builtin_trap(), 0) : 1");
    // The trap leaves the code under the qubit without rejoining, which is refused before the loop is looked at.
    Program { name: "gate-trap", declarations: "", body: &body, qubits: 1 }.refused(
        1,
        &[0, 257],
        "not handled yet: a branch on a value that depends on a qubit, whose arms do not rejoin",
    );
}

#[test]
fn gate_loops_with_large_ordinary_counts_are_issued_in_full() {
    // Each count is ordinary; where C skips the loop the device still runs its passes, gates disabled.
    for (name, prefix, counter, n) in [
        ("gate-count-wraps", "", "uint64_t i = (prep == 1 ? 2 : 0); i != n; ++i", 3),
        ("gate-count-wide", "", "uint64_t i = 0; i < n; ++i", 1),
        ("gate-count-prior-local", "", "unsigned i = 0; i < local; ++i", 1),
    ] {
        let mut body = gate_loop(prefix, counter, "1");
        if name == "gate-count-prior-local" {
            body = format!("    volatile unsigned b = n; unsigned local = b;\n{body}");
        }
        let program = Program { name, declarations: "", body: &body, qubits: 1 };
        program.agrees(n, &[0, 257], &readout(&[0, 8]), 0);
        for diagnostics in lowering_diagnostics(&program) {
            assert!(!diagnostics.contains(UNCOUNTED), "{name}: {diagnostics}");
        }
    }
    let body = counted_gate_loop("1").replace("if (q) {", "if (q == n) {");
    let program = Program { name: "gate-count-equal", declarations: "", body: &body, qubits: 1 };
    program.agrees(1, &[0, 257], &readout(&[0, 8]), 0);
    for diagnostics in lowering_diagnostics(&program) {
        assert!(!diagnostics.contains(UNCOUNTED), "gate-count-equal: {diagnostics}");
    }
}

#[test]
fn an_uncounted_loop_nested_in_a_counted_gate_loop_is_warned_about() {
    let body = gate_loop(
        "",
        "unsigned i = 0; i < 2 + column; ++i",
        "1) {\n            #pragma clang loop unroll(disable)\n            for (unsigned j = 0; j != 1; j += n) XBIT(y, 0);\n        } if (0",
    );
    let program = Program { name: "gate-count-nested", declarations: "", body: &body, qubits: 1 };
    program.agrees(1, &[0, 1], &readout(&[0, 8]), 0);
    for diagnostics in lowering_diagnostics(&program) {
        assert!(diagnostics.contains(UNCOUNTED), "gate-count-nested: {diagnostics}");
    }
}

#[test]
fn an_ordinary_guard_inside_the_qubit_condition_may_be_folded_and_the_warning_remains() {
    // LLVM folds `if (n)` into the condition `q & n` around it; the control program then runs the loop also for
    // n = 0, where it never ends (not run here), and the warning is what the programmer gets.
    let body = gate_loop("", "unsigned j = 0; j != 1; j += n", "1").replace("if (q) {", "if (q & n) if (n) {");
    let program = Program { name: "gate-count-folded-guard", declarations: "", body: &body, qubits: 1 };
    program.agrees(1, &[0, 257], &readout(&[0, 8]), 0);
    for diagnostics in lowering_diagnostics(&program) {
        assert!(diagnostics.contains(UNCOUNTED), "gate-count-folded-guard: {diagnostics}");
    }
}

#[test]
fn a_gate_loop_without_a_found_count_is_issued_with_a_warning() {
    // With a stride of 2 the loop never ends once entered, so the device does not end where C skips it.
    let body = gate_loop("", "unsigned i = 0; i != 1; i += n", "1");
    let program = Program { name: "gate-count-unknown", declarations: "", body: &body, qubits: 1 };
    program.agrees(1, &[0, 257], &readout(&[0, 8]), 0);
    for diagnostics in lowering_diagnostics(&program) {
        assert!(diagnostics.contains(UNCOUNTED), "{diagnostics}");
        assert!(diagnostics.contains("it must end for every ordinary input"), "{diagnostics}");
    }
}

#[test]
#[ignore = "LLVM's optimiser reads the held byte before the qubit test that guards it, with or without a loop; needs the front to classify accesses before optimising"]
fn disabled_gate_loop_never_reads_a_held_byte_to_prepare_control() {
    Program {
        name: "gate-control-load",
        declarations: r#"
uint8_t held = 1;
__attribute__((noinline)) void fill(void) {
    uint8_t r = 0;
    XBIT(r, 0);
    held = r;
    XBIT(r, 0);
}
"#,
        body: r#"
    uint8_t y = 0;
    if (prep == 1) fill();
    ZBIT(q, 1);
    if (q && held) {
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i < n; ++i) XBIT(y, 0);
    }
    packet key = READ(q);
    packet target = READ(y);
    return key | (target << 8) | ((packet)READ_BYTES(held, 1) << 16);
"#,
        qubits: 1,
    }
    .agrees(1, &[65536, 65793], &readout(&[0, 8, 16]), 0);
}

#[test]
#[ignore = "moving the helper into an applied function changes its caller's return address"]
fn moved_helper_observes_the_original_callers_return_address() {
    Program {
        name: "return-address-ancestor",
        declarations: r#"
uintptr_t expected;
__attribute__((noinline)) uint8_t check_caller(uint8_t q) {
    return 5 + (q != 0 && (uintptr_t)__builtin_return_address(1) == expected);
}
"#,
        body: r#"
    expected = (uintptr_t)__builtin_return_address(0);
    uint8_t x = check_caller(q);
    packet value = READ(x);
    return value | ((packet)READ(q) << 8);
"#,
        qubits: 1,
    }
    .agrees(0, &[5, 6 | (1 << 8)], &readout(&[0, 8]), 0);
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

#[test]
fn replay_of_a_nonzero_return_address_test_clears_the_predicate() {
    Program {
        name: "return-address-predicate",
        declarations: RETADDR_HELPER,
        body: &RETADDR_USE.replace("QUALIFIER", ""),
        qubits: 1,
    }
    .agrees(0, &[256, 257], &return_address_readout(), 0);
}

#[test]
#[ignore = "replaying the full return address at a second call site leaves a nonzero word"]
fn replay_of_a_full_return_address_clears_the_word() {
    Program {
        name: "return-address-word",
        declarations: RETADDR_HELPER,
        body: &RETADDR_USE.replace("QUALIFIER", "volatile"),
        qubits: 1,
    }
    .agrees(0, &[256, 257], &return_address_readout(), 0);
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

fn compare_ordinary_inputs(name: &str, declarations: &str, body: &str, expected: [u128; 2]) {
    let source = write_c(
        name,
        &format!(
            "{PRELUDE}{declarations}\npacket entry(unsigned prep, unsigned n, unsigned column) {{\n{body}\n}}\n{DRIVER}"
        ),
    );
    let executable = native_build(&source, name, &["-DNATIVE"]);
    let native = [0, 1].map(|n| {
        let output = Command::new(&executable).args(["0", &n.to_string()]).output().expect("run native C");
        assert!(output.status.success(), "{name} n={n}: {output:?}");
        let stdout = std::str::from_utf8(&output.stdout).expect("native stdout");
        u128::from_str_radix(stdout.trim(), 16).expect("native packet")
    });
    assert_eq!(native, expected, "{name}");

    let mut failures = Vec::new();
    for (mode, clang, flags) in MODES {
        let name = format!("{name}-{mode}");
        let module = lower(&source, &name, clang, flags);
        for (n, wanted) in [0, 1].into_iter().zip(native) {
            let (found, _) = run_with_stats(&module, "entry", &[0, n, 0], 0);
            let comparison = format!("{name} n={n}: native={wanted}, device={found:?}");
            println!("{comparison}");
            if found != Outcome::Returned(Some(wanted)) {
                failures.push(comparison);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "dropping a qubit argument lets one of two applied calls leave its write behind"]
fn both_qubit_dependent_calls_undo_their_writes() {
    compare_ordinary_inputs(
        "two-call-writes",
        r#"
unsigned input_seen, successor_seen;

__attribute__((noinline)) static unsigned record_input(unsigned x) {
    input_seen = 1;
    return x;
}

__attribute__((noinline)) unsigned record_successor(unsigned x) {
    successor_seen = 1;
    return x + 1;
}
"#,
        r#"
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    ZBIT(y, 0);
    y ^= record_input(q);
    y ^= record_successor(q);
    unsigned value = READ(y);
    return value + 16u * (n && input_seen == successor_seen);
"#,
        [3, 19],
    );
}

#[test]
#[ignore = "merging ordinary and qubit-dependent calls undoes the ordinary call's write"]
fn an_ordinary_call_selected_by_a_condition_keeps_its_write() {
    compare_ordinary_inputs(
        "conditional-call-write",
        r#"
unsigned recorded;

__attribute__((noinline)) unsigned record_call(unsigned x) {
    recorded = 7;
    return x + 1;
}
"#,
        r#"
    uint8_t q = 0, y = 0;
    XBIT(q, 0);
    ZBIT(y, 0);
    unsigned result;
    if (n) result = record_call(q);
    else result = record_call(2);
    y ^= result;
    XBIT(q, 0);
    unsigned value = READ(y);
    if (n) return value;
    return value + 16u * recorded;
"#,
        [115, 2],
    );
}

const READ_STORED_BYTE: &str = r#"
    store_byte(n);
#ifndef NATIVE
    if (n) return bennett_measure_bytes((const void *)&held, sizeof held);
#endif
    return held;
"#;

#[test]
#[ignore = "merging ordinary and qubit-dependent stores makes both paths leave qubits in the byte"]
fn a_constant_store_selected_by_a_condition_keeps_the_byte_ordinary() {
    compare_ordinary_inputs(
        "conditional-byte-value",
        r#"
volatile uint8_t held;

__attribute__((noinline)) void store_byte(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (n) held = q;
    else held = 2;
    XBIT(q, 0);
}
"#,
        READ_STORED_BYTE,
        [2, 1],
    );
}

#[test]
#[ignore = "exchanging the byte before the store condition leaves it holding qubits on the skipped path"]
fn skipping_a_qubit_dependent_store_keeps_the_byte_ordinary() {
    compare_ordinary_inputs(
        "conditional-byte-store",
        r#"
volatile uint8_t held;

__attribute__((noinline)) void store_byte(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (n) held = q + 1;
    XBIT(q, 0);
}
"#,
        READ_STORED_BYTE,
        [0, 2],
    );
}
