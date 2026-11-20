//! An external device receives the commands for register and memory instructions.

use std::collections::VecDeque;
use std::ops::Range;

use bennett_exec::{Device, Executor, GateOp, Options, Outcome, RunError, SimulatedDevice, StartError, Wire};
use bennett_ir::{Fault, FixedGate, Gate};

#[derive(Debug, PartialEq)]
enum Command {
    Allocate(Range<Wire>),
    Gate(GateOp),
    Measure(Wire),
    Discard(Range<Wire>),
    ReleaseZero(Range<Wire>),
}

#[derive(Default)]
struct Recorder {
    commands: Vec<Command>,
    bits: VecDeque<bool>,
}

impl Device for Recorder {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.commands.push(Command::Allocate(wires));
        Ok(())
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        self.commands.push(Command::Gate(GateOp {
            kind: gate.kind,
            target: gate.target,
            controls: gate.controls.to_vec(),
        }));
        Ok(())
    }

    fn measure(&mut self, wire: Wire) -> Result<bool, RunError> {
        self.commands.push(Command::Measure(wire));
        self.bits.pop_front().ok_or(RunError::Device("measurement source exhausted"))
    }

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.commands.push(Command::Discard(wires));
        Ok(())
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.commands.push(Command::ReleaseZero(wires));
        Ok(())
    }
}

#[test]
fn register_and_memory_commands_observe_measurement_and_release() {
    let module = bennett_ir::load(
        "memory 128
        data 0 = i8 5
        func @main() -> i8 {
        entry:
            %q = qalloc 1
            x %q
            x [0:1] ctrl %q
            %low = measure i2 [0]
            %byte = measure i8 [0]
            %m = measure %q
            qfree %q
            br %m, yes, no
        yes:
            %loaded = load i8 0
            ret %loaded
        no:
            ret 0
        }",
    )
    .expect("valid program");
    let mut device = Recorder {
        bits: [false, true, false, true, false, false, false, false, false, true, true].into(),
        ..Recorder::default()
    };
    let run = Executor::run_on(&module, module.function_named("main").expect("entry"), &[], None, &mut device)
        .expect("started");
    assert_eq!(run.outcome, Outcome::Returned(Some(130)));
    assert_eq!(run.steps, 10);
    assert!(device.bits.is_empty());

    let x = |target, controls| Command::Gate(GateOp { kind: Gate::Fixed(FixedGate::X), target, controls });
    // Each register id names its own range of 2^32 wires; the cell is register 1.
    let cell = 1 << 32;
    let mut expected = vec![
        Command::Allocate(0..1),
        x(0, vec![]),
        Command::Allocate(cell..cell + 8),
        x(cell, vec![]),
        x(cell + 2, vec![]),
        x(cell + 1, vec![0]),
    ];
    expected.extend((cell..cell + 2).chain(cell..cell + 8).map(Command::Measure));
    expected.extend([
        x(cell + 1, vec![]),
        x(cell + 7, vec![]),
        Command::ReleaseZero(cell..cell + 8),
        Command::Measure(0),
        Command::Discard(0..1),
    ]);
    assert_eq!(device.commands, expected);
}

#[test]
fn faults_and_device_stops_end_the_run_before_the_next_command() {
    let module = bennett_ir::load(
        "memory 128
        func @main() {
        entry:
            %q = qalloc 1
            qfree %q
            x %q
            ret
        }
        func @measure() {
        entry:
            %q = qalloc 1
            %bit = measure %q
            x %q
            ret
        }",
    )
    .expect("valid program");
    let mut device = Recorder::default();
    let run = Executor::run_on(&module, module.function_named("main").expect("entry"), &[], None, &mut device)
        .expect("started");
    assert_eq!((run.outcome, run.steps), (Outcome::Faulted(Fault::DanglingRegister), 2));
    assert_eq!(device.commands, [Command::Allocate(0..1), Command::Discard(0..1)]);

    let mut device = Recorder::default();
    let run = Executor::run_on(&module, module.function_named("measure").expect("entry"), &[], None, &mut device)
        .expect("started");
    assert_eq!((run.outcome, run.steps), (Outcome::Stopped(RunError::Device("measurement source exhausted")), 1));
    assert_eq!(device.commands, [Command::Allocate(0..1), Command::Measure(0)]);
}

#[test]
fn a_module_with_apply_is_refused_before_any_device_commands() {
    let module = bennett_ir::load(include_str!("../../../examples/thesis.bn")).expect("valid example");
    let mut device = Recorder::default();
    let error = Executor::run_on(&module, module.function_named("main").expect("entry"), &[], None, &mut device)
        .expect_err("apply needs lowering");
    assert_eq!(error, StartError::ApplyNotLowered);
    assert!(device.commands.is_empty());
}

#[test]
fn the_default_device_commands_expand_a_named_block() {
    use bennett_exec::{BlockOp, GateBlock};

    let mut device = Recorder::default();
    device.allocate(0..2).expect("input wires");
    let copy = |target, control| GateOp { kind: Gate::Fixed(FixedGate::X), target, controls: vec![control] };
    let block = GateBlock::from_ops(
        "copy through scratch",
        [
            BlockOp::Allocate(2..3),
            BlockOp::Gate(copy(2, 0)),
            BlockOp::Gate(copy(1, 2)),
            BlockOp::Gate(copy(2, 0)),
            BlockOp::ReleaseZero(2..3),
        ],
    );
    device.call(&block).expect("call");
    device.repeat(&block, 2).expect("two more calls");
    let one_call = [
        Command::Allocate(2..3),
        Command::Gate(copy(2, 0)),
        Command::Gate(copy(1, 2)),
        Command::Gate(copy(2, 0)),
        Command::ReleaseZero(2..3),
    ];
    assert_eq!(device.commands.len(), 16);
    for commands in device.commands[1..].chunks_exact(one_call.len()) {
        assert_eq!(commands, one_call);
    }
}

// The simulator behind the device interface samples a run's measurements as the simulator's own run does
// with the same seed: a three-qubit GHZ state measured as a register reads 0 or 7, never a mixed value,
// both values occur over the seeds, and each seed gives the simulator's own result. A qubit left in one
// is not released in zero.
#[test]
fn the_simulated_device_samples_like_the_simulator() {
    let module = bennett_ir::load(
        "memory 64
        func @main() -> i3 {
        entry:
            %q = qalloc 3
            h %q[0]
            x %q[1] ctrl %q[0]
            x %q[2] ctrl %q[1]
            %m = measure %q
            ret %m
        }",
    )
    .expect("valid module");
    let main = module.function_named("main").expect("main");
    let mut seen = [false; 2];
    for seed in 0..16 {
        let mut device = SimulatedDevice::new(seed, None);
        let run = Executor::run_on(&module, main, &[], None, &mut device).expect("start");
        let own = Executor::new(&module, main, &[], Options { seed, ..Options::default() }).expect("start").run();
        assert_eq!(run.outcome, own, "seed {seed}");
        let Outcome::Returned(Some(value)) = run.outcome else { panic!("{:?}", run.outcome) };
        assert!(value == 0 || value == 7, "seed {seed}: {value}");
        seen[usize::from(value == 7)] = true;
        assert_eq!(device.configurations(), 1, "the measurement keeps one configuration");
    }
    assert_eq!(seen, [true, true]);
    let mut device = SimulatedDevice::new(0, None);
    device.allocate(0..1).expect("allocate");
    device.gate(GateOp { kind: Gate::Fixed(FixedGate::X), target: 0, controls: &[] }).expect("set the wire");
    assert_eq!(device.release_zero(0..1), Err(RunError::Device("release_zero requires zero wires")));
}

// Releasing one wire of two leaves the other where it was: wire 1, set to one, still measures one.
#[test]
fn a_partial_release_leaves_the_other_wire_unchanged() {
    let mut device = SimulatedDevice::new(0, None);
    device.allocate(0..2).expect("allocate");
    device.gate(GateOp { kind: Gate::Fixed(FixedGate::X), target: 1, controls: &[] }).expect("set wire 1");
    device.release_zero(0..1).expect("wire 0 is zero");
    assert_eq!(device.configurations(), 1);
    assert_eq!(device.measure_many(std::slice::from_ref(&(1..2))), Ok(vec![1]));
    assert_eq!(device.measure(1), Ok(true));
}

// The gate lowering takes scratch wires from just below 2^64 (`crates/emit/src/lower.rs`), and the OpenQASM
// writer forwards them to its simulation: wires there are allocated, checked and released like any other.
#[test]
fn wires_just_below_the_top_of_the_wire_space_are_ordinary() {
    let mut device = SimulatedDevice::new(0, None);
    let top = Wire::MAX - 3..Wire::MAX;
    device.allocate(top.clone()).expect("allocate the top wires");
    assert_eq!(device.allocate(Wire::MAX - 1..Wire::MAX), Err(RunError::Device("wire is already allocated")));
    device.gate(GateOp { kind: Gate::Fixed(FixedGate::X), target: Wire::MAX - 1, controls: &[] }).expect("set one");
    assert_eq!(device.measure_many(std::slice::from_ref(&top)), Ok(vec![0b100]));
    device.gate(GateOp { kind: Gate::Fixed(FixedGate::X), target: Wire::MAX - 1, controls: &[] }).expect("clear it");
    device.release_zero(top.clone()).expect("the top wires are zero");
    device.allocate(top).expect("released wires can be allocated again");
}
