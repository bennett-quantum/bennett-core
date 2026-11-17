//! Memory written under a qubit (docs/DESIGN.md, "Memory written under a qubit") and the measurement a program
//! writes to read it, `bennett_measure_bytes` (docs/MARKERS.md, "Measurement").
#[allow(dead_code, reason = "the native reference of the fallback oracle, not every entry point")]
#[path = "fallback_support/native.rs"]
mod native;

use std::collections::BTreeMap;
use std::process::Command;

use bennett_exec::{Executor, Outcome, Snapshot};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, bitcode, c_dir, entry, front, lower, lower_text, options, root, run, run_with_stats,
    state_before_first_measurement, write_c,
};
use bennett_ir::ValidModule;
use native::{INITIAL_IMAGE, Native};

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];

fn returned(outcome: Outcome, context: &str) -> u128 {
    let Outcome::Returned(Some(value)) = outcome else { panic!("{context}: {outcome:?}") };
    value
}

/// On ordinary bytes the marker reads them, little-endian, as many as it is told; on a whole qubit variable it
/// measures it, as `bennett_measure` does.
#[test]
fn measure_bytes_reads_ordinary_memory_and_measures_a_whole_qubit_variable() {
    let source = write_c(
        "measure-bytes",
        "#include \"bennett.h\"\nunsigned char g[4] = {1, 2, 3, 4};\nunsigned long long entry(void) {\n    unsigned short x = 0;\n    bennett_x(bennett_bit(&x, 1)); bennett_x(bennett_bit(&x, 9));\n    unsigned long long low = bennett_measure_bytes(g + 1, 2);\n    unsigned long long all = bennett_measure_bytes(g, 4);\n    return low + (all << 16) + (bennett_measure_bytes(&x, 2) << 48);\n}\n",
    );
    let expected = 0x0302 + (0x0403_0201 << 16) + (0x0202 << 48);
    for (mode, clang, flags) in MODES {
        let module = lower(&source, &format!("measure-bytes-{mode}"), clang, flags);
        assert_eq!(returned(run(&module, "entry", &[], 0), mode), expected, "{mode}");
    }
}

#[test]
fn measure_bytes_needs_a_constant_count_outside_a_qubit_condition() {
    let cases = [
        (
            "measure-bytes-count",
            "unsigned long long entry(unsigned n) {\n    unsigned char g[8] = {0};\n    _Bool q = 0; bennett_h(&q);\n    return bennett_measure_bytes(g, n) + bennett_measure(&q);\n}\n",
            "bennett_measure_bytes with a number of bytes that is not a constant from 1 to 8",
        ),
        (
            "measure-bytes-wide",
            "unsigned char g[16];\nunsigned long long entry(void) {\n    _Bool q = 0; bennett_h(&q);\n    return bennett_measure_bytes(g, 9) + bennett_measure(&q);\n}\n",
            "bennett_measure_bytes with a number of bytes that is not a constant from 1 to 8",
        ),
        (
            "measure-bytes-controlled",
            "unsigned char g[4];\nunsigned long long entry(void) {\n    _Bool q = 0; bennett_h(&q);\n    unsigned long long r = 0;\n    if (q) r = bennett_measure_bytes(g, 1);\n    return r;\n}\n",
            "a measurement under the control of a value that depends on a qubit",
        ),
        (
            "measure-bytes-part",
            "unsigned long long entry(void) {\n    unsigned short x = 0; bennett_h(bennett_bit(&x, 0));\n    return bennett_measure_bytes(&x, 1);\n}\n",
            "a measurement of part of qubit variable 'x'",
        ),
    ];
    for (name, body, message) in cases {
        let source = write_c(name, &format!("#include \"bennett.h\"\n{body}"));
        let error = lower_text(&source, name, CLANG_FLAGS, &[]).expect_err(name);
        assert!(error.contains(message), "{name}: {error}");
    }
}

/// Lowers `source` and returns the device text and the front's diagnostics (warnings included).
fn lower_with_diagnostics(source: &std::path::Path, name: &str, clang: &[&str], flags: &[&str]) -> (String, String) {
    let include = format!("-I{}", root().join("front/include").display());
    let clang: Vec<&str> = clang.iter().copied().chain([include.as_str()]).collect();
    let bc = bitcode(source, name, &clang);
    let bn = bc.with_extension("bn");
    let output = Command::new(front())
        .args(["--stack-bytes=65536", "--region-budget=100000"])
        .args(flags)
        .arg(&bc)
        .arg("-o")
        .arg(&bn)
        .output()
        .expect("front runs");
    let diagnostics = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{name}: {diagnostics}");
    (std::fs::read_to_string(bn).expect("device text"), diagnostics)
}

/// The address of the fixtures' twelve-byte image (the front's `; @image` data comment).
fn image_address(text: &str) -> usize {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "; @image" {
            let data = lines.next().expect("data line").trim().strip_prefix("data ").expect("data");
            return data.split_whitespace().next().expect("address").parse().expect("number");
        }
    }
    panic!("no @image in the device program")
}

/// The front's shadows of bytes in memory (docs/DESIGN.md, "Memory written under a qubit"): the register's slot
/// name and the address of its first byte, read from the exchange's first gate `x [ADDR:0] ctrl %name[0]`.
fn shadows(text: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("x [") else { continue };
        let Some((address, rest)) = rest.split_once(":0] ctrl %") else { continue };
        let Some(name) = rest.strip_suffix("[0]") else { continue };
        let entry = (name.to_string(), address.parse().expect("address"));
        if !found.contains(&entry) {
            found.push(entry);
        }
    }
    found
}

/// The image in one basis configuration: the machine's memory, with each byte that holds qubits read from the
/// configuration's value of its register, and, while a shadow holds bytes (between its two exchanges), those
/// bytes read from the shadow.
fn image(module: &ValidModule, text: &str, state: &Snapshot, config: &[u64], address: usize) -> [u8; 12] {
    let mut bytes: [u8; 12] = state.classical.memory[address..address + 12].try_into().expect("image");
    let window = address..address + 12;
    for &(at, register) in &state.quantum.cells {
        let at = usize::try_from(at).expect("address");
        if window.contains(&at) {
            bytes[at - address] = u8::try_from(config[register]).expect("a byte");
        }
    }
    let frame = state.classical.frames.last().expect("a frame");
    let f = module.function(frame.func);
    for (name, start) in shadows(text) {
        let slot = f.slot_names.iter().position(|s| *s == name).expect("shadow slot");
        let register = usize::try_from(state.classical.slots[frame.base + slot] & 0xffff_ffff).expect("register");
        let Some(&value) = config.get(register) else { continue };
        let held = state.quantum.cells.iter().any(|&(at, _)| usize::try_from(at).is_ok_and(|at| at == start));
        if !held {
            continue; // before the first exchange: the bytes are ordinary and the shadow is 0
        }
        for k in 0..8 {
            if window.contains(&(start + k)) && value >> (8 * k) & 0xff != 0 {
                bytes[start + k - address] |= (value >> (8 * k)) as u8;
            }
        }
    }
    bytes
}

/// The columns of the C variables q, r, y in a state standing in `experiment`.
fn columns(module: &ValidModule, state: &Snapshot) -> [usize; 3] {
    let frame = state.classical.frames.last().expect("a frame");
    let f = module.function(frame.func);
    ["q", "r", "y"].map(|name| {
        let slot = f.slot_names.iter().position(|s| s == name).expect("slot");
        usize::try_from(state.classical.slots[frame.base + slot] & 0xffff_ffff).expect("register")
    })
}

/// The fallback oracle's memory fixtures, whose stores run under q and r (`front/tests/c/fallback_oracle`), the
/// driver measuring the cells with `bennett_measure_bytes` (`-DMEASURED_CELLS`): for each definite input the device's q, r, y and
/// whole image (guard bytes included, bytes that still hold qubits read from their registers) are native C's,
/// with nothing released that is not zero; for every preparation of the oracle (the eight columns, a complex
/// and a real product state whose eight weights all differ) the exact weight of each result just before the
/// first measurement is native C's; and the front warns that the bytes hold qubits.
#[test]
fn the_fallback_memory_fixtures_give_native_results_and_weights() {
    for name in ["store", "memset", "nested", "memcpy", "loop"] {
        let native = Native::compile(name, "-O0");
        let source = c_dir().join(format!("fallback_oracle/{name}.c"));
        for (mode, clang, flags) in MODES {
            let context = format!("{name} {mode}");
            let clang: Vec<&str> = clang.iter().copied().chain(["-DMEASURED_CELLS"]).collect();
            let (text, diagnostics) = lower_with_diagnostics(&source, &format!("held-{name}-{mode}"), &clang, flags);
            assert!(diagnostics.contains("hold qubits from here on, entangled"), "{context}: {diagnostics}");
            let module = bennett_ir::load(&text).expect("valid");
            let address = image_address(&text);
            for basis in 0..8 {
                run(&module, "experiment", &[basis], 0);
                let mut executor =
                    Executor::new(&module, entry(&module, "experiment"), &[basis], options(0)).expect("starts");
                let Outcome::Returned(Some(value)) = executor.run() else { panic!("{context}: basis {basis}") };
                let state = executor.snapshot();
                let config = &state.quantum.amplitudes[0].0;
                assert_eq!(state.quantum.amplitudes.len(), 1, "{context}: a definite input ends definite");
                let found = native::Image {
                    bits: (value & 7) as usize,
                    memory: image(&module, &text, &state, config, address),
                };
                let expected = &native.images[basis as usize];
                assert_eq!(found, *expected, "WRONG RESULT {context}, basis {basis}");
                assert_eq!(value >> 3, u128::from(u32::from_le_bytes(expected.cells())), "{context}: cells");
            }
            for preparation in 0..=9 {
                let expected = native.measured(&native::prepared(preparation));
                let state = state_before_first_measurement(&module, "experiment", &[preparation as u64]);
                let named = columns(&module, &state);
                let mut weights = BTreeMap::<u128, f64>::new();
                for (config, amplitude) in &state.quantum.amplitudes {
                    let bits = named.iter().enumerate().fold(0, |b, (i, &c)| b | (config[c] as usize) << i);
                    let key = native::Image { bits, memory: image(&module, &text, &state, config, address) }.measured();
                    *weights.entry(key).or_default() += amplitude.norm_sqr();
                }
                weights.retain(|_, w| *w > 1e-14);
                for key in expected.keys().chain(weights.keys()) {
                    let (e, a) = (expected.get(key).copied().unwrap_or(0.0), weights.get(key).copied().unwrap_or(0.0));
                    assert!(
                        (e - a).abs() < 1e-9,
                        "WRONG RESULT {context}: preparation {preparation}, {key}: {a}, not {e}"
                    );
                }
            }
        }
    }
    assert_eq!(INITIAL_IMAGE[4..8], [1, 2, 3, 0]);
}

/// Programs of the entry `entry`, a global `g` of four bytes {1, 2, 3, 4} and qubits q, r.
fn program(name: &str, body: &str) -> std::path::PathBuf {
    write_c(name, &format!("#include \"bennett.h\"\nunsigned char g[4] = {{1, 2, 3, 4}};\n{body}\n"))
}

/// A store under a qubit and its inverse under the same qubit leave the bytes as they were and q unentangled:
/// a Hadamard on q then gives 0 every time, which it would not if the old value had been kept or measured.
#[test]
fn a_store_and_its_inverse_under_a_qubit_keep_the_interference() {
    let source = program(
        "held-undone",
        "unsigned long long entry(void) {\n    _Bool q = 0; bennett_h(&q);\n    if (q) g[0] += 6;\n    if (q) g[0] -= 6;\n    bennett_h(&q);\n    return bennett_measure(&q) | bennett_measure_bytes(g, 4) << 8;\n}",
    );
    for (mode, clang, flags) in MODES {
        let module = lower(&source, &format!("held-undone-{mode}"), clang, flags);
        for seed in 0..16 {
            assert_eq!(returned(run(&module, "entry", &[], seed), mode), 0x0403_0201_00, "{mode} seed {seed}");
        }
    }
}

/// A store of a qubit-dependent value outside any condition, a local array written under a qubit (it becomes a
/// qubit variable), and a store whose old value no clearing recovers (kept, with the warning and the count):
/// each gives C's value for each value of the qubits, with C's weights.
#[test]
fn straight_stores_local_arrays_and_kept_old_values_give_the_c_values() {
    // q, r in |+>; the result packs what C computes, so its distribution is exact.
    let cases: [(&str, &str, fn(u64, u64) -> u64, bool); 3] = [
        (
            "held-straight",
            "g[1] = q + 2 * r;\n    return bennett_measure_bytes(g, 4) | bennett_measure(&q) << 32 | bennett_measure(&r) << 33;",
            |q, r| 0x0403_0001 | (q + 2 * r) << 8 | q << 32 | r << 33,
            false,
        ),
        (
            "held-local",
            "unsigned char a[4] = {1, 2, 3, 4};\n    if (q) a[3] = 5;\n    if (q) a[3] += 2;\n    a[0] = r;\n    return bennett_measure_bytes(a, 4) | bennett_measure(&q) << 32 | bennett_measure(&r) << 33;",
            |q, r| 0x0003_0200 | r | (if q == 1 { 7 } else { 4 }) << 24 | q << 32 | r << 33,
            false,
        ),
        (
            "held-retained",
            "if (q) g[0] = 7;\n    if (r) g[0] = 9;\n    return bennett_measure_bytes(g, 1) | bennett_measure(&q) << 8 | bennett_measure(&r) << 9;",
            |q, r| if r == 1 { 9 } else if q == 1 { 7 } else { 1 } | q << 8 | r << 9,
            true,
        ),
    ];
    for (name, body, f, kept) in cases {
        let source = program(
            name,
            &format!(
                "unsigned long long entry(void) {{\n    _Bool q = 0, r = 0; bennett_h(&q); bennett_h(&r);\n    {body}\n}}"
            ),
        );
        for (mode, clang, flags) in MODES {
            let (text, diagnostics) = lower_with_diagnostics(&source, &format!("{name}-{mode}"), clang, flags);
            assert_eq!(
                diagnostics.contains("keeps the old value in 8 qubits each time it runs"),
                kept,
                "{name} {mode}: {diagnostics}"
            );
            let module = bennett_ir::load(&text).expect("valid");
            let mut seen = BTreeMap::<u128, usize>::new();
            for seed in 0..64 {
                let (outcome, stats) = run_with_stats(&module, "entry", &[], seed);
                assert_eq!(stats.nonzero_releases != 0, kept, "{name} {mode}: releases");
                *seen.entry(returned(outcome, name)).or_default() += 1;
            }
            let expected: std::collections::BTreeSet<u128> = (0..4).map(|b| u128::from(f(b & 1, b >> 1))).collect();
            assert_eq!(seen.keys().copied().collect::<std::collections::BTreeSet<_>>(), expected, "{name} {mode}");
        }
    }
}

/// Ordinary code may not read memory that holds qubits; the message asks for the measurement. Other refusals:
/// more bytes than one register, an object the front cannot name, a read at a position that is not constant
/// outside code applied whole, and a store into memory in code with gates.
#[test]
fn memory_that_holds_qubits_is_refused_to_ordinary_code_and_the_rest_is_named() {
    let cases = [
        (
            "held-unmeasured",
            "unsigned long long entry(void) { _Bool q = 0; bennett_h(&q); if (q) g[0] = 7; return g[0]; }",
            "a value read from memory that holds qubits (qubit variable holding the 1 byte at @g+0), used by ordinary code (measure the bytes first, with bennett_measure_bytes)",
        ),
        (
            "held-wide",
            "unsigned char w[16];\nunsigned long long entry(void) { _Bool q = 0; bennett_h(&q); if (q) { w[0] = 1; w[15] = 2; } return bennett_measure_bytes(w, 8) + bennett_measure(&q); }",
            "more than one register holds",
        ),
        (
            "held-unnamed",
            "unsigned char *p;\nunsigned long long entry(void) { _Bool q = 0; bennett_h(&q); if (q) *p = 1; return bennett_measure(&q); }",
            "into memory whose object the front cannot name",
        ),
        (
            "held-variable-read",
            "unsigned long long entry(unsigned i) { _Bool q = 0; bennett_h(&q); if (q) g[0] = 7; unsigned char v = g[i & 3]; return bennett_measure(&q) + v; }",
            "not handled yet",
        ),
        (
            "held-gates",
            "unsigned long long entry(void) { _Bool q = 0, y = 0; bennett_h(&q); if (q) { g[0] = 7; bennett_x(&y); } return bennett_measure(&q) + bennett_measure(&y); }",
            "in code that also has gates",
        ),
    ];
    for (name, body, message) in cases {
        let error = lower_text(&program(name, body), name, CLANG_FLAGS, &[]).expect_err(name);
        assert!(error.contains(message), "{name}: {error}");
    }
}
