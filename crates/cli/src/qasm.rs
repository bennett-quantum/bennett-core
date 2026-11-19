//! Stream the commands of one device run as OpenQASM 3.

use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::ops::Range;

use bennett_exec::{Device, GateOp, Outcome, RunError, Wire};
use bennett_ir::Gate;

#[derive(Clone, Copy)]
struct Qubit {
    register: u64,
    index: u64,
}

pub(crate) struct QasmDevice<W, I> {
    output: W,
    write_error: Cell<Option<io::Error>>,
    bits: I,
    wires: HashMap<Wire, Qubit>,
    next_register: u64,
}

impl<W: Write, I> QasmDevice<W, I> {
    pub(crate) fn new(mut output: W, seed: u64, bits: I) -> io::Result<Self> {
        writeln!(output, "OPENQASM 3.0;\ninclude \"stdgates.inc\";")?;
        writeln!(output, "// Bennett run with synthetic measurement outcomes (seed {seed}).")?;
        writeln!(output, "// An annotated outcome can have probability zero.")?;
        writeln!(output, "// A release_zero request assumes a replay with the recorded outcomes.")?;
        Ok(Self { output, write_error: Cell::new(None), bits, wires: HashMap::new(), next_register: 0 })
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

impl<W: Write, I: Iterator<Item = bool>> Device for QasmDevice<W, I> {
    fn allocate(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        if wires.is_empty() {
            return Ok(());
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
        self.command(|output, live| {
            for _ in gate.controls {
                write!(output, "ctrl @ ")?;
            }
            match gate.kind {
                Gate::Fixed(kind) => write!(output, "{} ", kind.mnemonic())?,
                // Debug formatting round-trips the finite binary64 angle, including signed zero.
                Gate::Rotate(axis, angle) => write!(output, "{}({angle:?}) ", axis.mnemonic())?,
            }
            write_operands(output, live, gate.controls.iter().copied().chain(std::iter::once(gate.target)))?;
            writeln!(output, ";")
        })
    }

    fn measure(&mut self, wire: Wire) -> Result<bool, RunError> {
        let bit = self.bits.next().ok_or(RunError::Device("measurement source exhausted"))?;
        self.command(|output, live| {
            write!(output, "measure ")?;
            write_operands(output, live, [wire])?;
            writeln!(output, "; // outcome {}", u8::from(bit))
        })?;
        Ok(bit)
    }

    fn discard(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.release(wires, "discard", " (partial trace)")
    }

    fn release_zero(&mut self, wires: Range<Wire>) -> Result<(), RunError> {
        self.release(wires, "release_zero requested for", "")
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
