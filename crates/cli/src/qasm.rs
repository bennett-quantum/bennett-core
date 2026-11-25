//! Stream the commands of one device run as OpenQASM 3.

use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::ops::Range;

use bennett_exec::{Device, GateOp, Outcome, RunError, SimulatedDevice, Wire};
use bennett_ir::{FixedGate, Gate, Rotation};

#[derive(Clone, Copy)]
struct Qubit {
    register: u64,
    index: u64,
}

/// Where the writer takes measurement outcomes.
pub(crate) enum Outcomes {
    /// The run's own simulation, which carries out every command before it is written.
    Simulated(Box<SimulatedDevice>),
    /// Seeded bits independent of the gates, for a circuit the simulation cannot hold.
    Synthetic(Box<dyn Iterator<Item = bool>>),
}

/// Writes each command and annotates each measurement with its outcome. With simulated outcomes the
/// simulation carries out each command first.
pub(crate) struct QasmDevice<W> {
    output: W,
    write_error: Cell<Option<io::Error>>,
    outcomes: Outcomes,
    wires: HashMap<Wire, Qubit>,
    next_register: u64,
}

impl<W: Write> QasmDevice<W> {
    pub(crate) fn new(mut output: W, seed: u64, outcomes: Outcomes) -> io::Result<Self> {
        writeln!(output, "OPENQASM 3.0;\ninclude \"stdgates.inc\";")?;
        match outcomes {
            Outcomes::Simulated(_) => writeln!(
                output,
                "// Bennett run; measurement outcomes sampled from the run's own simulation (seed {seed})."
            )?,
            Outcomes::Synthetic(_) => {
                writeln!(
                    output,
                    "// Bennett run; synthetic measurement outcomes (seed {seed}), not from a simulation."
                )?;
                writeln!(output, "// An annotated outcome can have probability zero.")?;
            }
        }
        writeln!(output, "// A release_zero request assumes a replay with the recorded outcomes.")?;
        Ok(Self { output, write_error: Cell::new(None), outcomes, wires: HashMap::new(), next_register: 0 })
    }

    /// The simulation, when it carries out the commands.
    fn simulation(&mut self) -> Option<&mut SimulatedDevice> {
        match &mut self.outcomes {
            Outcomes::Simulated(simulation) => Some(simulation),
            Outcomes::Synthetic(_) => None,
        }
    }

    // The device interface returns a static diagnostic. Keep the I/O cause for the CLI.
    pub(crate) fn take_write_error(&self) -> Option<io::Error> {
        self.write_error.take()
    }

    fn command(&mut self, emit: impl FnOnce(&mut W, &HashMap<Wire, Qubit>) -> io::Result<()>) -> Result<(), RunError> {
        emit(&mut self.output, &self.wires).map_err(|error| {
            self.write_error.set(Some(error));
            RunError::Device("cannot write OpenQASM output")
        })
    }

    fn release(&mut self, wires: Range<Wire>, comment: &str, suffix: &str) -> Result<(), RunError> {
        if wires.is_empty() {
            return Ok(());
        }
        self.command(|output, live| {
            write!(output, "// {comment} ")?;
            write_operands(output, live, wires.clone())?;
            writeln!(output, "{suffix}")
        })?;
        for wire in wires {
            self.wires.remove(&wire);
        }
        Ok(())
    }
}

impl<W: Write> Device for QasmDevice<W> {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        if wires.is_empty() {
            return Ok(());
        }
        if let Some(simulation) = self.simulation() {
            simulation.allocate(wires.clone())?;
        }
        let register = self.next_register;
        let next = register.checked_add(1).ok_or(RunError::Device("OpenQASM register numbers exhausted"))?;
        let first = wires.start;
        let width = wires.end - first;
        // OpenQASM declarations leave the initial state undefined. Allocation prepares zero.
        self.command(|output, _| writeln!(output, "qubit[{width}] q{register};\nreset q{register};"))?;
        // A device wire may be reused. Each QASM allocation has a fresh identity.
        self.wires.extend(wires.map(|wire| (wire, Qubit { register, index: wire - first })));
        self.next_register = next;
        Ok(())
    }

    fn gate(&mut self, gate: GateOp<&[Wire]>) -> Result<(), RunError> {
        if let Some(simulation) = self.simulation() {
            simulation.gate(gate)?;
        }
        self.command(|output, live| {
            for _ in gate.controls {
                write!(output, "ctrl @ ")?;
            }
            match gate.kind {
                Gate::Fixed(kind) => write!(output, "{} ", fixed_name(kind))?,
                // Debug formatting round-trips the finite binary64 angle, including signed zero.
                Gate::Rotate(axis, angle) => write!(output, "{}({angle:?}) ", rotation_name(axis))?,
            }
            write_operands(output, live, gate.controls.iter().copied().chain(std::iter::once(gate.target)))?;
            writeln!(output, ";")
        })
    }

    fn measure(&mut self, wire: Wire) -> Result<bool, RunError> {
        let bit = match &mut self.outcomes {
            Outcomes::Simulated(simulation) => simulation.measure(wire)?,
            Outcomes::Synthetic(bits) => bits.next().ok_or(RunError::Device("measurement source exhausted"))?,
        };
        self.write_measurement(wire, bit)?;
        Ok(bit)
    }

    /// One joint sample of all the ranges, as the simulator takes it, written one qubit at a time. Synthetic
    /// outcomes take one bit per qubit in the same order.
    fn measure_many(&mut self, ranges: &[Range<Wire>]) -> Result<Vec<u64>, RunError> {
        let words = match &mut self.outcomes {
            Outcomes::Simulated(simulation) => simulation.measure_many(ranges)?,
            Outcomes::Synthetic(bits) => ranges
                .iter()
                .map(|range| {
                    range.clone().enumerate().try_fold(0, |word, (bit, _)| {
                        let next = bits.next().ok_or(RunError::Device("measurement source exhausted"))?;
                        Ok(word | (u64::from(next) << bit))
                    })
                })
                .collect::<Result<_, RunError>>()?,
        };
        for (range, word) in ranges.iter().zip(&words) {
            for (bit, wire) in range.clone().enumerate() {
                self.write_measurement(wire, (word >> bit) & 1 == 1)?;
            }
        }
        Ok(words)
    }

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        if let Some(simulation) = self.simulation() {
            simulation.discard(wires.clone())?;
        }
        self.release(wires, "discard", " (partial trace)")
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        if let Some(simulation) = self.simulation() {
            simulation.release_zero(wires.clone())?;
        }
        self.release(wires, "release_zero requested for", "")
    }
}

impl<W: Write> QasmDevice<W> {
    fn write_measurement(&mut self, wire: Wire, bit: bool) -> Result<(), RunError> {
        self.command(|output, live| {
            write!(output, "measure ")?;
            write_operands(output, live, [wire])?;
            writeln!(output, "; // outcome {}", u8::from(bit))
        })
    }
}

/// The gate's name in OpenQASM's `stdgates.inc`, spelled out here rather than taken from the device
/// language's own mnemonics, which name the same gates for another reader.
fn fixed_name(gate: FixedGate) -> &'static str {
    match gate {
        FixedGate::X => "x",
        FixedGate::H => "h",
        FixedGate::Z => "z",
        FixedGate::S => "s",
        FixedGate::Sdg => "sdg",
        FixedGate::T => "t",
        FixedGate::Tdg => "tdg",
    }
}

/// The rotation's name in `stdgates.inc`, whose `p(λ)` is `diag(1, e^{iλ})`, as the device's.
fn rotation_name(axis: Rotation) -> &'static str {
    match axis {
        Rotation::Rx => "rx",
        Rotation::Ry => "ry",
        Rotation::Rz => "rz",
        Rotation::P => "p",
    }
}

pub(crate) fn write_run_end(output: &mut impl Write, outcome: Outcome, run_status: u8) -> io::Result<()> {
    match outcome {
        Outcome::Returned(Some(value)) => writeln!(output, "// returned {value} (run status {run_status})"),
        Outcome::Returned(None) => writeln!(output, "// returned (run status {run_status})"),
        Outcome::Faulted(fault) => {
            writeln!(output, "// fault {fault} (code {}, run status {run_status})", fault.code())
        }
        Outcome::Stopped(error) => writeln!(output, "// stopped {error} (run status {run_status})"),
    }
}

fn write_operands(
    output: &mut impl Write,
    live: &HashMap<Wire, Qubit>,
    wires: impl IntoIterator<Item = Wire>,
) -> io::Result<()> {
    for (position, wire) in wires.into_iter().enumerate() {
        if position > 0 {
            write!(output, ", ")?;
        }
        let Qubit { register, index } = live[&wire];
        write!(output, "q{register}[{index}]")?;
    }
    Ok(())
}
