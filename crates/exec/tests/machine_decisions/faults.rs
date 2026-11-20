use bennett_exec::{Options, Outcome};
use bennett_ir::Fault;

use crate::common::{assert_same_state, bounded, executor, load};

/// DESIGN, "apply": on a fault `out` is unchanged and `fault ^= c`. These literal codes are
/// deliberately independent of Fault::code: a coordinated enum/region regression must fail too.
const CODES: &[(&str, u64)] = &[
    ("budget", 1),
    ("divide_by_zero", 2),
    ("out_of_bounds", 3),
    ("bad_indirect", 4),
    ("unreachable", 5),
    ("stack_overflow", 6),
    ("effect_in_region", 7),
    ("quantum_in_region", 8),
    ("dangling_register", 9),
    ("qubit_alias", 10),
    ("bad_stack_restore", 12),
    ("exit_in_region", 13),
    ("exit_type_mismatch", 14),
    ("bad_longjmp", 16),
    ("qubit_memory_access", 17),
    ("bad_angle", 18),
    ("out_of_memory", 19),
];

#[test]
fn every_fault_xors_into_dirty_fault_registers() {
    let mut cases = 0;
    for width in [5, 6, 17, 64] {
        for &(name, code) in CODES {
            let text = format!(
                "memory 4096\nfunc @seed(i{width} %v) -> i{width} {{\nentry:\n ret %v\n}}\n\
                 func @fail() -> i2 {{\nentry:\n fault {name}\n}}\n\
                 func @main(i{width} %v) {{\nentry:\n %f = qalloc {width}\n %scratch = qalloc 5\n\
                 apply @seed(%v) -> %f, %scratch budget 32\n %out = qalloc 2\n x %out[1]\n\
                 apply @fail() -> %out, %f budget 32\n ret\n}}"
            );
            let module = load(&text);
            for low in 0..32 {
                let dirty = low | if width > 5 { 1 << (width - 1) } else { 0 };
                let mut e = executor(
                    &module,
                    "main",
                    &[dirty],
                    Options { reversible: true, allow_region_faults: true, ..bounded() },
                );
                let start = e.snapshot();
                assert_eq!(e.run(), Outcome::Returned(None), "{name} q{width} {dirty} ");
                assert_eq!(e.snapshot().quantum.amplitudes.len(), 1);
                assert!((e.snapshot().quantum.amplitude(&[dirty ^ code, 0, 2]).re - 1.0).abs() < 1e-12);
                e.reverse().expect("faulting regions reverse");
                assert_same_state(&e.snapshot(), &start, "dirty fault register reverses");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 4 * 17 * 32, "four widths, seventeen codes, 32 dirty values");
}

#[test]
fn fault_register_admission_covers_the_entire_register_width_domain() {
    // SYNTAX's apply form says "5 qubits or more", including 64 and wider registers, rather than exactly five.
    for width in (1..=66).chain([128, 129, 1000]) {
        let text = format!(
            "memory 256\nfunc @fail() -> i1 {{\nentry:\n fault bad_longjmp\n}}\n\
             func @main() {{\nentry:\n %o = qalloc 1\n %f = qalloc {width}\n\
             apply @fail() -> %o, %f budget 8\n ret\n}}"
        );
        assert_eq!(bennett_ir::load(&text).is_ok(), width >= 5, "q{width}");
    }
    for code in 0..=63 {
        let expected = CODES.iter().find(|&&(_, value)| value == code).map(|&(name, _)| name);
        assert_eq!(Fault::from_code(code).map(Fault::name), expected, "code {code}");
    }
}

#[test]
fn invalid_jumps_and_indirect_calls_give_distinct_codes_with_coherent_dirty_targets() {
    let text = "memory 1024\nfunc @bad(i1 %choice) -> i2 {\nentry:\n br %choice, jump, indirect\njump:\n longjmp 16, 1\nindirect:\n %r = call_indirect fn() -> i2 999()\n ret %r\n}\nfunc @main() {\nentry:\n %q = qalloc 1\n h %q\n %out = qalloc 2\n x %out[0]\n h %out[1]\n %f = qalloc 64\n h %f[0]\n s %f[0]\n h %f[4]\n tdg %f[4]\n h %f[63]\n apply @bad(%q) -> %out, %f budget 32\n ret\n}";
    let module = load(text);
    let mut e = executor(&module, "main", &[], Options { allow_region_faults: true, reversible: true, ..bounded() });
    let before = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None));
    let quantum = e.snapshot().quantum;
    assert_eq!(quantum.amplitudes.len(), 32);
    for q in 0..2 {
        for out in [1, 3] {
            for low in 0..2 {
                for bit4 in 0..2 {
                    for high in 0..2 {
                        let dirty = low | (bit4 << 4) | (high << 63);
                        let code = if q == 0 { 4 } else { 16 };
                        let phase =
                            (low as f64) * std::f64::consts::FRAC_PI_2 - (bit4 as f64) * std::f64::consts::FRAC_PI_4;
                        let expected = num_complex::Complex64::from_polar(1.0 / 32.0f64.sqrt(), phase);
                        assert!((quantum.amplitude(&[q, out, dirty ^ code]) - expected).norm() < 1e-12);
                    }
                }
            }
        }
    }
    e.reverse().expect("no measurement");
    assert_same_state(&e.snapshot(), &before, "coherent dirty fault targets reverse");
}
