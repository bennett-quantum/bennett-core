//! Randomly generated programs. Each is written and read back, run forward one step at a time with
//! the harness snapshotting every step, then stepped back with the state compared against the
//! snapshot of every step on the way. Generation is seeded, so a failure names a reproducible seed.

mod common;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{
    BinOp, Block, CastOp, Const, DataItem, DataSegment, Fault, FixedGate, FuncId, Function, Gate, Inst, Module,
    Operand, Param, Predicate, Qubit, Rotation, Signature, Slot, SwitchCase, Target, Terminator, Type, Width, read,
    validate, write,
};
use common::assert_same_state;
use rand::seq::IndexedRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const SEEDS: u64 = 300;
const FUNCTIONS: usize = 3;
const MAX_BLOCKS: usize = 4;
const MAX_INSTS: usize = 8;
const MEMORY: u64 = 1024;
const STEP_BUDGET: u64 = 200;
/// Generated loops may allocate and rotate a qubit on every pass, doubling the state each time; this
/// limit stops such a run (`RunError::ConfigurationLimit`) instead of exhausting the host.
const MAX_CONFIGURATIONS: usize = 64;
const REGION_BUDGET: u64 = 30;

/// Every generated function takes two `i32` and returns an `i32`.
const VALUE: Width = match Width::new(32) {
    Some(width) => width,
    None => panic!("32 is a valid width"),
};

#[test]
fn random_programs_round_trip_and_reverse_step_by_step() {
    let (mut returned, mut faulted, mut stopped, mut region_steps, mut superposed) = (0, 0, 0, 0, 0);
    for seed in 0..SEEDS {
        let module = generate(seed);
        let text = write(&module);
        assert_eq!(read(&text).as_ref(), Ok(&module), "seed {seed}: writer round trip\n{text}");
        let module =
            validate(module).unwrap_or_else(|e| panic!("seed {seed}: generated an invalid program: {e}\n{text}"));
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let max = VALUE.mask() as u64;
        let args = [rng.random_range(0..=max), rng.random_range(0..=max)];
        // Random regions fault on some branches; this test is about reversal, so the run goes on.
        let options = Options {
            budget: Some(STEP_BUDGET),
            max_configurations: Some(MAX_CONFIGURATIONS),
            seed,
            reversible: true,
            allow_region_faults: true,
        };
        let mut executor = Executor::new(&module, FuncId(0), &args, options).unwrap_or_else(|e| panic!("{e}"));
        let history = run_recording(&mut executor);
        match executor.outcome() {
            Some(Outcome::Returned(_)) => returned += 1,
            Some(Outcome::Faulted(_)) => faulted += 1,
            Some(Outcome::Stopped(_)) => stopped += 1,
            None => unreachable!("run_recording runs to the end"),
        }
        region_steps += executor.stats().region_steps;
        superposed += usize::from(executor.stats().peak_configurations > 1);
        for (steps, expected) in history.iter().enumerate().rev().skip(1) {
            assert_eq!(executor.step_back(), Ok(true), "seed {seed}");
            assert_same_state(&executor.snapshot(), expected, &format!("seed {seed}, back to step {steps}\n{text}"));
        }
        assert_eq!(executor.step_back(), Ok(false), "seed {seed}: nothing left to undo");
    }
    assert!(
        returned > 0 && faulted > 0,
        "both outcomes occur: {returned} returned, {faulted} faulted ({stopped} stopped at the configuration limit)"
    );
    assert!(region_steps > 0 && superposed > 0, "regions and superpositions are exercised");
}

/// Runs to the end, returning the snapshot after each completed step (index = step count).
fn run_recording(executor: &mut Executor<'_>) -> Vec<bennett_exec::Snapshot> {
    let mut history = vec![executor.snapshot()];
    loop {
        let outcome = executor.step();
        if executor.stats().steps as usize == history.len() {
            history.push(executor.snapshot());
        }
        if outcome.is_some() {
            return history;
        }
    }
}

fn generate(seed: u64) -> Module {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut bytes = vec![0u8; 16];
    rng.fill(bytes.as_mut_slice());
    let table = DataItem::FuncAddr { width: Width::WORD, func: FuncId(rng.random_range(0..FUNCTIONS as u32)) };
    let data = vec![DataSegment { address: 0, items: vec![DataItem::Bytes(bytes), table] }];
    let functions = (0..FUNCTIONS).map(|index| FunctionGen::new(&mut rng, index).generate()).collect();
    Module { memory_size: MEMORY, data, functions }
}

/// Builds one function. Slots are numbered in the order they appear in the text, so that the writer's
/// output reads back to the identical module.
struct FunctionGen<'r> {
    rng: &'r mut ChaCha8Rng,
    index: usize,
    slot_names: Vec<String>,
    /// The parameter types of every block, chosen up front so branches can target any block.
    block_params: Vec<Vec<Type>>,
    /// Integer values in scope: function parameters, block parameters, results so far in the block.
    ints: Vec<(Slot, Width)>,
    /// Registers allocated so far in the block and not freed.
    registers: Vec<(Slot, Width)>,
}

impl<'r> FunctionGen<'r> {
    fn new(rng: &'r mut ChaCha8Rng, index: usize) -> Self {
        let blocks = rng.random_range(1..=MAX_BLOCKS);
        let mut block_params = vec![Vec::new()];
        for _ in 1..blocks {
            let count = rng.random_range(0..=2);
            block_params.push((0..count).map(|_| Type::Int(random_width(rng))).collect());
        }
        FunctionGen { rng, index, slot_names: Vec::new(), block_params, ints: Vec::new(), registers: Vec::new() }
    }

    fn generate(mut self) -> Function {
        let params: Vec<Param> = (0..2).map(|_| Param { slot: self.new_slot(), ty: Type::Int(VALUE) }).collect();
        let function_params: Vec<(Slot, Width)> = params.iter().map(|p| (p.slot, VALUE)).collect();
        let mut blocks = Vec::new();
        for b in 0..self.block_params.len() {
            self.ints = function_params.clone();
            self.registers.clear();
            let block_params: Vec<Param> =
                self.block_params[b].clone().into_iter().map(|ty| Param { slot: self.new_slot(), ty }).collect();
            for param in &block_params {
                if let Type::Int(width) = param.ty {
                    self.ints.push((param.slot, width));
                }
            }
            let count = self.rng.random_range(0..=MAX_INSTS);
            let mut insts = Vec::new();
            for _ in 0..count {
                self.inst(&mut insts);
            }
            let term = self.terminator();
            blocks.push(Block { name: format!("b{b}"), params: block_params, insts, term });
        }
        Function {
            name: format!("f{}", self.index),
            params,
            result: Some(Type::Int(VALUE)),
            blocks,
            slot_names: self.slot_names,
        }
    }

    fn new_slot(&mut self) -> Slot {
        let slot = Slot(self.slot_names.len() as u32);
        self.slot_names.push(format!("v{}", slot.0));
        slot
    }

    fn define(&mut self, width: Width) -> Slot {
        let slot = self.new_slot();
        self.ints.push((slot, width));
        slot
    }

    /// An existing value of `width` most of the time, otherwise a random constant.
    fn operand(&mut self, width: Width) -> Operand {
        let candidates: Vec<Slot> = self.ints.iter().filter(|(_, w)| *w == width).map(|(s, _)| *s).collect();
        match candidates.choose(self.rng) {
            Some(&slot) if self.rng.random_bool(0.7) => Operand::Slot(slot),
            _ => constant(width, self.rng.random_range(0..=width.mask())),
        }
    }

    /// An address: mostly inside memory, sometimes just past it.
    fn address(&mut self) -> Operand {
        constant(Width::WORD, u128::from(self.rng.random_range(0..MEMORY + 16)))
    }

    fn some_width(&mut self) -> Width {
        match self.ints.choose(self.rng) {
            Some(&(_, width)) if self.rng.random_bool(0.6) => width,
            _ => random_width(self.rng),
        }
    }

    fn inst(&mut self, insts: &mut Vec<Inst>) {
        match self.rng.random_range(0..16) {
            0..=2 => {
                let (op, width) = (*BinOp::ALL.choose(self.rng).expect("ops"), self.some_width());
                let (lhs, rhs) = (self.operand(width), self.operand(width));
                insts.push(Inst::Binary { op, width, dst: self.define(width), lhs, rhs });
            }
            3 => {
                let (pred, width) = (*Predicate::ALL.choose(self.rng).expect("predicates"), self.some_width());
                let (lhs, rhs) = (self.operand(width), self.operand(width));
                insts.push(Inst::Compare { pred, width, dst: self.define(Width::BIT), lhs, rhs });
            }
            4 => {
                let width = self.some_width();
                let (cond, if_true, if_false) = (self.operand(Width::BIT), self.operand(width), self.operand(width));
                insts.push(Inst::Select { width, dst: self.define(width), cond, if_true, if_false });
            }
            5 => self.cast(insts),
            6 => {
                let (width, address) = (random_width(self.rng), self.address());
                insts.push(Inst::Load { width, dst: self.define(width), address });
            }
            7 => {
                let width = self.some_width();
                let (value, address) = (self.operand(width), self.address());
                insts.push(Inst::Store { width, value, address });
            }
            8 => {
                let size = constant(Width::WORD, u128::from(self.rng.random_range(0u64..64)));
                // Sometimes the allocation is freed again, and sometimes the stack pointer is set to any
                // value, which is mostly the fault `bad_stack_restore`.
                let saved = self.rng.random_bool(0.5).then(|| {
                    let saved = self.define(Width::WORD);
                    insts.push(Inst::StackSave { dst: saved });
                    saved
                });
                insts.push(Inst::Alloca { dst: self.define(Width::WORD), size });
                if let Some(saved) = saved {
                    insts.push(Inst::StackRestore { pointer: Operand::Slot(saved) });
                } else if self.rng.random_bool(0.1) {
                    let pointer = self.operand(Width::WORD);
                    insts.push(Inst::StackRestore { pointer });
                }
            }
            9 => self.call(insts),
            10 => {
                let byte = self.operand(Width::BYTE);
                insts.push(Inst::Effect { byte });
            }
            _ => self.quantum(insts),
        }
    }

    fn cast(&mut self, insts: &mut Vec<Inst>) {
        let (from, to) = (self.some_width(), random_width(self.rng));
        if from == to {
            return;
        }
        let op = if to < from {
            CastOp::Trunc
        } else if self.rng.random_bool(0.5) {
            CastOp::Zext
        } else {
            CastOp::Sext
        };
        let value = self.operand(from);
        insts.push(Inst::Cast { op, from, to, dst: self.define(to), value });
    }

    /// A direct call to a later function, or an indirect call through a function address that may be
    /// wrong in signature or not a function at all.
    fn call(&mut self, insts: &mut Vec<Inst>) {
        let args = vec![self.operand(VALUE), self.operand(VALUE)];
        if self.index + 1 < FUNCTIONS && self.rng.random_bool(0.5) {
            let callee = FuncId(self.rng.random_range(self.index + 1..FUNCTIONS) as u32);
            insts.push(Inst::Call { callee, args, dst: Some(self.define(VALUE)) });
            return;
        }
        let func = FuncId(self.rng.random_range(0..FUNCTIONS as u32));
        let address = self.define(Width::WORD);
        insts.push(Inst::FuncAddr { dst: address, func });
        let target = if self.rng.random_bool(0.8) { Operand::Slot(address) } else { self.operand(Width::WORD) };
        let result = if self.rng.random_bool(0.9) { VALUE } else { Width::BYTE };
        let signature = Signature { params: vec![Type::Int(VALUE); 2], result: Some(Type::Int(result)) };
        insts.push(Inst::CallIndirect { signature, target, args, dst: Some(self.define(result)) });
    }

    /// Allocates a register when none is at hand; otherwise mostly gates on the registers in scope.
    fn quantum(&mut self, insts: &mut Vec<Inst>) {
        let choice = if self.registers.is_empty() { 0 } else { self.rng.random_range(0..10) };
        match choice {
            0..=1 => {
                let width = Width::new(self.rng.random_range(1..=3)).expect("small width");
                let dst = self.new_slot();
                self.registers.push((dst, width));
                insts.push(Inst::Qalloc { dst, width });
            }
            2 => {
                let index = self.rng.random_range(0..self.registers.len());
                let (register, _) = self.registers.remove(index);
                insts.push(Inst::Qfree { register });
            }
            3..=4 => self.apply(insts),
            _ => self.gate(insts),
        }
    }

    fn gate(&mut self, insts: &mut Vec<Inst>) {
        let mut qubits: Vec<Qubit> = self
            .registers
            .iter()
            .flat_map(|&(register, width)| (0..width.bits() as u8).map(move |bit| Qubit { register, bit }))
            .collect();
        if qubits.is_empty() {
            return;
        }
        let target = qubits.swap_remove(self.rng.random_range(0..qubits.len()));
        let count = self.rng.random_range(0..=2);
        let controls = qubits.choose_multiple(self.rng, count).copied().collect();
        let gate = if self.rng.random_bool(0.7) {
            Gate::Fixed(*FixedGate::ALL.choose(self.rng).expect("gates"))
        } else {
            Gate::Rotate(*Rotation::ALL.choose(self.rng).expect("axes"), self.rng.random_range(-3.0..3.0))
        };
        insts.push(Inst::Gate { gate, target, controls });
    }

    /// `apply` of any function, its first argument a `q32` register when one is at hand (the callees
    /// take `i32`) and an integer otherwise.
    fn apply(&mut self, insts: &mut Vec<Inst>) {
        let out = self.new_slot();
        insts.push(Inst::Qalloc { dst: out, width: VALUE });
        let fault = self.new_slot();
        insts.push(Inst::Qalloc { dst: fault, width: Fault::CODE_WIDTH });
        let input = self.registers.iter().find(|(_, width)| *width == VALUE).map(|(slot, _)| Operand::Slot(*slot));
        let first = input.unwrap_or_else(|| self.operand(VALUE));
        let args = vec![first, self.operand(VALUE)];
        let callee = FuncId(self.rng.random_range(0..FUNCTIONS as u32));
        let budget = constant(Width::WORD, u128::from(REGION_BUDGET));
        insts.push(Inst::Apply { callee, args, out, fault, budget });
        self.registers.push((out, VALUE));
        self.registers.push((fault, Fault::CODE_WIDTH));
    }

    fn terminator(&mut self) -> Terminator {
        match self.rng.random_range(0..10) {
            0..=3 => Terminator::Return(Some(self.operand(VALUE))),
            4..=5 => Terminator::Jump(self.target()),
            6..=7 => {
                let cond = self.operand(Width::BIT);
                Terminator::Branch { cond, if_true: self.target(), if_false: self.target() }
            }
            8 => {
                let width = self.some_width();
                let value = self.operand(width);
                let mut cases: Vec<SwitchCase> = Vec::new();
                for _ in 0..self.rng.random_range(0..3) {
                    let case = self.rng.random_range(0..=width.mask().min(7));
                    if cases.iter().all(|c| c.value != case) {
                        cases.push(SwitchCase { value: case, target: self.target() });
                    }
                }
                Terminator::Switch { width, value, cases, default: self.target() }
            }
            _ => Terminator::Fault(*Fault::ALL.choose(self.rng).expect("faults")),
        }
    }

    fn target(&mut self) -> Target {
        let block = self.rng.random_range(0..self.block_params.len());
        let params = self.block_params[block].clone();
        let args = params
            .into_iter()
            .map(|ty| match ty {
                Type::Int(width) => self.operand(width),
                Type::Qreg(_) => unreachable!("generated blocks take integers"),
            })
            .collect();
        Target { block: bennett_ir::BlockId(block as u32), args }
    }
}

fn random_width(rng: &mut ChaCha8Rng) -> Width {
    let bits = *[1, 3, 8, 16, 32, 64, 65, 100, 128].choose(rng).expect("widths");
    Width::new(bits).expect("listed widths are valid")
}

fn constant(width: Width, value: u128) -> Operand {
    Operand::Const(Const::new(width, value & width.mask()).expect("masked to the width"))
}
