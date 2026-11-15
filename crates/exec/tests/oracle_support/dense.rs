use std::collections::HashMap;
use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4};

use bennett_ir::{
    BinOp, CastOp, DataItem, Fault, FixedGate, FuncId, Gate, Inst, Operand, Predicate, Rotation, ValidModule,
};
use nalgebra::{DVector, Matrix2, Vector2};
use num_complex::Complex64 as C;

pub const EPS: f64 = 1e-12;

/// Standard matrices; rotations follow exp(-i theta sigma/2), MARKERS.md “Gates”.
pub fn gate_matrix(gate: Gate) -> Matrix2<C> {
    let zero = C::new(0.0, 0.0);
    let one = C::new(1.0, 0.0);
    let i = C::new(0.0, 1.0);
    match gate {
        Gate::Fixed(g) => match g {
            FixedGate::X => Matrix2::new(zero, one, one, zero),
            FixedGate::H => Matrix2::new(one, one, one, -one) * C::new(FRAC_1_SQRT_2, 0.0),
            FixedGate::Z => Matrix2::new(one, zero, zero, -one),
            FixedGate::S => Matrix2::new(one, zero, zero, i),
            FixedGate::Sdg => Matrix2::new(one, zero, zero, -i),
            FixedGate::T => Matrix2::new(one, zero, zero, C::from_polar(1.0, FRAC_PI_4)),
            FixedGate::Tdg => Matrix2::new(one, zero, zero, C::from_polar(1.0, -FRAC_PI_4)),
        },
        Gate::Rotate(axis, angle) => {
            let c = C::new((angle / 2.0).cos(), 0.0);
            let s = C::new((angle / 2.0).sin(), 0.0);
            match axis {
                Rotation::Rx => Matrix2::new(c, -i * s, -i * s, c),
                Rotation::Ry => Matrix2::new(c, -s, s, c),
                Rotation::Rz => Matrix2::new(c - i * s, zero, zero, c + i * s),
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Dense {
    pub widths: Vec<u32>,
    pub state: DVector<C>,
}

impl Default for Dense {
    fn default() -> Self {
        Self { widths: Vec::new(), state: DVector::from_element(1, C::new(1.0, 0.0)) }
    }
}

impl Dense {
    fn offset(&self, register: usize) -> u32 {
        self.widths[..register].iter().sum()
    }

    pub fn config(&self, basis: usize) -> Vec<u64> {
        self.widths.iter().enumerate().map(|(r, &w)| ((basis >> self.offset(r)) & ((1 << w) - 1)) as u64).collect()
    }

    fn allocate(&mut self, width: u32) -> usize {
        let handle = self.widths.len();
        self.widths.push(width);
        assert!(self.widths.iter().sum::<u32>() <= 13, "dense oracle is for a few qubits only");
        self.state = self.state.clone().resize_vertically(self.state.len() << width, C::new(0.0, 0.0));
        handle
    }

    pub fn gate(&mut self, matrix: Matrix2<C>, target: (usize, u8), controls: &[(usize, u8)]) {
        let target = 1 << (self.offset(target.0) + u32::from(target.1));
        let controls: usize = controls.iter().map(|&(r, bit)| 1 << (self.offset(r) + u32::from(bit))).sum();
        // Tensor contraction of the 2x2 matrix with each target-bit slice of the dense vector.
        for low in 0..self.state.len() {
            if low & target == 0 && low & controls == controls {
                let pair = matrix * Vector2::new(self.state[low], self.state[low | target]);
                self.state[low] = pair[0];
                self.state[low | target] = pair[1];
            }
        }
    }

    pub fn permute(&mut self, mut image: impl FnMut(usize) -> usize) {
        // Dense permutation-matrix/vector product without materialising its quadratic zeros.
        let mut result = DVector::zeros(self.state.len());
        for column in 0..self.state.len() {
            result[image(column)] += self.state[column];
        }
        self.state = result;
    }
}

#[derive(Clone, Copy, Debug)]
enum Value {
    Int(u128),
    Reg(usize),
}
impl Value {
    fn int(self) -> u128 {
        match self {
            Self::Int(n) => n,
            Self::Reg(_) => panic!("expected ordinary integer"),
        }
    }
    fn reg(self) -> usize {
        match self {
            Self::Reg(r) => r,
            Self::Int(_) => panic!("expected register"),
        }
    }
}
fn read(slots: &[Value], op: Operand) -> Value {
    match op {
        Operand::Slot(s) => slots[s.index()],
        Operand::Const(c) => Value::Int(c.value()),
    }
}

#[derive(Debug)]
enum End {
    Fault(Fault),
    BeforeMeasure,
    Exit(u128),
}

/// Independent interpreter of a deliberately small subset, rejecting every unsupported operation.
/// It shares only the documented IR reader/types with the executor, not its arithmetic or region paths.
/// Measurement is a boundary: compare the complete complex state just BEFORE collapse.
pub struct Oracle<'a> {
    module: &'a ValidModule,
    pub dense: Dense,
    memory: Vec<u8>,
    sp: u64,
    pub steps: u64,
    budget: u64,
    region: bool,
}

impl<'a> Oracle<'a> {
    pub fn new(module: &'a ValidModule) -> Self {
        let mut memory = vec![0; module.memory_size as usize];
        for segment in &module.data {
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
                memory[address..address + bytes.len()].copy_from_slice(&bytes);
                address += bytes.len();
            }
        }
        Self {
            module,
            dense: Dense::default(),
            memory,
            sp: module.memory_size,
            steps: 0,
            budget: 100_000,
            region: false,
        }
    }

    pub fn prefix(&mut self, entry: &str, args: &[u64]) -> Option<Option<u128>> {
        let f = self.module.function_named(entry).expect("oracle entry");
        let args: Vec<_> = args.iter().map(|&n| Value::Int(n.into())).collect();
        match self.call(f, &args) {
            Ok(value) => Some(value.map(Value::int)),
            Err(End::BeforeMeasure) => None,
            Err(End::Exit(n)) => Some(Some(n)),
            other => panic!("oracle prefix: {other:?}"),
        }
    }

    pub fn classical(&mut self, entry: &str, args: &[u64], budget: u64) -> Result<Option<u128>, Fault> {
        self.budget = budget;
        self.region = true;
        let f = self.module.function_named(entry).expect("oracle entry");
        let args: Vec<_> = args.iter().map(|&n| Value::Int(n.into())).collect();
        match self.call(f, &args) {
            Ok(result) => Ok(result.map(Value::int)),
            Err(End::Fault(f)) => Err(f),
            other => panic!("classical oracle: {other:?}"),
        }
    }

    fn tick(&mut self) -> Result<(), End> {
        if self.steps == self.budget {
            return Err(End::Fault(Fault::Budget));
        }
        self.steps += 1;
        Ok(())
    }

    fn call(&mut self, id: FuncId, args: &[Value]) -> Result<Option<Value>, End> {
        let module = self.module;
        let f = module.function(id);
        let entry_sp = self.sp;
        self.sp = self
            .sp
            .checked_sub(8 * f.slot_count() as u64)
            .filter(|&p| p >= module.static_end())
            .ok_or(End::Fault(Fault::StackOverflow))?;
        let mut slots = vec![Value::Int(0); f.slot_count()];
        for (p, &value) in f.params.iter().zip(args) {
            slots[p.slot.index()] = value;
        }
        let mut block = &f.blocks[0];
        loop {
            for inst in &block.insts {
                if matches!(inst, Inst::Measure { .. }) && !self.region {
                    return Err(End::BeforeMeasure);
                }
                self.tick()?;
                let get = |op| read(&slots, op);
                let int = |op| get(op).int();
                let result = match inst {
                    Inst::Binary { op, width, lhs, rhs, .. } => {
                        Some(Value::Int(binary(*op, width.bits(), int(*lhs), int(*rhs)).map_err(End::Fault)?))
                    }
                    Inst::Compare { pred, width, lhs, rhs, .. } => {
                        Some(Value::Int(compare(*pred, width.bits(), int(*lhs), int(*rhs)) as u128))
                    }
                    Inst::Select { cond, if_true, if_false, .. } => {
                        Some(get(if int(*cond) != 0 { *if_true } else { *if_false }))
                    }
                    Inst::Cast { op, from, to, value, .. } => {
                        let n = int(*value);
                        Some(Value::Int(
                            if *op == CastOp::Sext { signed(n, from.bits()) as u128 } else { n } & mask(to.bits()),
                        ))
                    }
                    Inst::Load { width, address, .. } => {
                        let start = int(*address) as usize;
                        let bytes = self
                            .memory
                            .get(start..start.saturating_add(width.bytes() as usize))
                            .ok_or(End::Fault(Fault::OutOfBounds))?;
                        let mut n = [0; 16];
                        n[..bytes.len()].copy_from_slice(bytes);
                        Some(Value::Int(u128::from_le_bytes(n) & mask(width.bits())))
                    }
                    Inst::Store { width, address, value } => {
                        let start = int(*address) as usize;
                        let n = int(*value).to_le_bytes();
                        let dest = self
                            .memory
                            .get_mut(start..start.saturating_add(width.bytes() as usize))
                            .ok_or(End::Fault(Fault::OutOfBounds))?;
                        dest.copy_from_slice(&n[..dest.len()]);
                        None
                    }
                    Inst::Alloca { size, .. } => {
                        self.sp = self
                            .sp
                            .checked_sub(int(*size) as u64)
                            .map(|p| p & !7)
                            .filter(|&p| p >= module.static_end())
                            .ok_or(End::Fault(Fault::StackOverflow))?;
                        Some(Value::Int(self.sp.into()))
                    }
                    Inst::Call { callee, args, .. } => {
                        self.call(*callee, &args.iter().map(|&a| get(a)).collect::<Vec<_>>())?
                    }
                    Inst::CallIndirect { signature, target, args, .. } => {
                        let address = int(*target);
                        let function = address
                            .checked_sub(1)
                            .and_then(|index| module.functions.get(index as usize))
                            .ok_or(End::Fault(Fault::BadIndirect))?;
                        if !function.has_signature(signature) {
                            return Err(End::Fault(Fault::BadIndirect));
                        }
                        self.call(FuncId((address - 1) as u32), &args.iter().map(|&a| get(a)).collect::<Vec<_>>())?
                    }
                    Inst::Effect { .. } if self.region => return Err(End::Fault(Fault::EffectInRegion)),
                    Inst::Effect { .. } => None,
                    Inst::Qalloc { .. }
                    | Inst::Qfree { .. }
                    | Inst::Gate { .. }
                    | Inst::Measure { .. }
                    | Inst::Apply { .. }
                        if self.region =>
                    {
                        return Err(End::Fault(Fault::QuantumInRegion));
                    }
                    Inst::Qalloc { width, .. } => Some(Value::Reg(self.dense.allocate(width.bits()))),
                    Inst::Qfree { register } => {
                        let r = slots[register.index()].reg();
                        assert!(
                            self.dense
                                .state
                                .iter()
                                .enumerate()
                                .all(|(b, a)| self.dense.config(b)[r] == 0 || a.norm() <= EPS),
                            "oracle dirty free"
                        );
                        None
                    }
                    Inst::Gate { gate, target, controls } => {
                        let target = (slots[target.register.index()].reg(), target.bit);
                        let controls: Vec<_> =
                            controls.iter().map(|q| (slots[q.register.index()].reg(), q.bit)).collect();
                        self.dense.gate(gate_matrix(*gate), target, &controls);
                        None
                    }
                    Inst::Apply { callee, args, out, fault, budget } => {
                        let args: Vec<_> = args.iter().map(|&a| get(a)).collect();
                        self.apply(
                            *callee,
                            &args,
                            slots[out.index()].reg(),
                            slots[fault.index()].reg(),
                            int(*budget) as u64,
                        );
                        None
                    }
                    other => panic!("unsupported oracle instruction {other:?}"),
                };
                if let (Some(dst), Some(value)) = (inst.result(), result) {
                    slots[dst.index()] = value;
                }
            }
            self.tick()?;
            use bennett_ir::Terminator as T;
            let target = match &block.term {
                T::Return(value) => {
                    self.sp = entry_sp;
                    return Ok(value.map(|v| read(&slots, v)));
                }
                T::Fault(f) => return Err(End::Fault(*f)),
                T::Exit(v) => {
                    return Err(if self.region {
                        End::Fault(Fault::ExitInRegion)
                    } else {
                        End::Exit(read(&slots, *v).int())
                    });
                }
                T::Jump(t) => t,
                T::Branch { cond, if_true, if_false } => {
                    if read(&slots, *cond).int() != 0 {
                        if_true
                    } else {
                        if_false
                    }
                }
                T::Switch { value, cases, default, .. } => {
                    cases.iter().find(|c| c.value == read(&slots, *value).int()).map_or(default, |c| &c.target)
                }
            };
            let values: Vec<_> = target.args.iter().map(|&a| read(&slots, a)).collect();
            block = f.block(target.block);
            for (param, value) in block.params.iter().zip(values) {
                slots[param.slot.index()] = value;
            }
        }
    }

    fn apply(&mut self, callee: FuncId, args: &[Value], out: usize, fault: usize, budget: u64) {
        let dense = &self.dense;
        let mut cache = HashMap::new();
        let images: Vec<_> = (0..dense.state.len())
            .map(|basis| {
                let config = dense.config(basis);
                let args: Vec<_> = args
                    .iter()
                    .map(|&a| match a {
                        Value::Int(n) => n,
                        Value::Reg(r) => config[r].into(),
                    })
                    .collect();
                let result = cache.entry(args.clone()).or_insert_with(|| {
                    let mut region = Self {
                        module: self.module,
                        dense: Dense::default(),
                        memory: self.memory.clone(),
                        sp: self.sp,
                        steps: 0,
                        budget,
                        region: true,
                    };
                    region.call(callee, &args.into_iter().map(Value::Int).collect::<Vec<_>>())
                });
                let (register, value) = match result {
                    Ok(Some(v)) => (out, v.int() as usize),
                    Err(End::Fault(f)) => (fault, f.code() as usize),
                    other => panic!("oracle region: {other:?}"),
                };
                basis ^ (value << dense.offset(register))
            })
            .collect();
        self.dense.permute(|b| images[b]);
    }
}

fn mask(bits: u32) -> u128 {
    u128::MAX >> (128 - bits)
}
fn signed(n: u128, bits: u32) -> i128 {
    ((n << (128 - bits)) as i128) >> (128 - bits)
}
fn binary(op: BinOp, bits: u32, a: u128, b: u128) -> Result<u128, Fault> {
    use BinOp::*;
    let value = match op {
        Add => a.wrapping_add(b),
        Sub => a.wrapping_sub(b),
        Mul => a.wrapping_mul(b),
        And => a & b,
        Or => a | b,
        Xor => a ^ b,
        Udiv | Urem | Sdiv | Srem if b == 0 => return Err(Fault::DivideByZero),
        Udiv => a / b,
        Urem => a % b,
        Sdiv => signed(a, bits).wrapping_div(signed(b, bits)) as u128,
        Srem => signed(a, bits).wrapping_rem(signed(b, bits)) as u128,
        Shl | Lshr | Ashr if b >= u128::from(bits) => 0,
        Shl => a << b,
        Lshr => a >> b,
        Ashr => (signed(a, bits) >> b) as u128,
    };
    Ok(value & mask(bits))
}
fn compare(pred: Predicate, bits: u32, a: u128, b: u128) -> bool {
    use Predicate::*;
    match pred {
        Eq => a == b,
        Ne => a != b,
        Ult => a < b,
        Ule => a <= b,
        Ugt => a > b,
        Uge => a >= b,
        Slt => signed(a, bits) < signed(b, bits),
        Sle => signed(a, bits) <= signed(b, bits),
        Sgt => signed(a, bits) > signed(b, bits),
        Sge => signed(a, bits) >= signed(b, bits),
    }
}
