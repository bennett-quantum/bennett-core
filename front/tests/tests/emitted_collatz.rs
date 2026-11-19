//! Compile plain C through the front and check the emitted Collatz gates.

#[allow(dead_code)] // The general runner also provides sampling and destination exhaustion.
#[path = "../../../crates/emit/tests/support/bits.rs"]
mod bits;

use std::collections::BTreeSet;

use bennett_emit::{ApplyInput, Argument, CompiledApply, Construction, CountingDevice, compile_apply};
use bennett_exec::{BlockOp, Device, Executor, Options, Outcome};
use bennett_front_tests::{CLANG_FLAGS, STACK_BYTES, c_dir, entry, lower};
use bennett_ir::{Fault, FuncId, Inst, ValidModule};

const INPUT_BITS: usize = 8;
const INPUTS: u64 = 1 << INPUT_BITS;
const TARGET_BITS: usize = 8 + Fault::CODE_WIDTH.bits() as usize;
const INTERFACE: u64 = (INPUT_BITS + TARGET_BITS) as u64;
const REFERENCE_BUDGET: u64 = 2_000; // More than the longest run on an unsigned byte.

fn front_collatz(name: &str) -> (ValidModule, FuncId) {
    let module = lower(&c_dir().join("collatz_emitted.c"), name, CLANG_FLAGS, &[]);
    let entry = module.function(entry(&module, "entry"));
    let callee = entry
        .blocks
        .iter()
        .flat_map(|block| &block.insts)
        .find_map(|inst| {
            let Inst::Apply { callee, .. } = inst else { return None };
            Some(*callee)
        })
        .expect("Collatz is an applied call");
    (module, callee)
}

fn reference(module: &ValidModule, callee: FuncId, x: u64, budget: u64) -> (u64, u128) {
    let options = Options { budget: Some(budget), ..Options::default() };
    let mut run = Executor::new(module, callee, &[x], options).expect("start the applied function on definite input");
    let output = match run.run() {
        Outcome::Returned(Some(value)) => u128::from(value),
        Outcome::Faulted(Fault::Budget) => u128::from(Fault::Budget.code()) << 8,
        outcome => panic!("unexpected Collatz outcome for x={x}, B={budget}: {outcome:?}"),
    };
    (run.stats().steps, output)
}

fn check_budget(module: &ValidModule, callee: FuncId, budget: u64) -> CompiledApply {
    let compiled = compile_apply(ApplyInput {
        module,
        callee,
        args: &[Argument::Register(0..INPUT_BITS as u64)],
        out: INPUT_BITS as u64..16,
        fault: 16..INTERFACE,
        budget,
        entry_fault: None,
        stack_available: STACK_BYTES,
        scratch_start: INTERFACE,
    })
    .expect("emit Collatz");
    assert!(!matches!(compiled.construction, Construction::StepMachine { .. }), "B={budget}: {}", compiled.reason);
    let [BlockOp::Allocate(scratch), gates @ .., BlockOp::ReleaseZero(released)] = compiled.block.ops() else {
        panic!("expected a single scratch lifetime");
    };
    assert_eq!(scratch.start, INTERFACE);
    assert_eq!(scratch, released);
    let gates = gates
        .iter()
        .map(|op| match op {
            BlockOp::Gate(gate) => gate.clone(),
            BlockOp::Allocate(_) | BlockOp::ReleaseZero(_) => panic!("unexpected scratch lifetime"),
        })
        .collect::<Vec<_>>();
    let expected = (0..INPUTS).map(|x| reference(module, callee, x, budget).1).collect::<Vec<_>>();
    let layout = bits::Layout { x: INPUT_BITS, y: TARGET_BITS, scratch: (scratch.end - scratch.start) as usize };
    let states = (0..u128::from(INPUTS)).flat_map(|x| [(x, 0), (x, 0x15a5)]);
    bits::check_states(&gates, layout, states, |x| expected[x as usize])
        .unwrap_or_else(|error| panic!("B={budget}: {error}"));
    compiled
}

#[test]
fn c_collatz_uses_saved_decisions() {
    let (module, callee) = front_collatz("collatz_emitted");
    let (steps, _) = reference(&module, callee, 2, REFERENCE_BUDGET);
    for budget in [steps - 1, steps, steps + 1] {
        let compiled = check_budget(&module, callee, budget);
        assert!(matches!(compiled.construction, Construction::SavedDecisions { .. }));
    }
}

#[test]
#[ignore = "sweeps all distinct return budgets over all 256 inputs"]
fn c_collatz_budget_sweep_and_counts() {
    let (module, callee) = front_collatz("collatz_sweep");
    let returns = (0..INPUTS).map(|x| reference(&module, callee, x, REFERENCE_BUDGET)).collect::<Vec<_>>();
    let longest = returns.iter().map(|&(steps, _)| steps).max().expect("input domain");
    let budgets =
        returns.iter().flat_map(|&(steps, _)| [steps - 1, steps, steps + 1]).chain([0]).collect::<BTreeSet<_>>();
    println!("{} budgets, {INPUTS} inputs, two destination words per input", budgets.len());
    for budget in budgets {
        let compiled = check_budget(&module, callee, budget);
        if budget == longest {
            let mut counter = CountingDevice::default();
            counter.allocate(0..INTERFACE).expect("interface");
            compiled.send(&mut counter).expect("native gates");
            println!("B={budget}, {}: {:?}", compiled.construction, counter.counts());
        }
    }
}
