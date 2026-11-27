//! Handles as integers (`docs/DESIGN.md`, "Running a program"): the register made by the `k`-th successful
//! allocation of a run is named by the integer `k`, from 1, and no integer names two registers of one run. An
//! `i64` handle holds that integer; a `qN` slot holds it too, above the register's head column in the low 32
//! bits ([`register_value`]; no instruction lets a program read a `qN` value, and tests of the old layer read
//! the column there). This table maps each live number to the simulator's handle of its register and the
//! width. A `qN` value is resolved through the table and never through the simulator's handle, whose
//! generation is a 32-bit count per column that wraps after 2^32 reuses of the column.

use std::collections::BTreeMap;

use bennett_ir::{Fault, Qubits};

/// An allocated register: the simulator's handle of it (`QuantumState::reserve_register`) and its width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) register: u64,
    pub(crate) qubits: Qubits,
}

/// The allocation count and the allocated registers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Handles {
    /// The number of successful allocations so far, released registers included: the last handle given.
    count: u64,
    /// The registers allocated now, by handle.
    live: BTreeMap<u64, Entry>,
}

/// The value a `qN` slot holds for the register numbered `handle` whose simulator's handle is `register`: the
/// handle above the register's head column, the low 32 bits of `register`.
pub(crate) fn register_value(handle: u64, register: u64) -> u128 {
    u128::from(handle) << 32 | u128::from(register & u64::from(u32::MAX))
}

/// The handle a `qN` value of [`register_value`] holds; 0, never a handle, for a slot never written.
pub(crate) fn number_in(value: u128) -> u64 {
    (value >> 32) as u64
}

impl Handles {
    /// Fails with `out_of_handles` if the run has made 2^64 - 1 allocations, as many as handles number;
    /// otherwise the next allocation may go ahead.
    pub(crate) fn admit(&self) -> Result<(), Fault> {
        if self.count < u64::MAX { Ok(()) } else { Err(Fault::OutOfHandles) }
    }

    /// Records a successful allocation (after [`Handles::admit`]) and returns its handle.
    pub(crate) fn insert(&mut self, entry: Entry) -> u64 {
        self.count += 1;
        self.live.insert(self.count, entry);
        self.count
    }

    /// The register `handle` names; `dangling_register` if it names none (released, or never allocated).
    pub(crate) fn get(&self, handle: u64) -> Result<Entry, Fault> {
        self.live.get(&handle).copied().ok_or(Fault::DanglingRegister)
    }

    /// Retires `handle` when its register is released; the count stays.
    pub(crate) fn remove(&mut self, handle: u64) -> Entry {
        self.live.remove(&handle).expect("a released handle was allocated")
    }

    /// Undoes [`Handles::remove`].
    pub(crate) fn restore(&mut self, handle: u64, entry: Entry) {
        let previous = self.live.insert(handle, entry);
        debug_assert!(previous.is_none(), "a handle is released once");
    }

    /// Undoes [`Handles::insert`] of the last allocation: allocations are undone last first.
    pub(crate) fn unallocate(&mut self) -> Entry {
        let entry = self.live.remove(&self.count).expect("an allocation being undone is live");
        self.count -= 1;
        entry
    }

    /// The allocation count and each live handle with the simulator's handle of its register and its width,
    /// in handle order: the whole table, so that two states that differ only in which register a handle
    /// names are told apart.
    pub(crate) fn table(&self) -> (u64, Vec<(u64, u64, u32)>) {
        (self.count, self.live.iter().map(|(&handle, entry)| (handle, entry.register, entry.qubits.count())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(register: u64) -> Entry {
        Entry { register, qubits: Qubits::new(3).expect("a width") }
    }

    #[test]
    fn handles_count_from_one_and_are_never_reused() {
        let mut handles = Handles::default();
        assert_eq!(handles.get(0), Err(Fault::DanglingRegister));
        assert_eq!(handles.insert(entry(7)), 1);
        assert_eq!(handles.insert(entry(8)), 2);
        assert_eq!(handles.remove(1), entry(7));
        assert_eq!(handles.get(1), Err(Fault::DanglingRegister));
        assert_eq!(handles.insert(entry(7)), 3, "the released handle 1 is not given again");
        assert_eq!(handles.get(2), Ok(entry(8)));
        assert_eq!(handles.table(), (3, vec![(2, 8, 3), (3, 7, 3)]));
    }

    /// Two tables whose equal-width handles name each other's registers are different states.
    #[test]
    fn the_table_tells_apart_handles_that_exchange_registers() {
        let (mut first, mut second) = (Handles::default(), Handles::default());
        for (table, registers) in [(&mut first, [11, 12]), (&mut second, [12, 11])] {
            for register in registers {
                table.insert(Entry { register, qubits: Qubits::new(1).expect("a width") });
            }
        }
        assert_ne!(first.table(), second.table());
        assert_ne!(first, second);
    }

    #[test]
    fn a_register_value_holds_the_handle_above_the_head_column() {
        let value = register_value(u64::MAX, 7 << 32 | 5);
        assert_eq!((number_in(value), value & u128::from(u32::MAX)), (u64::MAX, 5));
        assert_eq!(number_in(0), 0);
    }

    #[test]
    fn the_last_handle_is_two_to_the_64_minus_one() {
        let mut handles = Handles { count: u64::MAX - 1, ..Handles::default() };
        assert_eq!(handles.admit(), Ok(()));
        assert_eq!(handles.insert(entry(0)), u64::MAX);
        assert_eq!(handles.admit(), Err(Fault::OutOfHandles));
        assert_eq!(handles.unallocate(), entry(0));
        assert_eq!(handles.admit(), Ok(()));
    }
}
