//! Run period finding with a modular exponentiation compiled from C.

#[path = "shor_support/baseline.rs"]
mod baseline;
#[allow(dead_code)] // The shared runner also provides sampling and destination exhaustion.
#[path = "../../../crates/emit/tests/support/bits.rs"]
mod bits;
#[path = "shor_support/count.rs"]
mod count;

use std::f64::consts::TAU;

use bennett_emit::{ApplyInput, Argument, CompiledApply, Construction, compile_apply};
use bennett_exec::{BlockOp, Executor, GateBlock};
use bennett_front_tests::{CLANG_FLAGS, STACK_BYTES, c_dir, entry, lower_text_with_budget};
use bennett_ir::{
    Fault, FixedGate, FuncId, Gate, Inst, MeasureSource, Operand, Qubit, Rotation, Slot, Type, ValidModule, load,
    validate,
};
use num_complex::Complex64;

const CASES: [(u64, u64); 2] = [(15, 7), (21, 2)];
const EXPONENT_BITS: u64 = 8;
const RESULT_BITS: u64 = 8;
const FAULT_START: u64 = EXPONENT_BITS + RESULT_BITS;
const INTERFACE: u64 = FAULT_START + Fault::CODE_WIDTH.bits() as u64;
const SAMPLES: usize = 1 << EXPONENT_BITS;
const TOLERANCE: f64 = 2e-14;
// On an eight-bit input the longest visit to powmod has eight passes, each
// taking at most eleven steps, plus the entry test, call, casts and returns.
const BUDGET: u64 = 96;

fn front_powmod(modulus: u64, base: u64, name: &str) -> (ValidModule, FuncId) {
    let base_flag = format!("-DBASE={base}");
    let modulus_flag = format!("-DMODULUS={modulus}");
    let mut flags = CLANG_FLAGS.to_vec();
    flags.extend([base_flag.as_str(), modulus_flag.as_str()]);
    let text = lower_text_with_budget(
        &c_dir().join("shor_powmod.c"),
        &format!("{name}_{modulus}_{base}"),
        &flags,
        &[],
        BUDGET,
    )
    .expect("lower C powmod");
    let module = load(&text).expect("valid front output");
    let main = entry(&module, "entry");
    let mut applies = module.function(main).blocks.iter().flat_map(|block| &block.insts).filter_map(|inst| {
        if let Inst::Apply { callee, args, .. } = inst
            && args
                .iter()
                .any(|arg| matches!(arg, Operand::Slot(slot) if matches!(module.slot_type(main, *slot), Type::Qreg(_))))
        {
            return Some(*callee);
        }
        None
    });
    let callee = applies.next().expect("one exponentiation consumes the entire exponent register");
    assert!(applies.next().is_none());
    (module, callee)
}

fn compile_oracle(module: &ValidModule, callee: FuncId) -> CompiledApply {
    require_oracle(ApplyInput {
        module,
        callee,
        args: &[Argument::Register(0..EXPONENT_BITS)],
        out: EXPONENT_BITS..FAULT_START,
        fault: FAULT_START..INTERFACE,
        budget: BUDGET,
        entry_fault: None,
        stack_available: STACK_BYTES,
        memory: None,
        scratch_start: INTERFACE,
    })
    .expect("compile the modular exponentiation")
}

fn require_oracle(input: ApplyInput<'_>) -> Result<CompiledApply, bennett_emit::ApplyError> {
    let compiled = compile_apply(input)?;
    assert_eq!(
        compiled.construction,
        Construction::ModularInPlace { passes: EXPONENT_BITS as usize },
        "{}: {}",
        compiled.construction,
        compiled.reason
    );
    Ok(compiled)
}

#[test]
fn c_powmod_emits_without_instruction_dispatch() {
    for (modulus, base) in CASES {
        let (module, callee) = front_powmod(modulus, base, "powmod_control");
        let oracle = compile_oracle(&module, callee);
        check_oracle(&oracle.block, &modular_powers(base, modulus));
    }
}

fn fourier(exponent: Slot) -> Vec<Inst> {
    let qubit = |bit| Qubit { register: exponent, bit };
    let mut gates = Vec::new();
    for target in (0..EXPONENT_BITS as u32).rev() {
        gates.push(Inst::Gate { gate: Gate::Fixed(FixedGate::H), target: qubit(target), controls: Vec::new() });
        for control in (0..target).rev() {
            let angle = TAU / f64::from(1u32 << (target - control + 1));
            // Rz on the control compensates the phase of controlled Rz.
            // Together they implement controlled phase up to a common phase.
            gates.push(Inst::Gate {
                gate: Gate::Rotate(Rotation::Rz, angle / 2.0),
                target: qubit(control),
                controls: Vec::new(),
            });
            gates.push(Inst::Gate {
                gate: Gate::Rotate(Rotation::Rz, angle),
                target: qubit(target),
                controls: vec![qubit(control)],
            });
        }
    }
    gates
}

fn modular_powers(base: u64, modulus: u64) -> Vec<u64> {
    // Compute successive residues independently of the C square-and-multiply loop.
    let mut next = 1;
    (0..SAMPLES)
        .map(|_| {
            let residue = next;
            next = (next * base) % modulus;
            residue
        })
        .collect()
}

fn exact_probabilities(residues: &[u64], modulus: u64) -> Vec<f64> {
    // Orthogonal residue values contribute incoherently to the exponent's
    // marginal. Add Fourier amplitudes only for equal residues.
    (0..SAMPLES)
        .map(|frequency| {
            let mut groups = vec![Complex64::default(); modulus as usize];
            for (exponent, &residue) in residues.iter().enumerate() {
                let phase = TAU * ((frequency * exponent) % SAMPLES) as f64 / SAMPLES as f64;
                groups[residue as usize] += Complex64::from_polar(1.0 / SAMPLES as f64, phase);
            }
            groups.iter().map(Complex64::norm_sqr).sum()
        })
        .collect()
}

fn check_period_harness(
    module: ValidModule,
    callee: FuncId,
    exact: &[f64],
    oracles: [&GateBlock; 2],
) -> (f64, f64, [usize; 2]) {
    let main = entry(&module, "entry");
    let mut module = module.into_module();
    let instructions = &mut module.functions[main.index()].blocks[0].insts;
    let (apply, exponent, fault) = instructions
        .iter()
        .enumerate()
        .find_map(|(index, inst)| {
            let Inst::Apply { callee: called, args, fault, .. } = inst else { return None };
            let [Operand::Slot(exponent)] = args.as_slice() else { return None };
            (*called == callee).then_some((index, *exponent, *fault))
        })
        .expect("the applied exponentiation is in the harness entry block");
    assert!(matches!(
        instructions[apply + 1],
        Inst::Measure { source: MeasureSource::Register(slot), .. } if slot == exponent
    ));
    let register_column = |slot| {
        instructions[..apply]
            .iter()
            .filter_map(|inst| match inst {
                Inst::Qalloc { dst, .. } => Some(*dst),
                _ => None,
            })
            .position(|allocated| allocated == slot)
            .expect("the register was allocated in the entry block")
    };
    let exponent_column = register_column(exponent);
    let fault_column = register_column(fault);
    // Drive Fourier gates in device IR and leave the measurement written in C.
    let qft = fourier(exponent);
    let measurement = apply + 1 + qft.len();
    instructions.splice(apply + 1..apply + 1, qft);
    let module = validate(module).expect("valid period finding harness");
    let options = bennett_front_tests::options(0);
    let mut direct = Executor::new(&module, main, &[], options).expect("direct harness");
    let mut emitted = Executor::new(&module, main, &[], options).expect("gate harness");
    let mut calls = 0;
    let mut compiler = |input: ApplyInput<'_>| {
        assert_eq!(input.callee, callee);
        calls += 1;
        Ok(require_oracle(input)?.block)
    };
    for _ in 0..=apply {
        assert_eq!(direct.step(), None);
        assert_eq!(emitted.step_emitted(&mut compiler), None);
    }
    assert_eq!(calls, 1);
    let expected = direct.snapshot();
    let actual = emitted.snapshot();
    assert_eq!(actual.classical, expected.classical);
    let amplitude_error = actual.quantum.distance(&expected.quantum).expect("same registers");
    assert!(amplitude_error < TOLERANCE, "oracle amplitude error {amplitude_error:e}");
    for _ in apply + 1..measurement {
        assert_eq!(emitted.step(), None);
    }
    let mut observed = vec![0.0; SAMPLES];
    for (assignment, amplitude) in emitted.snapshot().quantum.amplitudes {
        assert_eq!(assignment[fault_column], 0);
        // This Fourier circuit leaves the output bits in reverse order.
        let frequency = (assignment[exponent_column] as u8).reverse_bits();
        observed[usize::from(frequency)] += amplitude.norm_sqr();
    }
    let error = observed.iter().zip(exact).map(|(actual, exact)| (actual - exact).abs()).fold(0.0, f64::max);
    assert!(error < TOLERANCE, "period finding distribution error {error:e}");
    assert_eq!(emitted.step(), None);
    let measured = emitted.snapshot();
    let measured_value = measured.quantum.amplitudes[0].0[exponent_column];
    assert!(measured.quantum.amplitudes.iter().all(|(values, _)| values[exponent_column] == measured_value));
    assert!(exact[usize::from((measured_value as u8).reverse_bits())] > TOLERANCE);
    let harness_peaks = oracles.map(|oracle| count::harness_peak(&module, main, callee, oracle));
    (amplitude_error, error, harness_peaks)
}

fn check_oracle(block: &GateBlock, residues: &[u64]) {
    let [BlockOp::Allocate(scratch), gates @ .., BlockOp::ReleaseZero(released)] = block.ops() else {
        panic!("expected one scratch lifetime");
    };
    assert_eq!(scratch, released);
    assert_eq!(scratch.start, INTERFACE);
    let gates = gates
        .iter()
        .map(|op| match op {
            BlockOp::Gate(gate) => gate.clone(),
            BlockOp::Allocate(_) | BlockOp::ReleaseZero(_) => panic!("unexpected scratch lifetime"),
        })
        .collect::<Vec<_>>();
    let layout = bits::Layout {
        x: EXPONENT_BITS as usize,
        y: (INTERFACE - EXPONENT_BITS) as usize,
        scratch: (scratch.end - scratch.start) as usize,
    };
    let states = (0..SAMPLES as u128).flat_map(|x| [(x, 0), (x, 0x15a5)]);
    bits::check_states(&gates, layout, states, |x| u128::from(residues[x as usize]))
        .unwrap_or_else(|error| panic!("{}: {error}", block.name()));
}

#[test]
fn c_shor_period_distribution_and_counts() {
    for (modulus, base) in CASES {
        let (module, callee) = front_powmod(modulus, base, "shor_period");
        let oracle = compile_oracle(&module, callee);
        let residues = modular_powers(base, modulus);
        check_oracle(&oracle.block, &residues);
        let emitted = count::cancel_adjacent_toffolis(&oracle.block);
        let hand_built = count::cancel_adjacent_toffolis(&baseline::powmod(base, modulus));
        check_oracle(&hand_built, &residues);
        let generated_counts = count::isolated(&emitted);
        let baseline_counts = count::isolated(&hand_built);
        assert!(
            generated_counts.toffoli <= 2 * baseline_counts.toffoli,
            "N={modulus}: Toffolis {} vs {}",
            generated_counts.toffoli,
            baseline_counts.toffoli
        );
        let (amplitude_error, distribution_error, [emitted_peak, hand_peak]) =
            check_period_harness(module, callee, &exact_probabilities(&residues, modulus), [&emitted, &hand_built]);
        assert!(emitted_peak <= 2 * hand_peak, "N={modulus}: harness peak {emitted_peak} vs {hand_peak}");
        println!(
            "N={modulus}, base={base}, {EXPONENT_BITS} exponent bits, {}\nemitted {generated_counts:?}\nhand {baseline_counts:?}\nharness peaks: emitted {emitted_peak}, hand {hand_peak}\namplitude error {amplitude_error:e}, distribution error {distribution_error:e}",
            oracle.construction
        );
    }
}
