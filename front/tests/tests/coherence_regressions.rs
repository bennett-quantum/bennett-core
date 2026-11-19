//! Native truth tables and pre-readout states for short coherent C fragments.

use std::process::Command;

use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, entry, lower_text_with_diagnostics, native_build, options, run_with_stats, write_c,
};

#[path = "qubit_differential/state.rs"]
mod state;

type Mode<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);
const MODES: [Mode; 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];
// What the language promises for C compiled with the project's flags; IR that clang -O2 changed first is
// translated as it is handed over (docs/DESIGN.md, "What LLVM's passes may not change").
const PROJECT_MODES: [Mode; 2] = [MODES[0], MODES[2]];

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
#[ignore = "refused by name: preparing the gate loop's control reads memory, which the control program would run unasked"]
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
    .refused(
        0,
        &[5, 6 | (1 << 8)],
        "not handled yet: an applied call that may reach `__builtin_return_address` above level 0",
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
    compare_ordinary_inputs_keeping(name, declarations, body, expected, false);
}

// With `keeps`, an overwrite whose old value cannot be cleared releases it, at most one register per run, and the
// front warns that it keeps the old value; otherwise every released register is 0.
fn compare_ordinary_inputs_keeping(name: &str, declarations: &str, body: &str, expected: [u128; 2], keeps: bool) {
    compare_ordinary_inputs_in(name, declarations, body, expected, keeps, &MODES);
}

fn compare_ordinary_inputs_in(
    name: &str,
    declarations: &str,
    body: &str,
    expected: [u128; 2],
    keeps: bool,
    modes: &[Mode],
) {
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
    for &(mode, clang, flags) in modes {
        let name = format!("{name}-{mode}");
        let (text, diagnostics) = lower_text_with_diagnostics(&source, &name, clang, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid device program");
        if keeps && !diagnostics.contains("keeps the old value") {
            failures.push(format!("{name}: missing retention warning: {diagnostics}"));
        }
        for (n, wanted) in [0, 1].into_iter().zip(native) {
            let (found, stats) = run_with_stats(&module, "entry", &[0, n, 0], 0);
            let comparison =
                format!("{name} n={n}: native={wanted}, device={found:?}, nonzero releases={}", stats.nonzero_releases);
            println!("{comparison}");
            let releases = if keeps { stats.nonzero_releases > 1 } else { stats.nonzero_releases != 0 };
            if found != Outcome::Returned(Some(wanted)) || releases {
                failures.push(comparison);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn both_qubit_dependent_calls_undo_their_writes() {
    compare_ordinary_inputs_in(
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
    (void)READ(q); // every register measured: none is released holding a value
    return value + 16u * (n && input_seen == successor_seen);
"#,
        [3, 19],
        false,
        &PROJECT_MODES,
    );
}

#[test]
#[ignore = "refused by name: the choice between the calls starts in the entry block (docs/DESIGN.md, \"Not yet\")"]
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

fn refused_for_ordinary_inputs(name: &str, declarations: &str, body: &str, expected: [u128; 2], reason: &str) {
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
    for (mode, clang, flags) in MODES {
        let name = format!("{name}-{mode}");
        let refusal = lower_text_with_diagnostics(&source, &name, clang, flags).expect_err("named refusal");
        println!("{name}: {refusal}");
        assert!(refusal.starts_with("not handled yet: ") && refusal.contains(reason), "{name}: {refusal}");
    }
}

const SKIPPED_EXCHANGE: &str = "from whose exchange some ordinary path does not write every byte";
const MOVED_EXCHANGE: &str = "whose exchange would have to move earlier than the store that needs it";

#[test]
fn an_ordinary_condition_inside_code_under_a_qubit_keeps_a_returned_byte_ordinary() {
    // LLVM merges `q` and `n` into one condition; for n = 0 nothing is written, whatever q holds.
    refused_for_ordinary_inputs(
        "conditional-byte-under-qubit",
        r#"
uint8_t held;

__attribute__((noinline)) void store_byte(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (q) { if (n) held = q + 4; }
    XBIT(q, 0);
}
"#,
        READ_STORED_BYTE,
        [0, 5],
        SKIPPED_EXCHANGE,
    );
}

#[test]
fn a_store_under_an_ordinary_condition_in_a_loop_keeps_a_returned_byte_ordinary() {
    // The exchange would leave the loop for the block before it, above the loop's ordinary guard.
    refused_for_ordinary_inputs(
        "conditional-byte-in-loop",
        r#"
volatile uint8_t held;

__attribute__((noinline)) void store_byte(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    #pragma clang loop unroll(disable)
    for (unsigned i = 0; i < n; ++i)
        if (i == 0) held = q + 1;
    XBIT(q, 0);
}
"#,
        READ_STORED_BYTE,
        [0, 2],
        MOVED_EXCHANGE,
    );
}

#[test]
fn a_skipped_write_measured_before_returning_is_refused() {
    // C gives 0 and 85, and main's front too, only because the byte is measured before the return: for n = 0 no
    // path writes it after the exchange (route: a measurement of the bytes before the return with nothing in
    // between, with its proof, not built; docs/DESIGN.md, "Not yet").
    refused_for_ordinary_inputs(
        "conditional-byte-measured",
        r#"
uint8_t held;

__attribute__((noinline)) unsigned store_and_measure(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (q) { if (n) held = q + 4; }
    XBIT(q, 0);
    return READ_BYTES(held, 1);
}
"#,
        "    unsigned v = store_and_measure(n);\n    return v + 16u * held;",
        [0, 85],
        SKIPPED_EXCHANGE,
    );
}

#[test]
fn a_write_after_a_skipped_store_does_not_move_the_exchange() {
    // C gives 34 for both n, and main's front too; the exchange would have to move above `if (n)` (route as above).
    refused_for_ordinary_inputs(
        "conditional-byte-rewritten",
        r#"
volatile uint8_t held;

__attribute__((noinline)) unsigned store_twice(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (n) held = q;
    held = q + 1;
    XBIT(q, 0);
    return READ_BYTES(held, 1);
}
"#,
        "    unsigned v = store_twice(n);\n    return v + 16u * held;",
        [34, 34],
        MOVED_EXCHANGE,
    );
}

#[test]
fn a_write_of_another_byte_does_not_authorise_the_exchange_of_a_skipped_one() {
    refused_for_ordinary_inputs(
        "conditional-byte-hull",
        r#"
uint8_t a[3] = {9, 10, 11};

__attribute__((noinline)) void work(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (q) { a[0] = q + 4; if (n) a[1] = q + 6; }
    XBIT(q, 0);
}
"#,
        "    work(n);\n    return n ? READ_BYTES(a[1], 1) : a[1];",
        [10, 7],
        SKIPPED_EXCHANGE,
    );
}

#[test]
fn a_skipped_write_measured_before_returning_needs_ordinary_bytes_before_the_exchange() {
    // For n = 0 an earlier call left the byte holding qubits: the exchange's read of the old value would fault.
    refused_for_ordinary_inputs(
        "conditional-byte-preheld",
        r#"
uint8_t held = 13;

__attribute__((noinline)) void seed(void) {
    uint8_t q = 0;
    XBIT(q, 0);
    held = q + 6;
    XBIT(q, 0);
}

__attribute__((noinline)) unsigned work(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (q) { if (n) held = q + 4; }
    XBIT(q, 0);
    return READ_BYTES(held, 1);
}
"#,
        "    if (!n) seed();\n    return work(n);",
        [7, 5],
        SKIPPED_EXCHANGE,
    );
}

#[test]
fn a_skipped_write_measured_before_returning_refuses_an_ordinary_copy_in_between() {
    // For n = 0 the copy reads the byte ordinarily in C; on the device it would read the shadow.
    refused_for_ordinary_inputs(
        "conditional-byte-copy",
        r#"
uint8_t held = 13;
uint8_t copy = 3;

__attribute__((noinline)) void work(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (q) { if (n) held = q + 3; }
    XBIT(q, 0);
    if (!n) copy = held;
    (void)READ_BYTES(held, 1);
}
"#,
        "    work(n);\n    return copy;",
        [13, 3],
        SKIPPED_EXCHANGE,
    );
}

#[test]
fn a_loop_whose_every_pass_may_skip_the_store_does_not_write_the_byte() {
    refused_for_ordinary_inputs(
        "conditional-byte-back-edge",
        r#"
uint8_t held = 13;

__attribute__((noinline)) void work(unsigned n) {
    uint8_t q = 4;
    XBIT(q, 0);
    unsigned k = q;
    while (k) {
        if (n == k) held = q + k;
        k >>= 1;
        if (k & 2) continue;
        if (n == k + 5) break;
    }
    XBIT(q, 0);
}
"#,
        "    work(n);\n    return n ? READ_BYTES(held, 1) : held;",
        [13, 6],
        SKIPPED_EXCHANGE,
    );
}

#[test]
fn a_later_store_on_every_path_does_not_move_the_exchange_above_the_condition() {
    // C gives 3 for both n, and main's front too: every path writes the byte with `q + 2` after `if (n)`. The
    // exchange would have to move above `if (n)`; placement denies that by default (route: a store of every byte on
    // every path after the condition, with its proof, not built; docs/DESIGN.md, "Not yet").
    refused_for_ordinary_inputs(
        "conditional-byte-mandatory",
        r#"
volatile uint8_t held = 13;

__attribute__((noinline)) void work(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (n) held = q;
    held = q + 2;
    XBIT(q, 0);
}
"#,
        "    work(n);\n    return READ_BYTES(held, 1);",
        [3, 3],
        MOVED_EXCHANGE,
    );
}

#[test]
fn a_qubit_dependent_compare_exchange_is_refused_as_an_effect() {
    refused_for_ordinary_inputs(
        "compare-exchange-effect",
        "int cells[2];",
        r#"
    _Bool q = 0;
    XBIT(q, 0);
    int expected = 0;
    (void)__atomic_compare_exchange_n(&cells[q], &expected, 7, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
    return READ(q);
"#,
        [1, 1],
        "an effect on memory (the instruction `cmpxchg`) that depends on a qubit",
    );
}

#[test]
fn a_predicated_update_reads_the_shadow_it_writes_back() {
    // The first code under the qubit writes after a loop over an ordinary count, which the proof credits nothing;
    // the second becomes `cell = q ? cell : 9`, whose read of the old value only feeds that store back. The
    // bytes are measured before the return.
    compare_ordinary_inputs(
        "conditional-byte-predicated",
        "uint64_t cell = 5;",
        r#"
    uint8_t q = 0;
    XBIT(q, 0);
    if (q) {
        #pragma clang loop unroll(disable)
        for (unsigned j = 0; j < n; ++j) cell ^= j + q;
        cell -= 3;
    }
    if (!q) cell = 9;
    packet value = READ_BYTES(cell, 8);
    return value | ((packet)READ(q) << 64);
"#,
        [2 | 1 << 64, 1 | 1 << 64],
    );
}

#[test]
fn an_ordinary_copy_before_the_write_on_its_path_is_refused() {
    // Every path writes `g` (the later `g = r + 2`), so the exchange stands above `if (n)`; for n = 0 the copy into
    // `dest` runs before that write and would read the shadow where C reads the ordinary 13.
    refused_for_ordinary_inputs(
        "conditional-byte-copy-before-write",
        r#"
uint8_t held = 13;
uint8_t dest = 3;

__attribute__((noinline)) void work(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (n) { if (q) held = q + 3; }
    XBIT(q, 0);
    if (!n) dest = held;
    uint8_t r = 0;
    XBIT(r, 0);
    held = r + 2;
    XBIT(r, 0);
}
"#,
        "    work(n);\n    return dest;",
        [13, 3],
        "after their exchange and before the write that makes them hold qubits on this path",
    );
}

/// The step limit of the folded-guard witnesses: the loops C runs take a few dozen steps; the device's loop
/// that C skips never ends, or runs 2^32 passes, and exceeds it.
const FOLDED_GUARD_STEPS: u64 = 3000;

fn ends_like_native_within_a_few_steps(program: &Program<'_>, n: u64) {
    let source = program.source();
    let native = program.native_table(&source, n);
    let mut failures = Vec::new();
    for (mode, clang, flags) in MODES {
        let name = format!("{}-{mode}", program.name);
        let (text, _) = lower_text_with_diagnostics(&source, &name, clang, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid device program");
        for (input, &wanted) in native.iter().enumerate() {
            let mut limited = options(0);
            limited.budget = Some(FOLDED_GUARD_STEPS);
            let mut executor =
                Executor::new(&module, entry(&module, "entry"), &[input as u64 + 1, n, 0], limited).expect("starts");
            let found = executor.run();
            let released = executor.stats().nonzero_releases;
            println!("{name} q={input} n={n}: native={wanted:#x}, device={found:?}, nonzero releases={released}");
            if found != Outcome::Returned(Some(wanted)) || released != 0 {
                failures.push(format!("{name} q={input} n={n}: native={wanted:#x}, device={found:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const FOLDED_GUARD: &str = r#"
    uint8_t y = 0;
    ZBIT(q, 1);
    if (q & n) {
        if (n) {
            LOOP
        }
    }
    packet key = READ(q);
    return key | ((packet)READ(y) << 8);
"#;

#[test]
#[ignore = "defect: LLVM folds the ordinary guard `if (n)` into the qubit condition `q & n`; the control program then enters the loop for n = 0, where it never ends (the warning is the only protection until ordinary guards under a qubit are protected)"]
fn a_folded_guard_keeps_an_endless_loop_from_running() {
    let body = FOLDED_GUARD.replace(
        "LOOP",
        "#pragma clang loop unroll(disable)\n            for (unsigned j = 0; j != 1; j += n) XBIT(y, 0);",
    );
    ends_like_native_within_a_few_steps(
        &Program { name: "folded-guard-endless", declarations: "", body: &body, qubits: 1 },
        0,
    );
}

#[test]
#[ignore = "defect: LLVM folds the ordinary guard `if (n)` into the qubit condition `q & n`; the control program then runs the counted loop's 2^32 passes for n = 0, and no warning is given since the loop has a count"]
fn a_folded_guard_keeps_a_counted_loop_from_running() {
    let body = FOLDED_GUARD.replace(
        "LOOP",
        "unsigned j = 0;\n            #pragma clang loop unroll(disable)\n            do { XBIT(y, 0); } while (++j != n);",
    );
    ends_like_native_within_a_few_steps(
        &Program { name: "folded-guard-counted", declarations: "", body: &body, qubits: 1 },
        0,
    );
}

// Wrong results `main` has too, which the placement of the exchange does not fix yet (each needs the exchange to
// stand where the write is on every path, or the bytes to be shown ordinary before it).

#[test]
#[ignore = "defect: an ordinary copy from a stack local reads the local's shadow before any write on that path (the local is held from before `if (n)`), so the copy holds qubits and the caller's ordinary read faults"]
fn an_ordinary_copy_of_an_unwritten_local_stays_ordinary() {
    compare_ordinary_inputs(
        "held-local-copy",
        r#"
uint8_t dest = 3;
__attribute__((noinline)) void init(uint8_t *p, unsigned n) { *p = n + 13; }
__attribute__((noinline)) unsigned work(unsigned n) {
    uint8_t a;
    init(&a, n);
    uint8_t q = 0;
    XBIT(q, 0);
    if (n) { if (q) a = q + 3; }
    XBIT(q, 0);
    if (!n) dest = a;
    (void)READ_BYTES(a, 1);
    return 0;
}
"#,
        "    work(n);\n    return dest;",
        [13, 3],
    );
}

#[test]
#[ignore = "defect: a write under a qubit behind an ordinary condition counts as writing the bytes on every ordinary path, so an ordinary copy after the arms meet reads the shadow where C reads memory"]
fn a_write_behind_an_ordinary_condition_in_a_qubit_arm_does_not_hide_a_later_copy() {
    compare_ordinary_inputs_keeping(
        "held-arm-side",
        r#"
uint8_t held = 13, dest = 3, side = 0;
__attribute__((noinline)) unsigned work(unsigned n) {
    uint8_t q = 0;
    XBIT(q, 0);
    if (q) { side = q; if (n) held = q + 3; }
    XBIT(q, 0);
    if (!n) dest = held;
    uint8_t r = 0;
    XBIT(r, 0);
    held = r + 2;
    XBIT(r, 0);
    return 0;
}
"#,
        "    work(n);\n    return dest;",
        [13, 3],
        true,
    );
}

#[test]
#[ignore = "defect: a write after a measurement counts for the span before it, whose exchange then reads, ordinarily, bytes an earlier call left holding qubits"]
fn a_write_after_a_measurement_does_not_place_the_earlier_exchange() {
    compare_ordinary_inputs(
        "held-write-after-measurement",
        r#"
uint8_t held = 13;
__attribute__((noinline)) void seed(void) {
    uint8_t s = 0;
    XBIT(s, 0);
    held = s + 6;
    XBIT(s, 0);
}
__attribute__((noinline)) unsigned work(unsigned n) {
    unsigned v = 0;
    uint8_t q = 0;
    XBIT(q, 0);
    if (n) { if (q) held = q + 3; }
    XBIT(q, 0);
    v = READ_BYTES(held, 1);
    uint8_t r = 0;
    XBIT(r, 0);
    held = r + 2;
    XBIT(r, 0);
    return v;
}
"#,
        "    if (!n) seed();\n    return work(n);",
        [7, 4],
    );
}

#[test]
#[ignore = "defect: the exchange that opens a span reads the bytes' old value ordinarily, which faults where an earlier call left them holding qubits, although every path overwrites them"]
fn a_store_into_bytes_an_earlier_call_left_holding_qubits_does_not_fault() {
    compare_ordinary_inputs(
        "held-mandatory-preheld",
        r#"
uint8_t held = 0;
__attribute__((noinline)) void first(void) {
    uint8_t q = 0;
    XBIT(q, 0);
    held = q + 6;
    XBIT(q, 0);
}
__attribute__((noinline)) unsigned second(void) {
    uint8_t r = 0;
    XBIT(r, 0);
    held = r + 2;
    XBIT(r, 0);
    return READ_BYTES(held, 1);
}
"#,
        "    first();\n    return second();",
        [3, 3],
    );
}

#[test]
#[ignore = "a write through a restrict parameter leaves the caller's byte unclassified and permits a guarded read to fault on the path that skips it"]
fn a_write_through_restrict_preserves_a_later_read_guard() {
    compare_ordinary_inputs(
        "guarded-read-after-restrict-store",
        r#"
uint8_t cell[2] = {0, 1};

__attribute__((noinline)) void write_bit(uint8_t *restrict dst) {
    uint8_t bit = 0;
    XBIT(bit, 0);
    dst[0] = bit;
    XBIT(bit, 0);
}

__attribute__((noinline)) unsigned read_if_nonzero(unsigned count) {
    if (count) return count * cell[0];
    return 0;
}
"#,
        r#"
    if (n) write_bit(cell);
    unsigned value = read_if_nonzero(1 - n);
    if (n) return value + READ_BYTES(cell[0], 1);
    return value;
"#,
        [0, 1],
    );
}

#[test]
#[ignore = "memcpy from a qubit variable leaves the destination unclassified and permits a guarded read to fault on the path that skips it"]
fn copying_from_a_qubit_variable_preserves_a_later_read_guard() {
    compare_ordinary_inputs(
        "guarded-read-after-qubit-copy",
        r#"
uint8_t cell[2] = {0, 1};

__attribute__((noinline)) void copy_bit(void) {
    uint8_t bit = 0;
    XBIT(bit, 0);
    __builtin_memcpy(cell, &bit, 1);
    XBIT(bit, 0);
}

__attribute__((noinline)) unsigned read_if_nonzero(unsigned count) {
    if (count) return count * cell[0];
    return 0;
}
"#,
        r#"
    if (n) copy_bit();
    unsigned value = read_if_nonzero(1 - n);
    if (n) return value + READ_BYTES(cell[0], 1);
    return value;
"#,
        [0, 1],
    );
}

#[test]
#[ignore = "one call feeding two consumers is evaluated in a separate generated function for each, giving different return addresses"]
fn one_call_feeding_two_consumers_gives_one_value() {
    compare_ordinary_inputs(
        "call-feeding-two-stores",
        RETADDR_HELPER,
        r#"
    uint8_t q = 0, marker = 0;
    if (n) XBIT(q, 0);
    volatile uint64_t first = 0, second = 0;
    if (q) {
        uintptr_t address = f(q);
        first = address;
        ZBIT(marker, 0);
        second = address;
    }
    uint64_t a = READ(first), b = READ(second);
    return a ^ b;
"#,
        [0, 0],
    );
}

const RETURN_ADDRESS_HELPER: &str = r#"
__attribute__((noinline)) uintptr_t f(uint8_t q) {
    return q ? (uintptr_t)__builtin_return_address(0) : 0;
}
"#;

#[test]
fn a_frame_dependent_value_with_two_consumers_is_computed_once_into_a_register() {
    // The value is stored once into a register where C computes it; both words read the register, and the same
    // applied function clears it after them.
    for (name, declarations) in [
        (
            "register-frame-address",
            "__attribute__((noinline)) uintptr_t f(uint8_t q) { return q ? (uintptr_t)__builtin_frame_address(0) : 0; }",
        ),
        (
            "register-pointer-byte",
            "__attribute__((noinline)) static unsigned part(const void *p, unsigned i) { return ((const unsigned char *)p)[i]; }\n__attribute__((noinline)) uintptr_t f(uint8_t q) { int local; void *p = &local; return q ? part(&p, q - 1) : 0; }",
        ),
        ("register-return-address", RETURN_ADDRESS_HELPER),
    ] {
        let body = "    uintptr_t t = f(q);\n    volatile uint64_t x = t, y = t + n;\n    uint64_t a = READ(x), b = READ(y);\n    packet key = READ(q);\n    return (key << 64) | ((a + n) ^ b);";
        ends_like_native_within_a_few_steps(&Program { name, declarations, body, qubits: 1 }, 1);
    }
}

#[test]
fn a_call_with_two_consumers_under_a_qubit_with_gates_is_refused() {
    // Predication copies the code under the qubit for each operation; the register is not built there. The
    // check that the simplifier keeps classified instructions refuses this shape first, as without the register.
    Program {
        name: "repeated-two-stores",
        declarations: RETURN_ADDRESS_HELPER,
        body: "    volatile uint64_t x = 0, y = 0;\n    uint8_t z = 0;\n    if (q) { uintptr_t t = f(q); x = t; ZBIT(z, 0); y = t; }\n    uint64_t a = READ(x), b = READ(y);\n    return a ^ b;",
        qubits: 1,
    }
    .refused(1, &[0, 0], SEALED);
}

/// The refusal of the check that the simplifier keeps classified instructions (DESIGN, "Classification"), which comes before the register's for code under a qubit.
const SEALED: &str =
    "not handled yet: LLVM's simplifier removed an instruction whose class the front decided before it";

#[test]
fn arithmetic_with_two_consumers_is_still_copied() {
    Program {
        name: "repeated-arithmetic",
        declarations: "",
        body: "    volatile uint32_t x = 0, y = 0;\n    uint8_t z = 0;\n    if (q) { uint32_t t = q * 17u + 3u; x = t; ZBIT(z, 0); y = t * n + column; }\n    packet a = READ(x), b = READ(y);\n    return a | (b << 32) | ((packet)READ(q) << 64);",
        qubits: 1,
    }
    .agrees(1, &[0, 20 | (20 << 32) | (1 << 64)], &readout(&[0, 32, 64]), 0);
}

#[test]
fn a_replay_across_a_stack_reservation_is_refused() {
    // The value is computed into x, a variable-length array is reserved, and the overwrite clears x by computing
    // the value again: the second run would start from another stack pointer.
    Program {
        name: "repeated-across-reservation",
        declarations: RETURN_ADDRESS_HELPER,
        body: "    volatile uint64_t x = f(q);\n    volatile char buffer[n + 1];\n    buffer[0] = 1;\n    x = buffer[0] - 1;\n    return READ(x) | ((packet)READ(q) << 8);",
        qubits: 1,
    }
    .refused(1, &[0, 1 << 8], "not handled yet: a call to @f computed by one applied function at two places");
}

#[test]
fn a_replay_of_an_ordinary_call_moved_by_taint_all_across_a_stack_reservation_is_refused() {
    // Under --taint-all the ordinary call is moved into the applied functions, and the replay across the
    // variable-length array is refused as for any evaluation; in the other modes the call stays in the control
    // program, its value is passed in, and the program gives C's values.
    let program = Program {
        name: "repeated-ordinary-across-reservation",
        declarations: "__attribute__((noinline)) static uint32_t pure(uint32_t a) { return a * 3u + 1u; }",
        body: "    volatile uint64_t x = pure(n + 5u) + q;\n    volatile unsigned side = 0;\n    if (n) { volatile char buffer[n + 1]; buffer[0] = 1; side = buffer[0]; }\n    x = side - 1;\n    return READ(x) | ((packet)READ(q) << 8);",
        qubits: 1,
    };
    let source = program.source();
    let (_, clang, flags) = MODES[2];
    let refusal = lower_text_with_diagnostics(&source, "repeated-ordinary-across-reservation-taint", clang, flags)
        .expect_err("refused under --taint-all");
    assert!(refusal.contains("computed by one applied function at two places"), "{refusal}");
    let native = program.native_table(&source, 1);
    assert_eq!(native, [0, 1 << 8]);
    for (mode, clang, flags) in &MODES[..2] {
        let name = format!("repeated-ordinary-across-reservation-{mode}");
        let (text, _) = lower_text_with_diagnostics(&source, &name, clang, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid device program");
        for (input, &wanted) in native.iter().enumerate() {
            let (found, stats) = run_with_stats(&module, "entry", &[input as u64 + 1, 1, 0], 0);
            assert_eq!((found, stats.nonzero_releases), (Outcome::Returned(Some(wanted)), 0), "{name} q={input}");
        }
    }
}

#[test]
fn a_known_start_and_the_clearing_computation_are_one_function() {
    // `x ^= f(q)` into an x the front knows to be 0 is written `v ^ 0` in the computing slice and `v` in the
    // clearing one; written as one text, they are one function, so the return address is the same in both.
    Program {
        name: "return-address-known-start",
        declarations: RETADDR_HELPER,
        body: &RETADDR_USE.replace("QUALIFIER uint64_t x = f(q);", "volatile uint64_t x = 0;\n    x ^= f(q);"),
        qubits: 1,
    }
    .agrees(0, &[256, 257], &return_address_readout(), 0);
}

// The register for a value with several consumers is cleared by computing the value again; only at a place where
// that gives the value it holds (front/src/Evaluations.h). Each of these released a register that was not 0, or
// gave another value, when the place was found by a scan from the last consumer.
const SHARED_HELPERS: &str = r#"
static volatile unsigned message = 3;
__attribute__((noinline)) static uint8_t f(uint8_t a) { return a * 3 + 1; }
__attribute__((noinline)) static uint8_t g(uint8_t a) { return a * 5 + message; }
__attribute__((noinline)) static void update(unsigned n) { message = n; }
"#;
const SHARED_READOUT: &str = "    return READ(x) | (READ(y) << 8) | ((packet)READ(q) << 16);";

fn shared_body(body: &str) -> String {
    format!("    volatile uint8_t x = 0, y = 0;\n{body}\n{SHARED_READOUT}")
}

fn shared<'a>(name: &'a str, body: &'a str) -> Program<'a> {
    Program { name, declarations: SHARED_HELPERS, body, qubits: 1 }
}

#[test]
fn a_register_is_cleared_before_a_call_that_writes_memory_the_value_reads() {
    // g reads `message`, which update() writes: computing g(q) again after the call would give another value.
    shared("shared-before-call", &shared_body("    uint8_t t = g(q);\n    x = t;\n    y = t + 1;\n    update(n);"))
        .agrees(7, &[0x403, 0x1_0908], &readout(&[0, 8, 16]), 0);
}

#[test]
fn a_register_is_cleared_before_a_conditional_gate_on_what_the_value_reads() {
    // The gate on q is on a path between the consumers and the join after it, run or not.
    let body = shared_body(
        "    uint8_t t = f(q);\n    x = t;\n    y = t + 1;\n    x = 0;\n    y = 0;\n    if (n) XBIT(q, 0);",
    );
    let program = shared("shared-before-conditional-gate", &body);
    program.agrees(0, &[0, 0x1_0000], &readout(&[0, 8, 16]), 0);
    program.agrees(1, &[0x1_0000, 0], &readout(&[0, 8, 16]), 0);
}

#[test]
fn a_register_filled_under_an_ordinary_condition_is_cleared_on_the_paths_that_filled_it() {
    // The join after `if (n)` is also reached without computing t: the register is cleared on the edges from the
    // paths that computed it.
    shared(
        "shared-under-ordinary-condition",
        &shared_body("    if (n) {\n        uint8_t t = f(q);\n        x = t;\n        if (t & 4) XBIT(y, 0);\n    }"),
    )
    .agrees(1, &[0x1, 0x1_0104], &readout(&[0, 8, 16]), 0);
    shared(
        "shared-under-ordinary-condition-split",
        &shared_body("    if (n) {\n        uint8_t t = f(q);\n        if (n & 1) x = t; else y = t + 1;\n    }"),
    )
    .agrees(1, &[0x1, 0x1_0004], &readout(&[0, 8, 16]), 0);
    shared(
        "shared-under-ordinary-condition-goto",
        &shared_body("    if (n) {\n        uint8_t t = f(q);\n        x = t;\n        if (n & 2) goto done;\n        y = t + 1;\n    }\ndone:"),
    )
    .agrees(1, &[0x201, 0x1_0504], &readout(&[0, 8, 16]), 0);
}

#[test]
fn a_gate_between_two_consumers_on_what_the_value_reads_is_refused() {
    // y = t + 1 needs t after q changed: no register can be cleared by computing t again, and the second
    // consumer's computation reads q after the gate.
    shared(
        "shared-gate-between-consumers",
        &shared_body("    uint8_t t = f(q);\n    x = t;\n    XBIT(q, 0);\n    y = t + 1;"),
    )
    .refused(
        0,
        &[0x1_0201, 0x504],
        "not handled yet: the value of qubit variable 'q' read here is used after the variable changed",
    );
}

#[test]
fn a_measurement_before_every_consumer_is_done_is_refused() {
    // On the path through the early return the measurements come before the place after the consumers.
    shared(
        "shared-measured-before-done",
        &shared_body("    if (n) {\n        uint8_t t = f(q);\n        x = t;\n        if (n & 2) { y = t + 1; return READ(x) + READ(y) + READ(q); }\n    }"),
    )
    .refused(
        2,
        &[0x3, 0xa],
        "not handled yet: a call to @f whose value is consumed in 2 places, each computing it again in a function of its own: the copies run in different frames and may give different values; the register of its own that the front gives such a value is cleared by computing the value again after its consumers, and here a measurement, across which the front does not clear such a register yet, comes between",
    );
}

#[test]
fn shapes_main_accepts_that_the_register_refuses_are_refused_by_name() {
    // Main computes the value once per consumer; this front gives it a register or refuses by name (DESIGN,
    // "A register for a value with several consumers", with the routes). The two shapes under a qubit are refused
    // first by that check, as on the front without the register.
    let register = "not handled yet: a call to @f whose value is consumed in 2 places";
    for (name, body, native, reason) in [
        (
            "shared-under-qubit-with-gate",
            "    uint8_t z = 0;\n    if (q) {\n        uint8_t t = f(q);\n        x = t;\n        ZBIT(z, 0);\n        y = t;\n    }",
            [0, 0x1_0404],
            SEALED,
        ),
        (
            "shared-under-qubit-two-conditions",
            "    if (q) {\n        uint8_t t = f(q);\n        if (t & 4) XBIT(x, 0);\n        if (t & 4) XBIT(y, 0);\n    }",
            [0, 0x1_0101],
            SEALED,
        ),
        (
            "shared-overwritten-after-measurement",
            "    uint8_t t = f(q);\n    x = t;\n    y = t + 1;\n    (void)READ(x);\n    y = 0;",
            [0x1, 0x1_0004],
            register,
        ),
        (
            "shared-measured-own-store-between",
            "    uint8_t t = f(q);\n    x = t;\n    unsigned z = READ(x);\n    y = t + 1 + 0 * z;",
            [0x201, 0x1_0504],
            register,
        ),
    ] {
        let body = shared_body(body);
        shared(name, &body).refused(0, &native, reason);
    }
}

// Whole programs `unsigned entry(unsigned k)`: k = 0 or 1 sets the qubit; device and native C agree and every
// register is released at 0, in all three modes.
fn whole_program_like_native(name: &str, text: &str, native: [u128; 2]) {
    let source = write_c(name, text);
    let executable = native_build(&source, name, &["-DNATIVE"]);
    for (k, &wanted) in native.iter().enumerate() {
        let output = Command::new(&executable).arg(k.to_string()).output().expect("run native C");
        assert_eq!(std::str::from_utf8(&output.stdout).expect("native stdout").trim(), wanted.to_string(), "{name}");
    }
    let mut failures = Vec::new();
    for (mode, clang, flags) in MODES {
        let name = format!("{name}-{mode}");
        let (text, _) = lower_text_with_diagnostics(&source, &name, clang, flags).expect("lowers");
        let module = bennett_ir::load(&text).expect("valid device program");
        for (k, &wanted) in native.iter().enumerate() {
            let (found, stats) = run_with_stats(&module, "entry", &[k as u64], 0);
            println!("{name} k={k}: {found:?}, nonzero releases={}", stats.nonzero_releases);
            if found != Outcome::Returned(Some(wanted)) || stats.nonzero_releases != 0 {
                failures.push(format!("{name} k={k}: native={wanted}, device={found:?}, releases={stats:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_callee_writing_a_live_callers_local_comes_after_the_register_is_cleared() {
    // put() writes the caller's `cell`, which get() reads through the pointer: a local of a live caller is memory
    // the call may read, not private to it.
    whole_program_like_native("shared-ancestor-local", ANCESTOR_LOCAL, [1027, 66820]);
}

#[test]
fn a_register_is_cleared_before_a_loop_rather_than_in_it() {
    // A place inside the loop runs once per pass but the value is computed once: the clear goes before the loop.
    whole_program_like_native("shared-before-loop-gate", LOOP_AFTER_CONSUMERS, [66049, 1284]);
    whole_program_like_native("shared-before-loop-call", LOOP_WRITE_AFTER_CONSUMERS, [1027, 66820]);
}

const ANCESTOR_LOCAL: &str = r#"
#include <stdint.h>
#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>
#define X(q) ((q) ^= 1)
#define READ(q) (q)
#else
#include "bennett.h"
#define X(q) bennett_x(bennett_bit((void *)&(q),0))
#define READ(q) bennett_measure((const void *)&(q))
#endif
#define NOINLINE __attribute__((noinline))
static NOINLINE uint8_t get(uint8_t q, volatile unsigned *p) { return q + *p; }
static NOINLINE void put(volatile unsigned *p) { *p = 7; }
static NOINLINE unsigned inner(unsigned k, volatile unsigned *p) {
    volatile uint8_t q = 0;
    if (k) X(q);
    volatile uint8_t x = 0, y = 0;
    uint8_t t = get(q,p);
    x = t;
    y = t + 1;
    put(p);
    unsigned a = READ(x);
    unsigned b = READ(y);
    unsigned key = READ(q);
    return a + (b << 8) + (key << 16);
}
unsigned entry(unsigned k) {
    volatile unsigned cell = 3;
    return inner(k,&cell);
}
#ifdef NATIVE
int main(int argc, char **argv) { printf("%u\n",entry(atoi(argv[1]))); return 0; }
#endif
"#;

const LOOP_AFTER_CONSUMERS: &str = r#"
#include <stdint.h>
#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>
#define X(q) ((q) ^= 1)
#define READ(q) (q)
#else
#include "bennett.h"
#define X(q) bennett_x(bennett_bit((void *)&(q),0))
#define READ(q) bennett_measure((const void *)&(q))
#endif
#define NOINLINE __attribute__((noinline))

static volatile unsigned gcell=3, count, held;
static NOINLINE uint8_t f(uint8_t q) {return q*3+1;}
static NOINLINE uint8_t g(uint8_t q) {return q+gcell;}
static NOINLINE void put(unsigned k) {gcell=k+7;}
static NOINLINE int next(void) {return ++count < 3;}
unsigned entry(unsigned k) {
 volatile uint8_t q=0,z=0;
 volatile uint8_t x=0,y=0;
 if (k & 1) X(q);
uint8_t t=f(q); x=t; y=t+1; do {X(q);} while(next());
 unsigned a=READ(x), b=READ(y), c=READ(q), d=READ(z);
 return a+(b<<8)+(c<<16)+(d<<24);
}
#ifdef NATIVE
int main(int argc,char **argv){printf("%u\n",entry(atoi(argv[1])));}
#endif
"#;

const LOOP_WRITE_AFTER_CONSUMERS: &str = r#"
#include <stdint.h>
#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>
#define X(q) ((q) ^= 1)
#define READ(q) (q)
#else
#include "bennett.h"
#define X(q) bennett_x(bennett_bit((void *)&(q),0))
#define READ(q) bennett_measure((const void *)&(q))
#endif
#define NOINLINE __attribute__((noinline))

static volatile unsigned gcell=3, count, held;
static NOINLINE uint8_t f(uint8_t q) {return q*3+1;}
static NOINLINE uint8_t g(uint8_t q) {return q+gcell;}
static NOINLINE void put(unsigned k) {gcell=k+7;}
static NOINLINE int next(void) {return ++count < 3;}
unsigned entry(unsigned k) {
 volatile uint8_t q=0,z=0;
 volatile uint8_t x=0,y=0;
 if (k & 1) X(q);
uint8_t t=g(q); x=t; y=t+1; do {put(k);} while(next());
 unsigned a=READ(x), b=READ(y), c=READ(q), d=READ(z);
 return a+(b<<8)+(c<<16)+(d<<24);
}
#ifdef NATIVE
int main(int argc,char **argv){printf("%u\n",entry(atoi(argv[1])));}
#endif
"#;
