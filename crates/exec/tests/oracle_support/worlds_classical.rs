//! Persistent classical snapshots. Cold tables share std Rc ownership until a write.
use super::{Finish, Frame, Mark, Register, Timing, Value};
use rpds::{RedBlackTreeMap, Vector};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Classical {
    pub time: Timing,
    pub overlay: RedBlackTreeMap<u64, u8>,
    pub frames: Vector<Frame>,
    /// Includes dead slots, since a later activation may reuse them.
    pub slots: Vector<Value>,
    pub sp: u64,
    pub tape: Vector<u8>,
    /// DESIGN, History: one record per journalled step, including calls, marks and rewinds.
    /// This count is meaning, not the length or layout of our private snapshot history.
    pub journal_records: usize,
    pub finish: Option<Finish>,
    pub metadata: Rc<Metadata>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Metadata {
    pub registers: Vec<Register>,
    /// DESIGN, Qubits in memory: cells are part of the register table.
    pub cells: BTreeMap<u64, usize>,
    pub marks: Vec<Mark>,
    pub next_activation: u64,
}

// Preserve the interpreter's field notation; writes detach only this cold part of a snapshot.
impl Deref for Classical {
    type Target = Metadata;
    fn deref(&self) -> &Metadata {
        &self.metadata
    }
}
impl DerefMut for Classical {
    fn deref_mut(&mut self) -> &mut Metadata {
        Rc::make_mut(&mut self.metadata)
    }
}
