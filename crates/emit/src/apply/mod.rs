//! Compile an ordinary applied function to a reversible compute, copy and undo circuit.

mod arithmetic;
mod bits;
mod symbolic;

use std::fmt;
use std::ops::Range;

use bennett_exec::{Device, GateBlock, RunError, Wire};
use bennett_ir::{BlockId, Fault, FuncId, Type, ValidModule};

use self::bits::{Circuit, ONE, constant};
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

/// Gates and the number of instructions charged while following definite control.
#[derive(Debug)]
pub struct CompiledApply {
    /// Compute, copy and undo, including the scratch lifetime.
    pub block: GateBlock,
    /// Number of successful steps executed before return, fault or dynamic control.
    pub definite_steps: u64,
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

/// Follow definite control, emitting gates for data that depend on qubits.
/// A jump whose destination depends on qubits needs the step machine.
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
    while let Status::Running(point) = status {
        if spent == input.budget {
            status = Status::Faulted(Fault::Budget);
            break;
        }
        status = match frame.step(&mut circuit, &slots, point)? {
            Step::Write(next, dst, word) => {
                slots[dst.index()] = word;
                Status::Running(next)
            }
            Step::Jump(target) => Status::Running(frame.jump(&mut slots, target)),
            Step::Branch => return Err(frame.error(point, "br", "dispatch on a qubit-dependent program point")),
            Step::Return(word) => Status::Returned(word),
        };
        spent += 1;
    }
    let forward = circuit.forward_len();
    match status {
        Status::Returned(word) => circuit.copy(input.out, &word, ONE),
        Status::Faulted(fault) => {
            let word = constant(u128::from(fault.code()), Fault::CODE_WIDTH.bits() as usize);
            circuit.copy(input.fault, &word, ONE);
        }
        Status::Running(_) => unreachable!("the run ended before the result copy"),
    }
    Ok(CompiledApply {
        block: circuit.finish(forward, &input.module.function(input.callee).name),
        definite_steps: spent,
    })
}
