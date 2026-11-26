//! Compile an ordinary applied function to a reversible compute, copy and undo circuit.

mod arithmetic;
mod bits;
mod calls;
mod controls;
mod definite;
mod division;
mod flow;
#[cfg(test)]
mod gadget_tests;
mod machine;
mod memory;
mod modular;
mod modular_loop;
mod products;
mod ranges;
mod reach;
mod returns;
mod saved;
mod selectors;
mod shifts;
mod stack;
mod switches;
mod symbolic;
mod wires;

use std::fmt;
use std::ops::Range;

use bennett_exec::{Device, GateBlock, RunError, Wire};
use bennett_ir::{BlockId, Fault, Type, Width};

use self::bits::{Bit, Circuit, ONE, Word, constant};
use self::calls::Program;
use self::memory::{DISPATCH_REQUIRED, Memory};
use self::symbolic::{Point, Status, Step};
use self::wires::Layout;

pub use bennett_exec::{ApplyError, ApplyInput, ApplyMemory, Argument};
pub use memory::MemoryCounts;

/// The control construction selected for one applied function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Construction {
    /// The next instruction was definite until the return or fault.
    Definite,
    /// Qubit-dependent choices control gates directly, without instruction dispatch.
    Controlled {
        /// Number of emitted body passes for loops with program-proved counts.
        proved_passes: usize,
    },
    /// A proved residue update reuses the same word for each pass.
    ModularInPlace {
        /// Passes permitted by the width of the exponent.
        passes: usize,
    },
    /// Qubit-dependent control required instruction dispatch.
    StepMachine {
        /// Ticks after the definite prefix, including the attempt that triggered dispatch.
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
            Self::Controlled { proved_passes } => write!(f, "control gates ({proved_passes} proved loop passes)"),
            Self::ModularInPlace { passes } => {
                write!(f, "in-place modular multiplication ({passes} proved loop passes)")
            }
            Self::StepMachine { ticks } => write!(f, "step machine ({ticks} ticks)"),
            Self::SavedDecisions { decisions } => write!(f, "saved decisions ({decisions} decisions)"),
        }
    }
}

/// Which constructions the compiler may consider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApplyMode {
    /// Try in-place updates, control gates and saved decisions before using the step machine.
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
    /// Costs of the private memory versions in this call.
    pub memory: MemoryCounts,
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

/// Try proved in-place updates and structured control before instruction dispatch.
pub fn compile_apply(input: ApplyInput<'_>) -> Result<CompiledApply, ApplyError> {
    compile_apply_with(input, ApplyMode::Automatic)
}

/// Compile an applied function with the requested choice of control constructions.
pub fn compile_apply_with(input: ApplyInput<'_>, mode: ApplyMode) -> Result<CompiledApply, ApplyError> {
    compile_with_analysis_limit(input, mode, reach::MAX_ATTEMPTS)
}

fn compile_with_analysis_limit(
    input: ApplyInput<'_>,
    mode: ApplyMode,
    analysis_limit: usize,
) -> Result<CompiledApply, ApplyError> {
    check_interface(&input)?;
    let wires = Layout::new(&input);
    let args = wires.arguments(input.args);
    let input = ApplyInput {
        args: &args,
        out: wires.local(input.out.clone()),
        fault: wires.local(input.fault.clone()),
        scratch_start: wires.scratch_start(),
        ..input
    };
    let (program, mut slots) = Program::check(&input)?;
    let try_modular = mode == ApplyMode::Automatic
        && !program.has_dynamic_stack()
        && input.args.iter().any(|arg| matches!(arg, Argument::Register(_)));
    let mut circuit = Circuit::new(input.scratch_start);
    let fixed_stores = input.module.data.is_empty() || program.stores_have_definite_addresses();
    let mut memory = Memory::new(&input, fixed_stores);
    let mut status = match input.entry_fault {
        Some(fault) => Status::Faulted(fault),
        None => Status::Running(Point::entry(BlockId(0))),
    };
    let mut spent = 0;
    let mut candidate = None;
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
                if try_modular && candidate.is_none() && input.budget - spent > 2 {
                    candidate = modular_loop::Candidate::remember(&program, &slots, point, spent, &circuit);
                }
                let step = program.step_in(&mut circuit, &slots, point, &mut memory, ONE)?;
                memory.commit(&mut circuit);
                status = match step {
                    Step::Write(next, dst, word) => {
                        slots[dst.index()] = word;
                        Status::Running(next)
                    }
                    Step::Jump(target) => Status::Running(program.jump(&mut slots, point, target)),
                    Step::Transfer(next, writes) => {
                        for (slot, word) in writes {
                            slots[slot.index()] = word;
                        }
                        Status::Running(next)
                    }
                    Step::CheckedTransfer(next, writes, succeeded, faults) => {
                        // The attempt has been emitted. Only successful inputs
                        // move to the next instruction and spend one step.
                        for (slot, word) in writes {
                            slots[slot.index()] = word;
                        }
                        let remaining = input.budget - spent - 1;
                        let (mut copies, ticks) = machine::run(
                            &mut circuit,
                            &program,
                            &mut slots,
                            &mut memory,
                            (next, succeeded),
                            remaining,
                        )?;
                        copies.faulted.extend(faults);
                        break (
                            copies,
                            Construction::StepMachine { ticks: ticks + 1 },
                            "qubit-dependent instruction fault",
                        );
                    }
                    Step::Fault(fault) => {
                        status = Status::Faulted(fault);
                        continue;
                    }
                    Step::Switch(..) | Step::Indirect(..) => {
                        let refusal = if matches!(step, Step::Switch(..)) {
                            controls::Refusal::MultiwayDispatch
                        } else {
                            controls::Refusal::IndirectDispatch
                        };
                        let remaining = input.budget - spent;
                        let (copies, ticks) =
                            machine::run(&mut circuit, &program, &mut slots, &mut memory, (point, ONE), remaining)?;
                        break (copies, Construction::StepMachine { ticks }, refusal.reason());
                    }
                    Step::Branch(..) => {
                        if let Some(candidate) = candidate.take()
                            && let Some((plan, checkpoint)) = candidate.prove(&input, &program, &memory)
                        {
                            let mut in_place = circuit.suffix(&checkpoint);
                            let residue = plan.emit(&mut in_place);
                            if wires.fits(&in_place) {
                                circuit.replace_after(checkpoint, in_place);
                                break (
                                    Copies { returned: vec![(ONE, residue)], faulted: Vec::new() },
                                    Construction::ModularInPlace { passes: plan.passes() },
                                    "a proved modular residue update with a whole-function budget",
                                );
                            }
                        }
                        let remaining = input.budget - spent;
                        let reason = match mode {
                            ApplyMode::Automatic => {
                                // Preserve the cheaper fixed-charge schedule for covered copied loops.
                                let has_calls = program.has_calls();
                                let can_save = !program.has_memory();
                                let mut saved =
                                    if has_calls && can_save { saved::Loop::prove(&program, point) } else { None };
                                let reason = if saved.is_some() {
                                    "one proved loop with fixed continuation and exit charges"
                                } else if has_calls && point.copy == 0 {
                                    // Keep the established construction when the first dynamic
                                    // choice is in the applied entry.
                                    "copied calls use the shared step machine"
                                } else {
                                    let checkpoint = circuit.checkpoint();
                                    let reason = match controls::run(&mut circuit, &program, &slots, point, remaining) {
                                        Ok((copies, proved_passes)) => {
                                            break (
                                                copies,
                                                Construction::Controlled { proved_passes },
                                                "qubit-dependent choices use controls within the shared budget",
                                            );
                                        }
                                        Err(controls::Failure::Control(refusal)) => {
                                            if can_save && refusal == controls::Refusal::UnprovedLoop {
                                                saved = saved::Loop::prove(&program, point);
                                            }
                                            refusal.reason()
                                        }
                                        // Recheck the access against the exact shared clock.
                                        Err(controls::Failure::Lowering(ApplyError::Unsupported {
                                            route: DISPATCH_REQUIRED,
                                            ..
                                        })) => DISPATCH_REQUIRED,
                                        Err(controls::Failure::Lowering(error)) => return Err(error),
                                    };
                                    circuit.rewind(checkpoint);
                                    reason
                                };
                                if let Some(plan) = saved {
                                    let bounds = reach::analyze(&program, &input, slots.len(), analysis_limit)
                                        .unwrap_or_default();
                                    let remaining = bounds.last_attempt.map_or(remaining, |last| last - spent);
                                    let (copies, decisions) =
                                        plan.run(&mut circuit, &program, &mut slots, remaining, &bounds.widths)?;
                                    break (
                                        copies,
                                        Construction::SavedDecisions { decisions },
                                        "one proved loop with fixed continuation and exit charges",
                                    );
                                }
                                reason
                            }
                            ApplyMode::StepMachine => "step machine requested for a qubit-dependent jump",
                        };
                        let (copies, ticks) =
                            machine::run(&mut circuit, &program, &mut slots, &mut memory, (point, ONE), remaining)?;
                        break (copies, Construction::StepMachine { ticks }, reason);
                    }
                    Step::Return(word) => Status::Returned(word),
                    Step::Inactive => unreachable!("the definite prefix is active"),
                };
                if let Status::Running(next) = status
                    && candidate.as_ref().is_some_and(|candidate| candidate.exited(next))
                {
                    candidate = None;
                }
                spent += 1;
            }
        }
    };
    let forward = circuit.forward_len();
    copies.write(&mut circuit, input.out, input.fault);
    Ok(CompiledApply {
        block: wires.finish(circuit, forward, &input.module.function(input.callee).name)?,
        definite_steps: spent,
        construction,
        reason,
        memory: memory.counts(),
    })
}
