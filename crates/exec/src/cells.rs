//! Qubits in memory (`docs/DESIGN.md`, "Qubits in memory"): gates that take addresses.
//!
//! The memory of a basis state is the classical memory, except at the bytes that hold qubits, the
//! **cells**, whose values are the configuration's. A cell is an 8-qubit register column bound to a byte
//! address (`QuantumState::cells`, part of the register table); the classical memory under a cell holds 0.
//! The first gate at a byte that is not a cell **promotes** it: a fresh column is allocated, the byte's
//! value is XORed into it, and the memory byte becomes 0. The gate's record journals the promotion
//! (address and old byte), so undoing the gate un-promotes: the inverse gate, then the byte XORed out,
//! the column un-allocated, the byte written back.
//!
//! Ordinary code that touches a cell (a load, a store, a jump record), outside `apply` or inside it, faults
//! `qubit_memory_access` before changing anything, whatever the cell's qubits hold (the maintainer's decision
//! of 2026-11-17). A byte is a cell from the gate that promoted it until that gate is undone, or until a
//! measurement measures all eight of its bits: then it is ordinary memory again, holding the result
//! (`forward.rs`, `measure_memory`; the maintainer's decision of 2026-11-17).

use std::ops::Range;

use bennett_ir::{Fault, Gate, Inst, Place, Width};

use crate::device::Wire;
use crate::device_io::{DeviceContext, wire};
use crate::executor::{Executor, Halt, RunError};
use crate::gates;
use crate::journal::Undo;

/// A qubit of a gate, resolved: a register qubit, or a bit of a memory byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resolved {
    Register(Wire),
    Memory { byte: u64, bit: u8 },
}

impl<'m> Executor<'m> {
    /// Resolves a place. A memory place outside memory is `out_of_bounds`.
    fn resolve_place(&self, place: &Place) -> Result<Resolved, Fault> {
        match *place {
            Place::Register(qubit) => {
                Ok(Resolved::Register(wire(self.quantum.resolve(self.handle(qubit.register))?, qubit.bit)))
            }
            Place::Memory { address, bit } => {
                let byte = self.read_word(&address).checked_add(u64::from(bit / 8));
                let byte = byte.filter(|&byte| byte < self.memory.len() as u64).ok_or(Fault::OutOfBounds)?;
                Ok(Resolved::Memory { byte, bit: bit % 8 })
            }
        }
    }

    /// The qubit a resolved place names; a memory byte must be a cell.
    fn resolve_wire(&self, resolved: Resolved) -> Wire {
        match resolved {
            Resolved::Register(qubit) => qubit,
            Resolved::Memory { byte, bit } => {
                wire(self.quantum.cell(byte).expect("a gate's memory byte is a cell"), u32::from(bit))
            }
        }
    }

    /// Makes the memory byte at `address` a cell: a fresh column holding its value, the byte 0. Returns
    /// the byte's old value.
    fn promote(&mut self, address: u64, context: &mut DeviceContext<'_>) -> Result<u8, RunError> {
        let mut old = [0];
        self.memory.read_into(address as usize..address as usize + 1, &mut old);
        let handle = self.allocate_register(Width::BYTE.into(), context)?;
        let column = self.quantum.resolve(handle).expect("a fresh register is live");
        self.xor_register(column, u64::from(old[0]), context)?;
        self.memory.write(address as usize, &[0]);
        self.quantum.bind_cell(address, Some(column));
        Ok(old[0])
    }

    /// Undoes `promote` of the cell at `address`, whose byte was `old`: the column holds `old` in every
    /// configuration again once every later step is undone.
    fn unpromote(&mut self, address: u64, old: u8) {
        let column = self.quantum.cell(address).expect("an un-promoted byte is a cell");
        self.quantum.xor_register(column, u64::from(old));
        self.quantum.unallocate_column(column, Width::BYTE.into());
        self.quantum.bind_cell(address, None);
        self.memory.write(address as usize, &[old]);
    }

    /// `gate target ctrl controls` with qubits in memory. Faults
    /// before changing anything for program faults; promotes the memory bytes that are not cells yet, journalling each, then
    /// applies the gate.
    pub(crate) fn gate_at(
        &mut self,
        gate: Gate,
        target: &Place,
        controls: &[Place],
        context: &mut DeviceContext<'_>,
    ) -> Result<(), Halt> {
        let places = std::iter::once(target).chain(controls);
        let resolved = places.map(|place| self.resolve_place(place)).collect::<Result<Vec<_>, _>>()?;
        if resolved.iter().enumerate().any(|(k, place)| resolved[..k].contains(place)) {
            return Err(Fault::QubitAlias.into());
        }
        // The bytes to promote, distinct, in the order the gate names them. The simulator admits their
        // qubits all at once, and a promotion that fails takes back those before it: a stop leaves the
        // state as it was.
        let mut fresh: Vec<u64> = Vec::new();
        for place in &resolved {
            if let Resolved::Memory { byte, .. } = *place
                && self.quantum.cell(byte).is_none()
                && !fresh.contains(&byte)
            {
                fresh.push(byte);
            }
        }
        if context.external.is_none() {
            self.quantum.admit(fresh.len() as u64 * u64::from(Width::BYTE.bits()))?;
        }
        let mut promoted: Vec<(u64, u8)> = Vec::new();
        for byte in fresh {
            match self.promote(byte, context) {
                Ok(old) => promoted.push((byte, old)),
                Err(error) => {
                    if context.external.is_none() {
                        for &(byte, old) in promoted.iter().rev() {
                            self.unpromote(byte, old);
                        }
                    }
                    return Err(error.into());
                }
            }
        }
        let target = self.resolve_wire(resolved[0]);
        let controls: Vec<Wire> = resolved[1..].iter().map(|&place| self.resolve_wire(place)).collect();
        if let Err(error) = self.device_gate(gate, target, &controls, context) {
            if context.external.is_none() {
                for &(byte, old) in promoted.iter().rev() {
                    self.unpromote(byte, old);
                }
            }
            return Err(error.into());
        }
        if promoted.is_empty() {
            self.record(Undo::GateAt);
        } else {
            for &(byte, old) in &promoted {
                self.record_word(u128::from(byte) << 8 | u128::from(old));
            }
            self.record_word(promoted.len() as u128);
            self.record(Undo::GateAtPromoted);
        }
        self.advance();
        Ok(())
    }

    /// Undoes the `GateAt` or `GateAtPromoted` record on top of the journal: the inverse gate, then the
    /// promotions undone, the latest first.
    pub(crate) fn undo_gate_at(&mut self, promoting: bool) {
        let Inst::GateAt { gate, target, controls } = self.retreat() else { unreachable!("a GateAt record") };
        let places = std::iter::once(target).chain(controls);
        let resolved: Vec<Resolved> =
            places.map(|place| self.resolve_place(place).expect("a gate that ran resolves again")).collect();
        let qubits: Vec<Wire> = resolved.iter().map(|&place| self.resolve_wire(place)).collect();
        // Undoing returns to a state the run held, so the configuration limit is not applied.
        self.device_gate(gates::inverse(*gate), qubits[0], &qubits[1..], &mut DeviceContext::default())
            .expect("no limit");
        if !promoting {
            return;
        }
        let journal = self.journal.as_mut().expect("undoing needs a journal");
        let count = journal.pop_word() as usize;
        // The words list the latest promotion last, so they pop latest first.
        let promoted: Vec<u128> = (0..count).map(|_| journal.pop_word()).collect();
        for word in promoted {
            self.unpromote((word >> 8) as u64, word as u8);
        }
    }

    /// Faults `qubit_memory_access` if any byte of `range`, which ordinary code is about to read or write, is
    /// a cell.
    pub(crate) fn check_no_cells(&self, range: Range<usize>) -> Result<(), Fault> {
        if self.quantum.has_cells_in(range.start as u64..range.end as u64) {
            return Err(Fault::QubitMemoryAccess);
        }
        Ok(())
    }
}
