//! Slow, test-only meaning of DESIGN.md's journalled machine. No executor code is used.
//!
//! A basis key contains the ENTIRE machine, including history. Classical snapshots deliberately
//! use persistent collections for shared history, slots, frames and byte overlays. This is not a
//! production journal or a journal-cost model. Gates and apply
//! never put quantum values into a snapshot. Memory is an overlay over one immutable classical base.
//! Measurement is a boundary (all outcomes are available via `measurement_branches`); seeded RNG
//! trajectories and configuration-limit policy are not part of the unitary reference meaning.

use rpds::{RedBlackTreeMap, Vector};

#[path = "worlds_classical.rs"]
mod classical;
pub use classical::Classical;
use std::cell::{Cell, RefCell};

#[path = "worlds_replay.rs"]
mod replay;
#[path = "worlds_time.rs"]
mod time;
use std::collections::BTreeMap;
use time::MarkBudget;
pub use time::{Diagnostic, Timing};

use bennett_ir::{
    BlockId, CastOp, DataItem, Fault, FuncId, Inst, Keep, KeepTarget, MeasureSource, Operand, Place, Slot, Target,
    Terminator, Type, ValidModule,
};
use num_complex::Complex64 as C;

use super::dense::{binary, compare, gate_matrix, signed};

pub type Superposition = BTreeMap<World, C>;
type Promotion = (u64, usize, u8);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    Int(u128),
    Register(usize, u64),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frame {
    pub function: FuncId,
    pub block: BlockId,
    pub instruction: usize,
    pub base: usize,
    pub entry_sp: u64,
    pub activation: u64,
    pub result: Option<Slot>,
    pub fault: Option<Slot>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Register {
    pub width: Option<u32>,
    pub generation: u64,
    pub allocated_at: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mark {
    pub handle: usize,
    /// Private snapshot boundary, distinct from the public journal-record count.
    pub history_position: usize,
    pub activation: u64,
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Finish {
    Returned(Option<u128>),
    Faulted(Fault),
    BeforeMeasurement,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Classical,
    Mark,
    Measurement,
    Allocate(usize),
    Free(usize),
    LeftoverFree(usize),
    LeftBehind,
    Catch(Fault),
    /// The operation is read from the program position in `before`.
    Gate,
    /// Resolved sites and first-gate moves; no quantum values are saved by a gate.
    MemoryGate {
        target: (usize, u8),
        controls: Vec<(usize, u8)>,
        promoted: Vec<Promotion>,
    },
    /// Ordinary accesses retain the destroyed/read byte, independently for each basis world.
    Cells(Vec<(usize, u8, u8)>),
    Read(Vec<(usize, u64)>),
    Apply,
    JoinedRewind {
        start: Box<Classical>,
        visit: usize,
    },
    Rewind {
        start: Box<Classical>,
        count: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Record {
    pub before: Classical,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct World {
    pub classical: Classical,
    pub quantum: Vec<u64>,
    pub history: Vector<Record>,
}

impl World {
    fn mark_position(&self, handle: usize) -> Result<usize, Fault> {
        self.classical
            .marks
            .iter()
            .find(|m| m.handle == handle && m.valid && m.activation == self.frame().activation)
            .map(|m| m.history_position)
            .ok_or(Fault::BadRewind)
    }

    pub fn frame(&self) -> &Frame {
        self.classical.frames.last().expect("active frame")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.classical.frames.get_mut(self.classical.frames.len() - 1).expect("active frame")
    }

    pub fn read(&self, operand: Operand) -> Value {
        match operand {
            Operand::Const(c) => Value::Int(c.value()),
            Operand::Slot(s) => self.classical.slots[self.frame().base + s.index()].clone(),
        }
    }

    pub fn integer(&self, operand: Operand) -> Result<u128, Fault> {
        match self.read(operand) {
            Value::Int(n) => Ok(n),
            Value::Register(id, generation) => {
                self.live(id, generation)?;
                Ok(u128::from(self.quantum[id]))
            }
        }
    }

    fn live(&self, id: usize, generation: u64) -> Result<usize, Fault> {
        self.classical
            .registers
            .get(id)
            .filter(|r| r.generation == generation && r.width.is_some())
            .map(|_| id)
            .ok_or(Fault::DanglingRegister)
    }

    pub(super) fn register(&self, slot: Slot) -> Result<usize, Fault> {
        match self.read(Operand::Slot(slot)) {
            Value::Register(id, generation) => self.live(id, generation),
            Value::Int(_) => Err(Fault::DanglingRegister),
        }
    }

    fn put(&mut self, slot: Slot, value: Value) {
        let index = self.frame().base + slot.index();
        self.classical.slots[index] = value;
    }
}

fn add(state: &mut Superposition, world: World, amplitude: C) {
    // Time does not distinguish worlds. A merged world's clock is the later clock.
    if let Some((mut existing, a)) = state.remove_entry(&world) {
        existing.classical.time.clock = existing.classical.time.clock.max(world.classical.time.clock);
        state.insert(existing, a + amplitude);
    } else {
        state.insert(world, amplitude);
    }
}

fn clean(state: &mut Superposition) {
    state.retain(|_, a| a.norm() > f64::EPSILON);
}

pub fn basis(world: World) -> Superposition {
    BTreeMap::from([(world, C::new(1.0, 0.0))])
}

pub struct Model<'a> {
    pub program: &'a ValidModule,
    pub machine_meaning: bool,
    pub diagnostics: RefCell<Vec<Diagnostic>>,
    pub base: Vec<u8>,
    entry_result: Option<Type>,
    region: bool,
    region_depth: usize,
    /// Rounds of worlds run again while un-running a rewind (nested ones included, this model's own check
    /// that a rewind has an inverse excluded): the work charged to a budget (DESIGN, "History", Cost).
    replayed: Cell<u64>,
    unrunning: Cell<u32>,
    checking: Cell<u32>,
    replaying: Cell<u32>,
    replay_left: Cell<Option<u64>>,
    replay_exhausted: Cell<bool>,
}

impl<'a> Model<'a> {
    pub fn new(program: &'a ValidModule, entry: &str) -> Self {
        let mut base = vec![0; program.memory_size as usize];
        for segment in &program.data {
            let mut address = segment.address as usize;
            for item in &segment.items {
                let bytes = match item {
                    DataItem::Bytes(b) => b.clone(),
                    DataItem::Zero(n) => vec![0; *n as usize],
                    DataItem::Int(c) => c.value().to_le_bytes()[..c.width().bytes() as usize].to_vec(),
                    DataItem::FuncAddr { width, func } => {
                        (u128::from(func.0) + 1).to_le_bytes()[..width.bytes() as usize].to_vec()
                    }
                };
                base[address..address + bytes.len()].copy_from_slice(&bytes);
                address += bytes.len();
            }
        }
        Self {
            program,
            machine_meaning: false,
            diagnostics: RefCell::new(Vec::new()),
            base,
            entry_result: program.function(program.function_named(entry).expect("entry")).result,
            region: false,
            region_depth: 0,
            replayed: Cell::new(0),
            unrunning: Cell::new(0),
            checking: Cell::new(0),
            replaying: Cell::new(0),
            replay_left: Cell::new(None),
            replay_exhausted: Cell::new(false),
        }
    }

    /// Rounds run again while un-running rewinds so far (`replayed`).
    pub fn replayed(&self) -> u64 {
        self.replayed.get()
    }

    /// As [`Model::run`], for a `budget` that bounds the rounds and the rounds run again together: a round
    /// at the budget faults `budget`, and so does a rewind whose running again would pass it, leaving the
    /// state as it was.
    pub fn run_budgeted(&self, mut state: Superposition, budget: usize) -> Superposition {
        let (mut rounds, mut charged) = (0u64, 0u64);
        while !state.keys().all(|w| w.classical.finish.is_some()) && rounds + charged < budget as u64 {
            let before = self.replayed();
            let next = self.step_with_replay_budget(&state, budget as u64 - rounds - charged - 1);
            let charge = self.replayed() - before;
            if rounds + charged + charge + 1 > budget as u64 {
                break;
            }
            (rounds, charged, state) = (rounds + 1, charged + charge, next);
        }
        for (mut w, a) in std::mem::take(&mut state) {
            if w.classical.finish.is_none() {
                w.classical.finish = Some(Finish::Faulted(Fault::Budget));
            }
            add(&mut state, w, a);
        }
        state
    }

    pub fn initial(&self, entry: &str, args: &[u128]) -> World {
        let mut world = World {
            classical: Classical {
                time: Timing::default(),
                overlay: RedBlackTreeMap::new(),
                frames: Vector::new(),
                // Eight-way slot nodes copy fewer pointers per retained point update than
                // the crate's default 32-way nodes (the table hit the 1 GiB cap with those).
                slots: Vector::new_with_bits(3),
                sp: self.program.memory_size,
                tape: Vector::new(),
                metadata: Default::default(),
                journal_records: 0,
                finish: None,
            },
            quantum: Vec::new(),
            history: Vector::new(),
        };
        // An entry frame that does not fit ends the run before its first step, with no frame.
        if let Err(fault) = self.call(
            &mut world,
            self.program.function_named(entry).expect("entry"),
            args.iter().copied().map(Value::Int).collect(),
            None,
        ) {
            world.classical.finish = Some(Finish::Faulted(fault));
        }
        world
    }

    pub fn memory(&self, world: &World) -> Vec<u8> {
        let mut memory = self.base.clone();
        for (&address, &byte) in &world.classical.overlay {
            memory[address as usize] = byte;
        }
        memory
    }

    fn read_memory(&self, world: &World, address: u64, bytes: usize) -> Result<Vec<u8>, Fault> {
        let end = address.checked_add(bytes as u64).ok_or(Fault::OutOfBounds)?;
        if end > self.program.memory_size {
            return Err(Fault::OutOfBounds);
        }
        Ok((address..end)
            .map(|a| {
                world.classical.cells.get(&a).map_or_else(
                    || world.classical.overlay.get(&a).copied().unwrap_or(self.base[a as usize]),
                    |&id| world.quantum[id] as u8,
                )
            })
            .collect())
    }

    fn write_memory(&self, world: &mut World, address: u64, bytes: &[u8]) -> Result<(), Fault> {
        self.read_memory(world, address, bytes.len())?;
        for (offset, &byte) in bytes.iter().enumerate() {
            let address = address + offset as u64;
            let byte = if let Some(&id) = world.classical.cells.get(&address) {
                world.quantum[id] = u64::from(byte);
                0
            } else {
                byte
            };
            if byte == self.base[address as usize] {
                world.classical.overlay.remove_mut(&address);
            } else {
                world.classical.overlay.insert_mut(address, byte);
            }
        }
        Ok(())
    }

    // DESIGN: the first gate allocates an eight-qubit column and moves the old byte.
    fn promote(&self, w: &mut World, address: u64) -> Result<(usize, Option<Promotion>), Fault> {
        if let Some(&id) = w.classical.cells.get(&address) {
            return Ok((id, None));
        }
        let old = self.read_memory(w, address, 1)?[0];
        self.write_memory(w, address, &[0])?;
        let id = w.classical.registers.iter().position(|r| r.width.is_none()).unwrap_or(w.classical.registers.len());
        let generation = w.classical.registers.get(id).map_or(1, |r| r.generation + 1);
        let info = Register { width: Some(8), generation, allocated_at: w.history.len() };
        if id == w.classical.registers.len() {
            w.classical.registers.push(info);
            w.quantum.push(old.into());
        } else {
            w.classical.registers[id] = info;
            w.quantum[id] = old.into();
        }
        w.classical.cells.insert(address, id);
        Ok((id, Some((address, id, old))))
    }

    fn touched(&self, w: &World, address: u64, bytes: usize) -> Result<Vec<(usize, u8, u8)>, Fault> {
        self.read_memory(w, address, bytes)?;
        Ok(w.classical
            .cells
            .range(address..address + bytes as u64)
            .map(|(_, &id)| (id, w.quantum[id] as u8, w.quantum[id] as u8))
            .collect())
    }

    fn keeps(&self, w: &World, keep: &[Keep], mark: Option<usize>) -> Result<Vec<(usize, u64)>, Fault> {
        let mut values = Vec::new();
        for k in keep {
            let value = w.integer(k.value)?;
            match k.target {
                KeepTarget::Register(r) => values.push((w.register(r)?, value as u64)),
                KeepTarget::Memory(a) => {
                    let a = w.integer(a)? as u64;
                    let ty = match k.value {
                        Operand::Const(c) => Type::Int(c.width()),
                        Operand::Slot(s) => self.program.slot_type(w.frame().function, s),
                    };
                    let Type::Int(width) = ty else { unreachable!() };
                    self.read_memory(w, a, width.bytes() as usize)?;
                    for i in 0..width.bytes() {
                        let id = *w.classical.cells.get(&(a + i)).ok_or(Fault::BadRewind)?;
                        values.push((id, ((value >> (8 * i)) & 255) as u64));
                    }
                }
            }
        }
        if values.iter().any(|&(id, _)| mark.is_some_and(|h| w.classical.registers[id].allocated_at >= h)) {
            return Err(Fault::BadRewind);
        }
        Ok(values)
    }

    fn call(&self, w: &mut World, func: FuncId, args: Vec<Value>, result: Option<Slot>) -> Result<(), Fault> {
        let function = self.program.function(func);
        let entry_sp = w.classical.sp;
        let sp = entry_sp
            .checked_sub(8 * function.slot_count() as u64)
            .filter(|&p| p >= self.program.static_end())
            .ok_or(Fault::StackOverflow)?;
        let base = w.classical.frames.last().map_or(0, |f| f.base + self.program.function(f.function).slot_count());
        let mut values = Vec::new();
        for (param, value) in function.params.iter().zip(args) {
            values.push(match (param.ty, value) {
                (Type::Int(_), Value::Register(id, generation)) => {
                    w.live(id, generation)?;
                    Value::Int(w.quantum[id].into())
                }
                (_, v) => v,
            });
        }
        while w.classical.slots.len() < base + function.slot_count() {
            w.classical.slots.push_back_mut(Value::Int(0));
        }
        w.classical.next_activation += 1;
        w.classical.frames.push_back_mut(Frame {
            function: func,
            block: BlockId(0),
            instruction: 0,
            base,
            entry_sp,
            activation: w.classical.next_activation,
            result,
            fault: None,
        });
        w.classical.sp = sp;
        for (param, value) in function.params.iter().zip(values) {
            w.put(param.slot, value);
        }
        Ok(())
    }

    /// DESIGN, `measure`: a register's value, or the `width` bits from bit 0 of the integer at an address,
    /// read as a load reads them (cells' bytes from their registers, other bytes from memory).
    fn measured(&self, w: &World, source: &MeasureSource) -> Result<u128, Fault> {
        match *source {
            MeasureSource::Register(register) => Ok(w.quantum[w.register(register)?].into()),
            MeasureSource::Memory { width, address } => {
                let bytes = self.read_memory(w, w.integer(address)? as u64, width.bytes() as usize)?;
                let mut n = [0; 16];
                n[..bytes.len()].copy_from_slice(&bytes);
                Ok(u128::from_le_bytes(n) & width.mask())
            }
        }
    }

    /// The value the `measure` at `w`'s position would see in this basis world, or `None` when the next
    /// step is not a measurement or faults.
    pub fn measurement_at(&self, w: &World) -> Option<u128> {
        let Some(Inst::Measure { source, .. }) = self.instruction(&w.classical) else { return None };
        self.measured(w, source).ok()
    }

    fn instruction(&self, classical: &Classical) -> Option<&Inst> {
        let f = classical.frames.last()?;
        self.program.function(f.function).block(f.block).insts.get(f.instruction)
    }

    /// One documented instruction on EVERY world. Finished worlds wait unchanged. A dirty free or
    /// failed inverse is atomic across the participating superposition, as on today's machine.
    pub fn step(&self, state: &Superposition) -> Superposition {
        if self.replaying.get() == 0 && self.checking.get() == 0 {
            self.diagnostics.borrow_mut().clear();
        }
        let mut groups = BTreeMap::<_, Superposition>::new();
        for (w, &a) in state {
            add(groups.entry((w.classical.clone(), w.history.clone())).or_default(), w.clone(), a);
        }
        let mut next = BTreeMap::new();
        for group in groups.into_values() {
            match self.step_group(&group) {
                Ok(outputs) => {
                    for (w, a) in outputs {
                        add(&mut next, w, a);
                    }
                }
                Err(fault) => {
                    self.diagnostics.borrow_mut().push(Diagnostic::World(fault));
                    for (mut w, a) in group {
                        if fault != Fault::Budget
                            && self.replaying.get() == 0
                            && self.unrunning.get() == 0
                            && self.checking.get() == 0
                        {
                            w.classical.time.clock = w.classical.time.clock.saturating_add(1);
                        }
                        w.classical.finish = Some(Finish::Faulted(fault));
                        add(&mut next, w, a);
                    }
                }
            }
        }
        clean(&mut next);
        next
    }

    fn step_group_raw(&self, state: &Superposition) -> Result<Superposition, Fault> {
        let first = state.keys().next().expect("nonempty group");
        if first.classical.finish.is_none()
            && let Some(Inst::Rewind { mark, keep }) = self.instruction(&first.classical)
        {
            return self.rewind_group(state, *mark, keep);
        }
        // DESIGN's explicit debt: unlike a load, longjmp over disagreeing record cells
        // currently faults bad_longjmp for the entire world rather than splitting it.
        if first.classical.finish.is_none()
            && self.instruction(&first.classical).is_none()
            && let Terminator::Longjmp { buffer, .. } =
                self.program.function(first.frame().function).block(first.frame().block).term
        {
            let address = first.integer(buffer)? as u64;
            let bytes = self.read_memory(first, address, 32)?;
            if state.keys().any(|w| self.read_memory(w, address, 32).as_ref() != Ok(&bytes)) {
                return Err(Fault::BadLongjmp);
            }
        }
        if let Some(Inst::Qfree { register }) = self.instruction(&first.classical)
            && !self.region
            && !first.classical.frames.iter().any(|f| f.fault.is_some())
        {
            let id = first.register(*register)?;
            if state.keys().any(|w| w.quantum[id] != 0) {
                if !self.machine_meaning {
                    return Err(Fault::DirtyFree);
                }
                let mut next = BTreeMap::new();
                for (w, &a) in state {
                    let mut out = w.clone();
                    out.classical.registers[id].generation += 1;
                    out.classical.time.leftovers.insert(id);
                    out.frame_mut().instruction += 1;
                    out.classical.journal_records += 1;
                    out.history.push_back_mut(Record { before: w.classical.clone(), action: Action::LeftoverFree(id) });
                    add(&mut next, out, a);
                }
                return Ok(next);
            }
        }
        let mut next = BTreeMap::new();
        for (w, &a) in state {
            for (out, b) in self.forward(w.clone())? {
                add(&mut next, out, a * b);
            }
        }
        clean(&mut next);
        Ok(next)
    }

    /// Returns the final complete state AND its per-world outcomes, without projecting out history.
    /// `limit` is the number of forward rounds, never a bound on un-run/replay work.
    pub fn run(&self, mut state: Superposition, limit: usize) -> Superposition {
        for _ in 0..limit {
            if state.keys().all(|w| w.classical.finish.is_some()) {
                return state;
            }
            state = self.step(&state);
        }
        for (mut w, a) in std::mem::take(&mut state) {
            if w.classical.finish.is_none() {
                w.classical.finish = Some(Finish::Faulted(Fault::Budget));
            }
            add(&mut state, w, a);
        }
        state
    }

    fn forward(&self, mut w: World) -> Result<Superposition, Fault> {
        if w.classical.finish.is_some() {
            return Ok(basis(w));
        }
        let before = w.classical.clone();
        let action = if let Some(inst) = self.instruction(&before) {
            if (self.region || w.classical.frames.iter().any(|f| f.fault.is_some()))
                && matches!(
                    inst,
                    Inst::Qalloc { .. }
                        | Inst::Qfree { .. }
                        | Inst::Gate { .. }
                        | Inst::GateAt { .. }
                        | Inst::Measure { .. }
                        | Inst::Apply { .. }
                )
            {
                return Err(Fault::QuantumInRegion);
            }
            if let Inst::Measure { source, .. } = inst {
                self.measured(&w, source)?;
                w.classical.finish = Some(Finish::BeforeMeasurement);
                return Ok(basis(w));
            }
            if let Inst::Rewind { mark, keep } = inst {
                return self.rewind_group(&basis(w), *mark, keep);
            }
            let mut result = None;
            let mut action = Action::Classical;
            match inst {
                Inst::Binary { op, width, lhs, rhs, .. } => {
                    result = Some(Value::Int(binary(*op, width.bits(), w.integer(*lhs)?, w.integer(*rhs)?)?));
                }
                Inst::Compare { pred, width, lhs, rhs, .. } => {
                    result = Some(Value::Int(compare(*pred, width.bits(), w.integer(*lhs)?, w.integer(*rhs)?) as u128));
                }
                Inst::Select { cond, if_true, if_false, .. } => {
                    result = Some(w.read(if w.integer(*cond)? != 0 { *if_true } else { *if_false }));
                }
                Inst::Cast { op, from, to, value, .. } => {
                    let n = w.integer(*value)?;
                    result = Some(Value::Int(
                        if *op == CastOp::Sext { signed(n, from.bits()) as u128 } else { n } & to.mask(),
                    ));
                }
                Inst::Load { width, address, .. } => {
                    let address = w.integer(*address)? as u64;
                    action = Action::Cells(self.touched(&w, address, width.bytes() as usize)?);
                    let bytes = self.read_memory(&w, address, width.bytes() as usize)?;
                    let mut n = [0; 16];
                    n[..bytes.len()].copy_from_slice(&bytes);
                    result = Some(Value::Int(u128::from_le_bytes(n) & width.mask()));
                }
                Inst::Store { width, address, value } => {
                    let a = w.integer(*address)? as u64;
                    let n = w.integer(*value)?.to_le_bytes();
                    let cells = self.touched(&w, a, width.bytes() as usize)?;
                    self.write_memory(&mut w, a, &n[..width.bytes() as usize])?;
                    action =
                        Action::Cells(cells.into_iter().map(|(id, old, _)| (id, old, w.quantum[id] as u8)).collect());
                }
                Inst::Alloca { size, .. } => {
                    w.classical.sp = w
                        .classical
                        .sp
                        .checked_sub(w.integer(*size)? as u64)
                        .map(|p| p & !7)
                        .filter(|&p| p >= self.program.static_end())
                        .ok_or(Fault::StackOverflow)?;
                    result = Some(Value::Int(w.classical.sp.into()));
                }
                Inst::StackSave { .. } => result = Some(Value::Int(w.classical.sp.into())),
                Inst::ReturnAddress { level, .. } => {
                    // docs/SYNTAX.md: the caller's function address in the high half, the position after its
                    // call in the low half (every instruction and terminator counted in block order); the
                    // outermost frame gives 0xFFFF_FFFF_0000_0000 and a level beyond it 0.
                    // A region keeps its caller's frames, so the frames below it are read directly.
                    let frames: Vec<_> =
                        w.classical.frames.iter().map(|f| (f.function, f.block, f.instruction)).collect();
                    let callee = frames.len() as i64 - 1 - i64::from(*level);
                    let address = if callee < 0 {
                        0
                    } else if callee == 0 {
                        0xFFFF_FFFF_0000_0000
                    } else {
                        let (function, block, instruction) = frames[callee as usize - 1];
                        let blocks = &self.program.function(function).blocks;
                        let before: usize = blocks.iter().take(block.0 as usize).map(|b| b.insts.len() + 1).sum();
                        (u64::from(function.0) + 1) << 32 | (before + instruction + 1) as u64
                    };
                    result = Some(Value::Int(address.into()));
                }
                Inst::StackRestore { pointer } => {
                    let p = w.integer(*pointer)? as u64;
                    let ceiling =
                        w.frame().entry_sp - 8 * self.program.function(w.frame().function).slot_count() as u64;
                    if p < w.classical.sp || p > ceiling {
                        return Err(Fault::BadStackRestore);
                    }
                    w.classical.sp = p;
                }
                Inst::Setjmp { buffer, .. } => {
                    let f = w.frame();
                    let words = [
                        ((f.function.0 as u64) << 32) | (w.classical.frames.len() - 1) as u64,
                        ((f.instruction as u64) << 32) | f.block.0 as u64,
                        f.entry_sp,
                        w.classical.sp,
                    ];
                    let bytes: Vec<_> = words.into_iter().flat_map(u64::to_le_bytes).collect();
                    let address = w.integer(*buffer)? as u64;
                    let cells = self.touched(&w, address, bytes.len())?;
                    self.write_memory(&mut w, address, &bytes)?;
                    action =
                        Action::Cells(cells.into_iter().map(|(id, old, _)| (id, old, w.quantum[id] as u8)).collect());
                    result = Some(Value::Int(0));
                }
                Inst::Call { callee, args, dst } | Inst::CallBudget { callee, args, dst, .. } => {
                    let reads = args
                        .iter()
                        .zip(&self.program.function(*callee).params)
                        .filter_map(|(a, p)| match (w.read(*a), p.ty) {
                            (Value::Register(id, _), Type::Int(_)) => Some((id, w.quantum[id])),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if !reads.is_empty() {
                        action = Action::Read(reads);
                    }
                    let args = args.iter().map(|a| w.read(*a)).collect();
                    let guard = if let Inst::CallBudget { fault, budget, .. } = inst {
                        Some((*fault, w.integer(*budget)? as u64))
                    } else {
                        None
                    };
                    self.call(&mut w, *callee, args, *dst)?;
                    if let Some((fault, budget)) = guard {
                        w.frame_mut().fault = Some(fault);
                        let activation = w.frame().activation;
                        w.classical.time.guards.insert(activation, budget);
                    }
                    w.classical.journal_records += 1;
                    w.history.push_back_mut(Record { before, action });
                    return Ok(basis(w));
                }
                Inst::CallIndirect { signature, target, args, dst } => {
                    let address = w.integer(*target)?;
                    let index =
                        address.checked_sub(1).and_then(|i| usize::try_from(i).ok()).ok_or(Fault::BadIndirect)?;
                    if !self.program.functions.get(index).is_some_and(|f| f.has_signature(signature)) {
                        return Err(Fault::BadIndirect);
                    }
                    let reads = args
                        .iter()
                        .zip(&self.program.functions[index].params)
                        .filter_map(|(a, p)| match (w.read(*a), p.ty) {
                            (Value::Register(id, _), Type::Int(_)) => Some((id, w.quantum[id])),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if !reads.is_empty() {
                        action = Action::Read(reads);
                    }
                    let args = args.iter().map(|a| w.read(*a)).collect();
                    self.call(&mut w, FuncId(index as u32), args, *dst)?;
                    w.classical.journal_records += 1;
                    w.history.push_back_mut(Record { before, action });
                    return Ok(basis(w));
                }
                Inst::FuncAddr { func, .. } => result = Some(Value::Int(u128::from(func.0) + 1)),
                Inst::Effect { byte } => {
                    if self.region || w.classical.frames.iter().any(|f| f.fault.is_some()) {
                        return Err(Fault::EffectInRegion);
                    }
                    w.classical.tape.push_back_mut(w.integer(*byte)? as u8);
                }
                Inst::Qalloc { width, .. } => {
                    let id = w
                        .classical
                        .registers
                        .iter()
                        .position(|r| r.width.is_none())
                        .unwrap_or(w.classical.registers.len());
                    let generation = w.classical.registers.get(id).map_or(1, |r| r.generation + 1);
                    let info = Register { width: Some(width.bits()), generation, allocated_at: w.history.len() };
                    if id == w.classical.registers.len() {
                        w.classical.registers.push(info);
                        w.quantum.push(0);
                    } else {
                        w.classical.registers[id] = info;
                        w.quantum[id] = 0;
                    }
                    result = Some(Value::Register(id, generation));
                    action = Action::Allocate(id);
                }
                Inst::Qfree { register } => {
                    let id = w.register(*register)?;
                    if w.quantum[id] != 0 && self.machine_meaning {
                        w.classical.registers[id].generation += 1;
                        w.classical.time.leftovers.insert(id);
                        action = Action::LeftoverFree(id);
                    } else {
                        if w.quantum[id] != 0 {
                            return Err(Fault::DirtyFree);
                        }
                        w.classical.registers[id].width = None;
                        action = Action::Free(id);
                    }
                }
                Inst::Gate { .. } => action = Action::Gate,
                Inst::GateAt { target, controls, .. } => {
                    let mut promoted = Vec::new();
                    let mut sites = Vec::new();
                    for place in std::iter::once(target).chain(controls) {
                        let site = match *place {
                            Place::Register(q) => (w.register(q.register)?, q.bit),
                            Place::Memory { address, bit } => {
                                let address = (w.integer(address)? as u64)
                                    .checked_add(u64::from(bit / 8))
                                    .ok_or(Fault::OutOfBounds)?;
                                let (id, promotion) = self.promote(&mut w, address)?;
                                promoted.extend(promotion);
                                (id, bit % 8)
                            }
                        };
                        sites.push(site);
                    }
                    action = Action::MemoryGate { target: sites[0], controls: sites[1..].to_vec(), promoted };
                }
                Inst::Apply { .. } => action = Action::Apply,
                Inst::Mark { budget, .. } => {
                    let limit = budget.map(|b| w.integer(b).map(|b| b as u64)).transpose()?;
                    let handle = w.classical.journal_records + 1;
                    result = Some(Value::Int(handle as u128));
                    let activation = w.frame().activation;
                    w.classical.marks.push(Mark {
                        handle,
                        history_position: w.history.len() + 1,
                        activation,
                        valid: true,
                    });
                    let clock = w.classical.time.clock;
                    w.classical.time.marks.insert(
                        handle,
                        match limit {
                            None => MarkBudget::Unbudgeted,
                            Some(b) => clock
                                .saturating_add(1)
                                .checked_add(b)
                                .map_or(MarkBudget::Unbounded, MarkBudget::Deadline),
                        },
                    );
                    action = Action::Mark;
                }
                Inst::Measure { .. } | Inst::Rewind { .. } => unreachable!(),
            }
            if let (Some(dst), Some(value)) = (inst.result(), result) {
                w.put(dst, value);
            }
            w.frame_mut().instruction += 1;
            action
        } else {
            let term = &self.program.function(w.frame().function).block(w.frame().block).term;
            let mut action = Action::Classical;
            match term {
                Terminator::Jump(t) => self.jump(&mut w, t)?,
                Terminator::Branch { cond, if_true, if_false } => {
                    let target = if w.integer(*cond)? != 0 { if_true } else { if_false };
                    self.jump(&mut w, target)?;
                }
                Terminator::Switch { value, cases, default, .. } => {
                    let n = w.integer(*value)?;
                    self.jump(&mut w, cases.iter().find(|c| c.value == n).map_or(default, |c| &c.target))?;
                }
                Terminator::Return(value) => {
                    let value = value.map(|v| w.integer(v)).transpose()?;
                    if w.classical.frames.len() == self.region_depth + 1 {
                        w.classical.finish = Some(Finish::Returned(value));
                    } else {
                        let frame = {
                            let f = w.classical.frames.last().expect("callee").clone();
                            w.classical.frames.drop_last_mut();
                            f
                        };
                        self.leave(&mut w, frame.activation);
                        w.classical.sp = frame.entry_sp;
                        w.frame_mut().instruction += 1;
                        if let (Some(dst), Some(n)) = (frame.result, value) {
                            w.put(dst, Value::Int(n));
                        }
                        if let Some(fault) = frame.fault {
                            w.put(fault, Value::Int(0));
                        }
                        w.classical.time.guards.remove(&frame.activation);
                    }
                }
                Terminator::Exit(value) => {
                    if self.region || w.classical.frames.iter().any(|f| f.fault.is_some()) {
                        return Err(Fault::ExitInRegion);
                    }
                    let ty = match value {
                        Operand::Const(c) => Type::Int(c.width()),
                        Operand::Slot(s) => self.program.slot_type(w.frame().function, *s),
                    };
                    if Some(ty) != self.entry_result {
                        return Err(Fault::ExitTypeMismatch);
                    }
                    w.classical.finish = Some(Finish::Returned(Some(w.integer(*value)?)));
                }
                Terminator::Longjmp { buffer, value } => {
                    let address = w.integer(*buffer)? as u64;
                    action = Action::Cells(self.touched(&w, address, 32)?);
                    self.longjmp(&mut w, *buffer, *value)?;
                }
                Terminator::Fault(f) => return Err(*f),
            }
            action
        };
        let action = if matches!(&action, Action::Cells(c) if c.is_empty()) { Action::Classical } else { action };
        let record = Record { before, action };
        let mut state = self.quantum_action(w, &record, false)?;
        for (mut world, a) in std::mem::take(&mut state) {
            world.classical.journal_records += 1;
            world.history.push_back_mut(record.clone());
            add(&mut state, world, a);
        }
        Ok(state)
    }

    fn leave(&self, w: &mut World, activation: u64) {
        for m in &mut w.classical.marks {
            if m.activation == activation {
                m.valid = false;
            }
        }
    }

    fn jump(&self, w: &mut World, target: &Target) -> Result<(), Fault> {
        let values: Vec<_> = target.args.iter().map(|a| w.read(*a)).collect();
        w.frame_mut().block = target.block;
        w.frame_mut().instruction = 0;
        for (p, v) in self.program.function(w.frame().function).block(target.block).params.iter().zip(values) {
            w.put(p.slot, v);
        }
        Ok(())
    }

    fn longjmp(&self, w: &mut World, buffer: Operand, value: Operand) -> Result<(), Fault> {
        let bytes = self.read_memory(w, w.integer(buffer)? as u64, 32)?;
        let words: Vec<_> = bytes.as_chunks::<8>().0.iter().map(|b| u64::from_le_bytes(*b)).collect();
        let depth = (words[0] & 0xffff_ffff) as usize;
        let func = FuncId((words[0] >> 32) as u32);
        let block = BlockId(words[1] as u32);
        let inst = (words[1] >> 32) as usize;
        if depth < self.region_depth
            || w.classical.frames.iter().enumerate().any(|(i, f)| f.fault.is_some() && depth < i)
        {
            return Err(Fault::BadLongjmp);
        }
        let frame = w.classical.frames.get(depth).ok_or(Fault::BadLongjmp)?;
        if frame.function != func || frame.entry_sp != words[2] {
            return Err(Fault::BadLongjmp);
        }
        let Some(Inst::Setjmp { dst, .. }) =
            self.program.function(func).blocks.get(block.index()).and_then(|b| b.insts.get(inst))
        else {
            return Err(Fault::BadLongjmp);
        };
        let ceiling = frame.entry_sp - 8 * self.program.function(func).slot_count() as u64;
        let floor = w.classical.frames.get(depth + 1).map_or(w.classical.sp, |f| f.entry_sp);
        if words[3] < floor || words[3] > ceiling {
            return Err(Fault::BadLongjmp);
        }
        let n = w.integer(value)?;
        while w.classical.frames.len() > depth + 1 {
            let f = {
                let f = w.classical.frames.last().expect("discarded frame").clone();
                w.classical.frames.drop_last_mut();
                f
            };
            self.leave(w, f.activation);
        }
        w.classical.sp = words[3];
        w.frame_mut().block = block;
        w.frame_mut().instruction = inst + 1;
        w.put(*dst, Value::Int(n));
        Ok(())
    }

    fn quantum_action(&self, mut w: World, record: &Record, inverse: bool) -> Result<Superposition, Fault> {
        match record.action {
            Action::Gate | Action::MemoryGate { .. } => {
                let inst = self.instruction(&record.before).expect("gate");
                let (gate, target, bits) = match (&record.action, inst) {
                    (Action::MemoryGate { target, controls, .. }, Inst::GateAt { gate, .. }) => {
                        (*gate, *target, std::iter::once(*target).chain(controls.iter().copied()).collect::<Vec<_>>())
                    }
                    (Action::Gate, Inst::Gate { gate, target, controls }) => {
                        let current = std::mem::replace(&mut w.classical, record.before.clone());
                        let target = (w.register(target.register)?, target.bit);
                        let mut bits = vec![target];
                        for c in controls {
                            bits.push((w.register(c.register)?, c.bit));
                        }
                        w.classical = current;
                        (*gate, target, bits)
                    }
                    _ => unreachable!(),
                };
                let (target_id, target_bit) = target;
                let mut distinct = bits.clone();
                distinct.sort();
                distinct.dedup();
                if bits.len() != distinct.len() {
                    return Err(Fault::QubitAlias);
                }
                let enabled = bits[1..].iter().all(|&(r, b)| w.quantum[r] >> b & 1 == 1);
                if !enabled {
                    return Ok(basis(w));
                }
                let m = gate_matrix(gate);
                let m = if inverse { m.adjoint() } else { m };
                let input = ((w.quantum[target_id] >> target_bit) & 1) as usize;
                let mut state = BTreeMap::new();
                for output in 0..2 {
                    let mut out = w.clone();
                    out.quantum[target_id] =
                        (out.quantum[target_id] & !(1 << target_bit)) | ((output as u64) << target_bit);
                    add(&mut state, out, m[(output, input)]);
                }
                clean(&mut state);
                Ok(state)
            }
            Action::Apply => {
                let Some(Inst::Apply { callee, args, out, fault, budget }) = self.instruction(&record.before) else {
                    unreachable!()
                };
                let current = std::mem::replace(&mut w.classical, record.before.clone());
                let out = w.register(*out)?;
                let fault = w.register(*fault)?;
                if out == fault {
                    return Err(Fault::QubitAlias);
                }
                for arg in args {
                    if let Value::Register(id, generation) = w.read(*arg) {
                        w.live(id, generation)?;
                        if id == out || id == fault {
                            return Err(Fault::QubitAlias);
                        }
                    }
                }
                let values = args.iter().map(|a| w.integer(*a).map(Value::Int)).collect::<Result<Vec<_>, _>>()?;
                let budget = w.integer(*budget)? as usize;
                let mut region = w.clone();
                region.history = Vector::new();
                // The basis-state definition evaluates a private region with a fresh journal.
                // Entering @f is a call: it contributes one public record, even though this
                // reference never needs to undo that entry and stores no snapshot for it.
                // DESIGN's later caller-journal idiom leaves the absolute origin in tension
                // with apply's self-inverse law (F1). Cross-mode tests use differences of marks;
                // absolute-number comparisons here are explicitly scoped to fresh enumeration.
                region.classical.journal_records = 1;
                let cells: Vec<_> =
                    region.classical.cells.iter().map(|(&a, &id)| (a, region.quantum[id] as u8)).collect();
                region.classical.cells.clear();
                for (a, byte) in cells {
                    self.write_memory(&mut region, a, &[byte])?;
                }
                region.quantum.clear();
                region.classical.registers.clear();
                region.classical.marks.clear();
                region.classical.finish = None;
                let mut model = Model::new(self.program, &self.program.function(*callee).name);
                model.base = self.base.clone();
                model.region = true;
                model.region_depth = region.classical.frames.len();
                let result = match model.call(&mut region, *callee, values, None) {
                    Err(f) => Finish::Faulted(f),
                    Ok(()) => model
                        .run_budgeted(basis(region), budget)
                        .into_keys()
                        .next()
                        .expect("classical region")
                        .classical
                        .finish
                        .expect("region end"),
                };
                w.classical = current;
                match result {
                    Finish::Returned(Some(n)) => w.quantum[out] ^= n as u64,
                    Finish::Returned(None) => {}
                    Finish::Faulted(f) => {
                        if self.replaying.get() == 0 && self.checking.get() == 0 && self.unrunning.get() == 0 {
                            self.diagnostics.borrow_mut().push(Diagnostic::Region(f));
                        }
                        w.quantum[fault] ^= f.code();
                    }
                    Finish::BeforeMeasurement => unreachable!("quantum region faults first"),
                }
                Ok(basis(w))
            }
            _ => Ok(basis(w)),
        }
    }

    fn rewind_group(&self, inputs: &Superposition, mark: Slot, keep: &[Keep]) -> Result<Superposition, Fault> {
        if (self.region || inputs.keys().next().is_some_and(|w| w.classical.frames.iter().any(|f| f.fault.is_some())))
            && !keep.is_empty()
        {
            return Err(Fault::QuantumInRegion);
        }
        let first = inputs.keys().next().expect("nonempty rewind");
        let handle = first.integer(Operand::Slot(mark))? as usize;
        let h = first.mark_position(handle)?;
        self.rewind_time(first, handle, h)?;
        if h == 0
            || h > first.history.len()
            || first.history[h - 1].action != Action::Mark
            || first.history.iter().skip(h).any(|r| matches!(r.action, Action::Measurement | Action::LeftBehind))
        {
            return Err(Fault::BadRewind);
        }
        let mut state = BTreeMap::new();
        for (w, &a) in inputs {
            let mut out = w.clone();
            for (r, n) in self.keeps(w, keep, Some(h))? {
                out.quantum[r] ^= n;
            }
            add(&mut state, out, a);
        }
        let mut count = 0;
        self.unrunning.set(self.unrunning.get() + 1);
        let unrun = (|| {
            while state.keys().any(|w| w.history.len() > h) {
                state = self
                    .undo_state(&state)
                    .map_err(|_| if self.replay_exhausted.get() { Fault::Budget } else { Fault::BadRewind })?;
                count += 1;
            }
            Ok(())
        })();
        self.unrunning.set(self.unrunning.get() - 1);
        unrun?;
        let mut result = BTreeMap::new();
        for (mut w, a) in state {
            let mut start = Box::new(w.classical.clone());
            start.time.guards = first
                .history
                .get(h)
                .map_or_else(|| first.classical.time.guards.clone(), |r| r.before.time.guards.clone());
            let clock = self.rewind_time(first, handle, h)?;
            let leftovers = w.classical.time.leftovers.clone();
            w.classical.time = first.classical.time.clone();
            w.classical.time.leftovers.extend(leftovers);
            w.classical.time.clock = clock;
            if first.classical.time.marks.get(&handle).is_some_and(|b| !matches!(b, MarkBudget::Unbudgeted)) {
                w.classical.time.marks.insert(handle, MarkBudget::Unbounded);
            }
            let mut site = (*start).clone();
            let f = first.frame();
            site.frames.get_mut(site.frames.len() - 1).expect("frame").block = f.block;
            site.frames.get_mut(site.frames.len() - 1).expect("frame").instruction = f.instruction;
            w.frame_mut().block = f.block;
            w.frame_mut().instruction = f.instruction + 1;
            w.classical.journal_records += 1;
            site.time = first.classical.time.clone();
            site.time.rewind_visit = 1 + first
                .history
                .iter()
                .skip(h)
                .filter(|r| {
                    let f = r.before.frames.last().expect("record frame");
                    matches!(r.action, Action::Rewind { .. })
                        && (f.activation, f.block, f.instruction)
                            == (first.frame().activation, first.frame().block, first.frame().instruction)
                })
                .map(|r| r.before.time.rewind_visit)
                .max()
                .unwrap_or(0);
            w.history.push_back_mut(Record { before: site, action: Action::Rewind { start, count } });
            add(&mut result, w, a);
        }
        if result.keys().any(|w| w.classical.time.leftovers != first.classical.time.leftovers) {
            let mut barrier = BTreeMap::new();
            for (mut w, a) in result {
                w.history.push_back_mut(Record { before: w.classical.clone(), action: Action::LeftBehind });
                add(&mut barrier, w, a);
            }
            return Ok(barrier);
        }
        // Interim HANDOFF rule: a rewind must have an inverse on the state its history describes.
        // This also catches changed classical reads when worlds differ, without merging histories
        // and silently accepting norm loss/gain. No particular result is promised beyond this rule.
        self.checking.set(self.checking.get() + 1);
        let restored = self.undo_state(&result);
        self.checking.set(self.checking.get() - 1);
        let restored = restored.map_err(|_| Fault::BadRewind)?;
        let norm = |s: &Superposition| s.values().map(|a| a.norm_sqr()).sum::<f64>();
        if (norm(inputs) - norm(&result)).abs() > 1e-11
            || inputs.keys().chain(restored.keys()).any(|w| {
                (inputs.get(w).copied().unwrap_or_default() - restored.get(w).copied().unwrap_or_default()).norm()
                    > 1e-11
            })
        {
            return Err(Fault::BadRewind);
        }
        Ok(result)
    }

    /// Independent barrier interpretation of Worlds: run paths to their next rewind,
    /// wait for all paths, then undo the deepest marked region together. This uses no
    /// production scheduler or journal encoding. It compares completed coherent programs,
    /// not tick counts or simulator stop policies.
    pub fn run_joined(&self, state: Superposition, limit: usize) -> Superposition {
        self.joined_until(state, limit, None)
    }

    fn joined_until(&self, mut state: Superposition, limit: usize, stop: Option<&Frame>) -> Superposition {
        let stopped = |w: &World| {
            stop.is_some_and(|site| {
                let f = w.frame();
                (f.activation, f.block, f.instruction) == (site.activation, site.block, site.instruction)
            })
        };
        for _ in 0..limit {
            let running = state.keys().any(|w| {
                !stopped(w)
                    && w.classical.finish.is_none()
                    && !matches!(self.instruction(&w.classical), Some(Inst::Rewind { .. }))
            });
            if running {
                let mut next = BTreeMap::new();
                let mut active = BTreeMap::new();
                for (w, a) in state {
                    if stopped(&w)
                        || w.classical.finish.is_some()
                        || matches!(self.instruction(&w.classical), Some(Inst::Rewind { .. }))
                    {
                        add(&mut next, w, a);
                    } else {
                        add(&mut active, w, a);
                    }
                }
                for (w, a) in self.step(&active) {
                    add(&mut next, w, a);
                }
                state = next;
            } else {
                let deepest = state
                    .keys()
                    .filter_map(|w| {
                        if stopped(w) || w.classical.finish.is_some() {
                            return None;
                        }
                        let Some(Inst::Rewind { mark, .. }) = self.instruction(&w.classical) else {
                            return None;
                        };
                        Some(w.integer(Operand::Slot(*mark)).expect("integer mark") as usize)
                    })
                    .max();
                let Some(depth) = deepest else {
                    clean(&mut state);
                    return state;
                };
                let mut cohort = BTreeMap::new();
                let mut next = BTreeMap::new();
                for (w, a) in state {
                    let same = !stopped(&w)
                        && w.classical.finish.is_none()
                        && matches!(self.instruction(&w.classical),
                        Some(Inst::Rewind {mark,..}) if w.integer(Operand::Slot(*mark)) == Ok(depth as u128));
                    if same {
                        add(&mut cohort, w, a);
                    } else {
                        add(&mut next, w, a);
                    }
                }
                // Different rewind sites are an explicitly undecided schedule. This entry is
                // for coherent programs whose paths join at one site.
                let site = cohort.keys().next().expect("cohort").frame();
                assert!(
                    cohort.keys().all(|w| {
                        let f = w.frame();
                        (f.function, f.block, f.instruction, f.activation)
                            == (site.function, site.block, site.instruction, site.activation)
                    }),
                    "different rewind sites require a policy probe"
                );
                match self.joined_rewind(&cohort, depth) {
                    Ok(out) => {
                        for (w, a) in out {
                            add(&mut next, w, a);
                        }
                    }
                    Err(fault) => {
                        for (mut w, a) in cohort {
                            w.classical.finish = Some(Finish::Faulted(fault));
                            add(&mut next, w, a);
                        }
                    }
                }
                state = next;
            }
        }
        panic!("joined reference exhausted {limit} rounds");
    }

    // Local mismatches above a split fault that original world; a shared-history mismatch
    // faults the surviving rewind cohort. Inspect unique history before summing shares, using
    // exact records and full states rather than production ticks or 64-bit check hashes.
    fn joined_rewind(&self, inputs: &Superposition, handle: usize) -> Result<Superposition, Fault> {
        let h = inputs.keys().next().expect("rewind inputs").mark_position(handle)?;
        let mut groups = BTreeMap::<_, Superposition>::new();
        for (w, &a) in inputs {
            groups.entry((w.classical.clone(), w.history.clone())).or_default().insert(w.clone(), a);
        }
        let groups: Vec<_> = groups.into_values().collect();
        let mut survivors = BTreeMap::new();
        let mut faults = BTreeMap::new();
        for (i, group) in groups.iter().enumerate() {
            let first = group.keys().next().expect("group");
            // A barrier is still a step: exhausted call/mark bounds must take their
            // catch/fault before the rewind can un-run anything.
            if self.spent_guard(first).is_some() || (self.replaying.get() == 0 && self.deadline_spent(first)) {
                match self.step_group(group) {
                    Ok(outputs) => {
                        for (w, a) in outputs {
                            add(&mut faults, w, a);
                        }
                    }
                    Err(fault) => {
                        self.diagnostics.borrow_mut().push(Diagnostic::World(fault));
                        for (w, &a) in group {
                            let mut w = w.clone();
                            w.classical.finish = Some(Finish::Faulted(fault));
                            add(&mut faults, w, a);
                        }
                    }
                }
                continue;
            }
            self.charge_replay()?;

            let shared = groups
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, other)| {
                    first
                        .history
                        .iter()
                        .zip(&other.keys().next().expect("group").history)
                        .take_while(|(a, b)| a == b)
                        .count()
                })
                .max()
                .unwrap_or(h)
                .max(h);
            let Some(Inst::Rewind { keep, .. }) = self.instruction(&first.classical) else { unreachable!() };
            self.checking.set(self.checking.get() + 1);
            let preflight = (|| -> Result<(), Fault> {
                if self.replaying.get() == 0 && self.deadline_spent(first) {
                    return Err(Fault::Budget);
                }
                self.rewind_time(first, handle, h)?;
                let mut state = BTreeMap::new();
                for (w, &a) in group {
                    let mut w = w.clone();
                    for (id, n) in self.keeps(&w, keep, Some(h))? {
                        w.quantum[id] ^= n;
                    }
                    add(&mut state, w, a);
                }
                while state.keys().any(|w| w.history.len() > shared) {
                    state = self.undo_state(&state)?;
                    // Replaying a merging inner rewind can split this part again. Checks below
                    // that split belong to the joined sum, including checks of older reads.
                    let worlds: std::collections::BTreeSet<_> =
                        state.keys().map(|w| (&w.classical, &w.history)).collect();
                    if worlds.len() > 1 {
                        return Ok(());
                    }
                }
                Ok(())
            })();
            self.checking.set(self.checking.get() - 1);
            for (w, &a) in group {
                let mut w = w.clone();
                if let Err(fault) = preflight {
                    w.classical.finish = Some(Finish::Faulted(fault));
                    add(&mut faults, w, a);
                } else {
                    add(&mut survivors, w, a);
                }
            }
        }
        if !survivors.is_empty() {
            match self.joined_rewind_checked(&survivors, handle) {
                Ok(result) => {
                    for (w, a) in result {
                        add(&mut faults, w, a);
                    }
                }
                Err(f) => {
                    for (mut w, a) in survivors {
                        w.classical.finish = Some(Finish::Faulted(f));
                        add(&mut faults, w, a);
                    }
                }
            }
        }
        Ok(faults)
    }

    fn joined_rewind_checked(&self, inputs: &Superposition, handle: usize) -> Result<Superposition, Fault> {
        let first = inputs.keys().next().expect("rewind inputs");
        let h = first.mark_position(handle)?;
        let site = first.frame().clone();
        let mut state = BTreeMap::new();
        let mut visit = 1;
        for (w, &a) in inputs {
            let Some(Inst::Rewind { mark, keep }) = self.instruction(&w.classical) else { unreachable!() };
            self.rewind_time(w, handle, h)?;
            // Reuse validation, including liveness and mark ownership, independently of the inverse.
            if h == 0
                || h > w.history.len()
                || w.history[h - 1].action != Action::Mark
                || w.mark_position(handle)? != h
                || w.history.iter().skip(h).any(|r| matches!(r.action, Action::Measurement | Action::LeftBehind))
                || w.integer(Operand::Slot(*mark))? != handle as u128
            {
                return Err(Fault::BadRewind);
            }
            visit = 1 + w
                .history
                .iter()
                .skip(h)
                .filter_map(|r| {
                    let Action::JoinedRewind { visit, .. } = r.action else {
                        return None;
                    };
                    r.before
                        .frames
                        .last()
                        .filter(|f| {
                            (f.activation, f.block, f.instruction) == (site.activation, site.block, site.instruction)
                        })
                        .map(|_| visit)
                })
                .max()
                .unwrap_or(0);
            let mut out = w.clone();
            for (id, n) in self.keeps(w, keep, Some(h))? {
                out.quantum[id] ^= n;
            }
            add(&mut state, out, a);
        }
        // All shares meet before their common allocation/gates are undone. A forward
        // rewind preserves the budget consumed by each path, including at the shared prefix.
        self.unrunning.set(self.unrunning.get() + 1);
        let unrun = (|| {
            loop {
                let longest = state.keys().map(|w| w.history.len()).max().expect("state");
                if longest <= h {
                    break;
                }
                let (mut active, mut waiting) = (BTreeMap::new(), BTreeMap::new());
                for (w, a) in state {
                    if w.history.len() == longest {
                        add(&mut active, w, a);
                    } else {
                        add(&mut waiting, w, a);
                    }
                }
                for (w, a) in self.undo_state(&active)? {
                    add(&mut waiting, w, a);
                }
                state = waiting;
                clean(&mut state);
            }
            Ok::<_, Fault>(state)
        })();
        self.unrunning.set(self.unrunning.get() - 1);
        let state = unrun?;
        let mut result = BTreeMap::new();
        for (mut w, a) in state {
            let mut start = Box::new(w.classical.clone());
            start.time.guards = first
                .history
                .get(h)
                .map_or_else(|| first.classical.time.guards.clone(), |r| r.before.time.guards.clone());
            let mut before = w.classical.clone();
            let f = before.frames.get_mut(before.frames.len() - 1).expect("frame");
            (f.block, f.instruction) = (site.block, site.instruction);
            (w.frame_mut().block, w.frame_mut().instruction) = (site.block, site.instruction + 1);
            w.classical.journal_records += 1;
            let guards = w.classical.time.guards.clone();
            before.time = first.classical.time.clone();
            before.time.guards = guards.clone();
            w.classical.time = first.classical.time.clone();
            w.classical.time.guards = guards;
            let clock = inputs
                .keys()
                .map(|input| {
                    self.rewind_time(input, handle, h).map(|c| c.max(input.classical.time.clock.saturating_add(1)))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .max()
                .expect("cohort");
            if self.replaying.get() == 0 {
                w.classical.time.clock = clock;
            }
            if first.classical.time.marks.get(&handle).is_some_and(|b| !matches!(b, MarkBudget::Unbudgeted)) {
                w.classical.time.marks.insert(handle, MarkBudget::Unbounded);
            }
            for left in w.classical.time.guards.values_mut() {
                *left = left.saturating_sub(1);
            }
            w.history.push_back_mut(Record { before, action: Action::JoinedRewind { start, visit } });
            add(&mut result, w, a);
        }
        self.checking.set(self.checking.get() + 1);
        let restored = self.undo_state(&result);
        self.checking.set(self.checking.get() - 1);
        let restored = restored?;
        if inputs.keys().chain(restored.keys()).any(|w| {
            (inputs.get(w).copied().unwrap_or_default() - restored.get(w).copied().unwrap_or_default()).norm() > 1e-11
        }) {
            return Err(Fault::BadRewind);
        }
        Ok(result)
    }

    /// Condition on an observed outcome after checking its Born weight separately. This does not
    /// assume that two implementations consume the same pseudorandom numbers.
    pub fn collapse(&self, state: &Superposition, value: u128) -> Superposition {
        let (_, selected) = self.measurement_branches(state).remove(&value).expect("nonzero Born weight");
        let mut result = BTreeMap::new();
        for (mut w, a) in selected {
            let Some(Inst::Measure { dst, .. }) = self.instruction(&w.classical) else { unreachable!() };
            let mut before = w.classical.clone();
            before.finish = None;
            w.classical.finish = None;
            w.put(*dst, Value::Int(value));
            w.frame_mut().instruction += 1;
            w.classical.journal_records += 1;
            w.history.push_back_mut(Record { before, action: Action::Measurement });
            add(&mut result, w, a);
        }
        result
    }

    /// Born weights and normalized full-world post-measurement states. No arbitrary RNG sequence.
    pub fn measurement_branches(&self, state: &Superposition) -> BTreeMap<u128, (f64, Superposition)> {
        let mut branches: BTreeMap<u128, (f64, Superposition)> = BTreeMap::new();
        for (w, &a) in state {
            let Some(Inst::Measure { source, .. }) = self.instruction(&w.classical) else {
                panic!("measurement boundary")
            };
            let value = self.measured(w, source).expect("a measurement that does not fault");
            let entry = branches.entry(value).or_default();
            entry.0 += a.norm_sqr();
            add(&mut entry.1, w.clone(), a);
        }
        for (weight, state) in branches.values_mut() {
            for a in state.values_mut() {
                *a /= weight.sqrt();
            }
        }
        branches
    }
}

/// Plug-in boundary for a future executor adapter: no executor state or production history types.
/// The outcome distribution is Born weight by per-world finish, not a sampled trajectory.
pub fn execute(
    program: &ValidModule,
    entry: &str,
    initial: Superposition,
    rounds: usize,
) -> (Superposition, BTreeMap<Finish, f64>) {
    let state = Model::new(program, entry).run(initial, rounds);
    let mut outcomes = BTreeMap::new();
    for (w, a) in &state {
        *outcomes.entry(w.classical.finish.clone().expect("run ended or reached a boundary")).or_default() +=
            a.norm_sqr();
    }
    (state, outcomes)
}
