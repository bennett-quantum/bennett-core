//! Count the same oracle gate lists in isolation and in the C harness.

use bennett_emit::{ApplyInput, Argument, CountingDevice, Counts, compile_apply};
use bennett_exec::{BlockOp, Device, Executor, GateBlock, GateOp, Outcome, Wire};
use bennett_ir::{Fault, FixedGate, FuncId, Gate, ValidModule};

use super::{EXPONENT_BITS, INTERFACE, RESULT_BITS};

pub(super) fn cancel_adjacent_toffolis(block: &GateBlock) -> GateBlock {
    let mut reduced = Vec::with_capacity(block.ops().len());
    for op in block.ops() {
        if matches!(op, BlockOp::Gate(gate) if gate.kind == Gate::Fixed(FixedGate::X) && gate.controls.len() == 2)
            && reduced.last() == Some(op)
        {
            reduced.pop();
        } else {
            reduced.push(op.clone());
        }
    }
    GateBlock::from_ops(block.name(), reduced)
}

pub(super) fn isolated(block: &GateBlock) -> Counts {
    let mut counter = CountingDevice::default();
    counter.allocate(0..INTERFACE).expect("common interface");
    counter.call(block).expect("count the native gates");
    counter.counts()
}

// Each isolated block names the exponent, output, fault and scratch wires
// in order. The front can allocate those registers in a different order.
fn bind(block: &GateBlock, input: &ApplyInput<'_>) -> GateBlock {
    let [Argument::Register(exponent)] = input.args else {
        panic!("expected one exponent register");
    };
    assert_eq!(exponent.end - exponent.start, EXPONENT_BITS);
    assert_eq!(input.out.end - input.out.start, RESULT_BITS);
    assert_eq!(input.fault.end - input.fault.start, u64::from(Fault::CODE_WIDTH.bits()));
    let interface = exponent.clone().chain(input.out.clone()).chain(input.fault.clone()).collect::<Vec<_>>();
    let translate = |wire: Wire| {
        if wire < INTERFACE { interface[wire as usize] } else { input.scratch_start + (wire - INTERFACE) }
    };
    GateBlock::from_ops(
        block.name(),
        block.ops().iter().map(|op| match op {
            BlockOp::Allocate(wires) => BlockOp::Allocate(translate(wires.start)..translate(wires.end)),
            BlockOp::ReleaseZero(wires) => BlockOp::ReleaseZero(translate(wires.start)..translate(wires.end)),
            BlockOp::Gate(gate) => BlockOp::Gate(GateOp {
                kind: gate.kind,
                target: translate(gate.target),
                controls: gate.controls.iter().copied().map(translate).collect(),
            }),
        }),
    )
}

pub(super) fn harness_peak(module: &ValidModule, main: FuncId, callee: FuncId, oracle: &GateBlock) -> usize {
    let mut counter = CountingDevice::default();
    let mut oracle_calls = 0;
    let mut compiler = |input: ApplyInput<'_>| {
        if input.callee == callee {
            oracle_calls += 1;
            Ok(bind(oracle, &input))
        } else {
            Ok(compile_apply(input)?.block)
        }
    };
    let run = Executor::run_on_with_apply(module, main, &[], None, &mut counter, &mut compiler)
        .expect("start the full period finding harness");
    assert_eq!(run.outcome, Outcome::Returned(Some(0)), "synthetic measurements returned zero");
    assert_eq!(oracle_calls, 1);
    usize::try_from(counter.counts().peak_qubits).expect("a peak the host can index")
}
