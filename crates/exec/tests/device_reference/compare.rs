//! Observe the executor around measurements, after non-local jump operations and at completion.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use bennett_exec::{Executor, Options, Outcome, QuantumSnapshot, RegisterState, RunError, Snapshot};
use bennett_ir::{Fault, Qubits, Slot, Type, ValidModule, Width};
use num_complex::Complex64 as C;

use super::device_reference::{Basis, Density, Device, Event, Finish, RegisterValue};

const TOLERANCE: f64 = 1e-11;

#[derive(Debug, Default)]
pub struct Counts {
    pub programs: usize,
    pub operations: u64,
    pub states: usize,
    pub density_entries: usize,
    pub measurements: usize,
}

impl Counts {
    pub fn add(&mut self, run: &Self) {
        self.programs += run.programs;
        self.operations += run.operations;
        self.states += run.states;
        self.density_entries += run.density_entries;
        self.measurements += run.measurements;
    }
}

pub struct Run<'m> {
    pub reference: Device<'m>,
    pub measurements: Vec<(u128, BTreeMap<u128, f64>)>,
    pub counts: Counts,
}

struct Register {
    wire: u64,
    handle: u128,
    head: RegisterState,
}

/// The start wire of each allocation maps to the full range of bits it occupies in the snapshot.
type RegisterTable = BTreeMap<u64, Range<u64>>;

#[derive(Default)]
struct RegisterObservations {
    allocations: BTreeMap<u64, Register>,
    last_table: RegisterTable,
}

impl RegisterObservations {
    fn live_table(&self, quantum: &QuantumSnapshot) -> RegisterTable {
        let cells: BTreeSet<_> = quantum.cells.iter().map(|&(_, column)| 64 * column as u64).collect();
        quantum
            .wires
            .iter()
            .filter(|(_, packed)| !cells.contains(packed))
            .map(|(wires, packed)| (wires.start, *packed..packed + (wires.end - wires.start)))
            .collect()
    }

    fn changed(&self, next: &RegisterTable) -> Vec<u64> {
        self.last_table
            .keys()
            .chain(next.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|wire| self.last_table.get(wire) != next.get(wire))
            .collect()
    }

    fn allocate(&mut self, id: u64, width: Qubits, handle: u128, quantum: &QuantumSnapshot) {
        let table = self.live_table(quantum);
        let changed = self.changed(&table);
        assert_eq!(changed.len(), 1, "qalloc adds one register range");
        let wire = changed[0];
        let bits = &table[&wire];
        assert!(!self.last_table.contains_key(&wire), "qalloc adds a fresh range");
        assert_eq!(bits.end - bits.start, u64::from(width.count()), "qalloc width");
        let head = quantum.registers[(bits.start / 64) as usize];
        assert_eq!(head.width, Width::new(width.count().min(64)), "qalloc head width");
        assert_ne!(handle, 0, "qalloc produces a handle");
        assert!(
            self.allocations.values().all(|allocation| allocation.handle != handle),
            "qalloc never reuses a handle"
        );
        self.allocations.insert(id, Register { wire, handle, head });
        self.last_table = table;
    }

    fn release(&mut self, id: u64, quantum: &QuantumSnapshot) {
        let table = self.live_table(quantum);
        let register = &self.allocations[&id];
        assert_eq!(self.changed(&table), [register.wire], "qfree changes the released range");
        assert!(!table.contains_key(&register.wire), "qfree removes live wires");
        let head = (self.last_table[&register.wire].start / 64) as usize;
        assert_ne!(quantum.registers.get(head), Some(&register.head), "qfree retires the handle");
        self.last_table = table;
    }

    fn live_ranges(&self, quantum: &QuantumSnapshot, live: &[(u64, Qubits)]) -> Vec<Range<u64>> {
        let table = self.live_table(quantum);
        assert_eq!(self.last_table, table, "only qalloc and qfree change live register ranges");
        assert_eq!(table.len(), live.len(), "live register count");
        live.iter()
            .map(|(id, width)| {
                let allocation = &self.allocations[id];
                let bits = &table[&allocation.wire];
                assert_eq!(bits.end - bits.start, u64::from(width.count()), "live width of register {id}");
                assert_eq!(quantum.registers[(bits.start / 64) as usize], allocation.head, "live handle {id}");
                bits.clone()
            })
            .collect()
    }
}

pub fn run<'m>(module: &'m ValidModule, name: &str, args: &[u64], seed: u64, budget: u64) -> Run<'m> {
    run_through(module, name, args, seed, budget, None)
}

pub fn run_until_stop(module: &ValidModule, budget: u64, after_steps: u64, error: RunError) -> Device<'_> {
    let run = run_through(module, "main", &[], 0, budget, Some((after_steps, error)));
    assert_eq!(run.reference.finish, None, "the stop is specific to the simulator");
    assert_eq!(run.reference.steps, after_steps);
    run.reference
}

fn run_through<'m>(
    module: &'m ValidModule,
    name: &str,
    args: &[u64],
    seed: u64,
    budget: u64,
    stop: Option<(u64, RunError)>,
) -> Run<'m> {
    let entry = module.function_named(name).expect("entry function");
    let reference_args: Vec<_> = args.iter().copied().map(u128::from).collect();
    let mut reference = Device::new(module, entry, &reference_args, budget);
    let options = Options { seed, budget: Some(budget), allow_region_faults: true, ..Options::default() };
    let mut executor = Executor::new(module, entry, args, options).expect("executor starts");
    let mut counts = Counts { programs: 1, ..Counts::default() };
    let mut registers = RegisterObservations::default();
    let mut measurements = Vec::new();
    let mut jump_fault = false;

    if let Some(finish) = reference.finish {
        assert_eq!(executor.step(), Some(outcome(finish)), "entry fault");
    }
    while reference.finish.is_none() {
        if let Some((after_steps, error)) = stop
            && reference.steps == after_steps
        {
            let before = executor.snapshot();
            let stats = executor.stats();
            checkpoint(&reference, &before, &registers, &mut counts, true);
            let expected = Some(Outcome::Stopped(error));
            assert_eq!(executor.step(), expected, "simulator stop at step {after_steps}");
            let mut after = executor.snapshot();
            assert_eq!(after.classical.outcome, expected);
            after.classical.outcome = before.classical.outcome;
            assert_eq!(after.classical, before.classical, "stop leaves the classical state unchanged");
            assert_eq!(after.quantum, before.quantum, "stop leaves the quantum state unchanged");
            assert_eq!(executor.stats().live_qubits, stats.live_qubits, "stop leaves the qubit count unchanged");
            counts.operations = reference.steps;
            return Run { reference, measurements, counts };
        }
        let measurement = reference.next_measurement();
        if measurement.is_some() {
            checkpoint(&reference, &executor.snapshot(), &registers, &mut counts, true);
        }

        let jump = reference.next_is_nonlocal_jump();
        let actual_finish = executor.step();
        let observed = measurement.map(|dst| {
            assert_eq!(actual_finish, None, "executor must reach the measurement");
            let snapshot = executor.snapshot();
            let frame = snapshot.classical.frames.last().expect("measurement's frame");
            snapshot.classical.slots[frame.base + dst.index()]
        });

        match reference.step(observed) {
            Event::Ordinary => {}
            Event::Allocated { id, dst, width } => {
                let snapshot = executor.snapshot();
                let frame = snapshot.classical.frames.last().expect("allocator's frame");
                let handle = snapshot.classical.slots[frame.base + dst.index()];
                registers.allocate(id, width, handle, &snapshot.quantum);
            }
            Event::Released(id) => registers.release(id, &executor.snapshot().quantum),
            Event::Measured { outcome, probabilities } => {
                counts.measurements += 1;
                measurements.push((outcome, probabilities));
                checkpoint(&reference, &executor.snapshot(), &registers, &mut counts, true);
            }
        }
        assert_eq!(actual_finish, reference.finish.map(outcome), "outcome after {} steps", reference.steps);
        if jump {
            jump_fault = reference.finish.is_some();
            if !jump_fault {
                checkpoint(&reference, &executor.snapshot(), &registers, &mut counts, true);
            }
        }
    }
    assert_eq!(executor.outcome(), reference.finish.map(outcome), "completion");
    let bad_angle = reference.finish == Some(Finish::Faulted(Fault::BadAngle));
    checkpoint(&reference, &executor.snapshot(), &registers, &mut counts, jump_fault || bad_angle);
    counts.operations = reference.steps;
    Run { reference, measurements, counts }
}

fn outcome(finish: Finish) -> Outcome {
    match finish {
        Finish::Returned(value) => Outcome::Returned(value),
        Finish::Faulted(fault) => Outcome::Faulted(fault),
    }
}

fn checkpoint(
    reference: &Device<'_>,
    snapshot: &Snapshot,
    registers: &RegisterObservations,
    counts: &mut Counts,
    compare_frames: bool,
) {
    let actual = &snapshot.classical;
    let step = reference.steps;
    assert_eq!(actual.steps, step, "step count");
    assert_eq!(actual.memory, reference.memory, "memory at step {step}");
    assert_eq!(actual.tape, reference.tape, "tape at step {step}");
    if compare_frames {
        assert_eq!(actual.stack_pointer, reference.stack, "stack at step {step}");
        assert_eq!(actual.frames.len(), reference.frames.len(), "call depth at step {step}");
        for (actual_frame, frame) in actual.frames.iter().zip(&reference.frames) {
            assert_eq!(actual_frame.func, frame.function, "function at step {step}");
            assert_eq!(actual_frame.block, frame.block, "block at step {step}");
            assert_eq!(actual_frame.inst, frame.position, "position at step {step}");
            assert_eq!(actual_frame.entry_stack_pointer, frame.entry_stack, "entry stack at step {step}");
            for (index, &value) in frame.slots.iter().enumerate() {
                let expected = match reference.slot_type(frame.function, Slot(index as u32)) {
                    Type::Qreg(_) if value != 0 => {
                        registers.allocations[&u64::try_from(value).expect("reference handle")].handle
                    }
                    _ => value,
                };
                assert_eq!(actual.slots[actual_frame.base + index], expected, "slot {index} at step {step}");
            }
        }
    }
    let mut ranges = registers.live_ranges(&snapshot.quantum, &reference.quantum.live);
    let actual_cells: BTreeMap<_, _> = snapshot.quantum.cells.iter().copied().collect();
    assert_eq!(actual_cells.len(), snapshot.quantum.cells.len(), "duplicate cell address at step {step}");
    assert!(
        actual_cells.keys().eq(reference.quantum.cells.keys()),
        "cell addresses at step {step}: actual {actual_cells:?}, expected {:?}",
        reference.quantum.cells
    );
    ranges.resize(ranges.len() + actual_cells.len(), 0..0);
    for (&address, &column) in &reference.quantum.cells {
        let actual_column = actual_cells[&address];
        assert_eq!(snapshot.quantum.registers[actual_column].width, Some(Width::BYTE), "cell width at step {step}");
        let start = 64 * actual_column as u64;
        assert!(
            snapshot.quantum.wires.iter().any(|(wires, packed)| *packed == start && wires.end - wires.start == 8),
            "wire range for cell {address} at step {step}",
        );
        ranges[column] = start..start + 8;
    }
    let mut sorted: Vec<_> = ranges.iter().collect();
    sorted.sort_unstable_by_key(|bits| bits.start);
    assert!(sorted.windows(2).all(|pair| pair[0].end <= pair[1].start), "disjoint live ranges at step {step}");
    let actual_density = reduced(&snapshot.quantum, &ranges);
    counts.density_entries += assert_density(&actual_density, &reference.quantum.density, &format!("step {step}"));
    counts.states += 1;
}

/// The snapshot contains a purification. Cross terms survive only within the same assignment of
/// discarded qubits. The reference's matrix already has those dimensions traced out.
fn reduced(quantum: &QuantumSnapshot, keep: &[Range<u64>]) -> Density {
    let mut by_environment: BTreeMap<Vec<u64>, Vec<(Basis, C)>> = BTreeMap::new();
    for (configuration, amplitude) in &quantum.amplitudes {
        let mut discarded = configuration.clone();
        let live = keep
            .iter()
            .map(|range| {
                let mut value = RegisterValue::default();
                let mut packed = range.start;
                while packed < range.end {
                    let word = (packed / 64) as usize;
                    let offset = packed % 64;
                    let length = (64 - offset).min(range.end - packed);
                    let mask = u64::MAX >> (64 - length);
                    discarded[word] &= !(mask << offset);
                    let mut set = (configuration[word] >> offset) & mask;
                    while set != 0 {
                        value.flip((packed - range.start) as u32 + set.trailing_zeros());
                        set &= set - 1;
                    }
                    packed += length;
                }
                value
            })
            .collect();
        by_environment.entry(discarded).or_default().push((live, *amplitude));
    }
    let mut density = Density::new();
    for block in by_environment.values() {
        for (row, left) in block {
            for (col, right) in block {
                *density.entry((row.clone(), col.clone())).or_default() += left * right.conj();
            }
        }
    }
    density
}

/// The union includes every possibly nonzero entry. All omitted entries are zero on both sides.
pub fn assert_density(actual: &Density, expected: &Density, context: &str) -> usize {
    let entries: BTreeSet<_> = actual.keys().chain(expected.keys()).collect();
    for entry in &entries {
        let actual = actual.get(*entry).copied().unwrap_or_default();
        let expected = expected.get(*entry).copied().unwrap_or_default();
        assert!(
            (actual - expected).norm() <= TOLERANCE,
            "{context}: density entry {entry:?}: actual {actual:?}, expected {expected:?}"
        );
    }
    entries.len()
}
