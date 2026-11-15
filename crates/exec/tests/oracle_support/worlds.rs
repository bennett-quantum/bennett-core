//! Slow, test-only meaning of DESIGN.md's journalled machine. No executor code is used.
//!
//! A basis key contains the ENTIRE machine, including history. Classical snapshots deliberately
//! trade space for clarity; this is not an implementation or a journal-cost model. Gates and apply
//! never put quantum values into a snapshot. Memory is an overlay over one immutable classical base.
//! Measurement is a boundary (all outcomes are available via `measurement_branches`); seeded RNG
//! trajectories and configuration-limit policy are not part of the unitary reference meaning.

use std::collections::BTreeMap;

use bennett_ir::{
    BlockId, CastOp, DataItem, Fault, FuncId, Inst, Keep, Operand, Slot, Target, Terminator, Type, ValidModule,
};
use num_complex::Complex64 as C;

use super::dense::{binary, compare, gate_matrix, signed};

pub type Superposition = BTreeMap<World, C>;

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
pub struct Classical {
    pub overlay: BTreeMap<u64, u8>,
    pub frames: Vec<Frame>,
    /// Includes dead slots, since a later activation may reuse them.
    pub slots: Vec<Value>,
    pub sp: u64,
    pub tape: Vec<u8>,
    pub registers: Vec<Register>,
    pub marks: Vec<Mark>,
    pub next_activation: u64,
    pub finish: Option<Finish>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Classical,
    Mark,
    Measurement,
    Allocate(usize),
    Free(usize),
    /// The operation is read from the program position in `before`.
    Gate,
    Apply,
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
    pub history: Vec<Record>,
}

impl World {
    pub fn frame(&self) -> &Frame {
        self.classical.frames.last().expect("active frame")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.classical.frames.last_mut().expect("active frame")
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
    *state.entry(world).or_default() += amplitude;
}

fn clean(state: &mut Superposition) {
    state.retain(|_, a| a.norm() > f64::EPSILON);
}

pub fn basis(world: World) -> Superposition {
    BTreeMap::from([(world, C::new(1.0, 0.0))])
}

pub struct Model<'a> {
    pub program: &'a ValidModule,
    pub base: Vec<u8>,
    entry_result: Option<Type>,
    region: bool,
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
            base,
            entry_result: program.function(program.function_named(entry).expect("entry")).result,
            region: false,
        }
    }

    pub fn initial(&self, entry: &str, args: &[u128]) -> World {
        let mut world = World {
            classical: Classical {
                overlay: BTreeMap::new(),
                frames: Vec::new(),
                slots: Vec::new(),
                sp: self.program.memory_size,
                tape: Vec::new(),
                registers: Vec::new(),
                marks: Vec::new(),
                next_activation: 0,
                finish: None,
            },
            quantum: Vec::new(),
            history: Vec::new(),
        };
        self.call(
            &mut world,
            self.program.function_named(entry).expect("entry"),
            args.iter().copied().map(Value::Int).collect(),
            None,
        )
        .expect("entry fits memory");
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
        Ok((address..end).map(|a| world.classical.overlay.get(&a).copied().unwrap_or(self.base[a as usize])).collect())
    }

    fn write_memory(&self, world: &mut World, address: u64, bytes: &[u8]) -> Result<(), Fault> {
        self.read_memory(world, address, bytes.len())?;
        for (offset, &byte) in bytes.iter().enumerate() {
            let address = address + offset as u64;
            if byte == self.base[address as usize] {
                world.classical.overlay.remove(&address);
            } else {
                world.classical.overlay.insert(address, byte);
            }
        }
        Ok(())
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
        w.classical.slots.resize(w.classical.slots.len().max(base + function.slot_count()), Value::Int(0));
        w.classical.next_activation += 1;
        w.classical.frames.push(Frame {
            function: func,
            block: BlockId(0),
            instruction: 0,
            base,
            entry_sp,
            activation: w.classical.next_activation,
            result,
        });
        w.classical.sp = sp;
        for (param, value) in function.params.iter().zip(values) {
            w.put(param.slot, value);
        }
        Ok(())
    }

    fn instruction(&self, classical: &Classical) -> Option<&Inst> {
        let f = classical.frames.last()?;
        self.program.function(f.function).block(f.block).insts.get(f.instruction)
    }

    /// One documented instruction on EVERY world. Finished worlds wait unchanged. A dirty free or
    /// failed inverse is atomic across the participating superposition, as on today's machine.
    pub fn step(&self, state: &Superposition) -> Superposition {
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
                    for (mut w, a) in group {
                        w.classical.finish = Some(Finish::Faulted(fault));
                        add(&mut next, w, a);
                    }
                }
            }
        }
        clean(&mut next);
        next
    }

    fn step_group(&self, state: &Superposition) -> Result<Superposition, Fault> {
        let first = state.keys().next().expect("nonempty group");
        if first.classical.finish.is_none()
            && let Some(Inst::Rewind { mark, keep }) = self.instruction(&first.classical)
        {
            return self.rewind_group(state, *mark, keep);
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
            if self.region
                && matches!(
                    inst,
                    Inst::Qalloc { .. }
                        | Inst::Qfree { .. }
                        | Inst::Gate { .. }
                        | Inst::Measure { .. }
                        | Inst::Apply { .. }
                )
            {
                return Err(Fault::QuantumInRegion);
            }
            if let Inst::Measure { register, .. } = inst {
                w.register(*register)?;
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
                    let bytes = self.read_memory(&w, w.integer(*address)? as u64, width.bytes() as usize)?;
                    let mut n = [0; 16];
                    n[..bytes.len()].copy_from_slice(&bytes);
                    result = Some(Value::Int(u128::from_le_bytes(n) & width.mask()));
                }
                Inst::Store { width, address, value } => {
                    let a = w.integer(*address)? as u64;
                    let n = w.integer(*value)?.to_le_bytes();
                    self.write_memory(&mut w, a, &n[..width.bytes() as usize])?;
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
                    self.write_memory(&mut w, address, &bytes)?;
                    result = Some(Value::Int(0));
                }
                Inst::Call { callee, args, dst } => {
                    let args = args.iter().map(|a| w.read(*a)).collect();
                    self.call(&mut w, *callee, args, *dst)?;
                    w.history.push(Record { before, action });
                    return Ok(basis(w));
                }
                Inst::CallIndirect { signature, target, args, dst } => {
                    let address = w.integer(*target)?;
                    let index =
                        address.checked_sub(1).and_then(|i| usize::try_from(i).ok()).ok_or(Fault::BadIndirect)?;
                    if !self.program.functions.get(index).is_some_and(|f| f.has_signature(signature)) {
                        return Err(Fault::BadIndirect);
                    }
                    let args = args.iter().map(|a| w.read(*a)).collect();
                    self.call(&mut w, FuncId(index as u32), args, *dst)?;
                    w.history.push(Record { before, action });
                    return Ok(basis(w));
                }
                Inst::FuncAddr { func, .. } => result = Some(Value::Int(u128::from(func.0) + 1)),
                Inst::Effect { byte } => {
                    if self.region {
                        return Err(Fault::EffectInRegion);
                    }
                    w.classical.tape.push(w.integer(*byte)? as u8);
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
                    if w.quantum[id] != 0 {
                        return Err(Fault::DirtyFree);
                    }
                    w.classical.registers[id].width = None;
                    action = Action::Free(id);
                }
                Inst::Gate { .. } => action = Action::Gate,
                Inst::Apply { .. } => action = Action::Apply,
                Inst::Mark { .. } => {
                    let handle = w.history.len() + 1;
                    result = Some(Value::Int(handle as u128));
                    w.classical.marks.push(Mark { handle, activation: w.frame().activation, valid: true });
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
                    if w.classical.frames.len() == 1 {
                        w.classical.finish = Some(Finish::Returned(value));
                    } else {
                        let frame = w.classical.frames.pop().expect("callee");
                        self.leave(&mut w, frame.activation);
                        w.classical.sp = frame.entry_sp;
                        w.frame_mut().instruction += 1;
                        if let (Some(dst), Some(n)) = (frame.result, value) {
                            w.put(dst, Value::Int(n));
                        }
                    }
                }
                Terminator::Exit(value) => {
                    if self.region {
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
                Terminator::Longjmp { buffer, value } => self.longjmp(&mut w, *buffer, *value)?,
                Terminator::Fault(f) => return Err(*f),
            }
            Action::Classical
        };
        let record = Record { before, action };
        let mut state = self.quantum_action(w, &record, false)?;
        for (mut world, a) in std::mem::take(&mut state) {
            world.history.push(record.clone());
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
        let frame = w.classical.frames.get(depth).ok_or(Fault::BadIndirect)?;
        if frame.function != func || frame.entry_sp != words[2] {
            return Err(Fault::BadIndirect);
        }
        let Some(Inst::Setjmp { dst, .. }) =
            self.program.function(func).blocks.get(block.index()).and_then(|b| b.insts.get(inst))
        else {
            return Err(Fault::BadIndirect);
        };
        let ceiling = frame.entry_sp - 8 * self.program.function(func).slot_count() as u64;
        let floor = w.classical.frames.get(depth + 1).map_or(w.classical.sp, |f| f.entry_sp);
        if words[3] < floor || words[3] > ceiling {
            return Err(Fault::BadIndirect);
        }
        let n = w.integer(value)?;
        while w.classical.frames.len() > depth + 1 {
            let f = w.classical.frames.pop().expect("discarded frame");
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
            Action::Gate => {
                let Some(Inst::Gate { gate, target, controls }) = self.instruction(&record.before) else {
                    unreachable!()
                };
                let current = std::mem::replace(&mut w.classical, record.before.clone());
                let target_id = w.register(target.register)?;
                let mut bits = vec![(target_id, target.bit)];
                for c in controls {
                    bits.push((w.register(c.register)?, c.bit));
                }
                let mut distinct = bits.clone();
                distinct.sort();
                distinct.dedup();
                if bits.len() != distinct.len() {
                    return Err(Fault::QubitAlias);
                }
                let enabled = bits[1..].iter().all(|&(r, b)| w.quantum[r] >> b & 1 == 1);
                w.classical = current;
                if !enabled {
                    return Ok(basis(w));
                }
                let m = gate_matrix(*gate);
                let m = if inverse { m.adjoint() } else { m };
                let input = ((w.quantum[target_id] >> target.bit) & 1) as usize;
                let mut state = BTreeMap::new();
                for output in 0..2 {
                    let mut out = w.clone();
                    out.quantum[target_id] =
                        (out.quantum[target_id] & !(1 << target.bit)) | ((output as u64) << target.bit);
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
                region.history.clear();
                region.quantum.clear();
                region.classical.frames.clear();
                region.classical.slots.clear();
                region.classical.registers.clear();
                region.classical.marks.clear();
                region.classical.finish = None;
                let mut model = Model::new(self.program, &self.program.function(*callee).name);
                model.base = self.base.clone();
                model.region = true;
                let result = match model.call(&mut region, *callee, values, None) {
                    Err(f) => Finish::Faulted(f),
                    Ok(()) => model
                        .run(basis(region), budget)
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
                    Finish::Faulted(f) => w.quantum[fault] ^= f.code(),
                    Finish::BeforeMeasurement => unreachable!("quantum region faults first"),
                }
                Ok(basis(w))
            }
            _ => Ok(basis(w)),
        }
    }

    fn rewind_group(&self, inputs: &Superposition, mark: Slot, keep: &[Keep]) -> Result<Superposition, Fault> {
        if self.region && !keep.is_empty() {
            return Err(Fault::QuantumInRegion);
        }
        let first = inputs.keys().next().expect("nonempty rewind");
        let h = first.integer(Operand::Slot(mark))? as usize;
        if !first.classical.marks.iter().any(|m| m.handle == h && m.activation == first.frame().activation && m.valid)
            || h == 0
            || h > first.history.len()
            || first.history[h - 1].action != Action::Mark
            || first.history[h..].iter().any(|r| r.action == Action::Measurement)
        {
            return Err(Fault::BadRewind);
        }
        let mut state = BTreeMap::new();
        for (w, &a) in inputs {
            let mut out = w.clone();
            let mut keeps = Vec::new();
            for k in keep {
                let id = w.register(k.register)?;
                if w.classical.registers[id].allocated_at >= h {
                    return Err(Fault::BadRewind);
                }
                keeps.push((id, w.integer(k.value)? as u64));
            }
            for (r, n) in keeps {
                out.quantum[r] ^= n;
            }
            add(&mut state, out, a);
        }
        let mut count = 0;
        while state.keys().any(|w| w.history.len() > h) {
            state = self.undo_state(&state).map_err(|_| Fault::BadRewind)?;
            count += 1;
        }
        let mut result = BTreeMap::new();
        for (mut w, a) in state {
            let start = Box::new(w.classical.clone());
            let mut site = (*start).clone();
            let f = first.frame();
            site.frames.last_mut().expect("frame").block = f.block;
            site.frames.last_mut().expect("frame").instruction = f.instruction;
            w.frame_mut().block = f.block;
            w.frame_mut().instruction = f.instruction + 1;
            w.history.push(Record { before: site, action: Action::Rewind { start, count } });
            add(&mut result, w, a);
        }
        // Interim HANDOFF rule: a rewind must have an inverse on the state its history describes.
        // This also catches changed classical reads when worlds differ, without merging histories
        // and silently accepting norm loss/gain. No particular result is promised beyond this rule.
        let restored = self.undo_state(&result).map_err(|_| Fault::BadRewind)?;
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

    /// Undo a complete superposition, combining inverse-gate paths BEFORE testing clean discard.
    /// A per-input check would incorrectly reject paths whose temporary amplitudes cancel.
    pub fn undo_state(&self, inputs: &Superposition) -> Result<Superposition, Fault> {
        let mut groups = BTreeMap::<_, Superposition>::new();
        for (w, &a) in inputs {
            let record = w.history.last().ok_or(Fault::BadRewind)?;
            add(groups.entry(record.clone()).or_default(), w.clone(), a);
        }
        let mut result = BTreeMap::new();
        for (record, group) in groups {
            let mut state = BTreeMap::new();
            for (mut w, a) in group {
                w.history.pop();
                add(&mut state, w, a);
            }
            if let Action::Rewind { start, count } = &record.action {
                let mut reset = BTreeMap::new();
                for (mut w, a) in state {
                    w.classical = (**start).clone();
                    add(&mut reset, w, a);
                }
                state = reset;
                for _ in 0..*count {
                    state = self.step_group(&state)?;
                }
                for (mut w, a) in state {
                    if w.classical.finish.is_some() {
                        return Err(Fault::BadRewind);
                    }
                    let Some(Inst::Rewind { keep, .. }) = self.instruction(&record.before) else { unreachable!() };
                    let keeps = keep
                        .iter()
                        .map(|k| Ok((w.register(k.register)?, w.integer(k.value)? as u64)))
                        .collect::<Result<Vec<_>, Fault>>()?;
                    for (r, n) in keeps {
                        w.quantum[r] ^= n;
                    }
                    add(&mut result, w, a);
                }
                continue;
            }
            for (mut w, a) in state {
                match record.action {
                    Action::Measurement => return Err(Fault::BadRewind),
                    Action::Allocate(id) => {
                        if w.quantum[id] != 0 {
                            return Err(Fault::BadRewind);
                        }
                        w.quantum.truncate(record.before.registers.len());
                    }
                    Action::Free(id) => w.quantum[id] = 0,
                    _ => {}
                }
                for (mut out, b) in self.quantum_action(w, &record, true)? {
                    out.classical = record.before.clone();
                    add(&mut result, out, a * b);
                }
            }
        }
        clean(&mut result);
        Ok(result)
    }

    pub fn undo(&self, w: World) -> Result<Superposition, Fault> {
        self.undo_state(&basis(w))
    }

    /// Condition on an observed outcome after checking its Born weight separately. This does not
    /// assume that two implementations consume the same pseudorandom numbers.
    pub fn collapse(&self, state: &Superposition, value: u64) -> Superposition {
        let (_, selected) = self.measurement_branches(state).remove(&value).expect("nonzero Born weight");
        let mut result = BTreeMap::new();
        for (mut w, a) in selected {
            let Some(Inst::Measure { dst, .. }) = self.instruction(&w.classical) else { unreachable!() };
            let mut before = w.classical.clone();
            before.finish = None;
            w.classical.finish = None;
            w.put(*dst, Value::Int(value.into()));
            w.frame_mut().instruction += 1;
            w.history.push(Record { before, action: Action::Measurement });
            add(&mut result, w, a);
        }
        result
    }

    /// Born weights and normalized full-world post-measurement states. No arbitrary RNG sequence.
    pub fn measurement_branches(&self, state: &Superposition) -> BTreeMap<u64, (f64, Superposition)> {
        let mut branches: BTreeMap<u64, (f64, Superposition)> = BTreeMap::new();
        for (w, &a) in state {
            let Some(Inst::Measure { register, .. }) = self.instruction(&w.classical) else {
                panic!("measurement boundary")
            };
            let id = w.register(*register).expect("live measured register");
            let entry = branches.entry(w.quantum[id]).or_default();
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
