//! Compile an ordinary applied function to a reversible compute, copy and undo circuit.

mod arithmetic;
mod bits;
mod machine;
mod symbolic;

use std::fmt;
use std::ops::Range;

use bennett_exec::{Device, GateBlock, RunError, Wire};
use bennett_ir::{BlockId, Fault, FuncId, Type, ValidModule};

use self::bits::{Bit, Circuit, ONE, Word, constant};
use self::symbolic::{Frame, Point, Status, Step};

/// An argument resolved by the ordinary controller.
#[derive(Clone, Debug)]
pub enum Argument {
    /// A definite ordinary value.
    Value(u128),
    /// The wires holding the argument bits, least significant first.
    Register(Range<Wire>),
}

/// The resolved inputs and destinations of one applied function.
#[derive(Clone, Debug)]
pub struct ApplyInput<'a> {
    /// The validated module containing the function.
    pub module: &'a ValidModule,
    /// The function to apply.
    pub callee: FuncId,
    /// Arguments in parameter order.
    pub args: &'a [Argument],
    /// Result wires, least significant first.
    pub out: Range<Wire>,
    /// Fault-code wires, least significant first.
    pub fault: Range<Wire>,
    /// Steps available to the whole function.
    pub budget: u64,
    /// Any failure reserving the applied frame, before the first instruction.
    pub entry_fault: Option<Fault>,
    /// First free wire above all wires live on the device.
    pub scratch_start: Wire,
}

/// A reason gate compilation could not proceed. No device commands have been sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// The supplied argument or destination wire ranges do not match the interface.
    Interface,
    /// The circuit exhausted the wire address space.
    WireSpace,
    /// The instruction at the given program point needs a construction not implemented yet.
    Unsupported {
        /// Applied function.
        callee: FuncId,
        /// Block containing the instruction.
        block: BlockId,
        /// Zero-based instruction index, counting the terminator after the instructions.
        index: usize,
        /// Name of the operation.
        operation: &'static str,
        /// Construction needed to emit it.
        route: &'static str,
    },
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interface => f.write_str("invalid applied circuit interface"),
            Self::WireSpace => f.write_str("applied circuit exhausted the wire address space"),
            Self::Unsupported { callee, block, index, operation, route } => {
                write!(
                    f,
                    "cannot emit {operation} at function {}, block {}, instruction {index}: needs {route}",
                    callee.0, block.0
                )
            }
        }
    }
}

impl std::error::Error for ApplyError {}

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

impl ApplyInput<'_> {
    fn check_interface(&self) -> Result<(), ApplyError> {
        let function = self.module.functions.get(self.callee.index()).ok_or(ApplyError::Interface)?;
        let Some(Type::Int(result)) = function.result else { return Err(ApplyError::Interface) };
        let length = |range: &Range<Wire>| range.end.saturating_sub(range.start);
        let disjoint = |a: &Range<Wire>, b: &Range<Wire>| a.end <= b.start || b.end <= a.start;
        if length(&self.out) != u64::from(result.bits())
            || length(&self.fault) < u64::from(Fault::CODE_WIDTH.bits())
            || !disjoint(&self.out, &self.fault)
            || self.out.end.max(self.fault.end) > self.scratch_start
            || self.args.len() != function.params.len()
        {
            return Err(ApplyError::Interface);
        }
        for (arg, param) in self.args.iter().zip(&function.params) {
            let Type::Int(width) = param.ty else { return Err(ApplyError::Interface) };
            if let Argument::Register(wires) = arg
                && (length(wires) != u64::from(width.bits())
                    || wires.end > self.scratch_start
                    || !disjoint(wires, &self.out)
                    || !disjoint(wires, &self.fault))
            {
                return Err(ApplyError::Interface);
            }
        }
        Ok(())
    }
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

/// Follow definite control, switching to the step machine at a qubit-dependent jump.
pub fn compile_apply(input: ApplyInput<'_>) -> Result<CompiledApply, ApplyError> {
    input.check_interface()?;
    let frame = Frame { module: input.module, callee: input.callee };
    frame.check()?;
    let mut circuit = Circuit::new(input.scratch_start);
    let mut slots = frame.initial(&input);
    let mut status = match input.entry_fault {
        Some(fault) => Status::Faulted(fault),
        None => Status::Running(Point::entry(BlockId(0))),
    };
    let mut spent = 0;
    let (copies, construction) = loop {
        match status {
            Status::Returned(word) => {
                break (Copies { returned: vec![(ONE, word)], faulted: Vec::new() }, Construction::Definite);
            }
            Status::Faulted(code) => {
                break (Copies { returned: Vec::new(), faulted: vec![(ONE, code)] }, Construction::Definite);
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
                        let (copies, ticks) =
                            machine::run(&mut circuit, &frame, &mut slots, point, input.budget - spent)?;
                        break (copies, Construction::StepMachine { ticks });
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
    })
}
