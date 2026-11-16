//! Larger SQL oracles: native truth tables, full snapshots and the existing dense matrices.
use bennett_exec::{Executor, Options, Outcome, QuantumSnapshot};
use bennett_ir::{FixedGate, Gate, Inst, ValidModule};
use nalgebra::DVector;
use num_complex::Complex64 as C;
use std::time::Instant;

// Reuse the independent oracle, not the executor's gate or region implementation.
#[allow(dead_code)]
#[path = "../../../../crates/exec/tests/oracle_support/dense.rs"]
mod dense;
use dense::{Dense, gate_matrix};

fn initial(n: usize, prep: &str) -> Dense {
    let width = n.ilog2();
    let mut state = DVector::zeros(n * 64);
    state[0] = C::new(1.0, 0.0);
    let mut d = Dense { widths: vec![width, 1, 5], state };
    for b in 0..width {
        d.gate(gate_matrix(Gate::Fixed(FixedGate::H)), (0, b as u8), &[]);
    }
    if prep != "zero" {
        d.gate(gate_matrix(Gate::Fixed(FixedGate::X)), (1, 0), &[]);
    }
    if prep == "minus" {
        d.gate(gate_matrix(Gate::Fixed(FixedGate::H)), (1, 0), &[]);
    }
    d
}

fn oracle(d: &mut Dense, bits: &[usize]) {
    let n = bits.len();
    d.permute(|basis| basis ^ (bits[basis % n] * n));
}

fn diffusion(d: &mut Dense) {
    let width = d.widths[0] as u8;
    let h = gate_matrix(Gate::Fixed(FixedGate::H));
    let x = gate_matrix(Gate::Fixed(FixedGate::X));
    for b in 0..width {
        d.gate(h, (0, b), &[]);
        d.gate(x, (0, b), &[]);
    }
    let controls: Vec<_> = (0..width - 1).map(|b| (0, b)).collect();
    d.gate(gate_matrix(Gate::Fixed(FixedGate::Z)), (0, width - 1), &controls);
    for b in (0..width).rev() {
        d.gate(x, (0, b), &[]);
        d.gate(h, (0, b), &[]);
    }
    d.gate(x, (1, 0), &[]);
}

fn compare(actual: &QuantumSnapshot, expected: &Dense) -> f64 {
    assert!(actual.cells.is_empty());
    let columns = actual.amplitudes.first().expect("nonempty state").0.len();
    let mut error: f64 = 0.0;
    for basis in 0..expected.state.len() {
        let config = expected.config(basis);
        let found = if config[columns..].iter().all(|&v| v == 0) {
            actual.amplitude(&config[..columns])
        } else {
            C::new(0.0, 0.0)
        };
        error = error.max((found - expected.state[basis]).norm());
    }
    for (config, _) in &actual.amplitudes {
        assert!(config.iter().zip(&expected.widths).all(|(&v, &w)| v < 1 << w));
    }
    let norm: f64 = actual.amplitudes.iter().map(|(_, a)| a.norm_sqr()).sum();
    assert!((norm - 1.0).abs() < 1e-12, "state norm {norm}");
    assert!(error < 1e-12, "dense amplitude deviation {error:e}");
    error
}

pub fn run(module: &ValidModule, args: &[String]) {
    let mode = &args[0];
    let entry = module.function_named(&args[1]).expect("entry");
    let bits: Vec<usize> = args[2].bytes().map(|b| usize::from(b - b'0')).collect();
    assert!(bits.len().is_power_of_two() && bits.len() >= 4);
    assert!(bits.iter().all(|&b| b <= 1));
    let reversible = mode == "reverse" || mode == "coherent";
    let mut exec = Executor::new(
        module,
        entry,
        &[],
        Options {
            reversible,
            budget: Some(1_000_000_000),
            max_configurations: Some(bits.len() * 2),
            ..Options::default()
        },
    )
    .expect("start");
    let before = exec.snapshot();
    let start = Instant::now();
    if mode == "forward" || mode == "reverse" {
        assert_eq!(exec.run(), Outcome::Returned(Some(0)));
        assert_eq!(exec.tape(), args[2].as_bytes(), "native truth table");
    } else {
        let setup = module.function_named("experiment_setup").expect("setup");
        let mut estimate = Executor::new(module, setup, &[], Options::default()).expect("setup start");
        assert_eq!(estimate.run(), Outcome::Returned(Some(0)));
        for _ in 0..estimate.stats().steps + 1 {
            assert!(exec.step().is_none(), "setup stopped");
        }
        let fixed = exec.snapshot();
        let prep = &args[3];
        let mut expected = initial(bits.len(), prep);
        let mut calls = 0;
        let mut deviation: f64 = 0.0;
        loop {
            let old = exec.snapshot();
            let frame = old.classical.frames.last().expect("entry frame");
            assert_eq!(frame.func, entry);
            let block = module.function(entry).block(frame.block);
            match block.insts.get(frame.inst) {
                Some(Inst::Apply { .. }) => {
                    let stats = exec.stats();
                    assert!(exec.step().is_none(), "SQL apply stopped");
                    let new = exec.snapshot();
                    assert_eq!(new.classical.memory, old.classical.memory);
                    assert_eq!(new.classical.stack_pointer, old.classical.stack_pointer);
                    assert_eq!(new.classical.slots, old.classical.slots);
                    assert_eq!(new.classical.tape, old.classical.tape);
                    assert_eq!(new.quantum.registers, old.quantum.registers);
                    assert!(new.worlds.is_empty());
                    oracle(&mut expected, &bits);
                    deviation = deviation.max(compare(&new.quantum, &expected));
                    let now = exec.stats();
                    assert_eq!(now.region_faults, 0);
                    // Reversible outer execution retains one Apply tag so reversing
                    // can replay the oracle; the callee history itself is gone.
                    assert_eq!(now.journal_bytes, stats.journal_bytes.map(|bytes| bytes + 1));
                    assert_eq!(now.overlay_bytes, 0);
                    assert_eq!(now.worlds, 1);
                    assert_eq!(now.live_qubits, stats.live_qubits);
                    calls += 1;
                    println!(
                        "oracle {calls} steps {} work {} journal {} overlay {} runs {}",
                        now.region_steps - stats.region_steps,
                        now.region_cost.work - stats.region_cost.work,
                        now.region_cost.journal_bytes - stats.region_cost.journal_bytes,
                        now.region_cost.overlay_bytes - stats.region_cost.overlay_bytes,
                        now.region_cost.runs - stats.region_cost.runs
                    );
                    if mode == "grover" {
                        diffusion(&mut expected);
                    }
                }
                Some(Inst::Measure { .. }) => {
                    expected.gate(gate_matrix(Gate::Fixed(FixedGate::H)), (1, 0), &[]);
                    expected.gate(gate_matrix(Gate::Fixed(FixedGate::X)), (1, 0), &[]);
                    deviation = deviation.max(compare(&old.quantum, &expected));
                    assert_eq!(old.classical.memory, fixed.classical.memory);
                    assert_eq!(old.classical.stack_pointer, fixed.classical.stack_pointer);
                    assert_eq!(old.classical.tape, fixed.classical.tape);
                    assert_eq!(exec.stats().live_qubits, bits.len().ilog2());
                    assert!(old.worlds.is_empty());
                    let success: f64 = old
                        .quantum
                        .amplitudes
                        .iter()
                        .filter(|(c, _)| bits[c[0] as usize] == 1)
                        .map(|(_, a)| a.norm_sqr())
                        .sum();
                    let marked = bits.iter().sum::<usize>();
                    let theory = if marked == 0 {
                        0.0
                    } else {
                        let theta = ((marked as f64 / bits.len() as f64).sqrt()).asin();
                        (((2 * calls + 1) as f64) * theta).sin().powi(2)
                    };
                    assert!((success - theory).abs() < 1e-12);
                    println!("success {success:.17e} theory {theory:.17e}");
                    break;
                }
                None => {
                    deviation = deviation.max(compare(&old.quantum, &expected));
                    break;
                }
                _ => assert!(exec.step().is_none(), "stopped before observation"),
            }
        }
        assert!(calls > 0);
        let result = exec.run();
        let Outcome::Returned(Some(sampled)) = result else { panic!("{result:?}") };
        if mode == "grover" {
            assert!(sampled < bits.len() as u128);
            assert!(expected.state[sampled as usize].norm() > 1e-12, "sampled zero support");
            println!("sampled_row {sampled}");
        }
        println!("dense_deviation {deviation:e}");
    }
    println!("forward_seconds {:.6}", start.elapsed().as_secs_f64());
    println!("stats {:?}", exec.stats());
    if reversible {
        let start = Instant::now();
        exec.reverse().expect("reverse");
        let restored = exec.snapshot();
        assert_eq!(restored.classical, before.classical);
        assert_eq!(restored.worlds, before.worlds);
        assert!(restored.quantum.distance(&before.quantum).expect("register table") < 1e-12);
        assert_eq!(exec.stats().journal_bytes, Some(0));
        println!("reverse_seconds {:.6}", start.elapsed().as_secs_f64());
        println!("reverse identical");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_grover_keeps_phase_for_none_single_half_and_all_marked() {
        for n in [4usize, 8, 16, 32, 256] {
            for m in [0, 1, n / 2, n] {
                let bits: Vec<_> = (0..n).map(|r| usize::from(r < m)).collect();
                let mut d = initial(n, "minus");
                for k in 1..=3 {
                    oracle(&mut d, &bits);
                    diffusion(&mut d);
                    let theta = ((m as f64 / n as f64).sqrt()).asin();
                    for row in 0..n {
                        let a = if row < m {
                            (((2 * k + 1) as f64) * theta).sin() / (m as f64).sqrt()
                        } else {
                            (((2 * k + 1) as f64) * theta).cos() / ((n - m) as f64).sqrt()
                        } / 2.0f64.sqrt();
                        assert!((d.state[row].re - a).abs() < 1e-12);
                        assert!((d.state[row + n].re + a).abs() < 1e-12);
                    }
                    assert!((d.state.norm_squared() - 1.0).abs() < 1e-12);
                }
            }
        }
    }
}
