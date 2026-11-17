//! Test-only, independent compute/copy/uncompute on explicit basis configurations.
//!
//! Machine words encode status, result, fault, scheduled tick count, frame depth, fixed-size
//! frames (function/block/instruction/return slot and value slots), and byte memory. Each tick
//! has a separate record of the same size. For any total ordinary update f, the completed step
//! is (c,j) -> (j XOR f(c),c), with inverse (c,j) -> (j,c XOR f(j)), even on nonblank records.
//! No executor arithmetic, stepping, evaluation or reversal code is used here.

use std::collections::BTreeMap;

use bennett_ir::{BinOp, CastOp, Fault, Inst, Module, Operand, Predicate, Target, Terminator};
use num_complex::Complex;

pub mod swap_in;

const STATUS: usize = 0; // 0 running, 1 returned, 2 faulted
const RESULT: usize = 1;
const FAULT: usize = 2;
const TICKS: usize = 3;
const DEPTH: usize = 4;
const HEADER: usize = 5;
const FRAME_HEADER: usize = 4;
const FRAMES: usize = 3;

pub type Machine = Vec<u64>;

// The fixture chooses a common denominator for these exact complex coefficients.
pub type Vector<B> = BTreeMap<B, Complex<i64>>;

type Outcomes<const N: usize> = BTreeMap<[u64; N], (u64, u64)>;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Basis<B> {
    registers: B,
    machine: Machine,
    records: Vec<Machine>,
}

pub struct Reference<'m> {
    module: &'m Module,
    frame_size: usize,
    memory_start: usize,
    initial: Machine,
    parameter_slots: Vec<usize>,
}

impl<'m> Reference<'m> {
    pub fn new(module: &'m Module, name: &str) -> Self {
        let frame_size = FRAME_HEADER + module.functions.iter().map(|f| f.slot_count()).max().expect("functions");
        let memory_start = HEADER + FRAMES * frame_size;
        let mut initial = vec![0; memory_start + module.memory_size as usize];
        let func = module.function_named(name).expect("applied function");
        initial[HEADER] = u64::from(func.0);
        for segment in &module.data {
            for (i, byte) in segment.encode().into_iter().enumerate() {
                initial[memory_start + segment.address as usize + i] = u64::from(byte);
            }
        }
        let parameter_slots =
            module.function(func).params.iter().map(|param| HEADER + FRAME_HEADER + param.slot.index()).collect();
        Self { module, frame_size, memory_start, initial, parameter_slots }
    }

    pub fn initial_machine(&self) -> &[u64] {
        &self.initial
    }

    pub fn apply<B: Ord, const N: usize>(
        &self,
        vector: Vector<B>,
        budget: usize,
        arguments: impl Fn(&B) -> [u64; N],
        mut xor_targets: impl FnMut(&mut B, (u64, u64)),
    ) -> (Vector<B>, Outcomes<N>) {
        assert_eq!(N, self.parameter_slots.len());
        let blank_records = vec![vec![0; self.initial.len()]; budget];
        let mut vector = vector
            .into_iter()
            .map(|(registers, amplitude)| {
                (Basis { registers, machine: self.initial.clone(), records: blank_records.clone() }, amplitude)
            })
            .collect();
        let copy_arguments = |basis: &mut Basis<B>| {
            for (&slot, argument) in self.parameter_slots.iter().zip(arguments(&basis.registers)) {
                basis.machine[slot] ^= argument;
            }
        };
        vector = permute(vector, copy_arguments);
        for tick in 0..budget {
            vector = permute(vector, |b| self.tick(&mut b.machine, &mut b.records[tick], false));
        }
        let mut outcomes = BTreeMap::new();
        vector = permute(vector, |b| {
            assert_eq!(b.machine[TICKS], budget as u64, "all inputs take exactly B scheduled ticks");
            let input = arguments(&b.registers);
            let output = Self::output(&b.machine);
            if let Some(previous) = outcomes.insert(input, output) {
                assert_eq!(previous, output, "the computation does not read its targets");
            }
            xor_targets(&mut b.registers, output);
            assert_eq!(arguments(&b.registers), input, "the XOR targets are disjoint from the arguments");
        });
        for tick in (0..budget).rev() {
            vector = permute(vector, |b| self.tick(&mut b.machine, &mut b.records[tick], true));
        }
        vector = permute(vector, copy_arguments);
        let restored = vector
            .into_iter()
            .map(|(basis, amplitude)| {
                assert_eq!(basis.machine, self.initial, "every machine bit restored");
                assert!(basis.records.iter().flatten().all(|&word| word == 0), "every record bit restored");
                (basis.registers, amplitude)
            })
            .collect();
        (restored, outcomes)
    }

    pub fn tick(&self, machine: &mut Machine, record: &mut Machine, inverse: bool) {
        if inverse {
            std::mem::swap(machine, record);
        }
        let updated = self.next(machine);
        for (word, value) in record.iter_mut().zip(updated) {
            *word ^= value;
        }
        if !inverse {
            std::mem::swap(machine, record);
        }
    }

    fn output(c: &[u64]) -> (u64, u64) {
        match c[STATUS] {
            1 => (c[RESULT], 0),
            2 => (0, c[FAULT]),
            _ => (0, Fault::Budget.code()),
        }
    }

    fn next(&self, c: &[u64]) -> Machine {
        // Invalid encodings select identity. This defines f on every bit string, rather than
        // assuming a record is a reachable machine state when the inverse evaluates it.
        self.ordinary_step(c).unwrap_or_else(|| c.to_vec())
    }

    fn ordinary_step(&self, c: &[u64]) -> Option<Machine> {
        if c[STATUS] > 2 {
            return None;
        }
        let mut n = c.to_vec();
        n[TICKS] = c[TICKS].wrapping_add(1);
        if c[STATUS] != 0 {
            return Some(n); // A terminal status idles for all remaining scheduled ticks.
        }
        let depth = usize::try_from(c[DEPTH]).ok().filter(|&d| d < FRAMES)?;
        let frame = HEADER + depth * self.frame_size;
        let function = self.module.functions.get(usize::try_from(c[frame]).ok()?)?;
        let block = function.blocks.get(usize::try_from(c[frame + 1]).ok()?)?;
        let pc = usize::try_from(c[frame + 2]).ok().filter(|&p| p <= block.insts.len())?;
        let slots = frame + FRAME_HEADER;
        let read = |op: &Operand| match op {
            Operand::Const(k) => k.value() as u64,
            Operand::Slot(s) => c[slots + s.index()],
        };
        let fail = |n: &mut Machine, fault: Fault| {
            n[STATUS] = 2;
            n[FAULT] = fault.code();
        };
        if let Some(inst) = block.insts.get(pc) {
            n[frame + 2] += 1;
            match inst {
                Inst::Binary { op, width, dst, lhs, rhs } => {
                    if width.bits() > 64 {
                        return None;
                    }
                    let mask = u64::MAX >> (64 - width.bits());
                    let (a, b) = (read(lhs) & mask, read(rhs) & mask);
                    let value = match op {
                        BinOp::Add => a.wrapping_add(b),
                        BinOp::Sub => a.wrapping_sub(b),
                        BinOp::Mul => a.wrapping_mul(b),
                        BinOp::Xor => a ^ b,
                        BinOp::And => a & b,
                        BinOp::Udiv if b == 0 => {
                            fail(&mut n, Fault::DivideByZero);
                            return Some(n);
                        }
                        BinOp::Udiv => a / b,
                        _ => return None,
                    };
                    n[slots + dst.index()] = value & mask;
                }
                Inst::Compare { pred, width, dst, lhs, rhs } => {
                    if width.bits() > 64 {
                        return None;
                    }
                    let mask = u64::MAX >> (64 - width.bits());
                    let (a, b) = (read(lhs) & mask, read(rhs) & mask);
                    n[slots + dst.index()] = u64::from(match pred {
                        Predicate::Eq => a == b,
                        Predicate::Ult => a < b,
                        _ => return None,
                    });
                }
                Inst::Cast { op: CastOp::Zext | CastOp::Trunc, to, dst, value, .. } => {
                    if to.bits() > 64 {
                        return None;
                    }
                    n[slots + dst.index()] = read(value) & (u64::MAX >> (64 - to.bits()));
                }
                Inst::Load { width, dst, address } => {
                    if width.bits() != 8 {
                        return None;
                    }
                    if read(address) < self.module.memory_size {
                        n[slots + dst.index()] = c[self.memory_start + read(address) as usize] & 255;
                    } else {
                        fail(&mut n, Fault::OutOfBounds);
                    }
                }
                Inst::Store { width, value, address } => {
                    if width.bits() != 8 {
                        return None;
                    }
                    if read(address) < self.module.memory_size {
                        n[self.memory_start + read(address) as usize] = read(value) & 255;
                    } else {
                        fail(&mut n, Fault::OutOfBounds);
                    }
                }
                Inst::Call { callee, args, dst } => {
                    let callee_frame = frame + self.frame_size;
                    if depth + 1 == FRAMES {
                        fail(&mut n, Fault::StackOverflow);
                    } else {
                        n[callee_frame..callee_frame + self.frame_size].fill(0);
                        n[callee_frame] = u64::from(callee.0);
                        n[callee_frame + 3] = dst.map_or(0, |s| u64::from(s.0) + 1);
                        for (param, arg) in self.module.function(*callee).params.iter().zip(args) {
                            n[callee_frame + FRAME_HEADER + param.slot.index()] = read(arg);
                        }
                        n[DEPTH] += 1;
                    }
                }
                Inst::Effect { .. } => fail(&mut n, Fault::EffectInRegion),
                _ => return None,
            }
        } else {
            match &block.term {
                Terminator::Jump(target) => self.jump(&mut n, frame, target, read),
                Terminator::Branch { cond, if_true, if_false } => {
                    self.jump(&mut n, frame, if read(cond) == 1 { if_true } else { if_false }, read);
                }
                Terminator::Return(Some(value)) => {
                    if depth == 0 {
                        n[STATUS] = 1;
                        n[RESULT] = read(value);
                    } else {
                        let dst = c[frame + 3];
                        if dst != 0 {
                            let dst = usize::try_from(dst - 1).ok().filter(|&s| s < self.frame_size - FRAME_HEADER)?;
                            n[slots - self.frame_size + dst] = read(value);
                        }
                        n[frame..frame + self.frame_size].fill(0);
                        n[DEPTH] -= 1;
                    }
                }
                Terminator::Exit(_) => fail(&mut n, Fault::ExitInRegion),
                Terminator::Fault(fault) => fail(&mut n, *fault),
                _ => return None,
            }
        }
        Some(n)
    }

    fn jump(&self, n: &mut Machine, frame: usize, target: &Target, read: impl Fn(&Operand) -> u64) {
        let function = &self.module.functions[n[frame] as usize];
        n[frame + 1] = u64::from(target.block.0);
        n[frame + 2] = 0;
        // All operands come from c, so overlapping block parameters are assigned simultaneously.
        for (param, arg) in function.block(target.block).params.iter().zip(&target.args) {
            n[frame + FRAME_HEADER + param.slot.index()] = read(arg);
        }
    }
}

pub fn permute<B: Ord>(vector: Vector<B>, mut operation: impl FnMut(&mut B)) -> Vector<B> {
    let mut result = Vector::new();
    for (mut basis, amplitude) in vector {
        operation(&mut basis);
        assert!(result.insert(basis, amplitude).is_none(), "a permutation cannot merge basis configurations");
    }
    result
}
