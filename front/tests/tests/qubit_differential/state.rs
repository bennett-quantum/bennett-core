//! Compare native truth tables with amplitudes, or with matrix elements after a partial trace.

use std::collections::BTreeMap;

use bennett_exec::{Executor, Outcome, QuantumSnapshot, Stats};
use bennett_front_tests::{entry, options};
use bennett_ir::{
    BinOp, Const, FixedGate, Gate, Inst, MeasureSource, Operand, Place, Qubit, Slot, Type, ValidModule, Width, validate,
};
use num_complex::Complex64;

const TOLERANCE: f64 = 1e-12;

#[derive(Clone, Copy, Default)]
pub(super) struct Preparation {
    pub(super) z: u32,
    pub(super) s: u32,
}

impl Preparation {
    fn phase(self, input: usize) -> Complex64 {
        let sign = if (input & self.z as usize).count_ones().is_multiple_of(2) { 1.0 } else { -1.0 };
        Complex64::new(0.0, 1.0).powu((input & self.s as usize).count_ones()) * sign
    }

    pub(super) fn phased(self) -> bool {
        self.z | self.s != 0
    }
}

/// Where a measurement appears in native C's result packets.
#[derive(Clone, Copy)]
pub(super) struct Field {
    pub(super) packet: usize,
    pub(super) shift: u32,
}

impl Field {
    fn value(self, table: &[Vec<u128>], input: usize, width: Width) -> u64 {
        ((table[self.packet][input] >> self.shift) & width.mask()) as u64
    }
}

pub(super) struct Check {
    pub(super) fields: Vec<Field>,
    pub(super) preparation: Preparation,
    pub(super) return_to_zero: bool,
    pub(super) retained: Option<Vec<u64>>,
    pub(super) reuse_measured_byte: bool,
}

impl Check {
    pub(super) fn new(fields: Vec<Field>) -> Self {
        Self {
            fields,
            preparation: Preparation::default(),
            return_to_zero: false,
            retained: None,
            reuse_measured_byte: false,
        }
    }
}

#[derive(Default)]
pub(super) struct Counts {
    pub(super) total: usize,
    pub(super) phase: usize,
    pub(super) return_to_zero: usize,
    pub(super) retained: usize,
}

type Amplitudes = BTreeMap<Vec<u64>, Complex64>;

// Move each measured value into its own register by SWAP and give the original
// measurement's users a zero. This preserves the compiler's exchanges between
// measurements and its return cleanup. The classical return packet is unused.
fn observation_module(module: &ValidModule, reuse_measured_byte: bool) -> Result<(ValidModule, Vec<Width>), String> {
    let entry = entry(module, "entry");
    let mut instrumented = module.clone().into_module();
    let function = &mut instrumented.functions[entry.index()];
    let mut allocations = Vec::new();
    let mut widths = Vec::new();
    for block in &mut function.blocks {
        let mut insts = Vec::new();
        for inst in std::mem::take(&mut block.insts) {
            if let Inst::Measure { dst, source } = inst {
                let width = match source {
                    MeasureSource::Register(slot) => match module.slot_type(entry, slot) {
                        Type::Qreg(width) => width,
                        ty => return Err(format!("measurement source has type {ty:?}")),
                    },
                    MeasureSource::Memory { width, .. } => width,
                };
                // The byte reused by ordinary code must still be demoted by a measure.
                let demote_byte = reuse_measured_byte && widths.is_empty();
                let slot = Slot(function.slot_names.len() as u32);
                function.slot_names.push(format!("test.observed.{}", widths.len()));
                widths.push(width);
                allocations.push(Inst::Qalloc { dst: slot, width });
                for bit in 0..width.bits() as u8 {
                    let original = match source {
                        MeasureSource::Register(register) => Place::Register(Qubit { register, bit }),
                        MeasureSource::Memory { address, .. } => Place::Memory { address, bit },
                    };
                    let observed = Place::Register(Qubit { register: slot, bit });
                    insts.extend([cnot(observed, original), cnot(original, observed), cnot(observed, original)]);
                }
                if demote_byte {
                    insts.push(inst);
                } else {
                    let zero = Operand::Const(Const::new(width, 0).expect("zero fits"));
                    insts.push(Inst::Binary { op: BinOp::Add, width, dst, lhs: zero, rhs: zero });
                }
            } else {
                insts.push(inst);
            }
        }
        block.insts = insts;
    }
    function.blocks[0].insts.splice(0..0, allocations);
    let validated = validate(instrumented).map_err(|error| format!("invalid state observation program: {error}"))?;
    Ok((validated, widths))
}

fn cnot(target: Place, control: Place) -> Inst {
    let gate = Gate::Fixed(FixedGate::X);
    if let (Place::Register(target), Place::Register(control)) = (target, control) {
        Inst::Gate { gate, target, controls: vec![control] }
    } else {
        Inst::GateAt { gate, target, controls: vec![control] }
    }
}

// Only the byte-reuse recipe needs a real measurement, of the zero left by
// the SWAP. Verify it cannot project the state and record its normalization
// factor so that the comparison still detects a common amplitude-scale error.
fn before_demoting_byte(executor: &mut Executor<'_>, module: &ValidModule) -> Result<f64, String> {
    loop {
        let snapshot = executor.snapshot();
        let frame = snapshot.classical.frames.last().expect("running state observation");
        let block = module.function(frame.func).block(frame.block);
        if let Some(Inst::Measure { source, .. }) = block.insts.get(frame.inst) {
            let MeasureSource::Memory { width: Width::BYTE, address } = *source else {
                return Err(format!("expected the byte-reuse measurement, found {source:?}"));
            };
            let address = match address {
                Operand::Slot(slot) => snapshot.classical.slots[frame.base + slot.index()],
                Operand::Const(value) => value.value(),
            } as u64;
            let column = snapshot
                .quantum
                .cells
                .iter()
                .find_map(|&(at, column)| (at == address).then_some(column))
                .ok_or_else(|| format!("byte at {address} did not get qubits at readout"))?;
            let mut weight = 0.0;
            for (basis, amplitude) in &snapshot.quantum.amplitudes {
                if basis[column] != 0 {
                    return Err(format!(
                        "byte at {address} is not zero at readout: {basis:?} has amplitude {amplitude}"
                    ));
                }
                weight += amplitude.norm_sqr();
            }
            return Ok(weight.sqrt());
        }
        if let Some(outcome) = executor.step() {
            return Err(format!("state observation ended before demoting the measured byte: {outcome:?}"));
        }
    }
}

fn observe(
    module: &ValidModule,
    trips: u64,
    column: u64,
    reuse_measured_byte: bool,
) -> Result<(QuantumSnapshot, Stats), String> {
    let mut executor = Executor::new(module, entry(module, "entry"), &[0, trips, column], options(0))
        .map_err(|error| format!("start state observation: {error}"))?;
    let scale = if reuse_measured_byte { before_demoting_byte(&mut executor, module)? } else { 1.0 };
    let outcome = executor.run();
    if !matches!(outcome, Outcome::Returned(Some(_))) {
        return Err(format!("state observation: {outcome:?}"));
    }
    let mut quantum = executor.snapshot().quantum;
    if reuse_measured_byte {
        for (_, amplitude) in &mut quantum.amplitudes {
            *amplitude *= scale;
        }
    }
    Ok((quantum, executor.stats()))
}

pub(super) fn compare(
    check: &Check,
    module: &ValidModule,
    table: &[Vec<u128>],
    trips: u64,
    retained_bits: u32,
    counts: &mut Counts,
) -> Result<(), String> {
    let (instrumented, widths) = observation_module(module, check.reuse_measured_byte)?;
    if widths.len() != check.fields.len() {
        return Err(format!("expected {} final measurements, found {widths:?}", check.fields.len()));
    }
    let (snapshot, stats) = observe(&instrumented, trips, 0, check.reuse_measured_byte)?;
    let expected: Vec<_> = (0..table[0].len())
        .map(|input| {
            let key =
                check.fields.iter().zip(&widths).map(|(field, &width)| field.value(table, input, width)).collect();
            let amplitude = check.preparation.phase(input) / (table[0].len() as f64).sqrt();
            (key, amplitude)
        })
        .collect();
    counts.total += 1;
    counts.phase += usize::from(check.preparation.phased());
    if let Some(old) = &check.retained {
        counts.retained += 1;
        compare_retained(&snapshot, &stats, widths.len(), expected, old, retained_bits)?;
    } else {
        compare_clean(&snapshot, widths.len(), expected)?;
    }

    if check.return_to_zero {
        let (unprepared, _) = observe(&instrumented, trips, 1, check.reuse_measured_byte)?;
        counts.total += 1;
        counts.return_to_zero += 1;
        let mut weight = 0.0;
        let mut zero_weight = 0.0;
        for (basis, amplitude) in &unprepared.amplitudes {
            weight += amplitude.norm_sqr();
            if basis.iter().all(|&word| word == 0) {
                zero_weight += amplitude.norm_sqr();
            }
        }
        // The norm has floating-point rounding from the Hadamards. Require exact support
        // on zero as well as unit total weight within tolerance.
        let probability = zero_weight / weight;
        if unprepared.amplitudes.iter().any(|(basis, _)| basis.iter().any(|&word| word != 0))
            || probability != 1.0
            || (weight - 1.0).abs() > TOLERANCE
        {
            return Err(format!(
                "return to zero: P(0)={probability}, weight={weight}, state={:?}",
                unprepared.amplitudes
            ));
        }
    }
    Ok(())
}

fn compare_clean(
    snapshot: &QuantumSnapshot,
    outputs: usize,
    expected: Vec<(Vec<u64>, Complex64)>,
) -> Result<(), String> {
    for (basis, amplitude) in &snapshot.amplitudes {
        if let Some(column) = (outputs..basis.len()).find(|&column| basis[column] != 0) {
            return Err(format!(
                "clean state: extra amplitude {amplitude} at {basis:?}; spare column {column} is {}, expected 0",
                basis[column]
            ));
        }
    }
    let actual = snapshot.amplitudes.iter().map(|(basis, amplitude)| (basis[..outputs].to_vec(), *amplitude)).collect();
    compare_sparse(&actual, &expected.into_iter().collect(), "clean state amplitude")
}

fn compare_retained(
    snapshot: &QuantumSnapshot,
    stats: &Stats,
    outputs: usize,
    expected: Vec<(Vec<u64>, Complex64)>,
    old: &[u64],
    retained_bits: u32,
) -> Result<(), String> {
    let spare_bits: u32 = snapshot.registers[outputs..].iter().filter_map(|r| r.width).map(Width::bits).sum();
    if (stats.nonzero_releases, stats.leftover_qubits, spare_bits) != (1, retained_bits, retained_bits) {
        return Err(format!(
            "retained state: expected only the release of {retained_bits} qubits, found {} nonzero releases with {} released and {spare_bits} spare qubits",
            stats.nonzero_releases, stats.leftover_qubits
        ));
    }

    // Partial trace: sum a(output, spare) * conj(a(other, spare)) over the same
    // released-register value, for every pair of output configurations.
    let mut actual = Amplitudes::new();
    for (basis, amplitude) in &snapshot.amplitudes {
        let (output, spare) = basis.split_at(outputs);
        for (other, other_amplitude) in &snapshot.amplitudes {
            let (other_output, other_spare) = other.split_at(outputs);
            if spare == other_spare {
                *actual.entry([output, other_output].concat()).or_default() += amplitude * other_amplitude.conj();
            }
        }
    }
    let mut density = Amplitudes::new();
    for (v, (basis, amplitude)) in expected.iter().enumerate() {
        for (w, (other, other_amplitude)) in expected.iter().enumerate() {
            if old[v] == old[w] {
                density.insert([basis.as_slice(), other.as_slice()].concat(), amplitude * other_amplitude.conj());
            }
        }
    }
    compare_sparse(&actual, &density, "retained density matrix element")
}

fn compare_sparse(actual: &Amplitudes, expected: &Amplitudes, context: &str) -> Result<(), String> {
    for key in expected.keys().chain(actual.keys()) {
        let found = actual.get(key).copied().unwrap_or_default();
        let wanted = expected.get(key).copied().unwrap_or_default();
        let distance = (found - wanted).norm();
        if !distance.is_finite() || distance > TOLERANCE {
            return Err(format!("{context} at {key:?}: device={found}, native prediction={wanted}"));
        }
    }
    Ok(())
}
