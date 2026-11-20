//! Connect one applied call to a compiler and validate its circuit before issuing commands.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::ops::Range;

use bennett_ir::{BlockId, Fault, FixedGate, FuncId, Gate, ValidModule, Width};

use crate::device::{BlockOp, Device, GateBlock, Wire};
use crate::executor::RunError;
use crate::quantum::QuantumState;

/// An argument resolved by the ordinary controller.
#[derive(Clone, Debug)]
pub enum Argument {
    /// A definite ordinary value.
    Value(u128),
    /// The wires holding the argument bits, least significant first.
    Register(Range<Wire>),
}

/// Borrowed ordinary memory at the call boundary. Holding a byte in qubits is
/// an access fault regardless of the byte's value.
#[derive(Clone, Copy)]
pub struct ApplyMemory<'a> {
    bytes: &'a [u8],
    held: &'a BTreeMap<u64, usize>,
}

impl<'a> ApplyMemory<'a> {
    /// The ordinary bytes and held byte addresses at the call boundary.
    pub fn new(bytes: &'a [u8], held: &'a BTreeMap<u64, usize>) -> Self {
        Self { bytes, held }
    }

    /// Check bounds before checking occupancy and return the entry bytes.
    pub fn read(self, address: u64, width: Width) -> Result<&'a [u8], Fault> {
        let range = crate::memory::range(self.bytes.len(), address, width)?;
        if self.held.range(address..range.end as u64).next().is_some() {
            return Err(Fault::QubitMemoryAccess);
        }
        Ok(&self.bytes[range])
    }
}

impl fmt::Debug for ApplyMemory<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApplyMemory").field("bytes", &self.bytes.len()).field("held_bytes", &self.held.len()).finish()
    }
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
    /// Stack bytes available below the applied entry frame, if it fit.
    pub stack_available: u64,
    /// The caller's memory at the call boundary, needed by loads and stores.
    pub memory: Option<ApplyMemory<'a>>,
    /// First free wire above all wires live on the device.
    pub scratch_start: Wire,
}

/// Stack bytes reserved by a frame with this many slots.
pub fn frame_bytes(slots: usize) -> Option<u64> {
    u64::try_from(slots).ok()?.checked_mul(crate::executor::SLOT_BYTES)
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

/// Produces a complete X, CNOT and Toffoli circuit with a single private scratch lifetime.
pub trait ApplyCompiler {
    /// Compile one applied function. The circuit is checked before it can reach the device.
    fn compile(&mut self, input: ApplyInput<'_>) -> Result<GateBlock, RunError>;
}

impl<F> ApplyCompiler for F
where
    F: for<'a> FnMut(ApplyInput<'a>) -> Result<GateBlock, RunError>,
{
    fn compile(&mut self, input: ApplyInput<'_>) -> Result<GateBlock, RunError> {
        self(input)
    }
}

/// A native flip in the circuit's dense local wire numbering.
struct Flip {
    target: usize,
    controls: [usize; 2],
}

pub(crate) struct AppliedCircuit {
    interface: Vec<Wire>,
    flips: Vec<Flip>,
    scratch: usize,
}

impl AppliedCircuit {
    pub(crate) fn check(input: &ApplyInput<'_>, block: &GateBlock) -> Result<Self, RunError> {
        let [BlockOp::Allocate(scratch), gates @ .., BlockOp::ReleaseZero(released)] = block.ops() else {
            return Err(RunError::Device("apply circuit needs one scratch lifetime"));
        };
        if scratch != released || scratch.start < input.scratch_start || scratch.end < scratch.start {
            return Err(RunError::Device("apply circuit has an invalid scratch lifetime"));
        }
        let mut interface: Vec<_> = input.out.clone().chain(input.fault.clone()).collect();
        for arg in input.args {
            if let Argument::Register(wires) = arg {
                interface.extend(wires.clone());
            }
        }
        interface.sort_unstable();
        interface.dedup();
        let index: HashMap<_, _> = interface.iter().enumerate().map(|(index, &wire)| (wire, index)).collect();
        let scratch_count = usize::try_from(scratch.end - scratch.start)
            .map_err(|_| RunError::Device("apply circuit has too many wires"))?;
        let scratch_start = interface.len();
        let one = scratch_start
            .checked_add(scratch_count)
            .filter(|&index| index < usize::MAX)
            .ok_or(RunError::Device("apply circuit has too many wires"))?;
        let lookup = |wire| {
            if scratch.contains(&wire) {
                Ok(scratch_start + (wire - scratch.start) as usize)
            } else {
                index.get(&wire).copied().ok_or(RunError::Device("apply circuit names a wire outside its interface"))
            }
        };
        let mut flips = Vec::with_capacity(gates.len());
        for op in gates {
            let BlockOp::Gate(gate) = op else {
                return Err(RunError::Device("apply circuit needs one scratch lifetime"));
            };
            if gate.kind != Gate::Fixed(FixedGate::X) || gate.controls.len() > 2 {
                return Err(RunError::Device("apply circuit requires X, CNOT and Toffoli gates"));
            }
            let target = lookup(gate.target)?;
            let mut controls = [one; 2];
            for (i, &wire) in gate.controls.iter().enumerate() {
                controls[i] = lookup(wire)?;
                if controls[i] == target || controls[..i].contains(&controls[i]) {
                    return Err(RunError::Device("apply circuit repeats a wire in one gate"));
                }
            }
            flips.push(Flip { target, controls });
        }
        Ok(Self { interface, flips, scratch: scratch_count })
    }

    pub(crate) fn send(block: &GateBlock, device: &mut dyn Device) -> Result<(), RunError> {
        for op in block.ops() {
            op.send(device)?;
        }
        Ok(())
    }

    /// Returns the total input weight whose fault code was XORed into the destination.
    pub(crate) fn simulate(&self, state: &mut QuantumState, fault: usize) -> Result<Option<f64>, RunError> {
        state.admit(self.scratch as u64)?;
        let bindings = self
            .interface
            .iter()
            .map(|&wire| state.device_wires.qubit(wire).ok_or(RunError::Device("apply wire is not allocated")))
            .collect::<Result<Vec<_>, _>>()?;
        let width = u64::BITS as usize;
        let one = bindings.len() + self.scratch;
        let mut wires = vec![0u64; one + 1];
        let mut source = state.amplitudes().iter();
        let mut result = BTreeMap::new();
        let mut fault_weight = None;
        loop {
            let mut batch = source.by_ref().take(width).map(|(config, &amp)| (config.clone(), amp)).collect::<Vec<_>>();
            if batch.is_empty() {
                break;
            }
            wires.fill(0);
            wires[one] = u64::MAX >> (width - batch.len());
            for (lane, (config, _)) in batch.iter().enumerate() {
                for (word, &qubit) in wires.iter_mut().zip(&bindings) {
                    *word |= u64::from(qubit.is_set(config)) << lane;
                }
            }
            // A native flip permutes definite inputs, so each bit in the word follows one input.
            for flip in &self.flips {
                wires[flip.target] ^= wires[flip.controls[0]] & wires[flip.controls[1]];
            }
            if wires[bindings.len()..one].iter().any(|&word| word != 0) {
                return Err(RunError::Device("apply circuit left nonzero scratch"));
            }
            for (lane, (config, amplitude)) in batch.iter_mut().enumerate() {
                let old_fault = config[fault];
                for (&word, &qubit) in wires.iter().zip(&bindings) {
                    qubit.assign(config, word >> lane & 1 != 0);
                }
                if config[fault] != old_fault {
                    *fault_weight.get_or_insert(0.0) += amplitude.norm_sqr();
                }
            }
            result.extend(batch);
        }
        state.replace_amplitudes(result);
        state.note_qubit_peak(self.scratch);
        Ok(fault_weight)
    }
}
