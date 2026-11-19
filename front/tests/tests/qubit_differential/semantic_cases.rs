//! Opt-in differential recipes for known compiler defects.

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::{Case, MODES, compare, lower_text, native_build, prepare, write_c};

type Generate = fn(&mut ChaCha8Rng) -> Case;

struct Family {
    switch: &'static str,
    recipes: &'static [Generate],
    modes: &'static [usize],
}

// These recipes check semantics that depend on what clang hands to the front.
// Clang -O2 can change the relevant calls and stores before the front sees them.
const PROJECT_MODES: &[usize] = &[0, 2];
const FAMILIES: &[Family] = &[Family {
    switch: "BENNETT_RANDOM_WRITING_CALLS",
    recipes: &[removed_call_argument, merged_writing_calls],
    modes: PROJECT_MODES,
}];

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
