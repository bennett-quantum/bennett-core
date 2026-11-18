//! An external device receives the commands for register and memory instructions.

use std::collections::VecDeque;
use std::ops::Range;

use bennett_exec::{Device, Executor, GateOp, Outcome, RunError, StartError, Wire};
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
