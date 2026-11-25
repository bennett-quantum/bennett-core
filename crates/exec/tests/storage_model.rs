//! A model test of the simulator's qubit storage (`docs/DESIGN.md`, "Running a program"): seeded random
//! sequences of allocations, releases and allocations again, at the two grains its clients use, compared
//! after every command with a model that is correct by inspection.
//!
//! - The device interface (`SimulatedDevice`, which the OpenQASM writer drives): wire ranges, single
//!   wires and empty ranges, near wire 0 and at the top of the wire numbering. The model is the set of
//!   live wires, each with its value: only X gates act, so the state is always a basis state, and a wire
//!   released in zero and allocated again holds 0.
//! - The controller: `qalloc`, `x` (controlled or not) and `qfree` of whole registers and `x` on memory
//!   bytes, in a generated program run step by step, stepped back and run forward again. The model is the
//!   live registers and bytes holding qubits with their values, and the qubits kept by releases that were
//!   not zero.
//!
//! The default run is a few hundred sequences; the ignored test runs many more.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

use bennett_exec::{Device, GateOp, Options, RunError, SimulatedDevice, Snapshot, Wire};
use bennett_ir::{FixedGate, Gate};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const NOT_ALLOCATED: RunError = RunError::Device("wire is not allocated");
const ALREADY_ALLOCATED: RunError = RunError::Device("wire is already allocated");
const NOT_ZERO: RunError = RunError::Device("release_zero requires zero wires");

/// The wires the device commands name: a stretch from 0 across several 64-wire columns, and the top of the
/// wire numbering (the last wire is `Wire::MAX - 1`: a range's end is exclusive).
const ZONES: [Range<Wire>; 2] = [0..300, Wire::MAX - 300..Wire::MAX];

#[derive(Clone, Debug)]
enum Command {
    Allocate(Range<Wire>),
    ReleaseZero(Range<Wire>),
    Discard(Range<Wire>),
    X(Wire),
    Measure(Vec<Range<Wire>>),
}

/// The live wires and their values.
#[derive(Default)]
struct WireModel {
    live: BTreeMap<Wire, bool>,
}

impl WireModel {
    fn all_live(&self, wires: &Range<Wire>) -> bool {
        wires.clone().all(|wire| self.live.contains_key(&wire))
    }

    /// What the device must answer to `command`, applied to the model.
    fn apply(&mut self, command: &Command) -> Result<Vec<u64>, RunError> {
        match command {
            Command::Allocate(wires) => {
                if wires.clone().any(|wire| self.live.contains_key(&wire)) {
                    return Err(ALREADY_ALLOCATED);
                }
                self.live.extend(wires.clone().map(|wire| (wire, false)));
            }
            Command::ReleaseZero(wires) => {
                if !self.all_live(wires) {
                    return Err(NOT_ALLOCATED);
                }
                if wires.clone().any(|wire| self.live[&wire]) {
                    return Err(NOT_ZERO);
                }
                self.live.retain(|wire, _| !wires.contains(wire));
            }
            Command::Discard(wires) => {
                if !self.all_live(wires) {
                    return Err(NOT_ALLOCATED);
                }
                self.live.retain(|wire, _| !wires.contains(wire));
            }
            Command::X(wire) => {
                let value = self.live.get_mut(wire).ok_or(NOT_ALLOCATED)?;
                *value = !*value;
            }
            Command::Measure(ranges) => {
                if !ranges.iter().all(|range| self.all_live(range)) {
                    return Err(NOT_ALLOCATED);
                }
                return Ok(ranges.iter().map(|range| self.word(range)).collect());
            }
        }
        Ok(Vec::new())
    }

    fn word(&self, range: &Range<Wire>) -> u64 {
        range.clone().enumerate().fold(0, |word, (bit, wire)| word | u64::from(self.live[&wire]) << bit)
    }

    /// The live wires in runs of consecutive wires, at most 64 each: one measurement of all of them.
    fn live_ranges(&self) -> Vec<Range<Wire>> {
        let mut ranges: Vec<Range<Wire>> = Vec::new();
        for &wire in self.live.keys() {
            match ranges.last_mut() {
                Some(last) if last.end == wire && last.end - last.start < 64 => last.end += 1,
                _ => ranges.push(wire..wire + 1),
            }
        }
        ranges
    }
}

fn send(device: &mut SimulatedDevice, command: &Command) -> Result<Vec<u64>, RunError> {
    match command {
        Command::Allocate(wires) => device.allocate(wires.clone()).map(|()| Vec::new()),
        Command::ReleaseZero(wires) => device.release_zero(wires.clone()).map(|()| Vec::new()),
        Command::Discard(wires) => device.discard(wires.clone()).map(|()| Vec::new()),
        Command::X(target) => {
            device.gate(GateOp { kind: Gate::Fixed(FixedGate::X), target: *target, controls: &[] }).map(|()| Vec::new())
        }
        Command::Measure(ranges) => device.measure_many(ranges),
    }
}

/// A wire of a zone: a live one most of the time, so that commands meet the storage's live state.
fn pick_wire(rng: &mut ChaCha8Rng, model: &WireModel, zone: &Range<Wire>) -> Wire {
    let live: Vec<Wire> = model.live.range(zone.clone()).map(|(&wire, _)| wire).collect();
    if !live.is_empty() && rng.random_bool(0.75) {
        live[rng.random_range(0..live.len())]
    } else {
        rng.random_range(zone.clone())
    }
}

/// A range of a zone, empty one time in eight, often short, sometimes across columns.
fn pick_range(rng: &mut ChaCha8Rng, model: &WireModel, zone: &Range<Wire>) -> Range<Wire> {
    let start = pick_wire(rng, model, zone);
    let length = match rng.random_range(0..8) {
        0 => 0,
        1..=4 => rng.random_range(1..=3),
        5 | 6 => rng.random_range(4..=70),
        _ => rng.random_range(60..=140),
    };
    start..start.saturating_add(length).min(zone.end)
}

fn pick_command(rng: &mut ChaCha8Rng, model: &WireModel) -> Command {
    let zone = &ZONES[usize::from(rng.random_bool(0.2))];
    match rng.random_range(0..20) {
        0..=5 => Command::Allocate(pick_range(rng, model, zone)),
        6..=9 => Command::ReleaseZero(pick_range(rng, model, zone)),
        10..=12 => Command::Discard(pick_range(rng, model, zone)),
        13..=17 => Command::X(pick_wire(rng, model, zone)),
        _ => {
            let range = pick_range(rng, model, zone);
            let range = range.start..range.end.min(range.start.saturating_add(64)).max(range.start + 1);
            Command::Measure(vec![range])
        }
    }
}

/// Checks the device against the model: every live wire measures its value, and a few wires that are not
/// live are refused.
fn compare(device: &mut SimulatedDevice, model: &WireModel, rng: &mut ChaCha8Rng) -> Result<(), String> {
    let ranges = model.live_ranges();
    if !ranges.is_empty() {
        let expected: Vec<u64> = ranges.iter().map(|range| model.word(range)).collect();
        let measured = device.measure_many(&ranges);
        if measured != Ok(expected.clone()) {
            return Err(format!("live wires {ranges:?} measure {measured:?}, model {expected:?}"));
        }
    }
    for zone in &ZONES {
        let wire = rng.random_range(zone.clone());
        if !model.live.contains_key(&wire) && device.measure(wire) != Err(NOT_ALLOCATED) {
            return Err(format!("wire {wire} is not live but measures"));
        }
    }
    Ok(())
}

/// Runs `commands` on a fresh device: the index of the first command whose answer or after-state differs
/// from the model, and how (a panic counts).
fn first_difference(commands: &[Command]) -> Option<(usize, String)> {
    let mut device = SimulatedDevice::new(0, None);
    let mut model = WireModel::default();
    let mut rng = ChaCha8Rng::seed_from_u64(0);
    for (index, command) in commands.iter().enumerate() {
        let expected = model.apply(command);
        let step = catch_unwind(AssertUnwindSafe(|| {
            let answer = send(&mut device, command);
            if answer != expected {
                return Err(format!("device {answer:?}, model {expected:?}"));
            }
            compare(&mut device, &model, &mut rng)
        }));
        match step {
            Ok(Ok(())) => {}
            Ok(Err(difference)) => return Some((index, difference)),
            Err(panic) => {
                let text =
                    panic.downcast_ref::<&str>().map(|s| (*s).to_owned()).or(panic.downcast_ref::<String>().cloned());
                return Some((index, format!("panic: {}", text.unwrap_or_default())));
            }
        }
    }
    None
}

/// The commands of `seed`.
fn device_sequence(seed: u64, length: usize) -> Vec<Command> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut model = WireModel::default();
    (0..length)
        .map(|_| {
            let command = pick_command(&mut rng, &model);
            // The model follows the commands so that later ones are drawn against the live wires.
            let _ = model.apply(&command);
            command
        })
        .collect()
}

/// Drops commands from a failing sequence while it still fails, for a short report.
fn shrink(mut commands: Vec<Command>) -> Vec<Command> {
    let mut index = 0;
    while index < commands.len() {
        let mut fewer = commands.clone();
        fewer.remove(index);
        if first_difference(&fewer).is_some() {
            commands = fewer;
        } else {
            index += 1;
        }
    }
    commands
}

fn check_device_sequences(seeds: Range<u64>, length: usize) {
    for seed in seeds {
        let commands = device_sequence(seed, length);
        if let Some((index, difference)) = first_difference(&commands) {
            let default_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let short = shrink(commands[..=index].to_vec());
            std::panic::set_hook(default_hook);
            let (_, short_difference) = first_difference(&short).unwrap_or_default();
            panic!(
                "seed {seed}, command {index} ({:?}): {difference}\nshortest failing: {short:?}: {short_difference}",
                commands[index]
            );
        }
    }
}

#[test]
fn device_commands_agree_with_the_set_of_live_wires() {
    check_device_sequences(0..200, 120);
}

#[test]
#[ignore = "a longer run of the same model test (about a minute); run with --include-ignored"]
fn device_commands_agree_with_the_set_of_live_wires_long() {
    check_device_sequences(1000..6000, 400);
}

/// The two defects this test was written for, as the sequences the review found them by: a release across
/// wires that a release in zero and an allocation again left in different columns, and an empty allocation
/// inside a live range.
#[test]
fn the_reviewed_sequences_agree_with_the_model() {
    for release in [Command::Discard(0..2), Command::ReleaseZero(0..2)] {
        let commands = [Command::Allocate(0..2), Command::ReleaseZero(1..2), Command::Allocate(1..2), release];
        assert_eq!(first_difference(&commands), None, "{commands:?}");
    }
    let commands = [Command::Allocate(0..2), Command::Allocate(1..1)];
    assert_eq!(first_difference(&commands), None, "{commands:?}");
}

/// A register of the generated program: its slot number, width and set bits.
struct Register {
    slot: usize,
    width: u32,
    set: BTreeSet<u32>,
}

/// The controller's model: live registers and bytes holding qubits with their values, qubits kept.
#[derive(Default)]
struct RegisterModel {
    registers: Vec<Register>,
    cells: BTreeMap<u64, u8>,
    kept: u32,
}

impl RegisterModel {
    fn held(&self) -> u32 {
        self.registers.iter().map(|register| register.width).sum::<u32>() + 8 * self.cells.len() as u32 + self.kept
    }
}

/// Bytes the generated programs turn into qubits, and where probes store what they measure.
const CELLS: Range<u64> = 1000..1006;
const PROBES: u64 = 2000;

/// A generated program: its text, the qubits held and kept after each step, and memory bytes expected.
type Program = (String, Vec<(u32, u32)>, Vec<(u64, u8)>);

fn pick_width(rng: &mut ChaCha8Rng) -> u32 {
    match rng.random_range(0..16) {
        0..=4 => rng.random_range(1..=8),
        5..=8 => rng.random_range(60..=70),
        9..=11 => rng.random_range(120..=136),
        12..=14 => rng.random_range(190..=260),
        _ => rng.random_range(1000..=1100),
    }
}

/// A program of `length` steps of `qalloc`, `x` and `qfree`, then probes that copy chosen bits of every live
/// register into fresh qubits, measure them and store them at `PROBES` on, and measure every byte holding
/// qubits in place. Returns the program, the model's qubits held and kept after each step, and the
/// expected memory bytes.
fn register_program(seed: u64, length: usize) -> Program {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut model = RegisterModel::default();
    let mut text = String::from("memory 4096\nfunc @main() {\nentry:\n");
    let mut counts = Vec::new();
    let mut slots = 0;
    for _ in 0..length {
        let choice = rng.random_range(0..12);
        if model.registers.is_empty() || choice < 3 || (choice < 5 && model.registers.len() < 3) {
            let width = pick_width(&mut rng);
            let _ = writeln!(text, "    %r{slots} = qalloc {width}");
            model.registers.push(Register { slot: slots, width, set: BTreeSet::new() });
            slots += 1;
        } else if choice < 7 {
            let index = rng.random_range(0..model.registers.len());
            let register = model.registers.remove(index);
            let _ = writeln!(text, "    qfree %r{}", register.slot);
            if !register.set.is_empty() {
                model.kept += register.width;
            }
        } else if choice < 9 {
            let address = rng.random_range(CELLS);
            let bit = rng.random_range(0..8u8);
            let _ = writeln!(text, "    x [{address}:{bit}]");
            *model.cells.entry(address).or_default() ^= 1 << bit;
        } else {
            let target = rng.random_range(0..model.registers.len());
            let bit = rng.random_range(0..model.registers[target].width);
            let control = rng.random_range(0..model.registers.len());
            let control_bit = rng.random_range(0..model.registers[control].width);
            let (slot, control_slot) = (model.registers[target].slot, model.registers[control].slot);
            if choice == 9 || (target, bit) == (control, control_bit) {
                let _ = writeln!(text, "    x %r{slot}[{bit}]");
                flip(&mut model.registers[target].set, bit);
            } else {
                let _ = writeln!(text, "    x %r{slot}[{bit}] ctrl %r{control_slot}[{control_bit}]");
                if model.registers[control].set.contains(&control_bit) {
                    flip(&mut model.registers[target].set, bit);
                }
            }
        }
        counts.push((model.held(), model.kept));
    }
    let mut expected = Vec::new();
    let mut probe = 0;
    for register in &model.registers {
        let mut bits: BTreeSet<u32> = [0, 1, 62, 63, 64, 65, register.width.saturating_sub(2), register.width - 1]
            .into_iter()
            .filter(|&bit| bit < register.width)
            .collect();
        bits.extend(register.set.iter().copied());
        bits.extend((0..4).map(|_| rng.random_range(0..register.width)));
        for bit in bits {
            let address = PROBES + probe;
            let _ = write!(
                text,
                "    %p{probe} = qalloc 1\n    x %p{probe}[0] ctrl %r{}[{bit}]\n    %m{probe} = measure %p{probe}\n    \
                 %z{probe} = zext i1 %m{probe} to i8\n    store i8 %z{probe}, {address}\n",
                register.slot
            );
            expected.push((address, u8::from(register.set.contains(&bit))));
            probe += 1;
        }
    }
    for (&address, &value) in &model.cells {
        let _ = writeln!(text, "    %c{address} = measure i8 [{address}]");
        expected.push((address, value));
    }
    text.push_str("    ret\n}\n");
    (text, counts, expected)
}

fn flip(set: &mut BTreeSet<u32>, bit: u32) {
    if !set.remove(&bit) {
        set.insert(bit);
    }
}

fn check_register_programs(seeds: Range<u64>, length: usize) {
    for seed in seeds {
        let (text, counts, expected) = register_program(seed, length);
        let module = common::load(&text);
        let options = Options { reversible: true, ..common::bounded() };
        let mut executor = common::executor(&module, "main", &[], options);
        let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x5eed);
        let mut snapshots: Vec<Snapshot> = vec![executor.snapshot()];
        let check = |executor: &bennett_exec::Executor<'_>, step: usize| {
            let stats = executor.stats();
            let (held, kept) = if step == 0 { (0, 0) } else { counts[step - 1] };
            assert_eq!(
                (stats.live_qubits, stats.leftover_qubits),
                (held, kept),
                "seed {seed}, after step {step}\n{text}"
            );
        };
        while snapshots.len() <= length {
            assert_eq!(executor.step(), None, "seed {seed}\n{text}");
            snapshots.push(executor.snapshot());
            check(&executor, snapshots.len() - 1);
            // Now and then step back a few steps, compare with the states passed, and run forward again.
            if rng.random_bool(0.2) {
                let back = rng.random_range(1..snapshots.len());
                for step in (snapshots.len() - 1 - back..snapshots.len() - 1).rev() {
                    assert_eq!(executor.step_back(), Ok(true), "seed {seed}\n{text}");
                    common::assert_same_state(
                        &executor.snapshot(),
                        &snapshots[step],
                        &format!("seed {seed}, back to {step}"),
                    );
                    check(&executor, step);
                }
                let from = snapshots.len() - back;
                for (step, expected) in snapshots.iter().enumerate().skip(from) {
                    assert_eq!(executor.step(), None, "seed {seed}\n{text}");
                    common::assert_same_state(&executor.snapshot(), expected, &format!("seed {seed}, again {step}"));
                }
            }
        }
        assert_eq!(executor.run(), bennett_exec::Outcome::Returned(None), "seed {seed}\n{text}");
        let memory = executor.snapshot().classical.memory;
        for (address, value) in expected {
            assert_eq!(memory[address as usize], value, "seed {seed}, byte {address}\n{text}");
        }
    }
}

#[test]
fn registers_agree_with_the_model_forward_and_back() {
    check_register_programs(0..40, 40);
}

#[test]
#[ignore = "a longer run of the same model test; run with --include-ignored"]
fn registers_agree_with_the_model_forward_and_back_long() {
    check_register_programs(1000..1400, 80);
}
