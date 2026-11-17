//! Memory written under a qubit (docs/DESIGN.md, "Memory written under a qubit") and the measurement a program
//! writes to read it, `bennett_measure_bytes` (docs/MARKERS.md, "Measurement").
use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, lower, lower_text, run, write_c};

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
