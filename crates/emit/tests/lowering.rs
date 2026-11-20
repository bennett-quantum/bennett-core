//! Phase-sensitive comparison of primitive and rewritten gates on the sparse simulator.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::ops::Range;

use bennett_emit::{CountingDevice, LowerControls};
use bennett_exec::{Device, Executor, GateBlock, GateOp, Options, Outcome, QuantumSnapshot, RunError, Wire};
use bennett_ir::{FixedGate, Gate, Inst, Qubit, Rotation, Slot, ValidModule};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

#[derive(Default)]
struct Program {
    instructions: String,
    allocations: BTreeMap<Wire, (usize, Range<Wire>)>,
    next_register: usize,
}

impl Program {
    fn place(&self, wire: Wire) -> String {
        let (_, (name, range)) = self.allocations.range(..=wire).next_back().expect("live wire");
        assert!(range.contains(&wire), "wire {wire} is not live");
        format!("%r{name}[{}]", wire - range.start)
    }

    fn decode(&self) -> ValidModule {
        let memory = 8 * self.next_register.max(1);
        let text = format!("memory {memory}\nfunc @main() {{\nentry:\n{}ret\n}}", self.instructions);
        bennett_ir::load(&text).unwrap_or_else(|error| panic!("{error}\n{text}"))
    }

    fn simulate(&self, input_width: u64) -> QuantumSnapshot {
        simulate(&self.decode(), input_width)
    }
}

impl Device for Program {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        if wires.is_empty() {
            return Ok(());
        }
        let width = wires.end - wires.start;
        let name = self.next_register;
        self.next_register += 1;
        assert!(self.allocations.insert(wires.start, (name, wires)).is_none());
        writeln!(self.instructions, "%r{name} = qalloc {width}").expect("string");
        Ok(())
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        let kind = match gate.kind {
            Gate::Fixed(kind) => kind.mnemonic().to_owned(),
            Gate::Rotate(axis, angle) => format!("{}({angle:?})", axis.mnemonic()),
        };
        let target = self.place(gate.target);
        write!(self.instructions, "{kind} {target}").expect("string");
        if !gate.controls.is_empty() {
            let controls = gate.controls.iter().map(|&wire| self.place(wire)).collect::<Vec<_>>().join(", ");
            write!(self.instructions, " ctrl {controls}").expect("string");
        }
        self.instructions.push('\n');
        Ok(())
    }

    fn measure(&mut self, _wire: Wire) -> Result<bool, RunError> {
        unreachable!("the compared circuits are unitary")
    }

    fn discard(&mut self, _wires: Range<Wire>) -> Result<(), RunError> {
        unreachable!("the lowering uses only zero releases")
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        if wires.is_empty() {
            return Ok(());
        }
        let (name, allocated) = self.allocations.remove(&wires.start).expect("live allocation");
        assert_eq!(wires, allocated);
        writeln!(self.instructions, "qfree %r{name}").expect("string");
        Ok(())
    }
}

fn simulate(module: &ValidModule, input_width: u64) -> QuantumSnapshot {
    let mut executor =
        Executor::new(module, module.function_named("main").expect("entry"), &[], Options::default()).expect("started");
    assert_eq!(executor.run(), Outcome::Returned(None));
    assert_eq!(executor.stats().live_qubits, input_width as u32);
    assert_eq!(executor.stats().nonzero_releases, 0);
    executor.snapshot().quantum
}

fn preparation(width: u64, rng: &mut ChaCha8Rng) -> Vec<GateOp> {
    let mut gates = Vec::new();
    for target in 0..width {
        for axis in [Rotation::Ry, Rotation::Rz] {
            let angle = rng.random_range(-3.0..3.0);
            gates.push(GateOp { kind: Gate::Rotate(axis, angle), target, controls: vec![] });
        }
        if target > 0 {
            let control = rng.random_range(0..target);
            gates.push(GateOp { kind: Gate::Fixed(FixedGate::X), target, controls: vec![control] });
        }
    }
    gates
}

fn simulate_original(width: u64, gates: &[GateOp]) -> QuantumSnapshot {
    // Build the reference instructions directly, independent of the writer's names and mnemonic formatting.
    let text = format!("memory 8\nfunc @main() {{\nentry:\n%input = qalloc {width}\nret\n}}");
    let mut module = bennett_ir::read(&text).expect("valid reference allocation");
    let qubit = |wire| Qubit { register: Slot(0), bit: wire as u32 };
    module.functions[0].blocks[0].insts.extend(gates.iter().map(|op| Inst::Gate {
        gate: op.kind,
        target: qubit(op.target),
        controls: op.controls.iter().copied().map(qubit).collect(),
    }));
    simulate(&bennett_ir::validate(module).expect("valid reference instructions"), width)
}

#[test]
fn every_gate_and_control_count_preserve_amplitudes_and_release_clean_scratch() {
    let gates =
        FixedGate::ALL.iter().copied().map(Gate::Fixed).chain(
            Rotation::ALL.iter().copied().flat_map(|axis| [-1.89, 0.713].map(|angle| Gate::Rotate(axis, angle))),
        );
    for (index, kind) in gates.enumerate() {
        for target in 0..=5 {
            let controls: Vec<Wire> = (0..target).collect();
            let gate = GateOp { kind, target, controls };
            for sample in 0..3 {
                let seed = (100 * index as u64) + 10 * target + sample;
                let mut gates = preparation(target + 1, &mut ChaCha8Rng::seed_from_u64(seed));
                gates.push(gate.clone());
                let mut rewritten = LowerControls::new(Program::default());
                rewritten.allocate(0..target + 1).expect("inputs");
                for op in &gates {
                    rewritten.gate(op.borrowed()).expect("send gate");
                }

                let original = simulate_original(target + 1, &gates);
                let lowered = rewritten.inner().simulate(target + 1);
                for (config, _) in original.amplitudes.iter().chain(&lowered.amplitudes) {
                    let distance = (original.amplitude(config) - lowered.amplitude(config)).norm();
                    assert!(distance <= 1e-12, "{kind:?}, {target} controls, sample {sample}, {config:?}: {distance}");
                }
            }
        }
    }
}

#[test]
fn decoded_cnot_uses_the_transmitted_target_and_control() {
    let mut output = Program::default();
    output.allocate(0..3).expect("wires");
    // Prepare only wire zero directly in the IR, outside the writer's gate-to-wire mapping.
    output.instructions.push_str("x %r0[0]\n");
    let x = Gate::Fixed(FixedGate::X);
    let block = GateBlock::new("copy", [GateOp { kind: x, target: 2, controls: vec![0] }]);
    output.repeat(&block, 3).expect("expanded CNOT block");
    assert_eq!(output.instructions.matches(" ctrl ").count(), 3);
    let state = output.simulate(3);
    assert_eq!(state.amplitudes.len(), 1);
    assert_eq!(state.amplitude(&[5]).re, 1.0);
    assert_eq!(state.amplitude(&[5]).im, 0.0);
}

#[test]
fn a_decoded_s_gate_preserves_the_quarter_turn_phase() {
    let mut output = Program::default();
    output.allocate(0..1).expect("wires");
    output.instructions.push_str("h %r0[0]\n");
    output.gate(GateOp { kind: Gate::Fixed(FixedGate::S), target: 0, controls: &[] }).expect("phase");
    let state = output.simulate(1);
    let phase = state.amplitude(&[1]);
    assert!(phase.re.abs() < 1e-12);
    assert!((phase.im - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
}

#[test]
fn decoded_gates_match_the_counter_without_using_the_writer_to_count() {
    let block = GateBlock::new(
        "exercise all gate classes",
        [
            GateOp { kind: Gate::Fixed(FixedGate::X), target: 0, controls: vec![] },
            GateOp { kind: Gate::Fixed(FixedGate::X), target: 1, controls: vec![0] },
            GateOp { kind: Gate::Fixed(FixedGate::S), target: 2, controls: vec![0, 1] },
            GateOp { kind: Gate::Rotate(Rotation::Ry, 0.7), target: 3, controls: vec![0, 1, 2] },
        ],
    );

    let mut writer = LowerControls::new(Program::default());
    writer.allocate(0..4).expect("inputs");
    writer.repeat(&block, 2).expect("transmit two calls");
    let module = writer.inner().decode();

    let mut decoded = CountingDevice::default();
    let run = Executor::run_on(&module, module.function_named("main").expect("entry"), &[], None, &mut decoded)
        .expect("run decoded commands");
    assert_eq!(run.outcome, Outcome::Returned(None));
    assert!(decoded.outcomes().is_empty());
    let mut counted = LowerControls::new(CountingDevice::default());
    counted.allocate(0..4).expect("inputs");
    counted.repeat(&block, 2).expect("count the original commands");
    assert_eq!(decoded.outcomes(), counted.inner().outcomes());
    let mut expected = counted.inner().counts();
    // Text represents a known-zero release as qfree. The preceding simulation checks its zero promise.
    expected.discarded_qubits += expected.zero_released_qubits;
    expected.zero_released_qubits = 0;
    assert_eq!(decoded.counts(), expected);
}

#[test]
fn two_lowered_block_calls_have_independent_scratch_names() {
    let mut lowered = LowerControls::new(Program::default());
    lowered.allocate(0..3).expect("inputs");
    let block = GateBlock::new(
        "double controlled H",
        [GateOp { kind: Gate::Fixed(FixedGate::H), target: 2, controls: vec![0, 1] }],
    );
    lowered.repeat(&block, 2).expect("two scratch lifetimes");
    let state = lowered.inner().simulate(3);
    assert_eq!(state.amplitude(&[0]).re, 1.0);
}

#[test]
fn a_tiny_rotation_round_trips_through_the_test_writer() {
    let mut output = Program::default();
    output.allocate(0..1).expect("input");
    let kind = Gate::Rotate(Rotation::Rx, 1e-18);
    output.gate(GateOp { kind, target: 0, controls: &[] }).expect("gate");
    let text = format!("memory 128\nfunc @main() {{\nentry:\n{}ret\n}}", output.instructions);
    let module = bennett_ir::load(&text).expect("valid output");
    let bennett_ir::Inst::Gate { gate, .. } = module.functions[0].blocks[0].insts[1] else {
        panic!("expected a decoded rotation");
    };
    assert_eq!(gate, kind);
}
