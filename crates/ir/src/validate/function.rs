//! Checks of one function: index ranges, SSA definitions, operand types and dominance.

use petgraph::algo::dominators;
use petgraph::graph::{DiGraph, NodeIndex};

use super::error::{Expected, Location, Problem, ValidationError};
use crate::{CastOp, Fault, Function, Gate, Inst, Module, Operand, Qubit, Slot, Terminator, Type, Width};

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
                let Some(dst) = inst.result() else { continue };
                let site = Site::Inst(b, i);
                let ty = self.result_type(site, inst);
                self.define(site, dst, ty, Def { block: b, position: i + 1 });
                if let Inst::Measure { dst, register } = inst {
                    measured.push((*dst, *register));
                }
            }
        }
        for (dst, register) in measured {
            if let Some(Type::Qreg(width)) = self.types[register.index()] {
                self.types[dst.index()] = Some(Type::Int(width));
            }
        }
    }

    fn define(&mut self, site: Site, slot: Slot, ty: Option<Type>, def: Def) {
        if self.defs[slot.index()].is_some() {
            let name = self.slot_name(slot);
            self.error(site, Problem::Redefined(name));
            return;
        }
        if let Some(Type::Qreg(width)) = ty
            && width.bits() > Width::MAX_QUBITS
        {
            self.error(site, Problem::RegisterTooWide(width.bits()));
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
            Inst::Cast { to, .. } => Some(Type::Int(*to)),
            Inst::Alloca { .. } | Inst::FuncAddr { .. } | Inst::StackSave { .. } => Some(Type::Int(Width::WORD)),
            Inst::Qalloc { width, .. } => Some(Type::Qreg(*width)),
            Inst::Call { callee, .. } => self.call_result(site, self.module.function(*callee).result),
            Inst::CallIndirect { signature, .. } => self.call_result(site, signature.result),
            Inst::Measure { .. }
            | Inst::Store { .. }
            | Inst::Effect { .. }
            | Inst::StackRestore { .. }
            | Inst::Qfree { .. }
            | Inst::Gate { .. }
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
            Inst::Qfree { register } | Inst::Measure { register, .. } => {
                self.register_width(site, *register);
            }
            Inst::Gate { gate, target, controls } => self.gate(site, *gate, *target, controls),
            Inst::Apply { callee, args, out, fault, budget } => {
                self.apply(site, *callee, args, *out, *fault);
                self.expect(site, *budget, Type::Int(Width::WORD));
            }
            Inst::FuncAddr { .. } | Inst::Qalloc { .. } | Inst::StackSave { .. } => {}
        }
    }

    fn gate(&mut self, site: Site, gate: Gate, target: Qubit, controls: &[Qubit]) {
        if let Gate::Rotate(_, angle) = gate
            && !angle.is_finite()
        {
            self.error(site, Problem::AngleNotFinite);
        }
        let qubits: Vec<Qubit> = std::iter::once(target).chain(controls.iter().copied()).collect();
        for (k, qubit) in qubits.iter().enumerate() {
            if let Some(width) = self.register_width(site, qubit.register)
                && u32::from(qubit.bit) >= width.bits()
            {
                self.error(site, Problem::QubitOutOfRange { bit: qubit.bit, bits: width.bits() });
            }
            if qubits[..k].contains(qubit) {
                let name = self.slot_name(qubit.register);
                self.error(site, Problem::RepeatedQubit(name, qubit.bit));
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
                        Some(Type::Int(w) | Type::Qreg(w)) if w == width => {}
                        Some(found) => self.mismatch(site, *arg, Expected::IntOrRegister(width.bits()), found),
                        None => {}
                    }
                }
            }
        }
        if let Some(width) = result {
            self.expect(site, Operand::Slot(out), Type::Qreg(width));
        }
        match self.types[fault.index()] {
            Some(Type::Qreg(width)) if width >= Fault::CODE_WIDTH => {}
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

    /// The width of the register in `slot`, reporting the slot if it is not a register.
    fn register_width(&mut self, site: Site, slot: Slot) -> Option<Width> {
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
