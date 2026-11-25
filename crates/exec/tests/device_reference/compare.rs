//! Observe the executor around measurements, after non-local jump operations and at completion.

use std::collections::{BTreeMap, BTreeSet};

use bennett_exec::{Executor, Options, Outcome, QuantumSnapshot, RegisterState, Snapshot};
use bennett_ir::{Slot, Type, ValidModule, Width};
use num_complex::Complex64 as C;

use super::device_reference::{Basis, Density, Device, Event, Finish};

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
    column: usize,
    handle: u128,
    state: RegisterState,
}

type RegisterTable = BTreeMap<usize, RegisterState>;

#[derive(Default)]
struct RegisterObservations {
    allocations: BTreeMap<u64, Register>,
    last_table: RegisterTable,
    retired: RegisterTable,
}

impl RegisterObservations {
    fn live_table(&self, quantum: &QuantumSnapshot) -> RegisterTable {
        let cells: BTreeSet<_> = quantum.cells.iter().map(|&(_, column)| column).collect();
        quantum
            .registers
            .iter()
            .copied()
            .enumerate()
            .filter(|(column, state)| {
                state.width.is_some() && !cells.contains(column) && self.retired.get(column) != Some(state)
            })
            .collect()
    }

    fn changed(&self, next: &RegisterTable) -> Vec<usize> {
        self.last_table
            .keys()
            .chain(next.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|column| self.last_table.get(column) != next.get(column))
            .collect()
    }

    fn allocate(&mut self, id: u64, width: Width, handle: u128, quantum: &QuantumSnapshot) {
        let table = self.live_table(quantum);
        let changed = self.changed(&table);
        assert_eq!(changed.len(), 1, "qalloc changes a single register column");
        let column = changed[0];
        let state = table[&column];
        assert_eq!(state.width, Some(width), "qalloc width");
        assert_ne!(handle, 0, "qalloc produces a handle");
        assert!(
            self.allocations.values().all(|allocation| allocation.handle != handle),
            "qalloc never reuses a handle"
        );
        self.allocations.insert(id, Register { column, handle, state });
        self.last_table = table;
    }

    fn release(&mut self, id: u64, quantum: &QuantumSnapshot) {
        let mut table = self.live_table(quantum);
        let register = &self.allocations[&id];
        assert_eq!(self.changed(&table), [register.column], "qfree changes the released column");
        assert_ne!(table.get(&register.column), Some(&register.state), "qfree retires the handle");
        // A released register may still occupy a column in the snapshot's purification.
        if let Some(state) = table.remove(&register.column) {
            self.retired.insert(register.column, state);
        }
        self.last_table = table;
    }

    fn live_columns(&self, quantum: &QuantumSnapshot, live: &[(u64, Width)]) -> Vec<usize> {
        let table = self.live_table(quantum);
        let observed: Vec<_> = self
            .allocations
            .iter()
            .filter_map(|(&id, register)| {
                let state = table.get(&register.column)?;
                (state == &register.state).then(|| (id, state.width.expect("live width")))
            })
            .collect();
        assert_eq!(observed, live, "live registers and widths");
        assert_eq!(self.last_table, table, "only qalloc and qfree change live register columns");
        live.iter().map(|(id, _)| self.allocations[id].column).collect()
    }
}

pub fn run<'m>(module: &'m ValidModule, name: &str, args: &[u64], seed: u64, budget: u64) -> Run<'m> {
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
    checkpoint(&reference, &executor.snapshot(), &registers, &mut counts, jump_fault);
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
    let mut columns = registers.live_columns(&snapshot.quantum, &reference.quantum.live);
    let actual_cells: BTreeMap<_, _> = snapshot.quantum.cells.iter().copied().collect();
    assert_eq!(actual_cells.len(), snapshot.quantum.cells.len(), "duplicate cell address at step {step}");
    assert!(
        actual_cells.keys().eq(reference.quantum.cells.keys()),
        "cell addresses at step {step}: actual {actual_cells:?}, expected {:?}",
        reference.quantum.cells
    );
    columns.resize(columns.len() + actual_cells.len(), usize::MAX);
    for (&address, &column) in &reference.quantum.cells {
        let actual_column = actual_cells[&address];
        assert_eq!(snapshot.quantum.registers[actual_column].width, Some(Width::BYTE), "cell width at step {step}");
        columns[column] = actual_column;
    }
    assert_eq!(columns.iter().collect::<BTreeSet<_>>().len(), columns.len(), "distinct live columns at step {step}");
    let actual_density = reduced(&snapshot.quantum, &columns);
    counts.density_entries += assert_density(&actual_density, &reference.quantum.density, &format!("step {step}"));
    counts.states += 1;
}

/// The snapshot contains a purification. Cross terms survive only within the same assignment of
/// discarded qubits. The reference's matrix already has those dimensions traced out.
fn reduced(quantum: &QuantumSnapshot, keep: &[usize]) -> Density {
    let mut by_environment: BTreeMap<Basis, Vec<(Basis, C)>> = BTreeMap::new();
    for (configuration, amplitude) in &quantum.amplitudes {
        let live = keep.iter().map(|&col| configuration[col]).collect();
        let discarded = configuration
            .iter()
            .enumerate()
            .filter_map(|(col, &bits)| (!keep.contains(&col)).then_some(bits))
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
