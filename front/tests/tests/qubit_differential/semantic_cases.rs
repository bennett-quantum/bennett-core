//! Opt-in differential recipes for known compiler defects.

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::state::{Check, Field};
use super::{
    Case, Expectation, MODES, compare, lower_text, lower_text_with_diagnostics, native_build, prepare, write_c,
};

type Generate = fn(&mut ChaCha8Rng) -> Case;

struct Family {
    switch: &'static str,
    recipes: &'static [Generate],
    modes: &'static [usize],
}

// These recipes check semantics that depend on what clang hands to the front.
// Clang -O2 can change the relevant calls and stores before the front sees them.
const PROJECT_MODES: &[usize] = &[0, 2];
const ALL_MODES: &[usize] = &[0, 1, 2];
const FAMILIES: &[Family] = &[
    Family {
        switch: "BENNETT_RANDOM_WRITING_CALLS",
        recipes: &[removed_call_argument, merged_writing_calls],
        modes: PROJECT_MODES,
    },
    Family { switch: "BENNETT_RANDOM_MIXED_STORES", recipes: &[merged_stores, skipped_store], modes: PROJECT_MODES },
    Family { switch: "BENNETT_RANDOM_GUARDED_LOADS", recipes: &[mixed_guard_load], modes: PROJECT_MODES },
    Family {
        switch: "BENNETT_RANDOM_GUARDED_GATE_LOOPS",
        recipes: &[unproved_division, skipped_read, proved_division],
        modes: ALL_MODES,
    },
];

pub(super) fn enabled_recipes() -> Vec<(Generate, usize)> {
    FAMILIES
        .iter()
        .filter(|family| std::env::var_os(family.switch).is_some())
        .flat_map(|family| {
            family.recipes.iter().flat_map(|&recipe| family.modes.iter().map(move |&mode| (recipe, mode)))
        })
        .collect()
}

fn writing_call(initial: u8, written: u8, choice: Option<(u64, u8)>) -> Case {
    let (helper, call, rollback, trips, packets) = match choice {
        Some((argument, add)) => (
            format!("unsigned write_cell(unsigned x) {{ cell = {written}U; return x + {add}U; }}"),
            "    unsigned value;\n    if (column) value = write_cell(q);\n    else value = write_cell(n);\n    y ^= value;",
            "if (column) ",
            argument,
            2,
        ),
        None => (
            format!("static unsigned write_cell(unsigned x) {{ cell = {written}U; return x; }}"),
            "    y ^= write_cell(q);",
            "",
            0,
            1,
        ),
    };
    // The native reference restores only the writes of the call that takes q.
    let body = format!(
        "uint8_t cell = {initial}U;\n\
         __attribute__((noinline)) {helper}\n\n\
         packet entry(unsigned prep, unsigned n, unsigned column) {{\n{}    \
             uint8_t y = 0;\n    ZBIT(y, 0);\n\
         #ifdef NATIVE\n    uint8_t saved = cell;\n#endif\n{call}\n\
         #ifdef NATIVE\n    {rollback}cell = saved;\n#endif\n    \
             packet key = bennett_measure(&q);\n    \
             return key | ((packet)bennett_measure(&y) << 8) | ((packet)cell << 16);\n}}\n",
        prepare(1),
    );
    Case::values(body, 1, trips, packets, false)
}

fn removed_call_argument(random: &mut ChaCha8Rng) -> Case {
    let initial = random.random::<u8>();
    writing_call(initial, initial ^ random.random_range(1..=255), None)
}

fn merged_writing_calls(random: &mut ChaCha8Rng) -> Case {
    let initial = random.random::<u8>();
    writing_call(
        initial,
        initial ^ random.random_range(1..=255),
        Some((random.random_range(2..=15), random.random_range(1..=15))),
    )
}

fn agrees(name: &str, case: Case, modes: &[usize], expected: &[Vec<u128>]) {
    let source = write_c(name, &case.source());
    let table = case.native_table(&native_build(&source, name, &["-DNATIVE"]));
    assert_eq!(table, expected, "{name}: native packets");
    let mut failures = Vec::new();
    for &mode in modes {
        let (mode, cflags, flags) = MODES[mode];
        let name = format!("{name}-{mode}");
        let result = lower_text(&source, &name, cflags, flags).and_then(|program| {
            let module = bennett_ir::load(&program).map_err(|error| format!("invalid device program: {error}"))?;
            compare(&case, &module, &table, &mut 0)
        });
        if let Err(error) = result {
            failures.push(format!("{name}: {error}"));
        }
    }
    assert!(failures.is_empty(), "native packets: {table:x?}\n{}", failures.join("\n"));
}

#[test]
#[ignore = "removing a qubit-dependent argument makes an applied call's global write survive"]
fn removed_argument_keeps_the_call_applied() {
    agrees("removed-call-argument", writing_call(3, 7, None), PROJECT_MODES, &[vec![0x30000, 0x30101]]);
}

#[test]
#[ignore = "merging ordinary and qubit-dependent calls undoes the ordinary call's global write"]
fn ordinary_call_keeps_its_write_beside_an_applied_call() {
    agrees(
        "merged-writing-calls",
        writing_call(3, 7, Some((2, 1))),
        PROJECT_MODES,
        &[vec![0x70300, 0x70301], vec![0x30100, 0x30201]],
    );
}

fn mixed_store(initial: u8, offset: u8, ordinary: Option<u8>) -> Case {
    // Native C reads the volatile byte directly: the native measurement helper reads through a pointer
    // without `volatile`, which is undefined for a volatile object.
    let otherwise = ordinary.map_or_else(String::new, |value| format!(" else cell = {value}U;"));
    let body = format!(
        r#"
volatile uint8_t cell = {initial}U;

__attribute__((noinline)) void choose_store(unsigned flag) {{
    uint8_t value = 0;
    XBIT(value, 0);
    if (flag) cell = {offset}U + value;{otherwise}
    XBIT(value, 0);
}}

packet entry(unsigned prep, unsigned n, unsigned column) {{
    choose_store(column);
#ifndef NATIVE
    if (column) return (packet)bennett_measure_bytes((const void *)&cell, 1) << 8;
#endif
    return (packet)cell << 8;
}}
"#,
    );
    Case::values(body, 0, 0, 2, false)
}

fn merged_stores(random: &mut ChaCha8Rng) -> Case {
    let initial = random.random::<u8>();
    mixed_store(initial, random.random_range(0..=254), Some(initial ^ random.random_range(1..=255)))
}

fn skipped_store(random: &mut ChaCha8Rng) -> Case {
    mixed_store(random.random(), random.random_range(0..=254), None)
}

#[test]
#[ignore = "merging a qubit-dependent store and a constant store makes the constant path hold qubits"]
fn constant_store_keeps_the_global_ordinary() {
    agrees("merged-stores", mixed_store(11, 0, Some(2)), PROJECT_MODES, &[vec![0x200], vec![0x100]]);
}

#[test]
#[ignore = "the exchange makes a global hold qubits on a path that skips its qubit-dependent store"]
fn skipped_store_keeps_the_global_ordinary() {
    agrees("skipped-store", mixed_store(11, 0, None), PROJECT_MODES, &[vec![0xb00], vec![0x100]]);
}

fn guarded_load(ordinary: u8, stored: u8, negated: bool) -> Case {
    let (predicate, initial) = if negated { ("!q || !cell", 1) } else { ("q && cell", 0) };
    let body = format!(
        r#"
uint8_t cell = {ordinary}U;

__attribute__((noinline)) void fill(void) {{
    uint8_t value = 0;
    XBIT(value, 0);
    cell = {stored}U ^ value;
    XBIT(value, 0);
}}

packet entry(unsigned prep, unsigned n, unsigned column) {{
    uint8_t q = 0, target = {initial};
    if (!column) fill();
    if (column) XBIT(q, 0);
    ZBIT(q, 1);
    if ({predicate}) XBIT(target, 0);
    packet result = bennett_measure(&target);
    return (result << 8) | ((packet)bennett_measure_bytes(&cell, 1) << 16);
}}
"#,
    );
    Case::values(body, 0, 0, 2, false)
}

fn mixed_guard_load(random: &mut ChaCha8Rng) -> Case {
    guarded_load(random.random_range(1..=255), random.random(), random.random_bool(0.5))
}

#[test]
#[ignore = "the lowered qubit condition reads a qubit-held global that C skips by short-circuiting"]
fn qubit_guard_short_circuits_the_global_load() {
    agrees("guarded-load", guarded_load(1, 0, false), PROJECT_MODES, &[vec![0x10000], vec![0x10100]]);
}

// Gate loops under a qubit-dependent condition. The control program runs the loop's ordinary code for every input,
// also where C does not enter the loop, so the front accepts a division there only by a divisor it proves nonzero
// and refuses every other division and every read of memory by name.
const UNASKED: &str = "not handled yet: in a loop with gates under the control of a value that depends on a qubit, whose ordinary code the control program runs for every input, also where C does not enter the loop: ";
const UNPROVED_DIVISOR: &[&str] = &[UNASKED, "whose divisor is not proved nonzero"];
const FAULTING_READ: &[&str] = &[UNASKED, "a read of memory, which can fault there"];

/// The parts of a gate loop: `before` runs ahead of the qubit's condition, `guard` wraps it, `prefix` runs under
/// it before the loop, `count` bounds the loop and `test` decides each pass's gate on `target`.
#[derive(Default)]
struct GateLoop {
    declarations: String,
    before: String,
    guard: String,
    prefix: String,
    count: String,
    test: String,
    readout: String,
}

impl GateLoop {
    /// With `superposed`, q is one qubit in equal superposition and is measured into the key byte. Otherwise q
    /// holds the definite value `column != 0`, so that C enters the loop only for column 1.
    fn case(&self, superposed: bool, trips: u64, expectation: Expectation) -> Case {
        let (prepare, key) = if superposed {
            (prepare(1), "bennett_measure(&q) | ")
        } else {
            (String::from("    unsigned char q = 0;\n    if (column) XBIT(q, 0);\n"), "")
        };
        let Self { declarations, before, guard, prefix, count, test, readout } = self;
        let body = format!(
            r#"{declarations}
packet entry(unsigned prep, unsigned n, unsigned column) {{
{prepare}    uint8_t target = 0;
    {before}
    ZBIT(q, 1);
    {guard}if (q) {{
        {prefix}
        #pragma clang loop unroll(disable)
        for (unsigned i = 0; i < {count}; ++i)
            if ({test}) XBIT(target, 0);
    }}
    packet key = {key}0;
    return key | ((packet)bennett_measure(&target) << 8){readout};
}}
"#
        );
        let state = superposed.then(|| Check::new(vec![Field { packet: 0, shift: 0 }, Field { packet: 0, shift: 8 }]));
        Case { body, qubits: u32::from(superposed), trips, packets: 2, expectation, state }
    }
}

/// `numerator OP divisor` in unsigned or (non-negative) signed arithmetic.
fn quotient(numerator: &str, divisor: &str, remainder: bool, signed: bool) -> String {
    let op = if remainder { '%' } else { '/' };
    if signed {
        format!("((int)({numerator}) {op} (int)({divisor}))")
    } else {
        format!("(({numerator}) {op} ({divisor}))")
    }
}

/// A divisor of 0 exactly where C skips the loop, used in each pass or once before the loop.
fn unproved_division_loop(multiplier: u8, offset: u8, remainder: bool, signed: bool, before_loop: bool) -> Case {
    let divisor = format!("column * {multiplier}U");
    let mut parts = GateLoop { count: String::from("n"), test: String::from("1"), ..GateLoop::default() };
    if before_loop {
        parts.prefix = format!("unsigned m = {};", quotient(&format!("n + {offset}U"), &divisor, remainder, signed));
        parts.count = String::from("m");
    } else {
        parts.test = format!("{} & 1", quotient(&format!("i + {offset}U"), &divisor, remainder, signed));
    }
    parts.case(false, 0, Expectation::Refused(UNPROVED_DIVISOR))
}

fn unproved_division(random: &mut ChaCha8Rng) -> Case {
    let case = unproved_division_loop(
        2 * random.random_range(0..=3) + 1,
        random.random_range(0..=15),
        random.random_bool(0.5),
        random.random_bool(0.5),
        random.random_bool(0.25),
    );
    Case { trips: random.random_range(1..=5), ..case }
}

#[derive(Clone, Copy)]
enum Read {
    /// A byte that holds a qubit where C skips the loop.
    Held,
    /// An index past the end of the array where C skips the loop.
    OutOfRange,
    /// A null pointer where C skips the loop.
    Null,
}

fn skipped_read_loop(read: Read, first: u8, second: u8) -> Case {
    let declarations = format!(
        r#"
uint8_t cells[3] = {{0, {first}U, {second}U}};

__attribute__((noinline)) void fill(void) {{
    uint8_t value = 0;
    XBIT(value, 0);
    cells[0] = value;
    XBIT(value, 0);
}}
"#,
    );
    let (before, test) = match read {
        Read::Held => ("fill();", "cells[(column != 0) + (i & 1U)]"),
        Read::OutOfRange => ("", "cells[(i & 1U) + (column ? 1U : 4096U)]"),
        Read::Null => ("const uint8_t *p = column ? &cells[1] : 0;", "p[i & 1U]"),
    };
    GateLoop {
        declarations,
        before: before.to_owned(),
        count: String::from("n"),
        test: test.to_owned(),
        readout: String::from(" | ((packet)bennett_measure_bytes(cells, 1) << 16)"),
        ..GateLoop::default()
    }
    .case(false, 0, Expectation::Refused(FAULTING_READ))
}

fn skipped_read(random: &mut ChaCha8Rng) -> Case {
    let read = [Read::Held, Read::OutOfRange, Read::Null][random.random_range(0..3)];
    let case = skipped_read_loop(read, random.random_range(0..=3), random.random_range(0..=3));
    Case { trips: random.random_range(1..=5), ..case }
}

#[derive(Clone, Copy)]
enum Proof {
    /// A nonzero constant.
    Constant(u8),
    /// `(source & mask) + low`, a range from `low` to `mask + low` computed from constants.
    Range { from_column: bool, mask: u8, low: u8 },
    /// `column * multiplier`, tested nonzero by the control program in each pass before the division.
    TestedInLoop(u8),
    /// `column * multiplier`, tested nonzero by an ordinary condition around the qubit's condition.
    Guarded(u8),
}

fn proved_division_loop(proof: Proof, offset: u8, remainder: bool, signed: bool) -> Case {
    let mut parts = GateLoop { count: String::from("n"), ..GateLoop::default() };
    let divisor = match proof {
        Proof::Constant(value) => format!("{value}U"),
        Proof::Range { from_column, mask, low } => {
            let source = if from_column { "column" } else { "n" };
            parts.before = format!("unsigned d = ({source} & {mask}U) + {low}U;");
            String::from("d")
        }
        Proof::TestedInLoop(multiplier) | Proof::Guarded(multiplier) => {
            parts.before = format!("unsigned d = column * {multiplier}U;");
            String::from("d")
        }
    };
    let division = format!("{} & 1", quotient(&format!("i + {offset}U"), &divisor, remainder, signed));
    parts.test = match proof {
        Proof::TestedInLoop(_) => format!("d && ({division})"),
        Proof::Constant(_) | Proof::Range { .. } | Proof::Guarded(_) => division,
    };
    if let Proof::Guarded(_) = proof {
        parts.guard = String::from("if (d) ");
    }
    parts.case(true, 0, Expectation::Clean)
}

const TESTED_MULTIPLIERS: &[u8] = &[1, 2, 4, 6, 8];

fn proved_division(random: &mut ChaCha8Rng) -> Case {
    let proof = match random.random_range(0..4) {
        0 => Proof::Constant(random.random_range(1..=9)),
        1 => Proof::Range {
            from_column: random.random_bool(0.5),
            mask: [1, 3, 7, 15, 255][random.random_range(0..5)],
            low: random.random_range(1..=3),
        },
        // Odd multipliers above 1 are refused today: see `gate_loop_divides_by_an_odd_multiple_tested_nonzero`.
        2 => Proof::TestedInLoop(TESTED_MULTIPLIERS[random.random_range(0..TESTED_MULTIPLIERS.len())]),
        _ => Proof::Guarded(TESTED_MULTIPLIERS[random.random_range(0..TESTED_MULTIPLIERS.len())]),
    };
    let case =
        proved_division_loop(proof, random.random_range(0..=15), random.random_bool(0.5), random.random_bool(0.5));
    Case { trips: random.random_range(1..=5), ..case }
}

fn refused(name: &str, case: &Case, expected: &[Vec<u128>]) {
    let Expectation::Refused(parts) = case.expectation else { panic!("{name}: not a refused case") };
    let source = write_c(name, &case.source());
    let table = case.native_table(&native_build(&source, name, &["-DNATIVE"]));
    assert_eq!(table, expected, "{name}: native packets");
    for &mode in ALL_MODES {
        let (mode, cflags, flags) = MODES[mode];
        let name = format!("{name}-{mode}");
        let refusal = lower_text_with_diagnostics(&source, &name, cflags, flags).expect_err("named refusal");
        assert!(parts.iter().all(|part| refusal.contains(part)), "{name}: {refusal}");
    }
}

#[test]
fn gate_loop_refuses_a_division_by_zero_where_c_skips_it() {
    // C enters the loop only for column 1, where the divisor is 1 or 3 and the target ends at 1.
    for (name, case) in [
        ("gate-loop-udiv", unproved_division_loop(1, 1, false, false, false)),
        ("gate-loop-srem", unproved_division_loop(3, 1, true, true, false)),
        ("gate-loop-udiv-before", unproved_division_loop(3, 1, false, false, true)),
    ] {
        refused(name, &Case { trips: 2, ..case }, &[vec![0], vec![0x100]]);
    }
}

#[test]
fn gate_loop_refuses_a_read_where_c_skips_it() {
    // For column 1 the three passes read cells 1, 2 and 1, all nonzero: the target ends at 1.
    for (name, read, cell0) in [
        ("gate-loop-held", Read::Held, 1),
        ("gate-loop-past-end", Read::OutOfRange, 0),
        ("gate-loop-null", Read::Null, 0),
    ] {
        let expected = [vec![cell0 << 16], vec![(cell0 << 16) | 0x100]];
        refused(name, &Case { trips: 3, ..skipped_read_loop(read, 1, 2) }, &expected);
    }
}

#[test]
fn gate_loop_divides_by_a_proved_divisor() {
    // For q = 1 the three passes compute i / 2 = 0, 0, 1: the target ends at 1 where the divisor is 2. The tested
    // and guarded divisors are 0 for column 0, where the loop divides nowhere and the target stays 0.
    let everywhere = [vec![0, 0x101], vec![0, 0x101]];
    let column1 = [vec![0, 0x001], vec![0, 0x101]];
    for (name, proof, expected) in [
        ("gate-loop-constant", Proof::Constant(2), &everywhere),
        ("gate-loop-ranged", Proof::Range { from_column: false, mask: 1, low: 1 }, &everywhere),
        ("gate-loop-tested", Proof::TestedInLoop(2), &column1),
        ("gate-loop-guarded", Proof::Guarded(2), &column1),
    ] {
        agrees(name, Case { trips: 3, ..proved_division_loop(proof, 0, false, false) }, ALL_MODES, expected);
    }
}

#[test]
#[ignore = "LLVM rewrites the test of `column * 3` as a test of `column`; the divisor proof does not see that it implies the product is nonzero, so the front refuses a division C guards"]
fn gate_loop_divides_by_an_odd_multiple_tested_nonzero() {
    for (name, proof) in
        [("gate-loop-tested-odd", Proof::TestedInLoop(3)), ("gate-loop-guarded-odd", Proof::Guarded(3))]
    {
        // For column 1 the passes compute i / 3 = 0, 0, 0, 1: the target ends at 1.
        agrees(
            name,
            Case { trips: 4, ..proved_division_loop(proof, 0, false, false) },
            ALL_MODES,
            &[vec![0, 0x001], vec![0, 0x101]],
        );
    }
}
