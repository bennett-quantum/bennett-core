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
    let mut expected = vec![
        Command::Allocate(0..1),
        x(0, vec![]),
        Command::Allocate(64..72),
        x(64, vec![]),
        x(66, vec![]),
        x(65, vec![0]),
    ];
    expected.extend((64..66).chain(64..72).map(Command::Measure));
    expected.extend([
        x(65, vec![]),
        x(71, vec![]),
        Command::ReleaseZero(64..72),
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
        assert_eq!(device.snapshot().amplitudes.len(), 1, "the measurement keeps one configuration");
    }
    assert_eq!(seen, [true, true]);
    let mut device = SimulatedDevice::new(0, None);
    device.allocate(0..1).expect("allocate");
    device.gate(GateOp { kind: Gate::Fixed(FixedGate::X), target: 0, controls: &[] }).expect("set the wire");
    assert_eq!(device.release_zero(0..1), Err(RunError::Device("release_zero requires zero wires")));
}
