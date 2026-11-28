//! Checks of one function: index ranges, SSA definitions, operand types and dominance.

use petgraph::algo::dominators;
use petgraph::graph::{DiGraph, NodeIndex};

use super::error::{Expected, Location, Problem, ValidationError};
use crate::{
    CastOp, Fault, FixedGate, Function, Gate, GateKind, Inst, MEMORY_QUBIT_BITS, MeasureSource, Module, Operand, Place,
    Qubits, Slot, Terminator, Type, Width,
};

/// Checks `function`, appending what is wrong to `errors`, and returns the type of every slot. The types
/// are meaningful only when no error was found.
pub(super) fn check(module: &Module, function: &Function, errors: &mut Vec<ValidationError>) -> Vec<Type> {
    let slots = function.slot_count();
    let mut checker = Checker { module, function, errors, types: vec![None; slots], defs: vec![None; slots] };
    if !checker.indices_in_range() {
        return Vec::new();
    }
    checker.definitions();
    checker.undefined_uses();
    checker.operand_types();
    checker.dominance();
    checker.types.into_iter().map(|ty| ty.unwrap_or(Type::Int(Width::WORD))).collect()
}

/// Where in the function a problem is.
#[derive(Clone, Copy)]
enum Site {
    Function,
    Block(usize),
    Inst(usize, usize),
    Terminator(usize),
}

/// Where a slot is defined: its block, and position 0 for parameters or `i + 1` for instruction `i`.
#[derive(Clone, Copy)]
struct Def {
    block: usize,
    position: usize,
}

struct Checker<'a> {
    module: &'a Module,
    function: &'a Function,
    errors: &'a mut Vec<ValidationError>,
    types: Vec<Option<Type>>,
    defs: Vec<Option<Def>>,
}

impl<'a> Checker<'a> {
    fn error(&mut self, site: Site, problem: Problem) {
        let function = self.function.name.clone();
        let block = |b: usize| self.function.blocks[b].name.clone();
        let location = match site {
            Site::Function => Location::Function(function),
            Site::Block(b) => Location::Block { function, block: block(b) },
            Site::Inst(b, i) => Location::Inst { function, block: block(b), number: i + 1 },
            Site::Terminator(b) => Location::Terminator { function, block: block(b) },
        };
        self.errors.push(ValidationError { location, problem });
    }

    fn slot_name(&self, slot: Slot) -> String {
        self.function.slot_names[slot.index()].clone()
    }

    /// Checks that every slot, block and function index is in range; nothing else can be checked
    /// otherwise.
    fn indices_in_range(&mut self) -> bool {
        let before = self.errors.len();
        let function = self.function;
        let Some(entry) = function.blocks.first() else {
            self.error(Site::Function, Problem::NoBlocks);
            return false;
        };
        if !entry.params.is_empty() {
            self.error(Site::Block(0), Problem::EntryParams);
        }
        for param in &function.params {
            self.slot_in_range(Site::Function, param.slot);
        }
        for (b, block) in function.blocks.iter().enumerate() {
            for param in &block.params {
                self.slot_in_range(Site::Block(b), param.slot);
            }
            for (i, inst) in block.insts.iter().enumerate() {
                let site = Site::Inst(b, i);
                inst.result().into_iter().for_each(|slot| self.slot_in_range(site, slot));
                inst.visit_uses(|slot| self.slot_in_range(site, slot));
                if let Inst::Call { callee: func, .. } | Inst::Apply { callee: func, .. } | Inst::FuncAddr { func, .. } =
                    inst
                    && func.index() >= self.module.functions.len()
                {
                    self.error(site, Problem::UnknownFunction(func.0));
                }
            }
            let site = Site::Terminator(b);
            block.term.visit_uses(|slot| self.slot_in_range(site, slot));
            block.term.visit_targets(|target| {
                if target.block.index() >= function.blocks.len() {
                    self.error(site, Problem::UnknownBlock(target.block.0));
                }
            });
        }
        self.errors.len() == before
    }

    fn slot_in_range(&mut self, site: Site, slot: Slot) {
        if slot.index() >= self.function.slot_count() {
            self.error(site, Problem::UnknownSlot(slot.0));
        }
    }

    /// Records where each slot is defined and its type; reports slots defined twice.
    fn definitions(&mut self) {
        let function = self.function;
        let entry = Def { block: 0, position: 0 };
        for param in &function.params {
            self.define(Site::Function, param.slot, Some(param.ty), entry);
        }
        let mut measured = Vec::new();
        for (b, block) in function.blocks.iter().enumerate() {
            for param in &block.params {
                self.define(Site::Block(b), param.slot, Some(param.ty), Def { block: b, position: 0 });
            }
            for (i, inst) in block.insts.iter().enumerate() {
                let site = Site::Inst(b, i);
                let Some(dst) = inst.result() else { continue };
                let ty = self.result_type(site, inst);
                self.define(site, dst, ty, Def { block: b, position: i + 1 });
                if let Inst::Measure { dst, source: MeasureSource::Register(register) } = inst {
                    measured.push((*dst, *register));
                }
            }
        }
        for (dst, register) in measured {
            if let Some(Type::Qreg(qubits)) = self.types[register.index()] {
                self.types[dst.index()] = qubits.as_width().map(Type::Int);
            }
        }
    }

    fn define(&mut self, site: Site, slot: Slot, ty: Option<Type>, def: Def) {
        if self.defs[slot.index()].is_some() {
            let name = self.slot_name(slot);
            self.error(site, Problem::Redefined(name));
            return;
        }
        self.defs[slot.index()] = Some(def);
        self.types[slot.index()] = ty;
    }

    /// The type of the instruction's result, if it can be known before other slots are typed.
    fn result_type(&mut self, site: Site, inst: &Inst) -> Option<Type> {
        match inst {
            Inst::Binary { width, .. } | Inst::Select { width, .. } | Inst::Load { width, .. } => {
                Some(Type::Int(*width))
            }
            Inst::Compare { .. } => Some(Type::Int(Width::BIT)),
            Inst::Setjmp { .. } => Some(Type::Int(Width::JUMP_VALUE)),
            Inst::Cast { to, .. } => Some(Type::Int(*to)),
            Inst::Measure { source: MeasureSource::Memory { width, .. }, .. } => Some(Type::Int(*width)),
            Inst::Alloca { .. } | Inst::FuncAddr { .. } | Inst::StackSave { .. } | Inst::ReturnAddress { .. } => {
                Some(Type::Int(Width::WORD))
            }
            Inst::Qalloc { width, .. } => Some(Type::Qreg(*width)),
            Inst::QallocBy { .. } => Some(Type::Int(Width::WORD)),
            Inst::Call { callee, .. } => self.call_result(site, self.module.function(*callee).result),
            Inst::CallIndirect { signature, .. } => self.call_result(site, signature.result),
            Inst::Measure { source: MeasureSource::Register(_), .. }
            | Inst::Store { .. }
            | Inst::Effect { .. }
            | Inst::StackRestore { .. }
            | Inst::Qfree { .. }
            | Inst::Gate { .. }
            | Inst::GateAt { .. }
            | Inst::RotateBy { .. }
            | Inst::GateBy { .. }
            | Inst::Apply { .. } => None,
        }
    }

    fn call_result(&mut self, site: Site, result: Option<Type>) -> Option<Type> {
        if result.is_none() {
            self.error(site, Problem::NoResult);
        }
        result
    }

    fn undefined_uses(&mut self) {
        let function = self.function;
        let mut reported = vec![false; function.slot_count()];
        let mut uses = Vec::new();
        for (b, block) in function.blocks.iter().enumerate() {
            for (i, inst) in block.insts.iter().enumerate() {
                inst.visit_uses(|slot| uses.push((Site::Inst(b, i), slot)));
            }
            block.term.visit_uses(|slot| uses.push((Site::Terminator(b), slot)));
        }
        for (site, slot) in uses {
            if self.defs[slot.index()].is_none() && !reported[slot.index()] {
                reported[slot.index()] = true;
                let name = self.slot_name(slot);
                self.error(site, Problem::Undefined(name));
            }
        }
    }

    fn operand_types(&mut self) {
        let function = self.function;
        for (b, block) in function.blocks.iter().enumerate() {
            for (i, inst) in block.insts.iter().enumerate() {
                self.inst_types(Site::Inst(b, i), inst);
            }
            self.terminator_types(Site::Terminator(b), &block.term);
        }
    }

    fn inst_types(&mut self, site: Site, inst: &Inst) {
        match inst {
            Inst::Binary { width, lhs, rhs, .. } | Inst::Compare { width, lhs, rhs, .. } => {
                self.expect(site, *lhs, Type::Int(*width));
                self.expect(site, *rhs, Type::Int(*width));
            }
            Inst::Select { width, cond, if_true, if_false, .. } => {
                self.expect(site, *cond, Type::Int(Width::BIT));
                self.expect(site, *if_true, Type::Int(*width));
                self.expect(site, *if_false, Type::Int(*width));
            }
            Inst::Cast { op, from, to, value, .. } => {
                self.expect(site, *value, Type::Int(*from));
                let changes_width_correctly = match op {
                    CastOp::Zext | CastOp::Sext => to > from,
                    CastOp::Trunc => to < from,
                };
                if !changes_width_correctly {
                    let problem = Problem::CastWidths { op: op.mnemonic(), from: from.bits(), to: to.bits() };
                    self.error(site, problem);
                }
            }
            Inst::Load { address, .. }
            | Inst::Setjmp { buffer: address, .. }
            | Inst::Alloca { size: address, .. }
            | Inst::StackRestore { pointer: address } => {
                self.expect(site, *address, Type::Int(Width::WORD));
            }
            Inst::Store { width, value, address } => {
                self.expect(site, *value, Type::Int(*width));
                self.expect(site, *address, Type::Int(Width::WORD));
            }
            Inst::Call { callee, args, .. } => {
                let params = self.module.function(*callee).params.iter().map(|p| p.ty);
                self.arguments(site, args, params);
            }
            Inst::CallIndirect { signature, target, args, .. } => {
                self.expect(site, *target, Type::Int(Width::WORD));
                self.arguments(site, args, signature.params.iter().copied());
            }
            Inst::Effect { byte } => self.expect(site, *byte, Type::Int(Width::BYTE)),
            Inst::Qfree { register } => {
                self.register_operand(site, *register);
            }
            Inst::QallocBy { width, .. } => self.expect(site, *width, Type::Int(Width::WORD)),
            Inst::Measure { source: MeasureSource::Register(register), .. } => {
                if let Some(qubits) = self.register_width(site, *register)
                    && qubits.as_width().is_none()
                {
                    self.error(site, Problem::MeasureTooWide(qubits.count()));
                }
            }
            Inst::Measure { source: MeasureSource::Memory { address, .. }, .. } => {
                self.expect(site, *address, Type::Int(Width::WORD));
            }
            Inst::Gate { gate, target, controls } => {
                let places: Vec<Place> =
                    std::iter::once(target).chain(controls).map(|&qubit| Place::Register(qubit)).collect();
                self.gate(site, *gate, &places);
            }
            Inst::GateAt { gate, target, controls } => {
                let places: Vec<Place> = std::iter::once(target).chain(controls).copied().collect();
                if places.iter().all(|place| matches!(place, Place::Register(_))) {
                    self.error(site, Problem::GateAtWithoutMemory);
                }
                self.gate(site, *gate, &places);
            }
            Inst::RotateBy { axis, angle, target, controls } => {
                self.expect(site, Operand::Slot(*angle), Type::Int(Width::WORD));
                let places: Vec<Place> = std::iter::once(target).chain(controls).copied().collect();
                // The angle is checked when the gate runs; zero stands for it here.
                self.gate(site, Gate::Rotate(*axis, 0.0), &places);
            }
            Inst::GateBy { kind, target, controls } => {
                let code = match kind {
                    GateKind::Gate(code) => code,
                    GateKind::Rotation { code, angle } => {
                        self.expect(site, *angle, Type::Int(Width::WORD));
                        code
                    }
                };
                self.expect(site, *code, Type::Int(Width::WORD));
                let places: Vec<Place> = std::iter::once(target).chain(controls).copied().collect();
                // The kind and the angle are checked when the gate runs; X stands for the gate here.
                self.gate(site, Gate::Fixed(FixedGate::X), &places);
            }
            Inst::Apply { callee, args, out, fault, budget } => {
                self.apply(site, *callee, args, *out, *fault);
                self.expect(site, *budget, Type::Int(Width::WORD));
            }
            Inst::FuncAddr { .. } | Inst::Qalloc { .. } | Inst::StackSave { .. } | Inst::ReturnAddress { .. } => {}
        }
    }

    /// Checks a gate's qubits, the target first. A memory qubit's address is an `i64`; the same address
    /// operand and bit twice is reported here, other aliasing between memory qubits only at run time
    /// (`qubit_alias`), since addresses are values.
    fn gate(&mut self, site: Site, gate: Gate, places: &[Place]) {
        if let Gate::Rotate(_, angle) = gate
            && !angle.is_finite()
        {
            self.error(site, Problem::AngleNotFinite);
        }
        for (k, place) in places.iter().enumerate() {
            match *place {
                Place::Register(qubit) => {
                    // An `i64` handle's width is known only when the gate runs (`bad_range`).
                    // An `i64` handle's register is known only when the gate runs: its handle is checked first
                    // there, and a repetition after it (`qubit_alias`).
                    if let Some(qubits) = self.register_or_handle(site, qubit.register) {
                        if qubit.bit >= qubits.count() {
                            let bit = u64::from(qubit.bit);
                            self.error(site, Problem::QubitOutOfRange { bit, bits: qubits.count() });
                        }
                        if places[..k].contains(place) {
                            let name = self.slot_name(qubit.register);
                            self.error(site, Problem::RepeatedQubit(name, qubit.bit));
                        }
                    }
                }
                Place::Memory { address, bit } => {
                    self.expect(site, address, Type::Int(Width::WORD));
                    if bit >= MEMORY_QUBIT_BITS {
                        self.error(site, Problem::MemoryBitOutOfRange(bit));
                    }
                    if places[..k].contains(place) {
                        self.error(site, Problem::RepeatedMemoryQubit(bit));
                    }
                }
                // The position is a value: its range and any repetition are checked when the gate runs. A
                // constant position is the reader's form of a literal from 2^32, past every `qN` register's
                // width; a smaller one is a register qubit (`Place::Register`) and refused here, so that a module
                // the validator accepts is read back from its text as itself (the text of a refused one may read
                // as another, valid instruction). Only an `i64` constant is ranged. A constant handle has only
                // this form, at any position.
                Place::Indexed { register, index } => {
                    let qubits = self.register_operand(site, register);
                    self.expect(site, index, Type::Int(Width::WORD));
                    if let (Operand::Slot(_), Operand::Const(position)) = (register, index)
                        && position.width() == Width::WORD
                    {
                        if position.value() <= u128::from(u32::MAX) {
                            self.error(site, Problem::SmallConstantPosition(position.value() as u32));
                        } else if let Some(qubits) = qubits {
                            let bit = position.value() as u64;
                            self.error(site, Problem::QubitOutOfRange { bit, bits: qubits.count() });
                        }
                    }
                }
            }
        }
    }

    fn apply(&mut self, site: Site, callee: crate::FuncId, args: &[Operand], out: Slot, fault: Slot) {
        let callee = self.module.function(callee);
        let result = match callee.result {
            Some(Type::Int(width)) => Some(width),
            _ => {
                self.error(site, Problem::RegionResult);
                None
            }
        };
        let params: Option<Vec<Width>> =
            callee.params.iter().map(|p| if let Type::Int(width) = p.ty { Some(width) } else { None }).collect();
        match params {
            None => self.error(site, Problem::RegionParams),
            Some(params) if params.len() != args.len() => {
                self.error(site, Problem::ArgumentCount { expected: params.len(), found: args.len() });
            }
            Some(params) => {
                for (arg, width) in args.iter().zip(params) {
                    match self.type_of(*arg) {
                        Some(Type::Int(w)) if w == width => {}
                        Some(Type::Qreg(qubits)) if qubits == Qubits::from(width) => {}
                        Some(Type::Qreg(qubits)) if qubits.as_width().is_none() => {
                            self.operand_too_wide(site, *arg, qubits);
                        }
                        Some(found) => self.mismatch(site, *arg, Expected::IntOrRegister(width.bits()), found),
                        None => {}
                    }
                }
            }
        }
        match (result, self.types[out.index()]) {
            (_, Some(Type::Qreg(qubits))) if qubits.as_width().is_none() => {
                self.operand_too_wide(site, Operand::Slot(out), qubits);
            }
            (Some(width), _) => self.expect(site, Operand::Slot(out), Type::Qreg(Qubits::from(width))),
            (None, _) => {}
        }
        match self.types[fault.index()] {
            Some(Type::Qreg(qubits)) if qubits >= Qubits::from(Fault::CODE_WIDTH) => {}
            Some(found) => {
                let expected = Expected::RegisterOfAtLeast(Fault::CODE_WIDTH.bits());
                self.mismatch(site, Operand::Slot(fault), expected, found);
            }
            None => {}
        }
        let is_target = |op: &Operand| *op == Operand::Slot(out) || *op == Operand::Slot(fault);
        if out == fault || args.iter().any(is_target) {
            let culprit = if out == fault || args.contains(&Operand::Slot(out)) { out } else { fault };
            let name = self.slot_name(culprit);
            self.error(site, Problem::RegionAlias(name));
        }
    }

    fn terminator_types(&mut self, site: Site, term: &Terminator) {
        match term {
            Terminator::Branch { cond, .. } => self.expect(site, *cond, Type::Int(Width::BIT)),
            Terminator::Switch { width, value, cases, .. } => {
                self.expect(site, *value, Type::Int(*width));
                for (k, case) in cases.iter().enumerate() {
                    if case.value > width.mask() || cases[..k].iter().any(|c| c.value == case.value) {
                        self.error(site, Problem::BadCase(case.value));
                    }
                }
            }
            Terminator::Return(value) => match (value, self.function.result) {
                (None, None) => {}
                (Some(value), Some(ty)) => self.expect(site, *value, ty),
                _ => self.error(site, Problem::ReturnMismatch),
            },
            Terminator::Exit(value) => {
                if let Some(Type::Qreg(_)) = self.type_of(*value) {
                    self.error(site, Problem::ExitOperand);
                }
            }
            Terminator::Longjmp { buffer, value } => {
                self.expect(site, *buffer, Type::Int(Width::WORD));
                self.expect(site, *value, Type::Int(Width::JUMP_VALUE));
            }
            Terminator::Jump(_) | Terminator::Fault(_) => {}
        }
        let function = self.function;
        term.visit_targets(|target| {
            let params = function.block(target.block).params.iter().map(|p| p.ty);
            self.arguments(site, &target.args, params);
        });
    }

    fn arguments(&mut self, site: Site, args: &[Operand], params: impl ExactSizeIterator<Item = Type>) {
        if args.len() != params.len() {
            self.error(site, Problem::ArgumentCount { expected: params.len(), found: args.len() });
            return;
        }
        for (arg, ty) in args.iter().zip(params) {
            self.expect(site, *arg, ty);
        }
    }

    fn type_of(&self, operand: Operand) -> Option<Type> {
        match operand {
            Operand::Slot(slot) => self.types[slot.index()],
            Operand::Const(value) => Some(Type::Int(value.width())),
        }
    }

    /// Reports `operand` unless it has type `expected`. Untyped (undefined) slots are reported elsewhere.
    fn expect(&mut self, site: Site, operand: Operand, expected: Type) {
        if let Some(found) = self.type_of(operand)
            && found != expected
        {
            self.mismatch(site, operand, Expected::Type(expected), found);
        }
    }

    /// Reports a register wider than any integer used as an `apply` argument or output.
    fn operand_too_wide(&mut self, site: Site, operand: Operand, qubits: Qubits) {
        let name = match operand {
            Operand::Slot(slot) => self.slot_name(slot),
            Operand::Const(value) => value.value().to_string(),
        };
        self.error(site, Problem::RegionOperandTooWide(name, qubits.count()));
    }

    /// The width of the register in `slot` if it is a `qN` register; `None` for an `i64` handle, whose width
    /// the register table gives at run time. Reports any other type.
    fn register_or_handle(&mut self, site: Site, slot: Slot) -> Option<Qubits> {
        match self.types[slot.index()]? {
            Type::Qreg(width) => Some(width),
            Type::Int(Width::WORD) => None,
            found @ Type::Int(_) => {
                self.mismatch(site, Operand::Slot(slot), Expected::RegisterOrHandle, found);
                None
            }
        }
    }

    /// [`Checker::register_or_handle`] for an operand: a constant is an `i64` handle, whose width is known
    /// only when the instruction runs.
    fn register_operand(&mut self, site: Site, operand: Operand) -> Option<Qubits> {
        match operand {
            Operand::Slot(slot) => self.register_or_handle(site, slot),
            Operand::Const(_) => {
                self.expect(site, operand, Type::Int(Width::WORD));
                None
            }
        }
    }

    /// The width of the register in `slot`, reporting the slot if it is not a register.
    fn register_width(&mut self, site: Site, slot: Slot) -> Option<Qubits> {
        match self.types[slot.index()]? {
            Type::Qreg(width) => Some(width),
            found @ Type::Int(_) => {
                self.mismatch(site, Operand::Slot(slot), Expected::AnyRegister, found);
                None
            }
        }
    }

    fn mismatch(&mut self, site: Site, operand: Operand, expected: Expected, found: Type) {
        let operand = match operand {
            Operand::Slot(slot) => format!("`%{}`", self.slot_name(slot)),
            Operand::Const(value) => format!("constant {}", value.value()),
        };
        self.error(site, Problem::TypeMismatch { operand, expected, found });
    }

    /// Reports every use, in a reachable block, that its definition does not dominate.
    fn dominance(&mut self) {
        let function = self.function;
        let mut graph = DiGraph::<(), ()>::new();
        let nodes: Vec<NodeIndex> = function.blocks.iter().map(|_| graph.add_node(())).collect();
        for (b, block) in function.blocks.iter().enumerate() {
            block.term.visit_targets(|target| {
                graph.add_edge(nodes[b], nodes[target.block.index()], ());
            });
        }
        let doms = dominators::simple_fast(&graph, nodes[0]);
        let dominates = |a: usize, b: usize| doms.dominators(nodes[b]).is_some_and(|mut it| it.any(|n| n == nodes[a]));
        for (b, block) in function.blocks.iter().enumerate() {
            let reachable = b == 0 || doms.immediate_dominator(nodes[b]).is_some();
            if !reachable {
                continue;
            }
            let mut uses = Vec::new();
            for (i, inst) in block.insts.iter().enumerate() {
                inst.visit_uses(|slot| uses.push((Site::Inst(b, i), i + 1, slot)));
            }
            let end = block.insts.len() + 1;
            block.term.visit_uses(|slot| uses.push((Site::Terminator(b), end, slot)));
            for (site, position, slot) in uses {
                let Some(def) = self.defs[slot.index()] else { continue };
                let ok = if def.block == b { def.position < position } else { dominates(def.block, b) };
                if !ok {
                    let name = self.slot_name(slot);
                    self.error(site, Problem::NotDominated(name));
                }
            }
        }
    }
}
