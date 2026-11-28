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

#[derive(Clone)]
struct Register {
    wire: u64,
    qreg_slot: Option<u128>,
    sim_handle: u64,
    head: RegisterState,
}

/// The start wire of each allocation maps to the full range of bits it occupies in the snapshot.
type RegisterTable = BTreeMap<u64, Range<u64>>;

#[derive(Clone, Default)]
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

    fn allocate(&mut self, id: u64, width: Qubits, slot_value: u128, is_qreg: bool, snapshot: &Snapshot) {
        let quantum = &snapshot.quantum;
        let table = self.live_table(quantum);
        let changed = self.changed(&table);
        assert_eq!(changed.len(), 1, "qalloc adds one register range");
        let wire = changed[0];
        let bits = &table[&wire];
        assert!(!self.last_table.contains_key(&wire), "qalloc adds a fresh range");
        assert_eq!(bits.end - bits.start, u64::from(width.count()), "qalloc width");
        let head = quantum.registers[(bits.start / 64) as usize];
        assert_eq!(head.width, Width::new(width.count().min(64)), "qalloc head width");
        let (count, ref live) = snapshot.classical.handles;
        assert_eq!(count, id, "qalloc advances allocation count");
        let &(_, sim_handle, live_width) =
            live.iter().find(|&&(handle, _, _)| handle == id).expect("qalloc records live handle");
        assert_eq!(live_width, width.count(), "qalloc handle table width");
        let qreg_slot = if is_qreg {
            assert_ne!(slot_value, 0, "qalloc produces a handle");
            assert!(
                self.allocations.values().all(|allocation| allocation.qreg_slot != Some(slot_value)),
                "qalloc never reuses a qN handle"
            );
            Some(slot_value)
        } else {
            assert_eq!(slot_value, u128::from(id), "qalloc i64 returns allocation number");
            None
        };
        self.allocations.insert(id, Register { wire, qreg_slot, sim_handle, head });
        self.last_table = table;
    }

    fn release(&mut self, id: u64, allocation_count: u64, snapshot: &Snapshot) {
        let quantum = &snapshot.quantum;
        let table = self.live_table(quantum);
        let register = &self.allocations[&id];
        assert_eq!(self.changed(&table), [register.wire], "qfree changes the released range");
        assert!(!table.contains_key(&register.wire), "qfree removes live wires");
        let head = (self.last_table[&register.wire].start / 64) as usize;
        assert_ne!(quantum.registers.get(head), Some(&register.head), "qfree retires the handle");
        let (count, ref live) = snapshot.classical.handles;
        assert_eq!(count, allocation_count, "qfree keeps allocation count");
        assert!(live.iter().all(|&(handle, _, _)| handle != id), "qfree removes handle from table");
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
    run_until_stop_with_args(module, "main", &[], budget, after_steps, error)
}

pub fn run_until_stop_with_args<'m>(
    module: &'m ValidModule,
    name: &str,
    args: &[u64],
    budget: u64,
    after_steps: u64,
    error: RunError,
) -> Device<'m> {
    let run = run_through(module, name, args, 0, budget, Some((after_steps, error)));
    assert_eq!(run.reference.finish, None, "the stop is specific to the simulator");
    assert_eq!(run.reference.steps, after_steps);
    run.reference
}

pub fn run_and_reverse<'m>(module: &'m ValidModule, name: &str, args: &[u64], seed: u64, budget: u64) -> Run<'m> {
    run_reversible(module, name, args, seed, budget, None)
}

pub fn run_partial_undo_and_redo<'m>(
    module: &'m ValidModule,
    name: &str,
    args: &[u64],
    seed: u64,
    budget: u64,
    undo_steps: usize,
) -> Run<'m> {
    run_reversible(module, name, args, seed, budget, Some(undo_steps))
}

fn run_reversible<'m>(
    module: &'m ValidModule,
    name: &str,
    args: &[u64],
    seed: u64,
    budget: u64,
    partial_undo: Option<usize>,
) -> Run<'m> {
    let entry = module.function_named(name).expect("entry function");
    let reference_args: Vec<_> = args.iter().copied().map(u128::from).collect();
    let mut reference = Device::new(module, entry, &reference_args, budget);
    let options =
        Options { seed, budget: Some(budget), allow_region_faults: true, reversible: true, ..Options::default() };
    let mut executor = Executor::new(module, entry, args, options).expect("executor starts");
    let mut counts = Counts { programs: 1, ..Counts::default() };
    let mut registers = RegisterObservations::default();
    let mut history: Vec<(Device<'m>, RegisterObservations, Snapshot)> = Vec::new();

    assert!(reference.finish.is_none(), "reversible run expects a valid entry");
    let initial = executor.snapshot();
    checkpoint(&reference, &initial, &registers, &mut counts, true);
    history.push((reference.clone(), registers.clone(), initial));

    while reference.finish.is_none() {
        assert!(reference.next_measurement().is_none(), "reversible comparison requires measurement-free runs");
        let actual_finish = executor.step();
        match reference.step(None) {
            Event::Ordinary => {}
            Event::Allocated { id, dst, width } => {
                let snapshot = executor.snapshot();
                let frame = snapshot.classical.frames.last().expect("allocator's frame");
                let slot_value = snapshot.classical.slots[frame.base + dst.index()];
                let is_qreg = matches!(reference.slot_type(frame.func, dst), Type::Qreg(_));
                registers.allocate(id, width, slot_value, is_qreg, &snapshot);
            }
            Event::Released(id) => registers.release(id, reference.quantum.allocation_count, &executor.snapshot()),
            Event::Measured { .. } => panic!("unexpected measurement in reversible run"),
        }
        assert_eq!(actual_finish, reference.finish.map(outcome), "outcome after {} steps", reference.steps);
        let snapshot = executor.snapshot();
        let compare_frames = reference.finish.is_none() || matches!(reference.finish, Some(Finish::Faulted(_)));
        checkpoint(&reference, &snapshot, &registers, &mut counts, compare_frames);
        if reference.finish.is_none() {
            history.push((reference.clone(), registers.clone(), snapshot));
        }
    }
    assert_eq!(executor.outcome(), reference.finish.map(outcome), "completion");

    if let Some(undo_steps) = partial_undo {
        assert!(undo_steps > 0, "partial undo requires at least one step");
        let mut last_step = None;
        for step_index in 0..undo_steps {
            assert!(executor.step_back().expect("reversible"), "partial undo step {step_index}");
            let snapshot = executor.snapshot();
            let step = snapshot.classical.steps as usize;
            let (ref expected_ref, ref expected_regs, ref expected_snap) = history[step];
            checkpoint(expected_ref, &snapshot, expected_regs, &mut counts, true);
            assert_eq!(snapshot.classical, expected_snap.classical, "partial undo classical state at step {step}");
            let distance = snapshot.quantum.distance(&expected_snap.quantum);
            assert!(
                distance.is_some_and(|d| d <= TOLERANCE),
                "partial undo quantum state at step {step} differs by {distance:?}"
            );
            last_step = Some(step);
        }
        let resume_step = last_step.expect("resumed step");
        let (mut replay_ref, mut replay_regs, _) = history[resume_step].clone();
        while replay_ref.finish.is_none() {
            let actual_finish = executor.step();
            match replay_ref.step(None) {
                Event::Ordinary => {}
                Event::Allocated { id, dst, width } => {
                    let snapshot = executor.snapshot();
                    let frame = snapshot.classical.frames.last().expect("allocator's frame");
                    let slot_value = snapshot.classical.slots[frame.base + dst.index()];
                    let is_qreg = matches!(replay_ref.slot_type(frame.func, dst), Type::Qreg(_));
                    replay_regs.allocate(id, width, slot_value, is_qreg, &snapshot);
                }
                Event::Released(id) => {
                    replay_regs.release(id, replay_ref.quantum.allocation_count, &executor.snapshot());
                }
                Event::Measured { .. } => panic!("unexpected measurement in replay run"),
            }
            assert_eq!(
                actual_finish,
                replay_ref.finish.map(outcome),
                "replay outcome after {} steps",
                replay_ref.steps
            );
            let compare_frames = replay_ref.finish.is_none() || matches!(replay_ref.finish, Some(Finish::Faulted(_)));
            checkpoint(&replay_ref, &executor.snapshot(), &replay_regs, &mut counts, compare_frames);
        }
        assert_eq!(replay_ref.finish, reference.finish, "replay matches original finish");
        assert_density(&replay_ref.quantum.density, &reference.quantum.density, "replay density");
    }

    let mut undone = 0u64;
    let mut previous_step: Option<usize> = None;
    while executor.step_back().expect("reversible") {
        undone += 1;
        let snapshot = executor.snapshot();
        let step = snapshot.classical.steps as usize;
        if let Some(prev) = previous_step {
            assert_eq!(step + 1, prev, "step_back walks steps down by one");
        }
        previous_step = Some(step);
        let (ref expected_ref, ref expected_regs, ref expected_snap) = history[step];
        checkpoint(expected_ref, &snapshot, expected_regs, &mut counts, true);
        assert_eq!(snapshot.classical, expected_snap.classical, "reversed classical state at step {step}");
        let distance = snapshot.quantum.distance(&expected_snap.quantum);
        assert!(
            distance.is_some_and(|d| d <= TOLERANCE),
            "reversed quantum state at step {step} differs by {distance:?}"
        );
    }
    let at_entry = executor.snapshot();
    assert_eq!(at_entry.classical, history[0].2.classical, "full reversal restores entry classical state");
    let distance = at_entry.quantum.distance(&history[0].2.quantum);
    assert!(distance.is_some_and(|d| d <= TOLERANCE), "full reversal restores entry quantum state: {distance:?}");
    match reference.finish {
        Some(Finish::Returned(_)) => assert_eq!(undone, reference.steps, "all returned steps undone"),
        Some(Finish::Faulted(_)) => {
            assert!(
                undone == reference.steps || undone == reference.steps + 1,
                "faulted reversal undid {undone} steps for {} completed steps",
                reference.steps
            );
        }
        None => unreachable!(),
    }

    counts.operations = reference.steps;
    Run { reference, measurements: Vec::new(), counts }
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
                let slot_value = snapshot.classical.slots[frame.base + dst.index()];
                let is_qreg = matches!(reference.slot_type(frame.func, dst), Type::Qreg(_));
                registers.allocate(id, width, slot_value, is_qreg, &snapshot);
            }
            Event::Released(id) => registers.release(id, reference.quantum.allocation_count, &executor.snapshot()),
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
    let check_fault_frames = matches!(
        reference.finish,
        Some(Finish::Faulted(
            Fault::BadAngle
                | Fault::BadWidth
                | Fault::OutOfHandles
                | Fault::BadRange
                | Fault::DanglingRegister
                | Fault::QubitAlias
        ))
    );
    checkpoint(&reference, &executor.snapshot(), &registers, &mut counts, jump_fault || check_fault_frames);
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
    let expected_handles: Vec<_> = reference
        .quantum
        .live
        .iter()
        .map(|&(id, width)| (id, registers.allocations[&id].sim_handle, width.count()))
        .collect();
    assert_eq!(actual.handles.0, reference.quantum.allocation_count, "allocation count at step {step}");
    assert_eq!(actual.handles.1, expected_handles, "live handles at step {step}");
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
                    Type::Qreg(_) if value != 0 => registers.allocations
                        [&u64::try_from(value).expect("reference handle")]
                        .qreg_slot
                        .expect("qN slot value"),
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
