//! Compile an ordinary applied function to a reversible compute, copy and undo circuit.

mod arithmetic;
mod bits;
mod definite;
mod machine;
mod saved;
mod symbolic;

use std::fmt;
use std::ops::Range;

use bennett_exec::{Device, GateBlock, RunError, Wire};
use bennett_ir::{BlockId, Fault, Type, Width};

use self::bits::{Bit, Circuit, ONE, Word, constant};
use self::symbolic::{Frame, Point, Status, Step};

pub use bennett_exec::{ApplyError, ApplyInput, Argument};

/// The control construction selected for one applied function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Construction {
    /// The next instruction was definite until the return or fault.
    Definite,
    /// A qubit-dependent jump required instruction dispatch.
    StepMachine {
        /// Ticks after the definite prefix, including the first qubit-dependent jump.
        ticks: u64,
    },
    /// A proof covered the first outcome of one loop and no instruction dispatch is needed.
    SavedDecisions {
        /// Number of decisions about whether to continue.
        decisions: u64,
    },
}

impl fmt::Display for Construction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Definite => f.write_str("definite control"),
            Self::StepMachine { ticks } => write!(f, "step machine ({ticks} ticks)"),
            Self::SavedDecisions { decisions } => write!(f, "saved decisions ({decisions} decisions)"),
        }
    }
}

/// Which constructions the compiler may consider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApplyMode {
    /// Prefer saved decisions when a loop proof succeeds.
    #[default]
    Automatic,
    /// Use the step machine at the first qubit-dependent jump, even for a proved loop.
    StepMachine,
}

/// Gates and the instruction charges of the construction.
#[derive(Debug)]
pub struct CompiledApply {
    /// Compute, copy and undo, including the scratch lifetime.
    pub block: GateBlock,
    /// Number of successful steps executed before return, fault or dynamic control.
    pub definite_steps: u64,
    /// Control construction used for the circuit.
    pub construction: Construction,
    /// Why the compiler selected this construction.
    pub reason: &'static str,
}

impl CompiledApply {
    /// Issue the entire circuit using the device commands directly.
    pub fn send(&self, device: &mut dyn Device) -> Result<(), RunError> {
        for op in self.block.ops() {
            op.send(device)?;
        }
        Ok(())
    }
}

fn check_interface(input: &ApplyInput<'_>) -> Result<(), ApplyError> {
    let function = input.module.functions.get(input.callee.index()).ok_or(ApplyError::Interface)?;
    let Some(Type::Int(result)) = function.result else { return Err(ApplyError::Interface) };
    let length = |range: &Range<Wire>| range.end.saturating_sub(range.start);
    let disjoint = |a: &Range<Wire>, b: &Range<Wire>| a.end <= b.start || b.end <= a.start;
    let max_qubits = u64::from(Width::MAX_QUBITS);
    if length(&input.out) != u64::from(result.bits())
        || length(&input.out) > max_qubits
        || !(u64::from(Fault::CODE_WIDTH.bits())..=max_qubits).contains(&length(&input.fault))
        || !disjoint(&input.out, &input.fault)
        || input.out.end.max(input.fault.end) > input.scratch_start
        || input.args.len() != function.params.len()
    {
        return Err(ApplyError::Interface);
    }
    for (arg, param) in input.args.iter().zip(&function.params) {
        let Type::Int(width) = param.ty else { return Err(ApplyError::Interface) };
        if let Argument::Register(wires) = arg
            && (length(wires) != u64::from(width.bits())
                || length(wires) > max_qubits
                || wires.end > input.scratch_start
                || !disjoint(wires, &input.out)
                || !disjoint(wires, &input.fault))
        {
            return Err(ApplyError::Interface);
        }
    }
    Ok(())
}

#[derive(Default)]
struct Copies {
    returned: Vec<(Bit, Word)>,
    faulted: Vec<(Bit, Fault)>,
}

impl Copies {
    fn write(self, circuit: &mut Circuit, out: Range<Wire>, fault: Range<Wire>) {
        for (enabled, value) in self.returned {
            circuit.copy(out.clone(), &value, enabled);
        }
        for (enabled, code) in self.faulted {
            let word = constant(u128::from(code.code()), Fault::CODE_WIDTH.bits() as usize);
            circuit.copy(fault.clone(), &word, enabled);
        }
    }
}

/// Follow definite control, then try saved decisions before the general step machine.
pub fn compile_apply(input: ApplyInput<'_>) -> Result<CompiledApply, ApplyError> {
    compile_apply_with(input, ApplyMode::Automatic)
}

/// Compile an applied function with the requested choice of control constructions.
pub fn compile_apply_with(input: ApplyInput<'_>, mode: ApplyMode) -> Result<CompiledApply, ApplyError> {
    check_interface(&input)?;
    let frame = Frame::check(&input)?;
    let mut circuit = Circuit::new(input.scratch_start);
    let mut slots = frame.initial(&input);
    let mut status = match input.entry_fault {
        Some(fault) => Status::Faulted(fault),
        None => Status::Running(Point::entry(BlockId(0))),
    };
    let mut spent = 0;
    let (copies, construction, reason) = loop {
        match status {
            Status::Returned(word) => {
                break (
                    Copies { returned: vec![(ONE, word)], faulted: Vec::new() },
                    Construction::Definite,
                    "control stays definite",
                );
            }
            Status::Faulted(code) => {
                break (
                    Copies { returned: Vec::new(), faulted: vec![(ONE, code)] },
                    Construction::Definite,
                    "control stays definite",
                );
            }
            Status::Running(_) if spent == input.budget => status = Status::Faulted(Fault::Budget),
            Status::Running(point) => {
                status = match frame.step(&mut circuit, &slots, point)? {
                    Step::Write(next, dst, word) => {
                        slots[dst.index()] = word;
                        Status::Running(next)
                    }
                    Step::Jump(target) => Status::Running(frame.jump(&mut slots, target)),
                    Step::Branch(..) => {
                        let remaining = input.budget - spent;
                        let reason = match mode {
                            ApplyMode::Automatic => {
                                if let Some(plan) = saved::Loop::prove(&frame, point) {
                                    let (copies, decisions) = plan.run(&mut circuit, &frame, &mut slots, remaining)?;
                                    break (
                                        copies,
                                        Construction::SavedDecisions { decisions },
                                        "one proved loop with fixed continuation and exit charges",
                                    );
                                }
                                "qubit-dependent jump without a single-loop proof"
                            }
                            ApplyMode::StepMachine => "step machine requested for a qubit-dependent jump",
                        };
                        let (copies, ticks) = machine::run(&mut circuit, &frame, &mut slots, point, remaining)?;
                        break (copies, Construction::StepMachine { ticks }, reason);
                    }
                    Step::Return(word) => Status::Returned(word),
                };
                spent += 1;
            }
        }
    };
    let forward = circuit.forward_len();
    copies.write(&mut circuit, input.out, input.fault);
    Ok(CompiledApply {
        block: circuit.finish(forward, &input.module.function(input.callee).name)?,
        definite_steps: spent,
        construction,
        reason,
    })
}
