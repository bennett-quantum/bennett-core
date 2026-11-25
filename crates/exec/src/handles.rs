//! Handles as integers (`docs/DESIGN.md`, "Running a program"): the register made by the `k`-th successful
//! allocation of a run is named by the integer `k`, from 1, and no integer names two registers of one run. An
//! `i64` handle holds that integer; a `qN` slot holds the simulator's own handle of the register, which no
//! instruction lets a program read. This table maps each number to the simulator's handle and the width, and
//! each simulator's handle back to its number.

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
    /// The handle of each register allocated now, by the simulator's handle of it.
    numbers: BTreeMap<u64, u64>,
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
        self.numbers.insert(entry.register, self.count);
        self.count
    }

    /// The handle of the register whose simulator's handle is `register`; `dangling_register` if no live
    /// register has it.
    pub(crate) fn number_of(&self, register: u64) -> Result<u64, Fault> {
        self.numbers.get(&register).copied().ok_or(Fault::DanglingRegister)
    }

    /// The register `handle` names; `dangling_register` if it names none (released, or never allocated).
    pub(crate) fn get(&self, handle: u64) -> Result<Entry, Fault> {
        self.live.get(&handle).copied().ok_or(Fault::DanglingRegister)
    }

    /// Retires `handle` when its register is released; the count stays.
    pub(crate) fn remove(&mut self, handle: u64) -> Entry {
        let entry = self.live.remove(&handle).expect("a released handle was allocated");
        self.numbers.remove(&entry.register);
        entry
    }

    /// Undoes [`Handles::remove`].
    pub(crate) fn restore(&mut self, handle: u64, entry: Entry) {
        let previous = self.live.insert(handle, entry);
        debug_assert!(previous.is_none(), "a handle is released once");
        self.numbers.insert(entry.register, handle);
    }

    /// Undoes [`Handles::insert`] of the last allocation: allocations are undone last first.
    pub(crate) fn unallocate(&mut self) -> Entry {
        let entry = self.live.remove(&self.count).expect("an allocation being undone is live");
        self.numbers.remove(&entry.register);
        self.count -= 1;
        entry
    }

    /// The allocation count and each live handle with its width, in handle order.
    pub(crate) fn table(&self) -> (u64, Vec<(u64, u32)>) {
        (self.count, self.live.iter().map(|(&handle, entry)| (handle, entry.qubits.count())).collect())
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
        assert_eq!(handles.number_of(8), Ok(2));
        assert_eq!(handles.remove(1), entry(7));
        assert_eq!(handles.get(1), Err(Fault::DanglingRegister));
        assert_eq!(handles.number_of(7), Err(Fault::DanglingRegister));
        assert_eq!(handles.insert(entry(7)), 3, "the released handle 1 is not given again");
        assert_eq!(handles.number_of(7), Ok(3));
        assert_eq!(handles.get(2), Ok(entry(8)));
        assert_eq!(handles.table(), (3, vec![(2, 3), (3, 3)]));
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
