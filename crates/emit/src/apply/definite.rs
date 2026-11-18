//! Prove constants for the operations that need them, independently of the step budget.

use bennett_ir::{BinOp, Inst, Operand, Slot, Width};

use super::arithmetic;
use super::bits::{Circuit, constant, definite};
use super::symbolic::Frame;
use super::{ApplyInput, Argument};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Value {
    Pending,
    Known(u128),
    Unknown,
}

impl Value {
    fn read(slots: &[Self], operand: &Operand) -> Self {
        match operand {
            Operand::Const(value) => Self::Known(value.value()),
            Operand::Slot(slot) => slots[slot.index()],
        }
    }

    fn join(self, next: Self) -> Self {
        match (self, next) {
            (Self::Pending, value) | (value, Self::Pending) => value,
            (Self::Known(a), Self::Known(b)) if a == b => self,
            (Self::Known(_), Self::Known(_) | Self::Unknown) | (Self::Unknown, Self::Known(_) | Self::Unknown) => {
                Self::Unknown
            }
        }
    }

    fn unary(self, fold: impl FnOnce(u128) -> u128) -> Self {
        match self {
            Self::Known(value) => Self::Known(fold(value)),
            Self::Pending | Self::Unknown => self,
        }
    }

    fn binary(self, other: Self, fold: impl FnOnce(u128, u128) -> u128) -> Self {
        match (self, other) {
            (Self::Pending, _) | (_, Self::Pending) => Self::Pending,
            (Self::Known(a), Self::Known(b)) => Self::Known(fold(a, b)),
            (Self::Unknown, Self::Known(_) | Self::Unknown) | (Self::Known(_), Self::Unknown) => Self::Unknown,
        }
    }
}

fn fold_inst(slots: &[Value], inst: &Inst, circuit: &mut Circuit) -> (Slot, Value) {
    let value = |operand| Value::read(slots, operand);
    let bits = |value, width: Width| constant(value, width.bits() as usize);
    let known = |word: &[super::bits::Bit]| definite(word).expect("folding definite operands");
    match inst {
        Inst::Binary { op, width, dst, lhs, rhs } => {
            let result = value(lhs).binary(value(rhs), |a, b| {
                let word = arithmetic::binary(circuit, *op, &bits(a, *width), &bits(b, *width))
                    .expect("checked the supported operations");
                known(&word)
            });
            (*dst, result)
        }
        Inst::Compare { pred, width, dst, lhs, rhs } => {
            let result = value(lhs).binary(value(rhs), |a, b| {
                known(&[arithmetic::compare(circuit, *pred, &bits(a, *width), &bits(b, *width))])
            });
            (*dst, result)
        }
        Inst::Cast { op, from, to, dst, value: operand } => {
            (*dst, value(operand).unary(|v| known(&arithmetic::cast(*op, bits(v, *from), to.bits() as usize))))
        }
        Inst::Select { dst, cond, if_true, if_false, .. } => {
            let result = match value(cond) {
                Value::Known(choice) => value(if choice == 1 { if_true } else { if_false }),
                Value::Pending => Value::Pending,
                Value::Unknown => value(if_true).join(value(if_false)),
            };
            (*dst, result)
        }
        Inst::Load { .. }
        | Inst::Store { .. }
        | Inst::Alloca { .. }
        | Inst::StackSave { .. }
        | Inst::StackRestore { .. }
        | Inst::Setjmp { .. }
        | Inst::ReturnAddress { .. }
        | Inst::Call { .. }
        | Inst::CallIndirect { .. }
        | Inst::FuncAddr { .. }
        | Inst::Effect { .. }
        | Inst::Qalloc { .. }
        | Inst::Qfree { .. }
        | Inst::Gate { .. }
        | Inst::GateAt { .. }
        | Inst::Measure { .. }
        | Inst::Apply { .. } => {
            unreachable!("checked the supported instructions")
        }
    }
}

/// The required constant inputs for a shift or multiply. A literal needs no data-flow proof.
pub(super) fn requirement(inst: &Inst) -> Option<(BinOp, &'static str)> {
    let Inst::Binary { op, lhs, rhs, .. } = inst else { return None };
    match op {
        BinOp::Mul if !literal(lhs) && !literal(rhs) => Some((*op, "multiplication of two qubit words")),
        BinOp::Shl | BinOp::Lshr | BinOp::Ashr if !literal(rhs) => Some((*op, "shifts by a qubit word")),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::And
        | BinOp::Xor
        | BinOp::Shl
        | BinOp::Lshr
        | BinOp::Ashr
        | BinOp::Udiv
        | BinOp::Sdiv
        | BinOp::Urem
        | BinOp::Srem
        | BinOp::Or => None,
    }
}

fn literal(operand: &Operand) -> bool {
    matches!(operand, Operand::Const(_))
}

#[derive(Default)]
pub(super) struct Constants(Vec<Value>);

impl Constants {
    pub(super) fn get(&self, operand: &Operand) -> Option<u128> {
        let value = match operand {
            Operand::Slot(slot) => *self.0.get(slot.index())?,
            Operand::Const(value) => Value::Known(value.value()),
        };
        match value {
            Value::Known(value) => Some(value),
            Value::Pending | Value::Unknown => None,
        }
    }

    pub(super) fn analyze(frame: &Frame<'_>, input: &ApplyInput<'_>) -> Self {
        let function = frame.module.function(frame.callee);
        if !function.blocks.iter().flat_map(|block| &block.insts).any(|inst| requirement(inst).is_some()) {
            return Self::default();
        }
        let mut slots = vec![Value::Pending; function.slot_count()];
        for (parameter, argument) in function.params.iter().zip(input.args) {
            let bennett_ir::Type::Int(width) = parameter.ty else { unreachable!("checked the circuit interface") };
            slots[parameter.slot.index()] = match argument {
                Argument::Value(value) => Value::Known(value & width.mask()),
                Argument::Register(_) => Value::Unknown,
            };
        }

        // Each slot can progress from pending to one constant and then to unknown.
        // Joining every incoming edge makes the proof valid across revisits to a block.
        let mut circuit = Circuit::new(0);
        loop {
            let mut changed = false;
            let mut update = |slots: &mut [Value], dst: Slot, value: Value| {
                let old = &mut slots[dst.index()];
                let next = old.join(value);
                changed |= next != *old;
                *old = next;
            };
            for block in &function.blocks {
                for inst in &block.insts {
                    let (dst, value) = fold_inst(&slots, inst, &mut circuit);
                    update(&mut slots, dst, value);
                }
                block.term.visit_targets(|target| {
                    for (param, operand) in function.block(target.block).params.iter().zip(&target.args) {
                        let value = Value::read(&slots, operand);
                        update(&mut slots, param.slot, value);
                    }
                });
            }
            if !changed {
                return Self(slots);
            }
        }
    }
}
