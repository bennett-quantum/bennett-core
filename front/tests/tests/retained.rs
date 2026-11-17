//! Overwrites of a qubit variable whose old value the front cannot compute back (docs/DESIGN.md, "Overwrites
//! that keep the old value"): accepted with a warning, the old value kept in a register released at once.
//! For each definite value of x the variable then holds what C computes; amplitudes of different old values
//! no longer interfere, so the check is on the weights of the measured values, from native C arithmetic.
use std::collections::BTreeMap;

use bennett_front_tests::state_before_first_measurement;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, column, front, lower_text, registers, root, write_c};

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];

/// x has bits 0 to 2 in superposition (x0 = 0..7, weight 1/8 each); `body` overwrites x; `f` is the same
/// overwrite in Rust.
fn check(name: &str, body: &str, f: impl Fn(u64) -> u64) {
    let source = write_c(
        name,
        &format!(
            "#include \"bennett.h\"\nunsigned long long entry(void) {{\n    unsigned char x = 0;\n    bennett_h(bennett_bit(&x, 0)); bennett_h(bennett_bit(&x, 1)); bennett_h(bennett_bit(&x, 2));\n    {body}\n    return bennett_measure(&x);\n}}\n"
        ),
    );
    let mut expected = BTreeMap::<u64, f64>::new();
    for x0 in 0..8 {
        *expected.entry(f(x0) & 0xff).or_default() += 1.0 / 8.0;
    }
    for (mode, clang, flags) in MODES {
        let text = lower_text(&source, &format!("{name}-{mode}"), clang, flags).expect("accepted");
        let module = bennett_ir::load(&text).expect("valid");
        let state = state_before_first_measurement(&module, "entry", &[]);
        let x = column(&registers(&module, "entry"), "x");
        let mut weights = BTreeMap::<u64, f64>::new();
        for (config, amplitude) in &state.quantum.amplitudes {
            *weights.entry(config[x]).or_default() += amplitude.norm_sqr();
        }
        assert_eq!(weights.keys().collect::<Vec<_>>(), expected.keys().collect::<Vec<_>>(), "{mode} {name}");
        for (value, weight) in &expected {
            assert!((weights[value] - weight).abs() < 1e-12, "{mode} {name}: x = {value}: {}", weights[value]);
        }
    }
}

#[test]
fn overwrites_that_cannot_be_undone_give_the_c_values_with_their_weights() {
    check("retained-square", "x = x * x;", |x| x * x);
    check("retained-double", "x += x;", |x| 2 * x);
    check("retained-divide", "x = x / 3;", |x| x / 3);
    check("retained-constant", "x = 5;", |_| 5);
    check("retained-controlled", "if (x & 4) x = x * x;", |x| if x & 4 != 0 { x * x } else { x });
}

/// The warning names the variable, says how many qubits each executed overwrite keeps, and what is lost.
#[test]
fn the_warning_gives_the_count_of_qubits_kept() {
    let source = write_c(
        "retained-warning",
        "#include \"bennett.h\"\nunsigned long long entry(void) {\n    unsigned short x = 0;\n    bennett_h(bennett_bit(&x, 0)); bennett_h(bennett_bit(&x, 1));\n    x = x / 3;\n    return bennett_measure(&x);\n}\n",
    );
    let include = format!("-I{}", root().join("front/include").display());
    let clang: Vec<&str> = CLANG_FLAGS.iter().copied().chain([include.as_str()]).collect();
    let bitcode = bennett_front_tests::bitcode(&source, "retained-warning", &clang);
    let output = std::process::Command::new(front())
        .args(["--stack-bytes=65536", "--region-budget=1000"])
        .arg(&bitcode)
        .output()
        .expect("front runs");
    assert!(output.status.success(), "{output:?}");
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostics.contains("warning: the overwrite of qubit variable 'x'")
            && diagnostics.contains("keeps the old value in 16 qubits each time it runs")
            && diagnostics.contains("interference between configurations with different old values is lost"),
        "{diagnostics}"
    );
}
