//! In-place updates of qubit variables, written as ordinary C (`front/tests/c/inplace.c`): on a uniform
//! superposition of every 8-bit x and y (and q, for a controlled update), every branch holds exactly
//! x = g(x0, y, q) as the native build computes it, and nothing else; the update followed by its inverse
//! gives back the start state.

use std::process::Command;
use std::sync::OnceLock;

use bennett_front_tests::{CLANG_FLAGS, branches, c_dir, column, lower, native_build};
use bennett_ir::ValidModule;
use num_complex::Complex64;

/// x and y in superposition, times 2 for q: the largest state here.
const MAX_CONFIGURATIONS: usize = 1 << 17;

/// How far apart amplitudes may be after a few dozen gates.
const TOLERANCE: f64 = 1e-12;

fn module(front_flags: &'static [&'static str]) -> &'static ValidModule {
    static PRECISE: OnceLock<ValidModule> = OnceLock::new();
    static COARSE: OnceLock<ValidModule> = OnceLock::new();
    let cell = if front_flags.is_empty() { &PRECISE } else { &COARSE };
    cell.get_or_init(|| {
        lower(&c_dir().join("inplace.c"), &format!("inplace{}", front_flags.concat()), CLANG_FLAGS, front_flags)
    })
}

/// The updates `inplace.c` lists.
fn updates() -> Vec<String> {
    let text = std::fs::read_to_string(c_dir().join("inplace.c")).unwrap_or_else(|e| panic!("inplace.c: {e}"));
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("X(").and_then(|rest| rest.split(',').next()))
        .map(str::to_owned)
        .collect()
}

/// g(x, y, q) for every q, y and x, from the native build: index (q * 256 + y) * 256 + x.
fn native(name: &str) -> Vec<u64> {
    static BUILD: OnceLock<std::path::PathBuf> = OnceLock::new();
    let executable = BUILD.get_or_init(|| native_build(&c_dir().join("inplace.c"), "inplace", &["-DNATIVE"]));
    let output = Command::new(executable).arg(name).output().unwrap_or_else(|e| panic!("native {name}: {e}"));
    assert!(output.status.success(), "native {name} failed");
    let values: Vec<u64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.parse().unwrap_or_else(|e| panic!("native {name}: {line:?}: {e}")))
        .collect();
    assert_eq!(values.len(), 2 * 256 * 256, "native {name}");
    values
}

/// The branches of `function`, as (x0, x, y, q), after checking that they are all present with equal
/// amplitudes: 2^16 of them, or 2^17 for a controlled update.
fn uniform_branches(module: &ValidModule, function: &str, controlled: bool) -> Vec<[u64; 4]> {
    let (columns, state) = branches(module, function, &[], MAX_CONFIGURATIONS);
    let expected = if controlled { 1 << 17 } else { 1 << 16 };
    assert_eq!(state.len(), expected, "@{function}: branches");
    let amplitude = Complex64::new(1.0 / (expected as f64).sqrt(), 0.0);
    let at = |name: &str| column(&columns, name);
    let (x0, x, y, q) = (at("x0"), at("x"), at("y"), at("q"));
    state
        .iter()
        .map(|(values, a)| {
            assert!((a - amplitude).norm() <= TOLERANCE, "@{function}: amplitude {a} on {values:?}");
            [values[x0], values[x], values[y], values[q]]
        })
        .collect()
}

fn check_update(front_flags: &'static [&'static str], name: &str) {
    let module = module(front_flags);
    let controlled = name.starts_with("controlled");
    let expected = native(name);
    let mut starts = std::collections::BTreeSet::new();
    for [x0, x, y, q] in uniform_branches(module, name, controlled) {
        let g = expected[((q * 256 + y) * 256 + x0) as usize];
        assert_eq!(x, g, "@{name} {front_flags:?}: x0 = {x0}, y = {y}, q = {q}");
        starts.insert((x0, y, q));
    }
    assert_eq!(starts.len(), if controlled { 1 << 17 } else { 1 << 16 }, "@{name}: every start value");
    for [x0, x, y, q] in uniform_branches(module, &format!("{name}_undone"), controlled) {
        assert_eq!(x, x0, "@{name}_undone {front_flags:?}: y = {y}, q = {q}");
    }
}

/// One test per update, so that they run in parallel; the list must be the one `inplace.c` has.
macro_rules! updates {
    ($($name:ident),* $(,)?) => {
        const NAMES: &[&str] = &[$(stringify!($name)),*];
        $(
            #[test]
            fn $name() {
                check_update(&[], stringify!($name));
                check_update(&["--taint-all"], stringify!($name));
            }
        )*
    };
}

mod update {
    use super::check_update;

    updates!(
        add,
        sub,
        xor,
        not,
        negate,
        increment,
        times3,
        rotate,
        rotate3,
        composed,
        subtracted_from,
        controlled_add,
        controlled_composed,
        controlled_either,
    );

    #[test]
    fn every_update_of_inplace_c_is_tested() {
        assert_eq!(NAMES, super::updates().as_slice());
    }
}

/// A byte swap and a multiplication on a 16-bit variable, every value of it in superposition.
#[test]
fn a_byte_swap_is_inverted_at_sixteen_bits() {
    let (columns, state) = branches(module(&[]), "swapped", &[], MAX_CONFIGURATIONS);
    assert_eq!(state.len(), 1 << 16);
    let (x0, x) = (column(&columns, "x0"), column(&columns, "x"));
    for (values, _) in state {
        let expected = u64::from((values[x0] as u16).swap_bytes().wrapping_mul(3).wrapping_add(1));
        assert_eq!(values[x], expected, "x0 = {}", values[x0]);
    }
}
