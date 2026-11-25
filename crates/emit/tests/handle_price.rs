//! Compare register and private-block argument passing with today's device IR.

#[allow(dead_code)]
#[path = "support/bits.rs"]
mod bits;
#[allow(dead_code)]
#[path = "../../../front/tests/tests/shor_support/count.rs"]
mod count;

use bennett_emit::{ApplyInput, ApplyMemory, ApplyMode, Argument, compile_apply_with};
use bennett_exec::{BlockOp, Executor, GateBlock, Options, Outcome};
use bennett_ir::{Const, Fault, FuncId, Inst, Operand, ValidModule, Width, load, validate};

const EXPONENT_BITS: u64 = 8;
const RESULT_BITS: u64 = 8;
const FAULT_START: u64 = EXPONENT_BITS + RESULT_BITS;
const INTERFACE: u64 = FAULT_START + Fault::CODE_WIDTH.bits() as u64;
const BUDGET: u64 = 96;

const SHOR: &str = include_str!("fixtures/shor_powmod_15.bn");
const SMALL: &str = r#"
memory 65536
func @identity(i8 %x) -> i8 {
entry:
    ret %x
}
func @predicate(i8 %x) -> i8 {
entry:
    %is_match = icmp eq i8 %x, 42
    %result = zext i1 %is_match to i8
    ret %result
}
"#;

const DRIVER: &str = r#"
func @handle_price_probe(i64 %budget) {
entry:
    %x = qalloc 8
    h %x[0]
    h %x[1]
    h %x[2]
    h %x[3]
    h %x[4]
    h %x[5]
    h %x[6]
    h %x[7]
    %out = qalloc 8
    %fault = qalloc 5
    apply @CALLEE(%x) -> %out, %fault budget %budget
    ret
}
"#;

#[derive(Clone, Copy, Debug)]
enum Route {
    Registers,
    Call,
    Indirect,
    Block,
}

impl Route {
    fn name(self, direct: &str) -> &str {
        match self {
            Self::Registers => direct,
            Self::Call | Self::Indirect | Self::Block => "handle_wrapper",
        }
    }

    fn budget(self) -> u64 {
        // The i64 adapter and the wrapper's return add eight steps even without memory.
        // The indirect form adds faddr. The full block uses the same adapter
        // (four steps), seven steps in invoke, and ten steps around invoke.
        BUDGET
            + match self {
                Self::Registers => 0,
                Self::Call => 8,
                Self::Indirect => 9,
                Self::Block => 21,
            }
    }

    fn wrapper(self, callee: &str) -> String {
        let wrapper = match self {
            Self::Registers => return String::new(),
            Self::Call => {
                r#"
func @handle_wrapper(i8 %x) -> i8 {
entry:
    %wide = zext i8 %x to i64
    %value = call @handle_scalar(%wide)
    %out = trunc i64 %value to i8
    ret %out
}
"#
            }
            Self::Indirect => {
                r#"
func @handle_wrapper(i8 %x) -> i8 {
entry:
    %wide = zext i8 %x to i64
    %g = faddr @handle_scalar
    %value = call_indirect fn(i64) -> i64 %g(%wide)
    %out = trunc i64 %value to i8
    ret %out
}
"#
            }
            // The 24-byte block starts at address 16, aligned to 16 bytes.
            // Its initial image is {g, 0, 0}. The input and output fields
            // each span one byte, at block offsets 8 and 16 respectively.
            // Field descriptors are fixed by the fixture. The model charges
            // image copying, input write, the entry call and extraction as
            // instructions; the handle device sets up the entry state and
            // snapshots the image and field list outside the budget.
            Self::Block => {
                r#"
data 64 = i64 @handle_scalar, i64 0, i64 0
func @handle_wrapper(i8 %x) -> i8 {
entry:
    %g_image = load i64 64
    %arg_image = load i64 72
    %result_image = load i64 80
    store i64 %g_image, 16
    store i64 %arg_image, 24
    store i64 %result_image, 32
    store i8 %x, 24
    call @invoke(i64 16)
    %readout = load i8 32
    ret %readout
}
func @invoke(i64 %block) {
entry:
    %arg_addr = add i64 %block, 8
    %out_addr = add i64 %block, 16
    %target = load i64 %block
    %arg = load i64 %arg_addr
    %value = call_indirect fn(i64) -> i64 %target(%arg)
    store i64 %value, %out_addr
    ret
}
"#
            }
        };
        format!(
            r#"
{wrapper}
func @handle_scalar(i64 %arg) -> i64 {{
entry:
    %narrow = trunc i64 %arg to i8
    %res = call @{callee}(%narrow)
    %value = zext i8 %res to i64
    ret %value
}}
"#
        )
    }
}

fn reference(module: &ValidModule, budget: u64, value: impl Fn(u64) -> u64) -> (Vec<u128>, Vec<u8>) {
    let main = module.function_named("handle_price_probe").expect("probe");
    let mut executor = Executor::new(module, main, &[budget], Options::default()).expect("evaluate all inputs");
    assert_eq!(executor.run(), Outcome::Returned(None));
    let snapshot = executor.snapshot();
    let state = snapshot.quantum;
    assert_eq!(state.amplitudes.len(), 256);
    let values = state
        .amplitudes
        .iter()
        .enumerate()
        .map(|(input, (configuration, _))| {
            assert_eq!(configuration, &[input as u64, value(input as u64), 0]);
            u128::from(configuration[1])
        })
        .collect();
    (values, snapshot.classical.memory)
}

fn shor_harness(module: ValidModule, applied: FuncId, budget: u64) -> (ValidModule, FuncId) {
    let entry = module.function_named("entry").expect("Shor entry");
    let original = module.function_named(SHOR_APPLY).expect("lowered Shor apply");
    let mut module = module.into_module();
    let mut calls = 0;
    for inst in module.functions[entry.index()].blocks.iter_mut().flat_map(|block| &mut block.insts) {
        if let Inst::Apply { callee, budget: steps, .. } = inst
            && *callee == original
        {
            *callee = applied;
            *steps = Operand::Const(Const::new(Width::WORD, budget.into()).expect("i64 budget"));
            calls += 1;
        }
    }
    assert_eq!(calls, 1);
    (validate(module).expect("retargeted Shor harness"), entry)
}

fn measure(
    name: &str,
    source: &str,
    direct: &str,
    route: Route,
    mode: ApplyMode,
    budget: u64,
    value: impl Fn(u64) -> u64,
) {
    let callee = route.name(direct);
    let wrapper = route.wrapper(direct);
    let probe = DRIVER.replace("CALLEE", callee);
    let module = load(&format!("{source}{wrapper}{probe}")).expect("valid benchmark device program");
    let applied = module.function_named(callee).expect("applied function");
    let (reference, bytes) = reference(&module, budget, value);
    let held = Default::default();
    let compiled = compile_apply_with(
        ApplyInput {
            module: &module,
            callee: applied,
            args: &[Argument::Register(0..EXPONENT_BITS)],
            out: EXPONENT_BITS..FAULT_START,
            fault: FAULT_START..INTERFACE,
            budget,
            entry_fault: None,
            stack_available: module.memory_size - module.static_end(),
            memory: Some(ApplyMemory::new(&bytes, &held)),
            scratch_start: INTERFACE,
        },
        mode,
    )
    .expect("compile benchmark");
    let block = count::cancel_adjacent_toffolis(&compiled.block);
    let [BlockOp::Allocate(scratch), gates @ .., BlockOp::ReleaseZero(released)] = block.ops() else {
        panic!("expected one scratch lifetime");
    };
    assert_eq!(scratch, released);
    assert_eq!(scratch.start, INTERFACE);
    let gates = gates
        .iter()
        .map(|op| match op {
            BlockOp::Gate(gate) => gate.clone(),
            BlockOp::Allocate(_) | BlockOp::ReleaseZero(_) => panic!("unexpected scratch lifetime"),
        })
        .collect::<Vec<_>>();
    let layout = bits::Layout {
        x: EXPONENT_BITS as usize,
        y: (INTERFACE - EXPONENT_BITS) as usize,
        scratch: (scratch.end - scratch.start) as usize,
    };
    bits::check_states(&gates, layout, (0..256).flat_map(|x| [(x, 0), (x, 0x15a5)]), |x| reference[x as usize])
        .unwrap_or_else(|error| panic!("{name}, {route:?}: {error}"));

    let mut native = [0; 3];
    for gate in &gates {
        native[gate.controls.len()] += 1;
    }
    let harness = if direct == SHOR_APPLY {
        let (module, entry) = shor_harness(module, applied, budget);
        // The harness counter supplies zero measurements. The oracle's two
        // lifetime operations suffice to count its live scratch. The Fourier
        // gates added by emitted_shor.rs allocate no wires either.
        let lifetime = GateBlock::from_ops(
            block.name(),
            [BlockOp::Allocate(scratch.clone()), BlockOp::ReleaseZero(released.clone())],
        );
        let peak = count::harness_peak(&module, entry, applied, &lifetime);
        if matches!(route, Route::Block) && mode == ApplyMode::Automatic {
            // Twice the hand-built Shor oracle's Toffoli count and peaks.
            assert!(native[2] <= 2 * 1_552, "private block Toffolis {}", native[2]);
            assert!(scratch.end <= 2 * 37, "private block isolated peak {}", scratch.end);
            assert!(peak <= 2 * 47, "private block harness peak {peak}");
        }
        peak.to_string()
    } else {
        "-".to_owned()
    };
    println!(
        "| {name} | {route:?} | {mode:?} | {budget} | {} | {} | {} | {} | {harness} | {} | {} | 256/256 |\n{}: {}",
        native[0],
        native[1],
        native[2],
        scratch.end,
        block.ops().len(),
        compiled.construction,
        compiled.reason,
        compiled.memory,
    );
}

const SHOR_APPLY: &str = "entry.apply";

#[test]
fn handle_price() {
    for route in [Route::Registers, Route::Call, Route::Indirect, Route::Block] {
        let budget = route.budget();
        measure("Identity", SMALL, "identity", route, ApplyMode::Automatic, budget, |x| x);
        measure("Predicate", SMALL, "predicate", route, ApplyMode::Automatic, budget, |x| u64::from(x == 42));
        measure("Shor 15", SHOR, SHOR_APPLY, route, ApplyMode::Automatic, budget, |x| [1, 7, 4, 13][(x % 4) as usize]);
    }
    for budget in [BUDGET, BUDGET + 6] {
        measure("Shor 15", SHOR, SHOR_APPLY, Route::Registers, ApplyMode::StepMachine, budget, |x| {
            [1, 7, 4, 13][(x % 4) as usize]
        });
    }
}
