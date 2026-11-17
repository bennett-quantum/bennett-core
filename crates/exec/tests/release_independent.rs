//! Independent partial-trace expectations, including the off-diagonal entries of reduced states.

mod common;

use bennett_exec::{Options, Outcome, QuantumSnapshot};
use bennett_ir::Fault;
use common::{assert_same_state, bounded, executor, load};
use num_complex::Complex64 as C;

/// Trace out every column except `keep`. Only equal discarded values contribute to a matrix entry.
fn reduced(state: &QuantumSnapshot, keep: &[usize]) -> Vec<Vec<C>> {
    let dimension = 1 << keep.len();
    let mut rho = vec![vec![C::default(); dimension]; dimension];
    let index =
        |config: &[u64]| keep.iter().enumerate().map(|(bit, &column)| (config[column] as usize) << bit).sum::<usize>();
    for (a, alpha) in &state.amplitudes {
        for (b, beta) in &state.amplitudes {
            if (0..a.len()).all(|column| keep.contains(&column) || a[column] == b[column]) {
                rho[index(a)][index(b)] += alpha * beta.conj();
            }
        }
    }
    rho
}

fn assert_density(actual: &[Vec<C>], expected: &[Vec<C>]) {
    assert_eq!(actual.len(), expected.len());
    for (row, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for (column, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!((actual - expected).norm() < 1e-12, "rho[{row},{column}]: {actual} != {expected}");
        }
    }
}

fn reversible() -> Options {
    Options { reversible: true, ..bounded() }
}

#[test]
fn bell_and_ghz_releases_trace_distinct_registers_in_every_order() {
    // Releasing any nonempty proper subset of (|000> + i|111>)/sqrt(2) leaves a diagonal mixture.
    // The two-register case is the Bell pair. First, interior and final columns all occur here.
    for registers in [2, 3] {
        let allocations: String = (0..registers).map(|i| format!("%q{i} = qalloc 1\n")).collect();
        let entangle: String = (1..registers).map(|i| format!("x %q{i} ctrl %q0\n")).collect();
        for mask in 1..(1 << registers) - 1 {
            for reverse_order in [false, true] {
                let mut dropped: Vec<_> = (0..registers).filter(|i| mask & (1 << i) != 0).collect();
                if reverse_order {
                    dropped.reverse();
                }
                let release: String = dropped.iter().map(|i| format!("qfree %q{i}\n")).collect();
                let keep: Vec<_> = (0..registers).filter(|i| mask & (1 << i) == 0).collect();
                let module = load(&format!(
                    "memory 512\nfunc @main() {{\nentry:\n{allocations}h %q0\ns %q0\n{entangle}{release}ret\n}}"
                ));
                let mut e = executor(&module, "main", &[], reversible());
                let entry = e.snapshot();
                assert_eq!(e.run(), Outcome::Returned(None));
                let mut expected = vec![vec![C::default(); 1 << keep.len()]; 1 << keep.len()];
                expected[0][0] = C::new(0.5, 0.0);
                let last = expected.len() - 1;
                expected[last][last] = C::new(0.5, 0.0);
                assert_density(&reduced(&e.snapshot().quantum, &keep), &expected);
                assert_eq!(e.stats().nonzero_releases, dropped.len() as u64);
                assert_eq!(e.stats().leftover_qubits, dropped.len() as u32);
                assert!((e.stats().nonzero_release_weight - dropped.len() as f64 / 2.0).abs() < 1e-12);
                e.reverse().expect("reverse release");
                assert_same_state(&e.snapshot(), &entry, "Bell/GHZ entry");
            }
        }
    }
}

#[test]
fn partial_entanglement_preserves_complex_coherence_and_tiny_release_weights() {
    for theta in [0.0, 1.3, std::f64::consts::PI, 2e-9, 2e-15] {
        let module = load(&format!(
            "memory 256\nfunc @main() {{\nentry:\n%a = qalloc 1\n%e = qalloc 1\nh %a\ns %a\nry({theta}) %e ctrl %a\nqfree %e\nret\n}}"
        ));
        let mut e = executor(&module, "main", &[], reversible());
        assert_eq!(e.run(), Outcome::Returned(None));
        let coherence = (theta / 2.0).cos() / 2.0;
        let expected =
            vec![vec![C::new(0.5, 0.0), C::new(0.0, -coherence)], vec![C::new(0.0, coherence), C::new(0.5, 0.0)]];
        assert_density(&reduced(&e.snapshot().quantum, &[0]), &expected);
        let weight = (theta / 2.0).sin().powi(2) / 2.0;
        assert_eq!(e.stats().nonzero_releases, u64::from(theta != 0.0));
        if weight == 0.0 {
            assert_eq!(e.stats().nonzero_release_weight, 0.0);
        } else {
            assert!((e.stats().nonzero_release_weight / weight - 1.0).abs() < 1e-12, "theta {theta}");
        }
    }
}

#[test]
fn releases_in_a_loop_never_reuse_retained_columns_and_reverse_each_predecessor() {
    for count in [1, 2, 7] {
        let module = load(&format!(
            "memory 1024
func @main() {{
entry:
 %a = qalloc 1
 h %a
 br loop(i8 0)
loop(i8 %i):
 %e = qalloc 2
 %clean = qalloc 3
 ry(0.7) %e[1] ctrl %a
 qfree %e
 qfree %clean
 %next = add i8 %i, 1
 %more = icmp ult i8 %next, {count}
 br %more, loop(%next), done
done:
 ret
}}"
        ));
        let mut e = executor(&module, "main", &[], reversible());
        let mut predecessors = Vec::new();
        loop {
            predecessors.push(e.snapshot());
            if let Some(outcome) = e.step() {
                assert_eq!(outcome, Outcome::Returned(None));
                break;
            }
        }
        let coherence = 0.35f64.cos().powi(count) / 2.0;
        assert_density(
            &reduced(&e.snapshot().quantum, &[0]),
            &[vec![C::new(0.5, 0.0), C::new(coherence, 0.0)], vec![C::new(coherence, 0.0), C::new(0.5, 0.0)]],
        );
        assert_eq!(e.stats().leftover_qubits, 2 * count as u32);
        assert_eq!(e.stats().nonzero_releases, count as u64);
        let weight = f64::from(count) * 0.35f64.sin().powi(2) / 2.0;
        assert!((e.stats().nonzero_release_weight - weight).abs() < 1e-12);
        for snapshot in predecessors.iter().rev() {
            assert!(e.step_back().expect("reverse step"));
            assert_same_state(&e.snapshot(), snapshot, "loop predecessor");
        }
        assert_eq!(e.stats().leftover_qubits, 0);
        assert_eq!(e.run(), Outcome::Returned(None));
        // The report counts forward executions, including replay, without affecting the state.
        assert_eq!(e.stats().nonzero_releases, 2 * count as u64);
    }
}

#[test]
fn tracing_an_environment_commutes_with_apply_on_its_entangled_target() {
    for release_before in [false, true] {
        let (before, after) = if release_before { ("qfree %env", "") } else { ("", "qfree %env") };
        let module = load(&format!(
            "memory 512
func @identity(i1 %x) -> i1 {{
entry:
 ret %x
}}
func @main() {{
entry:
 %arg = qalloc 1
 %out = qalloc 1
 %env = qalloc 1
 %fault = qalloc 5
 h %arg
 h %out
 s %out
 x %env ctrl %out
 {before}
 apply @identity(%arg) -> %out, %fault budget 1
 {after}
 ret
}}"
        ));
        let mut expected = vec![vec![C::default(); 4]; 4];
        // Four input terms: amplitude i^t / 2 at (arg=x, out=t XOR x, env=t).
        for t in 0..2 {
            for x in 0..2 {
                for y in 0..2 {
                    expected[x | ((t ^ x) << 1)][y | ((t ^ y) << 1)] += C::new(0.25, 0.0);
                }
            }
        }
        let mut e = executor(&module, "main", &[], reversible());
        let entry = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_density(&reduced(&e.snapshot().quantum, &[0, 1]), &expected);
        assert!(e.snapshot().quantum.amplitudes.iter().all(|(config, _)| config[3] == 0));
        e.reverse().expect("apply and release reverse");
        assert_same_state(&e.snapshot(), &entry, "apply-target entry");
    }
}

#[test]
fn released_handles_stay_dead_after_fresh_allocations_for_every_kind_of_use() {
    for use_stale in ["x %old", "%m = measure %old", "qfree %old", "apply @identity(%old) -> %fresh, %fault budget 1"] {
        for preparation in ["", "h %old"] {
            let module = load(&format!(
                "memory 512
func @identity(i1 %x) -> i1 {{
entry:
 ret %x
}}
func @main() {{
entry:
 %old = qalloc 1
 {preparation}
 qfree %old
 %fresh = qalloc 1
 %fault = qalloc 5
 {use_stale}
 ret
}}"
            ));
            let mut e = executor(&module, "main", &[], reversible());
            assert_eq!(e.run(), Outcome::Faulted(Fault::DanglingRegister), "{preparation}, {use_stale}");
        }
    }
}

#[test]
fn release_reports_do_not_change_program_measurements_or_console() {
    // These environments are uncorrelated with the live register. Their different reports are unobservable.
    for seed in 0..32 {
        let mut outputs = Vec::new();
        for prepare in ["", "x %env", "h %env", "ry(2e-9) %env"] {
            let module = load(&format!(
                "memory 256\nfunc @main() -> i1 {{\nentry:\n%q = qalloc 1\n%env = qalloc 1\n{prepare}\nqfree %env\nh %q\n%m = measure %q\n%b = zext i1 %m to i8\neffect %b\nret %m\n}}"
            ));
            let mut e = executor(&module, "main", &[], Options { seed, ..bounded() });
            let outcome = e.run();
            assert!(matches!(outcome, Outcome::Returned(Some(0 | 1))));
            outputs.push((outcome, e.tape().to_vec()));
        }
        assert!(outputs.iter().all(|output| output == &outputs[0]), "seed {seed}: {outputs:?}");
    }
}

#[test]
fn promoting_memory_after_a_release_cannot_reuse_its_unnamed_column() {
    let module = load(
        "memory 512
data 64 = i8 5
func @main() {
entry:
 %a = qalloc 1
 %env = qalloc 1
 h %a
 x %env ctrl %a
 qfree %env
 %fresh = qalloc 1
 x %fresh ctrl %a
 x [64:0]
 ret
}",
    );
    let mut e = executor(&module, "main", &[], reversible());
    let entry = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None));
    let snapshot = e.snapshot();
    assert_eq!(snapshot.quantum.cells, [(64, 3)]);
    assert_eq!(snapshot.quantum.amplitudes.len(), 2);
    for (config, amplitude) in &snapshot.quantum.amplitudes {
        assert_eq!(config[1], config[0], "released environment remains correlated");
        assert_eq!(config[2], config[0], "fresh register is independently controlled");
        assert_eq!(config[3], 4, "the cell starts at the byte's value, independent of released qubits");
        assert!((amplitude.re - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    }
    assert_density(
        &reduced(&snapshot.quantum, &[0, 2]),
        &[
            vec![C::new(0.5, 0.0), C::default(), C::default(), C::default()],
            vec![C::default(); 4],
            vec![C::default(); 4],
            vec![C::default(), C::default(), C::default(), C::new(0.5, 0.0)],
        ],
    );
    e.reverse().expect("reverse promotion after release");
    assert_same_state(&e.snapshot(), &entry, "promotion entry");
}
