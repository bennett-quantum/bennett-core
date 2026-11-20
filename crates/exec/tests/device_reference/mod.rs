//! Outer device-language reference from SYNTAX, DESIGN and APPLY-STEP.
//!
//! Classical control chooses instructions. Quantum instructions update a reduced density matrix
//! directly. The only inputs from a run under comparison are the outcomes chosen by its measurements.

mod integers;
mod quantum;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use bennett_ir::{
    BlockId, CastOp, DataItem, DataSegment, Fault, FuncId, Inst, MeasureSource, Operand, Qubit, Slot, Target,
    Terminator, Type, ValidModule, Width,
};

use crate::compute_copy_reference::Reference as ComputeCopy;
use integers::{binary, compare, mask, signed};
use quantum::Quantum;

pub use quantum::{Basis, Density};

const SLOT_BYTES: u64 = 8;
const MIN_MEASUREMENT_PROBABILITY: f64 = 1e-22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    Returned(Option<u64>),
    Faulted(Fault),
}

pub enum Event {
    Ordinary,
    Allocated { id: u64, dst: Slot, width: Width },
    Released(u64),
    Measured { outcome: u64, probabilities: BTreeMap<u64, f64> },
}

pub struct Frame {
    pub function: FuncId,
    pub block: BlockId,
    pub position: usize,
    pub slots: Vec<u64>,
    pub entry_stack: u64,
    stack_ceiling: u64,
    destination: Option<Slot>,
}

pub struct Device<'m> {
    module: &'m ValidModule,
    entry: FuncId,
    budget: u64,
    in_apply: bool,
    pub frames: Vec<Frame>,
    pub stack: u64,
    pub memory: Vec<u8>,
    pub tape: Vec<u8>,
    pub steps: u64,
    pub finish: Option<Finish>,
    pub quantum: Quantum,
}

impl<'m> Device<'m> {
    pub fn new(module: &'m ValidModule, entry: FuncId, args: &[u64], budget: u64) -> Self {
        let mut memory = vec![0; module.memory_size as usize];
        for segment in &module.data {
            let start = segment.address as usize;
            let bytes = segment.encode();
            memory[start..start + bytes.len()].copy_from_slice(&bytes);
        }
        Self::start(module, entry, args, budget, module.memory_size, memory, false)
    }

    fn start(
        module: &'m ValidModule,
        entry: FuncId,
        args: &[u64],
        budget: u64,
        stack: u64,
        memory: Vec<u8>,
        in_apply: bool,
    ) -> Self {
        let mut device = Self {
            module,
            entry,
            budget,
            in_apply,
            frames: Vec::new(),
            stack,
            memory,
            tape: Vec::new(),
            steps: 0,
            finish: None,
            quantum: Quantum::new(),
        };
        if let Err(fault) = device.enter(entry, args, None) {
            device.finish = Some(Finish::Faulted(fault));
        }
        device
    }

    pub fn slot_type(&self, function: FuncId, slot: Slot) -> Type {
        self.module.slot_type(function, slot)
    }

    pub fn frame(&self) -> &Frame {
        self.frames.last().expect("running frame")
    }

    fn instruction(&self) -> Option<&'m Inst> {
        let frame = self.frame();
        self.module.function(frame.function).block(frame.block).insts.get(frame.position)
    }

    pub fn next_measurement(&self) -> Option<Slot> {
        if self.finish.is_some() || self.steps >= self.budget {
            return None;
        }
        match self.instruction()? {
            Inst::Measure { source: MeasureSource::Register(reg), dst } => self.register(*reg).ok().map(|_| *dst),
            _ => None,
        }
    }

    pub fn step(&mut self, observed: Option<u64>) -> Event {
        assert!(self.finish.is_none());
        let result = if self.steps >= self.budget {
            Err(Fault::Budget)
        } else if let Some(inst) = self.instruction() {
            self.execute(inst, observed)
        } else {
            let frame = self.frame();
            let term = &self.module.function(frame.function).block(frame.block).term;
            self.terminate(term).map(|()| Event::Ordinary)
        };
        match result {
            Ok(event) => {
                self.steps += 1;
                event
            }
            Err(fault) => {
                self.finish = Some(Finish::Faulted(fault));
                Event::Ordinary
            }
        }
    }

    fn read(&self, operand: Operand) -> u64 {
        match operand {
            Operand::Slot(slot) => self.frame().slots[slot.index()],
            Operand::Const(value) => value.value().try_into().expect("integer through i64"),
        }
    }

    fn operand_type(&self, operand: Operand) -> Type {
        match operand {
            Operand::Slot(slot) => self.module.slot_type(self.frame().function, slot),
            Operand::Const(value) => Type::Int(value.width()),
        }
    }

    fn put(&mut self, slot: Slot, value: u64) {
        self.frames.last_mut().expect("running frame").slots[slot.index()] = value;
    }

    fn advance(&mut self) {
        self.frames.last_mut().expect("running frame").position += 1;
    }

    fn enter(&mut self, function: FuncId, args: &[u64], destination: Option<Slot>) -> Result<(), Fault> {
        let callee = self.module.function(function);
        let stack_ceiling = self.lower_stack(callee.slot_count() as u64 * SLOT_BYTES, u64::MAX)?;
        let mut slots = vec![0; callee.slot_count()];
        for (parameter, &argument) in callee.params.iter().zip(args) {
            slots[parameter.slot.index()] = match parameter.ty {
                Type::Int(width) => argument & mask(width),
                Type::Qreg(_) => argument,
            };
        }
        self.frames.push(Frame {
            function,
            block: BlockId(0),
            position: 0,
            slots,
            entry_stack: self.stack,
            stack_ceiling,
            destination,
        });
        self.stack = stack_ceiling;
        Ok(())
    }

    fn lower_stack(&self, bytes: u64, alignment_mask: u64) -> Result<u64, Fault> {
        self.stack
            .checked_sub(bytes)
            .map(|ptr| ptr & alignment_mask)
            .filter(|&ptr| ptr >= self.module.static_end())
            .ok_or(Fault::StackOverflow)
    }

    fn memory_range(&self, address: u64, width: Width) -> Result<Range<usize>, Fault> {
        let end = address.checked_add(width.bytes()).filter(|&end| end <= self.module.memory_size);
        end.map(|end| address as usize..end as usize).ok_or(Fault::OutOfBounds)
    }

    fn register(&self, slot: Slot) -> Result<usize, Fault> {
        self.quantum.position(self.read(Operand::Slot(slot))).ok_or(Fault::DanglingRegister)
    }

    fn qubit(&self, qubit: Qubit) -> Result<(usize, u8), Fault> {
        Ok((self.register(qubit.register)?, u8::try_from(qubit.bit).expect("a qubit of this reference's registers")))
    }

    fn execute(&mut self, inst: &Inst, observed: Option<u64>) -> Result<Event, Fault> {
        let mut event = Event::Ordinary;
        let value = match *inst {
            Inst::Binary { op, width, lhs, rhs, .. } => Some(binary(op, width, self.read(lhs), self.read(rhs))?),
            Inst::Compare { pred, width, lhs, rhs, .. } => Some(compare(pred, width, self.read(lhs), self.read(rhs))),
            Inst::Select { cond, if_true, if_false, .. } => {
                Some(self.read(if self.read(cond) == 1 { if_true } else { if_false }))
            }
            Inst::Cast { op, from, to, value, .. } => {
                let value = self.read(value);
                let extended = if op == CastOp::Sext { signed(value, from) as u64 } else { value };
                Some(extended & mask(to))
            }
            Inst::Load { width, address, .. } => {
                let mut bytes = [0; 8];
                let data = &self.memory[self.memory_range(self.read(address), width)?];
                bytes[..data.len()].copy_from_slice(data);
                Some(u64::from_le_bytes(bytes) & mask(width))
            }
            Inst::Store { width, address, value } => {
                let range = self.memory_range(self.read(address), width)?;
                let bytes = (self.read(value) & mask(width)).to_le_bytes();
                let len = range.len();
                self.memory[range].copy_from_slice(&bytes[..len]);
                None
            }
            Inst::Alloca { size, .. } => {
                self.stack = self.lower_stack(self.read(size), !7)?;
                Some(self.stack)
            }
            Inst::StackSave { .. } => Some(self.stack),
            Inst::StackRestore { pointer } => {
                let restored = self.read(pointer);
                if !(self.stack..=self.frame().stack_ceiling).contains(&restored) {
                    return Err(Fault::BadStackRestore);
                }
                self.stack = restored;
                None
            }
            Inst::FuncAddr { func, .. } => Some(func.address()),
            Inst::Call { callee, ref args, dst } => {
                self.call(callee, args, dst)?;
                return Ok(event);
            }
            Inst::CallIndirect { ref signature, target, ref args, dst } => {
                let callee = self
                    .module
                    .function_at(self.read(target))
                    .filter(|&id| self.module.function(id).has_signature(signature))
                    .ok_or(Fault::BadIndirect)?;
                self.call(callee, args, dst)?;
                return Ok(event);
            }
            Inst::Effect { .. } if self.in_apply => return Err(Fault::EffectInRegion),
            Inst::Effect { byte } => {
                self.tape.push(self.read(byte) as u8);
                None
            }
            Inst::Qalloc { .. }
            | Inst::Qfree { .. }
            | Inst::Gate { .. }
            | Inst::GateAt { .. }
            | Inst::Measure { .. }
            | Inst::Apply { .. }
                if self.in_apply =>
            {
                return Err(Fault::QuantumInRegion);
            }
            Inst::Qalloc { width, dst } => {
                let width = width.as_width().expect("a register of this reference's widths");
                let id = self.quantum.allocate(width);
                event = Event::Allocated { id, dst, width };
                Some(id)
            }
            Inst::Qfree { register } => {
                let position = self.register(register)?;
                let (id, _) = self.quantum.live[position];
                self.quantum.release(position);
                event = Event::Released(id);
                None
            }
            Inst::Gate { gate, target, ref controls } => {
                let target = self.qubit(target)?;
                let controls = controls.iter().map(|&q| self.qubit(q)).collect::<Result<Vec<_>, _>>()?;
                let distinct: BTreeSet<_> = std::iter::once(target).chain(controls.iter().copied()).collect();
                if distinct.len() != controls.len() + 1 {
                    return Err(Fault::QubitAlias);
                }
                self.quantum.gate(gate, target, &controls);
                None
            }
            Inst::Measure { source: MeasureSource::Register(slot), .. } => {
                let register = self.register(slot)?;
                let outcome = observed.expect("observation for reached register measurement");
                let probabilities = self.quantum.probabilities(register);
                let probability = probabilities.get(&outcome).copied().unwrap_or_default();
                assert!(
                    probability > MIN_MEASUREMENT_PROBABILITY,
                    "measurement of handle {} at step {} chose {outcome} with probability {probability:.3e}; probabilities: {probabilities:?}",
                    self.quantum.live[register].0,
                    self.steps,
                );
                self.quantum.project(register, outcome, probability);
                event = Event::Measured { outcome, probabilities };
                Some(outcome)
            }
            Inst::Apply { callee, ref args, out, fault, budget } => {
                self.apply(callee, args, out, fault, self.read(budget))?;
                None
            }
            Inst::GateAt { .. }
            | Inst::RotateBy { .. }
            | Inst::Measure { source: MeasureSource::Memory { .. }, .. }
            | Inst::Setjmp { .. }
            | Inst::ReturnAddress { .. } => {
                panic!("instruction outside the outer reference: {inst:?}")
            }
        };
        if let Some(value) = value {
            self.put(inst.result().expect("value instruction"), value);
        }
        self.advance();
        Ok(event)
    }

    fn call(&mut self, callee: FuncId, args: &[Operand], dst: Option<Slot>) -> Result<(), Fault> {
        let args: Vec<_> = args.iter().map(|&arg| self.read(arg)).collect();
        self.enter(callee, &args, dst)
    }

    fn jump(&mut self, target: &Target) {
        let args: Vec<_> = target.args.iter().map(|&op| self.read(op)).collect();
        let frame = self.frames.last_mut().expect("jump from a frame");
        let block = self.module.function(frame.function).block(target.block);
        for (param, argument) in block.params.iter().zip(args) {
            frame.slots[param.slot.index()] = argument;
        }
        frame.block = target.block;
        frame.position = 0;
    }

    fn terminate(&mut self, terminator: &Terminator) -> Result<(), Fault> {
        match terminator {
            Terminator::Jump(target) => self.jump(target),
            Terminator::Branch { cond, if_true, if_false } => {
                self.jump(if self.read(*cond) == 1 { if_true } else { if_false });
            }
            Terminator::Switch { value, cases, default, .. } => {
                let selector = u128::from(self.read(*value));
                let target = cases.iter().find(|case| case.value == selector).map_or(default, |case| &case.target);
                self.jump(target);
            }
            Terminator::Return(operand) => {
                let value = operand.map(|op| self.read(op));
                if self.frames.len() == 1 {
                    let scalar = matches!(self.module.function(self.entry).result, Some(Type::Int(_)));
                    self.finish = Some(Finish::Returned(value.filter(|_| scalar)));
                } else {
                    let callee = self.frames.pop().expect("returning call");
                    self.stack = callee.entry_stack;
                    if let Some(dst) = callee.destination {
                        self.put(dst, value.expect("call result"));
                    }
                    self.advance();
                }
            }
            Terminator::Exit(_) if self.in_apply => return Err(Fault::ExitInRegion),
            Terminator::Exit(operand) => {
                if Some(self.operand_type(*operand)) != self.module.function(self.entry).result {
                    return Err(Fault::ExitTypeMismatch);
                }
                self.finish = Some(Finish::Returned(Some(self.read(*operand))));
            }
            Terminator::Fault(fault) => return Err(*fault),
            Terminator::Longjmp { .. } => panic!("longjmp is outside the outer reference"),
        }
        Ok(())
    }

    fn apply(&mut self, callee: FuncId, args: &[Operand], out: Slot, fault: Slot, budget: u64) -> Result<(), Fault> {
        let out = self.register(out)?;
        let fault = self.register(fault)?;
        // A register operand supplies its bits. An integer operand supplies the same word for every basis.
        let arguments = args
            .iter()
            .map(|&op| match (op, self.operand_type(op)) {
                (Operand::Slot(slot), Type::Qreg(_)) => self.register(slot).map(Ok),
                _ => Ok(Err(self.read(op))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if out == fault || arguments.iter().any(|arg| *arg == Ok(out) || *arg == Ok(fault)) {
            return Err(Fault::QubitAlias);
        }
        let input = |basis: &[u64]| {
            arguments
                .iter()
                .map(|arg| match *arg {
                    Ok(register) => basis[register],
                    Err(value) => value,
                })
                .collect::<Vec<_>>()
        };

        // The compute/copy reference can initialize bytes via data segments. Copy the current
        // bytes below the existing floor into its module without changing the stack's extent.
        // For writes above the floor, use the same independent ordinary rules with private memory.
        let floor = self.module.static_end() as usize;
        let copy_module = self.memory[floor..].iter().all(|&byte| byte == 0).then(|| {
            let mut module = (**self.module).clone();
            module.data = vec![DataSegment { address: 0, items: vec![DataItem::Bytes(self.memory[..floor].to_vec())] }];
            module
        });
        let copy =
            copy_module.as_ref().map(|module| ComputeCopy::at(module, &module.function(callee).name, self.stack));
        let mut outputs = BTreeMap::new();
        for (row, col) in self.quantum.density.keys() {
            for basis in [row, col] {
                let values = input(basis);
                outputs.entry(values).or_insert_with_key(|args| match &copy {
                    Some(copy) => copy.finish(args, budget as usize).output,
                    None => self.private_run(callee, args, budget),
                });
            }
        }
        self.quantum.permute(|mut basis| {
            let (value, code) = outputs[&input(&basis)];
            basis[out] ^= value;
            basis[fault] ^= code;
            basis
        });
        Ok(())
    }

    fn private_run(&self, callee: FuncId, args: &[u64], budget: u64) -> (u64, u64) {
        let mut private = Self::start(self.module, callee, args, budget, self.stack, self.memory.clone(), true);
        loop {
            match private.finish {
                Some(Finish::Returned(Some(value))) => return (value, 0),
                Some(Finish::Faulted(fault)) => return (0, fault.code()),
                Some(Finish::Returned(None)) => panic!("apply requires an integer result"),
                None => {
                    private.step(None);
                }
            }
        }
    }
}
