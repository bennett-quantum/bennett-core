//! Independent replay-test families for DESIGN.md, History and Worlds. Keeps XOR per target;
//! three unequal keeps can cancel, and cancellation on a read input permits a simultaneous nonzero
//! keep into a different target. Whole-world and dense oracles calculate the successful results.
#[allow(dead_code, reason = "shared independent oracle helpers")]
mod oracle_support;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{FixedGate, Gate, Rotation, ValidModule};
use nalgebra::DVector;
use num_complex::Complex64 as C;
use oracle_support::dense::{Dense, gate_matrix};
use oracle_support::worlds::{Model, basis};
use oracle_support::worlds_executor::assert_worlds;

fn run(module: &ValidModule) -> bennett_exec::Snapshot {
    let mut e = Executor::new(
        module,
        module.function_named("main").expect("main"),
        &[],
        Options { reversible: true, budget: Some(1000), max_configurations: Some(4096), ..Options::default() },
    ).expect("start");
    let before = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None));
    let after = e.snapshot();
    e.reverse().expect("all generated successful programs reverse");
    assert_eq!(e.snapshot().classical, before.classical);
    assert!(e.snapshot().quantum.distance(&before.quantum).is_some_and(|d| d < 1e-11));
    assert!(e.snapshot().worlds.is_empty());
    after
}

fn module(text: &str) -> ValidModule {
    bennett_ir::load(text).unwrap_or_else(|err| panic!("{err}\n{text}"))
}

/// Three or four keeps, every assignment of each to q or out, and all two-bit values.
/// Only input-cancelling cases are used here: refusals are covered with private-state comparisons.
#[test]
fn generated_net_keeps_match_the_whole_world_reference() {
    let mut count = 0;
    for n in [3, 4] {
        for targets in 0..(1 << n) {
            // Exhaust the first three values; derive the fourth independently of the net on q.
            for values in 0..64 {
                let values: Vec<usize> = (0..n).map(|i| (values >> (2 * (i % 3))) & 3).collect();
                let mut net = [0; 2];
                let keeps: Vec<_> = values.iter().enumerate().map(|(i, value)| {
                    let target = (targets >> i) & 1;
                    net[target] ^= value;
                    format!("i2 {value} -> {}", ["%q", "%out"][target])
                }).collect();
                if net[0] != 0 {
                    continue;
                }
                let text = format!(
                    "memory 128\nfunc @id(i2 %v) -> i2 {{\nentry:\nret %v\n}}\n\
                     func @main() {{\nentry:\n%q = qalloc 2\n%out = qalloc 2\n\
                     ry(0.71) %q[0]\nh %q[1]\ns %q[1]\nry(0.43) %out[0]\n\
                     %outer = mark\n%inner = mark\n%v = call @id(%q)\nrewind %inner\n\
                     %w = call @id(%q)\nrewind %outer keep ({})\nret\n}}\n", keeps.join(", ")
                );
                let p = module(&text);
                let m = Model::new(&p, "main");
                let expected = m.run_joined(basis(m.initial("main", &[])), 200);
                let actual = run(&p);
                assert_worlds(&m, &expected, &actual);
                assert_eq!(actual.classical.memory, vec![0; 128]);
                assert!(actual.worlds.is_empty());
                count += 1;
            }
        }
    }
    println!("{count} three/four-keep programs: full-world reference and reversal agree");
}

/// The same computed, nonzero net keep with two cancelling input keeps and two cancelling output
/// keeps, for every register/memory placement. Multi-byte keeps overlap at byte 17: their high bytes
/// cancel there while their low bytes change byte 16. This is cancellation per column, not per operand.
#[test]
fn generated_overlapping_memory_keeps_match_a_dense_calculation() {
    let mut count = 0;
    for layout in 0..4 {
        for a in 0..4 {
            for b in 0..4 {
                for nested in [false, true] {
                    let q_mem = layout & 1 != 0;
                    let out_mem = layout & 2 != 0;
                    let q = |bit| if q_mem { format!("[8:{bit}]") } else { format!("%q[{bit}]") };
                    let out = |bit| if out_mem { format!("[16:{bit}]") } else { format!("%out[{bit}]") };
                    let q_target = if q_mem { "[8]" } else { "%q" };
                    let out_target = if out_mem { "[16]" } else { "%out" };
                    let read = if q_mem {
                        "%byte = load i8 8\n%v = trunc i8 %byte to i2"
                    } else { "%v = call @id(%q)" };
                    let extra = if out_mem {
                        // Net low byte a; high byte b^b=0. Byte 17 is read since the mark.
                        format!(", i16 {} -> [16], i8 {b} -> [17]", a | (b << 8))
                    } else { format!(", i2 {a} -> %out") };
                    let inner = if nested {
                        format!("%inner = mark\n{read}\nrewind %inner\n")
                    } else { String::new() };
                    let text = format!(
                        "memory 128\nfunc @id(i2 %v) -> i2 {{\nentry:\nret %v\n}}\n\
                         func @main() {{\nentry:\n%q = qalloc 2\n%out = qalloc 2\n\
                         z [8]\nz [16]\nz [17]\nry(0.71) {q0}\nh {q1}\ns {q1}\nry(0.43) {o0}\n\
                         %outer = mark\n{inner}{read}\n%guard = load i8 17\n%w = xor i2 %v, 3\n\
                         ry(0.37) {o1}\nrewind %outer keep (i2 1 -> {q_target}, i2 2 -> {q_target}, i2 3 -> {q_target}, \
                         %w -> {out_target}, i2 {b} -> {out_target}, i2 {b} -> {out_target}{extra})\nret\n}}\n",
                        q0=q(0), q1=q(1), o0=out(0), o1=out(1)
                    );
                    let p = module(&text);
                    let actual = run(&p);
                    let mut prepared = Dense { widths: vec![2,2], state: DVector::zeros(16) };
                    prepared.state[0] = C::new(1.0, 0.0);
                    prepared.gate(gate_matrix(Gate::Rotate(Rotation::Ry, 0.71)), (0,0), &[]);
                    prepared.gate(gate_matrix(Gate::Fixed(FixedGate::H)), (0,1), &[]);
                    prepared.gate(gate_matrix(Gate::Fixed(FixedGate::S)), (0,1), &[]);
                    prepared.gate(gate_matrix(Gate::Rotate(Rotation::Ry, 0.43)), (1,0), &[]);
                    let mut expected = DVector::<C>::zeros(16);
                    for value in 0..4 {
                        let mut part = prepared.clone();
                        for i in 0..16 {
                            if i & 3 != value { part.state[i] = C::default(); }
                        }
                        part.gate(gate_matrix(Gate::Rotate(Rotation::Ry, 0.37)), (1,1), &[]);
                        part.permute(|i| i ^ (((value ^ 3) ^ a) << 2));
                        part.gate(gate_matrix(Gate::Rotate(Rotation::Ry, -0.37)), (1,1), &[]);
                        expected += part.state;
                    }
                    let column = |address| actual.quantum.cells.iter().find(|&&(a,_)| a == address).expect("cell").1;
                    let qc = if q_mem { column(8) } else { 0 };
                    let oc = if out_mem { column(16) } else { 1 };
                    let guard = column(17);
                    for i in 0..16 {
                        let amplitude: C = actual.quantum.amplitudes.iter().filter(|(c,_)| c[qc] == (i&3) as u64 && c[oc] == (i>>2) as u64).map(|(_,a)| *a).sum();
                        assert!((amplitude - expected[i]).norm() < 1e-11, "layout {layout}, a {a}, b {b}, nested {nested}, basis {i}");
                    }
                    assert!(actual.quantum.amplitudes.iter().all(|(c,_)| c[guard] == 0));
                    assert!(actual.worlds.is_empty());
                    count += 1;
                }
            }
        }
    }
    println!("{count} mixed register/memory overlapping-keep programs: dense amplitudes and reversal agree");
}
