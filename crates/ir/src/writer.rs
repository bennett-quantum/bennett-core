//! The text writer: the inverse of the reader. `read(&write(m)) == m` for every module the validator
//! accepts.

use std::fmt::{self, Write};

use crate::{
    Block, DataItem, DataSegment, Function, Gate, Inst, Module, Operand, Param, Qubit, Signature, Slot, Target,
    Terminator, Type,
};

/// Writes a module in its text form.
pub fn write(module: &Module) -> String {
    let mut text = String::new();
    // Writing to a `String` cannot fail.
    let _ = Writer { module, out: &mut text }.module();
    text
}

struct Writer<'a> {
    module: &'a Module,
    out: &'a mut String,
}

impl Writer<'_> {
    fn module(&mut self) -> fmt::Result {
        writeln!(self.out, "memory {}", self.module.memory_size)?;
        for segment in &self.module.data {
            self.data_segment(segment)?;
        }
        for function in &self.module.functions {
            writeln!(self.out)?;
            self.function(function)?;
        }
        Ok(())
    }

    fn data_segment(&mut self, segment: &DataSegment) -> fmt::Result {
        write!(self.out, "data {} = ", segment.address)?;
        for (i, item) in segment.items.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            match item {
                DataItem::Bytes(bytes) => write_string(self.out, bytes),
                DataItem::Int(value) => write!(self.out, "{} {}", Type::Int(value.width()), value.value())?,
                DataItem::FuncAddr { width, func } => {
                    write!(self.out, "{} @{}", Type::Int(*width), self.module.function(*func).name)?;
                }
            }
        }
        writeln!(self.out)
    }

    fn function(&mut self, function: &Function) -> fmt::Result {
        write!(self.out, "func @{}", function.name)?;
        write_params(self.out, function, &function.params)?;
        if let Some(result) = function.result {
            write!(self.out, " -> {result}")?;
        }
        writeln!(self.out, " {{")?;
        for block in &function.blocks {
            self.block(function, block)?;
        }
        writeln!(self.out, "}}")
    }

    fn block(&mut self, function: &Function, block: &Block) -> fmt::Result {
        self.out.push_str(&block.name);
        if !block.params.is_empty() {
            write_params(self.out, function, &block.params)?;
        }
        writeln!(self.out, ":")?;
        for inst in &block.insts {
            self.out.push_str("    ");
            self.inst(function, inst)?;
            writeln!(self.out)?;
        }
        self.out.push_str("    ");
        self.terminator(function, &block.term)?;
        writeln!(self.out)
    }

    fn inst(&mut self, f: &Function, inst: &Inst) -> fmt::Result {
        if let Some(dst) = inst.result() {
            write!(self.out, "%{} = ", f.slot_names[dst.index()])?;
        }
        let op = |operand: &Operand| OperandText { function: f, operand: *operand, typed: false };
        let name = |slot: &Slot| SlotText(f, *slot);
        match inst {
            Inst::Binary { op: o, width, lhs, rhs, .. } => {
                write!(self.out, "{} {} {}, {}", o.mnemonic(), Type::Int(*width), op(lhs), op(rhs))
            }
            Inst::Compare { pred, width, lhs, rhs, .. } => {
                write!(self.out, "icmp {} {} {}, {}", pred.mnemonic(), Type::Int(*width), op(lhs), op(rhs))
            }
            Inst::Select { width, cond, if_true, if_false, .. } => {
                write!(self.out, "select {} {}, {}, {}", Type::Int(*width), op(cond), op(if_true), op(if_false))
            }
            Inst::Cast { op: o, from, to, value, .. } => {
                write!(self.out, "{} {} {} to {}", o.mnemonic(), Type::Int(*from), op(value), Type::Int(*to))
            }
            Inst::Load { width, address, .. } => write!(self.out, "load {} {}", Type::Int(*width), op(address)),
            Inst::Store { width, value, address } => {
                write!(self.out, "store {} {}, {}", Type::Int(*width), op(value), op(address))
            }
            Inst::Alloca { size, .. } => write!(self.out, "alloca {}", op(size)),
            Inst::StackSave { .. } => write!(self.out, "stacksave"),
            Inst::StackRestore { pointer } => write!(self.out, "stackrestore {}", op(pointer)),
            Inst::Call { callee, args, .. } => {
                write!(self.out, "call @{}", self.module.function(*callee).name)?;
                write_args(self.out, f, args)
            }
            Inst::CallIndirect { signature, target, args, .. } => {
                self.out.push_str("call_indirect ");
                write_signature(self.out, signature)?;
                write!(self.out, " {}", op(target))?;
                write_args(self.out, f, args)
            }
            Inst::FuncAddr { func, .. } => write!(self.out, "faddr @{}", self.module.function(*func).name),
            Inst::Effect { byte } => write!(self.out, "effect {}", op(byte)),
            Inst::Qalloc { width, .. } => write!(self.out, "qalloc {}", width.bits()),
            Inst::Qfree { register } => write!(self.out, "qfree {}", name(register)),
            Inst::Gate { gate, target, controls } => self.gate(f, *gate, *target, controls),
            Inst::Measure { register, .. } => write!(self.out, "measure {}", name(register)),
            Inst::Apply { callee, args, out, fault, budget } => {
                write!(self.out, "apply @{}", self.module.function(*callee).name)?;
                write_args(self.out, f, args)?;
                write!(self.out, " -> {}, {} budget {}", name(out), name(fault), op(budget))
            }
        }
    }

    fn gate(&mut self, f: &Function, gate: Gate, target: Qubit, controls: &[Qubit]) -> fmt::Result {
        match gate {
            Gate::Fixed(fixed) => self.out.push_str(fixed.mnemonic()),
            Gate::Rotate(axis, angle) => write!(self.out, "{}({angle:?})", axis.mnemonic())?,
        }
        write!(self.out, " {}", QubitText(f, target))?;
        for (i, control) in controls.iter().enumerate() {
            let separator = if i == 0 { " ctrl " } else { ", " };
            write!(self.out, "{separator}{}", QubitText(f, *control))?;
        }
        Ok(())
    }

    fn terminator(&mut self, f: &Function, term: &Terminator) -> fmt::Result {
        let op = |operand: &Operand| OperandText { function: f, operand: *operand, typed: false };
        match term {
            Terminator::Jump(target) => {
                self.out.push_str("br ");
                write_target(self.out, f, target)
            }
            Terminator::Branch { cond, if_true, if_false } => {
                write!(self.out, "br {}, ", op(cond))?;
                write_target(self.out, f, if_true)?;
                self.out.push_str(", ");
                write_target(self.out, f, if_false)
            }
            Terminator::Switch { width, value, cases, default } => {
                write!(self.out, "switch {} {}", Type::Int(*width), op(value))?;
                for case in cases {
                    write!(self.out, ", {}: ", case.value)?;
                    write_target(self.out, f, &case.target)?;
                }
                self.out.push_str(", default: ");
                write_target(self.out, f, default)
            }
            Terminator::Return(None) => write!(self.out, "ret"),
            Terminator::Return(Some(value)) => write!(self.out, "ret {}", op(value)),
            Terminator::Fault(fault) => write!(self.out, "fault {fault}"),
        }
    }
}

fn write_params(out: &mut String, f: &Function, params: &[Param]) -> fmt::Result {
    out.push('(');
    for (i, param) in params.iter().enumerate() {
        let separator = if i == 0 { "" } else { ", " };
        write!(out, "{separator}{} {}", param.ty, SlotText(f, param.slot))?;
    }
    out.push(')');
    Ok(())
}

fn write_signature(out: &mut String, signature: &Signature) -> fmt::Result {
    out.push_str("fn(");
    for (i, ty) in signature.params.iter().enumerate() {
        let separator = if i == 0 { "" } else { ", " };
        write!(out, "{separator}{ty}")?;
    }
    out.push(')');
    if let Some(result) = signature.result {
        write!(out, " -> {result}")?;
    }
    Ok(())
}

fn write_args(out: &mut String, f: &Function, args: &[Operand]) -> fmt::Result {
    out.push('(');
    for (i, arg) in args.iter().enumerate() {
        let separator = if i == 0 { "" } else { ", " };
        write!(out, "{separator}{}", OperandText { function: f, operand: *arg, typed: true })?;
    }
    out.push(')');
    Ok(())
}

fn write_target(out: &mut String, f: &Function, target: &Target) -> fmt::Result {
    out.push_str(&f.block(target.block).name);
    if target.args.is_empty() { Ok(()) } else { write_args(out, f, &target.args) }
}

/// Writes bytes as a string literal, escaping everything but printable ASCII.
fn write_string(out: &mut String, bytes: &[u8]) {
    out.push('"');
    for &byte in bytes {
        match byte {
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b' '..=b'~' => out.push(char::from(byte)),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push('"');
}

struct SlotText<'a>(&'a Function, Slot);

impl fmt::Display for SlotText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0.slot_names[self.1.index()])
    }
}

struct QubitText<'a>(&'a Function, Qubit);

impl fmt::Display for QubitText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Qubit { register, bit } = self.1;
        let name = SlotText(self.0, register);
        if bit == 0 { write!(f, "{name}") } else { write!(f, "{name}[{bit}]") }
    }
}

/// An operand; constants carry their type when `typed`, as in argument lists.
struct OperandText<'a> {
    function: &'a Function,
    operand: Operand,
    typed: bool,
}

impl fmt::Display for OperandText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.operand {
            Operand::Slot(slot) => write!(f, "{}", SlotText(self.function, slot)),
            Operand::Const(value) if self.typed => write!(f, "{} {}", Type::Int(value.width()), value.value()),
            Operand::Const(value) => write!(f, "{}", value.value()),
        }
    }
}
