//! Loops with gates under a condition that depends on a qubit (docs/DESIGN.md, "Code under a qubit"). The
//! expected states are computed here from the C by hand: per value of the condition's register, the gates
//! the C applies, multiplied out on one qubit.
use bennett_front_tests::state_before_first_measurement;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, column, lower, lower_text, registers, write_c};
use num_complex::Complex64;

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];
const PASSES: [u64; 5] = [0, 1, 2, 3, 6];

type Qubit = [Complex64; 2];

fn h(v: Qubit) -> Qubit {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    [(v[0] + v[1]) * s, (v[0] - v[1]) * s]
}

/// Checks the state of `entry(n)` before its first measurement against `expected`: (value of the condition's
/// register, amplitude of y = 0 and of y = 1); every other register 0.
fn check(module: &bennett_ir::ValidModule, entry: &str, n: u64, control: &str, expected: &[(u64, Qubit)], mode: &str) {
    let state = state_before_first_measurement(module, entry, &[n]);
    let columns = registers(module, entry);
    let (c, y) = (column(&columns, control), column(&columns, "y"));
    let mut wanted = Vec::new();
    for &(value, qubit) in expected {
        for (bit, amplitude) in qubit.iter().enumerate() {
            if amplitude.norm() > 1e-12 {
                wanted.push((value, bit as u64, *amplitude));
            }
        }
    }
    assert_eq!(state.quantum.amplitudes.len(), wanted.len(), "{mode} @{entry}({n}): configurations");
    for (config, amplitude) in &state.quantum.amplitudes {
        let (_, _, want) = wanted
            .iter()
            .find(|(value, bit, _)| config[c] == *value && config[y] == *bit)
            .unwrap_or_else(|| panic!("{mode} @{entry}({n}): unexpected configuration {columns:?} = {config:?}"));
        assert!((amplitude - want).norm() < 1e-12, "{mode} @{entry}({n}) {config:?}: {amplitude}, not {want}");
        for (i, name) in columns.iter().enumerate() {
            if i != c && i != y {
                assert_eq!(config[i], 0, "{mode} @{entry}({n}): register %{name} not 0");
            }
        }
    }
}

#[test]
fn gates_in_a_loop_with_an_ordinary_trip_count_act_where_the_condition_holds() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let zero: Qubit = [Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)];
    for (mode, clang, flags) in MODES {
        let module = lower(&c_dir().join("gate_loops.c"), &format!("gate-loops-{mode}"), clang, flags);
        for n in PASSES {
            // flips: y = n mod 2 where q = 1.
            let flipped = if n % 2 == 1 { [zero[1], zero[0]] } else { zero };
            let half = |q: Qubit| [q[0] * s, q[1] * s];
            check(&module, "flips", n, "q", &[(0, half(zero)), (1, half(flipped))], mode);
            // phases: where x * 3 % 5 == 1 (x = 2 of 0..3), H on every pass and Z after it on odd passes.
            let mut y = zero;
            for i in 0..n {
                y = h(y);
                if i % 2 == 1 {
                    y[1] = -y[1];
                }
            }
            let quarter = |q: Qubit| [q[0] * 0.5, q[1] * 0.5];
            let expected: Vec<(u64, Qubit)> = (0..4).map(|x| (x, quarter(if x == 2 { y } else { zero }))).collect();
            check(&module, "phases", n, "x", &expected, mode);
        }
    }
}

/// The loop stays one loop of the control program: its gates are inside it, controlled, and nothing is
/// unrolled (the number of gate lines does not depend on how many passes run).
#[test]
fn the_loop_stays_in_the_control_program_and_is_not_unrolled() {
    let text = lower_text(&c_dir().join("gate_loops.c"), "gate-loops-shape", CLANG_FLAGS, &[]).expect("lowers");
    let body = text.split("func @flips(").nth(1).expect("@flips").split("\n}\n").next().expect("body");
    let gates: Vec<&str> = body.lines().filter(|line| line.trim().starts_with("x %y")).collect();
    assert_eq!(gates.len(), 1, "one controlled X in the loop: {gates:?}");
    assert!(gates[0].contains(" ctrl "), "{gates:?}");
}

fn refusal(name: &str, body: &str) -> String {
    let source = write_c(
        name,
        &format!(
            "#include \"bennett.h\"\nunsigned long long entry(unsigned n) {{\n    unsigned char x = 0; _Bool y = 0, q = 0;\n    bennett_h(bennett_bit(&x, 0)); bennett_h(bennett_bit(&x, 1)); bennett_h(&q);\n{body}\n    return bennett_measure(&x) | bennett_measure(&y) << 8 | bennett_measure(&q) << 9;\n}}\n"
        ),
    );
    lower_text(&source, name, CLANG_FLAGS, &[]).expect_err("refused")
}

#[test]
fn a_gate_loop_whose_number_of_passes_depends_on_a_qubit_is_refused_with_the_route() {
    let error = refusal("gate-loops-trip", "    while (x > 0) { bennett_h(&y); x--; }");
    assert!(
        error.starts_with("not handled yet:")
            && error.contains("number of passes must be written as an ordinary value")
            && error.contains("does not consume its loop variable"),
        "{error}"
    );
}

#[test]
fn a_gate_loop_deciding_on_values_carried_from_pass_to_pass_is_refused_by_name() {
    let error = refusal(
        "gate-loops-carried",
        "    unsigned t = x;\n    for (unsigned i = 0; i < n; i++) { if (t & 1) bennett_h(&y); t = t * 5 + 1; }",
    );
    assert!(error.starts_with("not handled yet:") && error.contains("table of control bits"), "{error}");
}

#[test]
fn a_gate_in_the_loop_on_what_the_condition_reads_is_refused_by_name() {
    let error =
        refusal("gate-loops-own", "    if (q) for (unsigned i = 0; i < n; i++) { bennett_x(&y); bennett_h(&q); }");
    assert!(error.starts_with("not handled yet:") && error.contains("would change"), "{error}");
}
