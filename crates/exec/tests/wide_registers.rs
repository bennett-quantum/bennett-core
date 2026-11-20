//! Registers wider than 64 qubits (`docs/DESIGN.md`, "The device program"): the simulator stores one in
//! several columns of at most 64 qubits, which must not show in what a program finds. Checked at the widths
//! 64, 65, 128 and 1000: values set by gates across the column boundary, correlations between qubits in
//! different columns, `apply` with arguments and results of 65 and 128 qubits, releases of whole registers
//! (kept or removed whole) and forward-then-reverse back to the entry state.

mod common;

use bennett_exec::Outcome;
use common::{assert_reverses, bounded, run};

/// The widths of the brief: the last that fits one column, the first that needs two, the widest an integer
/// can read whole, and one well above.
const WIDTHS: [u32; 4] = [64, 65, 128, 1000];

/// Sets qubits 0, 63 and the top one (the same as 63 at width 64) with `x` and measures the register whole:
/// the integer with those bits.
#[test]
fn bits_set_on_both_sides_of_the_column_boundary_are_measured_in_place() {
    for width in WIDTHS.into_iter().filter(|&width| width <= 128) {
        let bits = std::collections::BTreeSet::from([0, 63, width - 1]);
        let gates: String = bits.iter().map(|bit| format!("    x %q[{bit}]\n")).collect();
        let module = common::load(&format!(
            "memory 64
func @main() -> i{width} {{
entry:
    %q = qalloc {width}
{gates}    %m = measure %q
    ret %m
}}"
        ));
        let expected = bits.iter().fold(0u128, |value, bit| value | 1 << bit);
        let (outcome, executor) = run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(expected)), "q{width}");
        assert_eq!(executor.stats().live_qubits, width, "q{width}");
    }
}

/// A Bell pair between qubit 0 and the top qubit, in different columns from 65 qubits on: every seed measures
/// both 0 or both 1, and over 16 seeds both outcomes occur.
#[test]
fn a_correlation_across_columns_survives_measurement() {
    for width in WIDTHS.into_iter().filter(|&width| width <= 128) {
        let top = width - 1;
        let module = common::load(&format!(
            "memory 64
func @main() -> i{width} {{
entry:
    %q = qalloc {width}
    h %q[0]
    x %q[{top}] ctrl %q[0]
    %m = measure %q
    ret %m
}}"
        ));
        let both = 1u128 | 1 << top;
        let mut seen = [false; 2];
        for seed in 0..16 {
            let (outcome, _) = run(&module, "main", &[], bennett_exec::Options { seed, ..bounded() });
            match outcome {
                Outcome::Returned(Some(0)) => seen[0] = true,
                Outcome::Returned(Some(value)) if value == both => seen[1] = true,
                other => panic!("q{width} seed {seed}: {other:?}"),
            }
        }
        assert_eq!(seen, [true, true], "q{width}");
    }
}

/// A register of 1000 qubits cannot be measured whole into an integer; its top 100 qubits are copied into
/// a fresh register with controlled X and that is measured (the route the validator names). Qubit 999 is
/// set; qubit 950 is correlated with qubit 700, which is in superposition.
#[test]
fn qubits_of_a_thousand_qubit_register_are_measured_through_a_copy() {
    let copy: String = (0..100).map(|bit| format!("    x %c[{bit}] ctrl %q[{}]\n", 900 + bit)).collect();
    let module = common::load(&format!(
        "memory 64
func @main() -> i100 {{
entry:
    %q = qalloc 1000
    x %q[999]
    h %q[700]
    x %q[950] ctrl %q[700]
    %c = qalloc 100
{copy}    %m = measure %c
    ret %m
}}"
    ));
    let mut seen = [false; 2];
    for seed in 0..16 {
        let (outcome, executor) = run(&module, "main", &[], bennett_exec::Options { seed, ..bounded() });
        match outcome {
            Outcome::Returned(Some(value)) if value == 1 << 99 => seen[0] = true,
            Outcome::Returned(Some(value)) if value == 1 << 99 | 1 << 50 => seen[1] = true,
            other => panic!("seed {seed}: {other:?}"),
        }
        let stats = executor.stats();
        assert_eq!((stats.live_qubits, stats.peak_live_qubits), (1100, 1100));
    }
    assert_eq!(seen, [true, true]);
}

/// At each width: a register entangled across its columns and then released is kept whole (all its
/// qubits left over, even those of columns that were zero); a register released while zero is removed
/// whole, and its ids are reused by the next allocation. Running forward and reversing every step returns
/// to the entry state exactly.
#[test]
fn releases_keep_or_remove_a_wide_register_whole_and_reverse_exactly() {
    for width in WIDTHS {
        let (top, middle) = (width - 1, width / 2);
        let module = common::load(&format!(
            "memory 64
func @main() -> i8 {{
entry:
    %a = qalloc {width}
    h %a[0]
    x %a[{top}] ctrl %a[0]
    ry(0.3) %a[{middle}]
    %clean = qalloc {width}
    x %clean[{top}] ctrl %a[{middle}]
    x %clean[{top}] ctrl %a[{middle}]
    qfree %clean
    %b = qalloc {width}
    h %b[0]
    qfree %a
    qfree %b
    %c = qalloc 65
    x %c[64]
    ret 3
}}"
        ));
        let (outcome, executor) = run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(3)), "q{width}");
        let stats = executor.stats();
        // `%b` is dirty only in its head column and is still kept whole.
        assert_eq!(stats.leftover_qubits, 2 * width, "q{width}");
        assert_eq!(stats.nonzero_releases, 2, "q{width}");
        // `%a` is zero only where qubit 0 is 0 (1/2) and the rotated qubit is 0 (cos^2 0.15); `%b` is
        // nonzero with weight 1/2.
        let weight = 1.0 - 0.5 * 0.15f64.cos().powi(2) + 0.5;
        assert!((stats.nonzero_release_weight - weight).abs() < 1e-12, "q{width}: {}", stats.nonzero_release_weight);
        // Qubits left over still count as live: `%a` and `%b` kept, then `%c`.
        assert_eq!(stats.peak_live_qubits, 2 * width + 65, "q{width}");
        assert_eq!(assert_reverses(&module, "main", &[], bounded()), Outcome::Returned(Some(3)), "q{width}");
    }
}

/// `apply` reads an argument register of 65 or 128 qubits as one integer and XORs a result of that width
/// into the output register, for each definite value: `x + 1`, with the low 64 qubits all 1 so that the
/// carry crosses the column boundary, and the top qubit in superposition. The outcome is checked against the
/// measured argument for 8 seeds, and the run without measurements reverses exactly.
#[test]
fn apply_reads_and_writes_registers_wider_than_one_column() {
    for width in [65, 128] {
        let top = width - 1;
        let ones: String = (0..64).map(|bit| format!("    x %x[{bit}]\n")).collect();
        let prepare = format!(
            "    %x = qalloc {width}
{ones}    h %x[{top}]
    h %x[3]
    %o = qalloc {width}
    %f = qalloc 70
    apply @inc(%x) -> %o, %f budget 10
"
        );
        let inc = format!(
            "func @inc(i{width} %x) -> i{width} {{
entry:
    %y = add i{width} %x, 1
    ret %y
}}"
        );
        let measured = common::load(&format!(
            "memory 256
{inc}
func @main() -> i1 {{
entry:
{prepare}    %mo = measure %o
    %mx = measure %x
    %mf = measure %f
    %next = add i{width} %mx, 1
    %same = icmp eq i{width} %mo, %next
    %clear = icmp eq i70 %mf, 0
    %ok = and i1 %same, %clear
    ret %ok
}}"
        ));
        for seed in 0..8 {
            let (outcome, _) = run(&measured, "main", &[], bennett_exec::Options { seed, ..bounded() });
            assert_eq!(outcome, Outcome::Returned(Some(1)), "q{width} seed {seed}");
        }
        let unmeasured = common::load(&format!(
            "memory 256
{inc}
func @main() {{
entry:
{prepare}    ret
}}"
        ));
        assert_eq!(assert_reverses(&unmeasured, "main", &[], bounded()), Outcome::Returned(None), "q{width}");
    }
}
