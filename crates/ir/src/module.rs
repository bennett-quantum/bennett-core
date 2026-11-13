//! The device program: modules, functions, blocks and instructions.

use crate::{BinOp, BlockId, CastOp, Const, Fault, FuncId, Gate, Operand, Predicate, Slot, Type, Width};

/// A device program: functions, an initial memory image and a memory size.
///
/// Memory is one flat little-endian byte array. Static data sits at the addresses its segments name;
/// the stack starts at the top of memory and grows down, and may not reach below [`Module::static_end`].
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    /// The size of memory in bytes.
    pub memory_size: u64,
    /// The initial contents of memory; every other byte starts at zero.
    pub data: Vec<DataSegment>,
    /// The functions, indexed by [`FuncId`].
    pub functions: Vec<Function>,
}

impl Module {
    /// The function with the given index.
    pub fn function(&self, id: FuncId) -> &Function {
        &self.functions[id.index()]
    }

    /// The function with the given name. A linear search, meant for choosing an entry point.
    pub fn function_named(&self, name: &str) -> Option<FuncId> {
        let index = self.functions.iter().position(|f| f.name == name)?;
        u32::try_from(index).ok().map(FuncId)
    }

    /// The function whose address is `address`, if there is one.
    pub fn function_at(&self, address: u64) -> Option<FuncId> {
        let index = address.checked_sub(1)?;
        let id = FuncId(u32::try_from(index).ok()?);
        (id.index() < self.functions.len()).then_some(id)
    }

    /// The first address above all static data: the lowest address the stack may use.
    pub fn static_end(&self) -> u64 {
        self.data.iter().map(|s| s.address.saturating_add(s.len())).max().unwrap_or(0)
    }
}

impl FuncId {
    /// The function's address as a value. Addresses start at 1, so that 0 is never a function.
    pub fn address(self) -> u64 {
        u64::from(self.0) + 1
    }
}

/// Bytes placed in memory before execution starts.
#[derive(Clone, Debug, PartialEq)]
pub struct DataSegment {
    /// The address of the first byte.
    pub address: u64,
    /// The contents, laid out one after another.
    pub items: Vec<DataItem>,
}

impl DataSegment {
    /// The number of bytes the segment occupies.
    pub fn len(&self) -> u64 {
        self.items.iter().map(DataItem::len).sum()
    }

    /// Whether the segment occupies no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The segment's bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for item in &self.items {
            item.encode(&mut bytes);
        }
        bytes
    }
}

/// One item of a data segment.
#[derive(Clone, Debug, PartialEq)]
pub enum DataItem {
    /// Raw bytes, written as a string.
    Bytes(Vec<u8>),
    /// An integer, little-endian in its width's bytes.
    Int(Const),
    /// The address of a function, little-endian in the width's bytes.
    FuncAddr {
        /// The width the address is stored at.
        width: Width,
        /// The function.
        func: FuncId,
    },
}

impl DataItem {
    /// The number of bytes the item occupies.
    pub fn len(&self) -> u64 {
        match self {
            DataItem::Bytes(bytes) => bytes.len() as u64,
            DataItem::Int(value) => value.width().bytes(),
            DataItem::FuncAddr { width, .. } => width.bytes(),
        }
    }

    /// Whether the item occupies no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn encode(&self, out: &mut Vec<u8>) {
        let (width, value) = match self {
            DataItem::Bytes(bytes) => return out.extend_from_slice(bytes),
            DataItem::Int(value) => (value.width(), value.value()),
            DataItem::FuncAddr { width, func } => (*width, u128::from(func.address())),
        };
        let bytes = value.to_le_bytes();
        out.extend_from_slice(&bytes[..width.bytes() as usize]);
    }
}

/// A function: typed parameters, at most one result, and a body of blocks in SSA form.
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    /// The name, without the `@`.
    pub name: String,
    /// The parameters, in order.
    pub params: Vec<Param>,
    /// The result type, if the function returns a value.
    pub result: Option<Type>,
    /// The body; block 0 is the entry and takes no parameters.
    pub blocks: Vec<Block>,
    /// The name of each slot, without the `%`, indexed by [`Slot`]. The length is the frame size.
    pub slot_names: Vec<String>,
}

impl Function {
    /// The number of slots in the function's frame.
    pub fn slot_count(&self) -> usize {
        self.slot_names.len()
    }

    /// The block with the given index.
    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id.index()]
    }

    /// Whether the function's parameter and result types are exactly `signature`.
    pub fn has_signature(&self, signature: &Signature) -> bool {
        self.result == signature.result
            && self.params.len() == signature.params.len()
            && self.params.iter().zip(&signature.params).all(|(p, ty)| p.ty == *ty)
    }
}

/// A typed slot that receives a value on entry: a function or block parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    /// The slot receiving the value.
    pub slot: Slot,
    /// The type of the value.
    pub ty: Type,
}

/// Parameter and result types, as named by `call_indirect`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// The parameter types.
    pub params: Vec<Type>,
    /// The result type, if any.
    pub result: Option<Type>,
}

/// A basic block: parameters, straight-line instructions and one terminator.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// The label.
    pub name: String,
    /// Block parameters, assigned by the branch that enters the block.
    pub params: Vec<Param>,
    /// The instructions, in order.
    pub insts: Vec<Inst>,
    /// The instruction that leaves the block.
    pub term: Terminator,
}

/// One qubit of a quantum register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Qubit {
    /// The slot holding the register handle.
    pub register: Slot,
    /// The qubit's index in the register, from 0 (least significant).
    pub bit: u8,
}

/// A non-terminating instruction.
#[derive(Clone, Debug, PartialEq)]
pub enum Inst {
    /// `dst = op width lhs, rhs`.
    Binary {
        /// The operation.
        op: BinOp,
        /// The width of operands and result.
        width: Width,
        /// The result.
        dst: Slot,
        /// The left operand.
        lhs: Operand,
        /// The right operand.
        rhs: Operand,
    },
    /// `dst = icmp pred width lhs, rhs`, an `i1`.
    Compare {
        /// The comparison.
        pred: Predicate,
        /// The width of the operands.
        width: Width,
        /// The `i1` result.
        dst: Slot,
        /// The left operand.
        lhs: Operand,
        /// The right operand.
        rhs: Operand,
    },
    /// `dst = select width cond, if_true, if_false`.
    Select {
        /// The width of the choices and result.
        width: Width,
        /// The result.
        dst: Slot,
        /// The `i1` condition.
        cond: Operand,
        /// The result when `cond` is 1.
        if_true: Operand,
        /// The result when `cond` is 0.
        if_false: Operand,
    },
    /// `dst = op from value to to`.
    Cast {
        /// The kind of conversion.
        op: CastOp,
        /// The operand width.
        from: Width,
        /// The result width.
        to: Width,
        /// The result.
        dst: Slot,
        /// The operand.
        value: Operand,
    },
    /// `dst = load width address`: little-endian, `width.bytes()` bytes.
    Load {
        /// The width of the loaded value.
        width: Width,
        /// The result.
        dst: Slot,
        /// The `i64` address.
        address: Operand,
    },
    /// `store width value, address`: little-endian, `width.bytes()` bytes.
    Store {
        /// The width of the stored value.
        width: Width,
        /// The value.
        value: Operand,
        /// The `i64` address.
        address: Operand,
    },
    /// `dst = alloca size`: reserves `size` bytes of stack, 8-byte aligned, until the function returns.
    Alloca {
        /// The `i64` address of the reserved bytes.
        dst: Slot,
        /// The `i64` number of bytes.
        size: Operand,
    },
    /// `dst = stacksave`: the stack pointer, for a later `stackrestore`.
    StackSave {
        /// The `i64` stack pointer.
        dst: Slot,
    },
    /// `stackrestore pointer`: frees what `alloca` reserved since `pointer` was saved, by setting the
    /// stack pointer back to it. `pointer` must be at or above the stack pointer and within the current
    /// frame's allocations; otherwise the fault `bad_stack_restore`.
    StackRestore {
        /// The `i64` stack pointer, as `stacksave` gave it.
        pointer: Operand,
    },
    /// `[dst =] call @callee(args)`.
    Call {
        /// The function called.
        callee: FuncId,
        /// The arguments.
        args: Vec<Operand>,
        /// The result; allowed only when the callee returns a value, which may be ignored.
        dst: Option<Slot>,
    },
    /// `[dst =] call_indirect fn(params) [-> result] target(args)`.
    CallIndirect {
        /// The signature the target must have.
        signature: Signature,
        /// The `i64` function address.
        target: Operand,
        /// The arguments.
        args: Vec<Operand>,
        /// The result; allowed only when the signature has one.
        dst: Option<Slot>,
    },
    /// `dst = faddr @func`, an `i64`.
    FuncAddr {
        /// The address.
        dst: Slot,
        /// The function.
        func: FuncId,
    },
    /// `effect byte`: writes an `i8` to the console.
    Effect {
        /// The `i8` byte.
        byte: Operand,
    },
    /// `dst = qalloc bits`: a fresh register of `width` qubits, all |0⟩.
    Qalloc {
        /// The register handle.
        dst: Slot,
        /// The number of qubits.
        width: Width,
    },
    /// `qfree register`: releases a register that is zero in every basis configuration.
    Qfree {
        /// The register handle.
        register: Slot,
    },
    /// `gate target [ctrl controls]`: a single-qubit gate, applied where every control is 1.
    Gate {
        /// The gate.
        gate: Gate,
        /// The qubit acted on.
        target: Qubit,
        /// The control qubits.
        controls: Vec<Qubit>,
    },
    /// `dst = measure register`: samples the register's value and collapses the state.
    Measure {
        /// The measured value, an integer of the register's width.
        dst: Slot,
        /// The register handle.
        register: Slot,
    },
    /// `apply @callee(args) -> out, fault budget b`: runs `callee` coherently, on each basis
    /// configuration separately. See `docs/DESIGN.md`.
    Apply {
        /// The function run coherently.
        callee: FuncId,
        /// The arguments: integers, or register handles read as integers.
        args: Vec<Operand>,
        /// The register the result is XORed into.
        out: Slot,
        /// The register a fault code is XORed into.
        fault: Slot,
        /// The `i64` step budget of each run of the callee.
        budget: Operand,
    },
}

impl Inst {
    /// The slot the instruction defines, if any.
    pub fn result(&self) -> Option<Slot> {
        match self {
            Inst::Binary { dst, .. }
            | Inst::Compare { dst, .. }
            | Inst::Select { dst, .. }
            | Inst::Cast { dst, .. }
            | Inst::Load { dst, .. }
            | Inst::Alloca { dst, .. }
            | Inst::StackSave { dst }
            | Inst::FuncAddr { dst, .. }
            | Inst::Qalloc { dst, .. }
            | Inst::Measure { dst, .. } => Some(*dst),
            Inst::Call { dst, .. } | Inst::CallIndirect { dst, .. } => *dst,
            Inst::Store { .. }
            | Inst::StackRestore { .. }
            | Inst::Effect { .. }
            | Inst::Qfree { .. }
            | Inst::Gate { .. }
            | Inst::Apply { .. } => None,
        }
    }

    /// Calls `visit` with every slot the instruction reads.
    pub fn visit_uses(&self, mut visit: impl FnMut(Slot)) {
        let mut operand = |op: &Operand| {
            if let Operand::Slot(slot) = op {
                visit(*slot);
            }
        };
        match self {
            Inst::Binary { lhs, rhs, .. } | Inst::Compare { lhs, rhs, .. } => {
                operand(lhs);
                operand(rhs);
            }
            Inst::Select { cond, if_true, if_false, .. } => {
                operand(cond);
                operand(if_true);
                operand(if_false);
            }
            Inst::Cast { value, .. } => operand(value),
            Inst::Load { address, .. } => operand(address),
            Inst::Store { value, address, .. } => {
                operand(value);
                operand(address);
            }
            Inst::Alloca { size, .. } => operand(size),
            Inst::StackRestore { pointer } => operand(pointer),
            Inst::Call { args, .. } => args.iter().for_each(operand),
            Inst::CallIndirect { target, args, .. } => {
                operand(target);
                args.iter().for_each(operand);
            }
            Inst::Effect { byte } => operand(byte),
            Inst::FuncAddr { .. } | Inst::Qalloc { .. } | Inst::StackSave { .. } => {}
            Inst::Qfree { register } | Inst::Measure { register, .. } => operand(&Operand::Slot(*register)),
            Inst::Gate { target, controls, .. } => {
                operand(&Operand::Slot(target.register));
                controls.iter().for_each(|c| operand(&Operand::Slot(c.register)));
            }
            Inst::Apply { args, out, fault, budget, .. } => {
                args.iter().for_each(&mut operand);
                operand(&Operand::Slot(*out));
                operand(&Operand::Slot(*fault));
                operand(budget);
            }
        }
    }
}

/// A branch target: a block and the values for its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// The block entered.
    pub block: BlockId,
    /// One value per block parameter.
    pub args: Vec<Operand>,
}

/// One arm of a `switch`.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    /// The value selecting this arm, canonical at the switch's width.
    pub value: u128,
    /// Where control goes.
    pub target: Target,
}

/// The instruction that ends a block.
#[derive(Clone, Debug, PartialEq)]
pub enum Terminator {
    /// `br target`.
    Jump(Target),
    /// `br cond, if_true, if_false`.
    Branch {
        /// The `i1` condition.
        cond: Operand,
        /// Taken when `cond` is 1.
        if_true: Target,
        /// Taken when `cond` is 0.
        if_false: Target,
    },
    /// `switch width value, case: target, ..., default: target`.
    Switch {
        /// The width of the value.
        width: Width,
        /// The value switched on.
        value: Operand,
        /// The arms, with distinct values.
        cases: Vec<SwitchCase>,
        /// Taken when no arm matches.
        default: Target,
    },
    /// `ret [value]`.
    Return(Option<Operand>),
    /// `fault name`: ends execution with the named fault.
    Fault(Fault),
}

impl Terminator {
    /// Calls `visit` with every branch target.
    pub fn visit_targets<'a>(&'a self, mut visit: impl FnMut(&'a Target)) {
        match self {
            Terminator::Jump(target) => visit(target),
            Terminator::Branch { if_true, if_false, .. } => {
                visit(if_true);
                visit(if_false);
            }
            Terminator::Switch { cases, default, .. } => {
                cases.iter().for_each(|case| visit(&case.target));
                visit(default);
            }
            Terminator::Return(_) | Terminator::Fault(_) => {}
        }
    }

    /// Calls `visit` with every branch target, mutably.
    pub fn visit_targets_mut(&mut self, mut visit: impl FnMut(&mut Target)) {
        match self {
            Terminator::Jump(target) => visit(target),
            Terminator::Branch { if_true, if_false, .. } => {
                visit(if_true);
                visit(if_false);
            }
            Terminator::Switch { cases, default, .. } => {
                cases.iter_mut().for_each(|case| visit(&mut case.target));
                visit(default);
            }
            Terminator::Return(_) | Terminator::Fault(_) => {}
        }
    }

    /// Calls `visit` with every slot the terminator reads, including branch arguments.
    pub fn visit_uses(&self, mut visit: impl FnMut(Slot)) {
        let mut operand = |op: &Operand| {
            if let Operand::Slot(slot) = op {
                visit(*slot);
            }
        };
        match self {
            Terminator::Branch { cond, .. } => operand(cond),
            Terminator::Switch { value, .. } => operand(value),
            Terminator::Return(Some(value)) => operand(value),
            Terminator::Jump(_) | Terminator::Return(None) | Terminator::Fault(_) => {}
        }
        self.visit_targets(|target| target.args.iter().for_each(&mut operand));
    }
}
