//! Instructions and terminators.

use super::error::{Failure, ReadErrorKind};
use super::lexer::Token;
use super::{Parser, Scope};
use crate::{
    BinOp, BlockId, CastOp, Const, Fault, FixedGate, FuncId, Gate, GateKind, Inst, MeasureSource, Operand, Place,
    Predicate, Qubit, Qubits, Rotation, Signature, Slot, SwitchCase, Target, Terminator, Type, Width,
};

/// The mnemonics that end a block.
const TERMINATORS: [&str; 6] = ["br", "switch", "ret", "exit", "longjmp", "fault"];

/// One line of a block body.
pub(super) enum Statement {
    Inst(Inst),
    Term(Terminator),
}

impl<'s> Parser<'s> {
    /// `[%name =] MNEMONIC ...` up to the end of the line.
    pub(super) fn statement(&mut self, scope: &mut Scope<'s>) -> Result<Statement, Failure> {
        let dst = match (self.cursor.peek(), self.cursor.peek_nth(1)) {
            (Some(Token::Local(name)), Some(Token::Equals)) => {
                let offset = self.cursor.offset();
                self.cursor.advance();
                self.cursor.advance();
                Some(Slot(scope.slots.define(name, offset)?))
            }
            _ => None,
        };
        let offset = self.cursor.offset();
        let mnemonic = self.cursor.word("an instruction")?;
        let line = Line { dst, offset };
        let statement = self.statement_body(scope, mnemonic, line)?;
        self.cursor.end_of_line()?;
        Ok(statement)
    }

    fn statement_body(&mut self, scope: &mut Scope<'s>, mnemonic: &str, line: Line) -> Result<Statement, Failure> {
        if let Some(op) = BinOp::from_mnemonic(mnemonic) {
            let dst = line.result(op.mnemonic())?;
            let width = self.int_type("an integer type")?;
            let (lhs, rhs) = self.operand_pair(scope, width)?;
            return Ok(Statement::Inst(Inst::Binary { op, width, dst, lhs, rhs }));
        }
        if let Some(op) = CastOp::from_mnemonic(mnemonic) {
            return self.cast(scope, op, line.result(op.mnemonic())?).map(Statement::Inst);
        }
        if let Some(gate) = FixedGate::from_mnemonic(mnemonic) {
            line.no_result(gate.mnemonic())?;
            return self.gate(scope, Gate::Fixed(gate)).map(Statement::Inst);
        }
        if let Some(axis) = Rotation::from_mnemonic(mnemonic) {
            line.no_result(axis.mnemonic())?;
            // `(%name)`: an angle computed at run time; `(NUMBER)`: a literal.
            if let Some(Token::Local(_)) = self.cursor.peek_nth(1) {
                self.cursor.expect(Token::LParen, "`(` and an angle")?;
                let angle = self.slot_use(scope, "an angle")?;
                self.cursor.expect(Token::RParen, "`)`")?;
                let (target, controls) = self.places(scope)?;
                return Ok(Statement::Inst(Inst::RotateBy { axis, angle, target, controls }));
            }
            let angle = self.angle()?;
            return self.gate(scope, Gate::Rotate(axis, angle)).map(Statement::Inst);
        }
        let inst = match mnemonic {
            // `gate(CODE)` and `rotate(CODE, %angle)`: the kind given at run time.
            "gate" | "rotate" => {
                line.no_result(if mnemonic == "rotate" { "rotate" } else { "gate" })?;
                self.cursor.expect(Token::LParen, "`(` and a gate code")?;
                let code = self.operand(scope, Width::WORD)?;
                let kind = if mnemonic == "rotate" {
                    self.cursor.expect(Token::Comma, "`,` and an angle")?;
                    GateKind::Rotation { code, angle: self.slot_use(scope, "an angle")? }
                } else {
                    GateKind::Gate(code)
                };
                self.cursor.expect(Token::RParen, "`)`")?;
                let (target, controls) = self.places(scope)?;
                Inst::GateBy { kind, target, controls }
            }
            "icmp" => self.compare(scope, line.result("icmp")?)?,
            "select" => self.select(scope, line.result("select")?)?,
            "load" => {
                let dst = line.result("load")?;
                let width = self.int_type("an integer type")?;
                Inst::Load { width, dst, address: self.operand(scope, Width::WORD)? }
            }
            "store" => {
                line.no_result("store")?;
                let width = self.int_type("an integer type")?;
                let value = self.operand(scope, width)?;
                self.cursor.expect(Token::Comma, "`,`")?;
                Inst::Store { width, value, address: self.operand(scope, Width::WORD)? }
            }
            "alloca" => Inst::Alloca { dst: line.result("alloca")?, size: self.operand(scope, Width::WORD)? },
            "stacksave" => Inst::StackSave { dst: line.result("stacksave")? },
            "retaddr" => {
                let dst = line.result("retaddr")?;
                let (text, offset) = self.cursor.number("a frame level")?;
                let level =
                    text.parse().map_err(|_| Failure { offset, kind: ReadErrorKind::BadNumber(text.to_owned()) })?;
                Inst::ReturnAddress { dst, level }
            }
            "stackrestore" => {
                line.no_result("stackrestore")?;
                Inst::StackRestore { pointer: self.operand(scope, Width::WORD)? }
            }
            "setjmp" => Inst::Setjmp { dst: line.result("setjmp")?, buffer: self.operand(scope, Width::WORD)? },
            "call" => {
                let callee = self.function_use()?;
                let args = self.arguments(scope)?;
                Inst::Call { callee, args, dst: line.dst }
            }
            "call_indirect" => self.call_indirect(scope, line.dst)?,
            "faddr" => Inst::FuncAddr { dst: line.result("faddr")?, func: self.function_use()? },
            "effect" => {
                line.no_result("effect")?;
                Inst::Effect { byte: self.operand(scope, Width::BYTE)? }
            }
            "qalloc" => {
                let dst = line.result("qalloc")?;
                // `qalloc i64 WIDTH`: the width computed at run time, the result an `i64` handle.
                if self.cursor.eat(Token::Word("i64")) {
                    Inst::QallocBy { dst, width: self.operand(scope, Width::WORD)? }
                } else {
                    Inst::Qalloc { dst, width: self.register_width()? }
                }
            }
            "qfree" => {
                line.no_result("qfree")?;
                Inst::Qfree { register: self.slot_use(scope, "a register")? }
            }
            "measure" => {
                let dst = line.result("measure")?;
                // `measure iN [ADDRESS]` names qubits in memory; `measure %r` a register.
                let source = if matches!(self.cursor.peek(), Some(Token::Word(_))) {
                    let width = self.int_type("an integer type or a register")?;
                    self.cursor.expect(Token::LBracket, "`[`")?;
                    let address = self.operand(scope, Width::WORD)?;
                    self.cursor.expect(Token::RBracket, "`]`")?;
                    MeasureSource::Memory { width, address }
                } else {
                    MeasureSource::Register(self.slot_use(scope, "a register or an integer type")?)
                };
                Inst::Measure { dst, source }
            }
            "apply" => {
                line.no_result("apply")?;
                self.apply(scope)?
            }
            _ if TERMINATORS.contains(&mnemonic) => {
                if line.dst.is_some() {
                    let kind = ReadErrorKind::ResultNotAllowed("a terminator");
                    return Err(Failure { offset: line.offset, kind });
                }
                return self.terminator(scope, mnemonic).map(Statement::Term);
            }
            _ => {
                let kind = ReadErrorKind::UnknownInstruction(mnemonic.to_owned());
                return Err(Failure { offset: line.offset, kind });
            }
        };
        Ok(Statement::Inst(inst))
    }

    fn terminator(&mut self, scope: &mut Scope<'s>, mnemonic: &str) -> Result<Terminator, Failure> {
        match mnemonic {
            "br" if matches!(self.cursor.peek(), Some(Token::Word(_))) => Ok(Terminator::Jump(self.target(scope)?)),
            "br" => {
                let cond = self.operand(scope, Width::BIT)?;
                self.cursor.expect(Token::Comma, "`,`")?;
                let if_true = self.target(scope)?;
                self.cursor.expect(Token::Comma, "`,`")?;
                Ok(Terminator::Branch { cond, if_true, if_false: self.target(scope)? })
            }
            "switch" => self.switch(scope),
            "ret" => self.ret(scope),
            "exit" => Ok(Terminator::Exit(self.typed_operand(scope)?)),
            "longjmp" => {
                let buffer = self.operand(scope, Width::WORD)?;
                self.cursor.expect(Token::Comma, "`,`")?;
                Ok(Terminator::Longjmp { buffer, value: self.operand(scope, Width::JUMP_VALUE)? })
            }
            _ => {
                let offset = self.cursor.offset();
                let name = self.cursor.word("a fault name")?;
                Fault::from_name(name)
                    .map(Terminator::Fault)
                    .ok_or_else(|| unexpected_word(offset, "a fault name", name))
            }
        }
    }

    /// `icmp PRED iN a, b`
    fn compare(&mut self, scope: &mut Scope<'s>, dst: Slot) -> Result<Inst, Failure> {
        let offset = self.cursor.offset();
        let word = self.cursor.word("a comparison predicate")?;
        let pred =
            Predicate::from_mnemonic(word).ok_or_else(|| unexpected_word(offset, "a comparison predicate", word))?;
        let width = self.int_type("an integer type")?;
        let (lhs, rhs) = self.operand_pair(scope, width)?;
        Ok(Inst::Compare { pred, width, dst, lhs, rhs })
    }

    /// `select iN cond, a, b`
    fn select(&mut self, scope: &mut Scope<'s>, dst: Slot) -> Result<Inst, Failure> {
        let width = self.int_type("an integer type")?;
        let cond = self.operand(scope, Width::BIT)?;
        self.cursor.expect(Token::Comma, "`,`")?;
        let (if_true, if_false) = self.operand_pair(scope, width)?;
        Ok(Inst::Select { width, dst, cond, if_true, if_false })
    }

    /// `zext iN value to iM`, and likewise `sext` and `trunc`.
    fn cast(&mut self, scope: &mut Scope<'s>, op: CastOp, dst: Slot) -> Result<Inst, Failure> {
        let from = self.int_type("an integer type")?;
        let value = self.operand(scope, from)?;
        self.cursor.expect(Token::Word("to"), "`to`")?;
        let to = self.int_type("an integer type")?;
        Ok(Inst::Cast { op, from, to, dst, value })
    }

    /// `call_indirect fn(TYPE, ...) [-> TYPE] target(ARGS)`
    fn call_indirect(&mut self, scope: &mut Scope<'s>, dst: Option<Slot>) -> Result<Inst, Failure> {
        self.cursor.expect(Token::Word("fn"), "`fn`")?;
        self.cursor.expect(Token::LParen, "`(`")?;
        let mut params = Vec::new();
        if !self.cursor.eat(Token::RParen) {
            loop {
                params.push(self.any_type("a parameter type")?);
                if self.cursor.eat(Token::RParen) {
                    break;
                }
                self.cursor.expect(Token::Comma, "`,` or `)`")?;
            }
        }
        let result = if self.cursor.eat(Token::Arrow) { Some(self.any_type("a result type")?) } else { None };
        let target = self.operand(scope, Width::WORD)?;
        let args = self.arguments(scope)?;
        Ok(Inst::CallIndirect { signature: Signature { params, result }, target, args, dst })
    }

    /// `GATE target [ctrl qubit, ...]`, after the mnemonic and any angle. A gate whose qubits are all register
    /// qubits at literal positions below 2^32 is `Gate`; one with a qubit in memory or a position given as an
    /// `i64` value (a run-time position, or a literal from 2^32) is `GateAt`.
    fn gate(&mut self, scope: &mut Scope<'s>, gate: Gate) -> Result<Inst, Failure> {
        let (target, controls) = self.places(scope)?;
        let register = |place: &Place| match place {
            Place::Register(qubit) => Some(*qubit),
            Place::Memory { .. } | Place::Indexed { .. } => None,
        };
        match (register(&target), controls.iter().map(register).collect::<Option<Vec<Qubit>>>()) {
            (Some(target), Some(controls)) => Ok(Inst::Gate { gate, target, controls }),
            _ => Ok(Inst::GateAt { gate, target, controls }),
        }
    }

    /// `target [ctrl qubit, ...]`: a gate's target and controls.
    fn places(&mut self, scope: &mut Scope<'s>) -> Result<(Place, Vec<Place>), Failure> {
        let target = self.place(scope)?;
        let mut controls = Vec::new();
        if self.cursor.eat(Token::Word("ctrl")) {
            controls.push(self.place(scope)?);
            while self.cursor.eat(Token::Comma) {
                controls.push(self.place(scope)?);
            }
        }
        Ok((target, controls))
    }

    /// A qubit in a register (`%register[BIT]`, or `%register[%index]` with the position computed at run
    /// time) or in memory: `[ADDRESS:BIT]`, the address an `i64` value or constant; `[ADDRESS]` names bit 0.
    fn place(&mut self, scope: &mut Scope<'s>) -> Result<Place, Failure> {
        if !self.cursor.eat(Token::LBracket) {
            if let (Some(Token::LBracket), Some(Token::Local(_))) = (self.cursor.peek_nth(1), self.cursor.peek_nth(2)) {
                let register = self.slot_use(scope, "a qubit")?;
                self.cursor.expect(Token::LBracket, "`[`")?;
                let index = Operand::Slot(self.slot_use(scope, "a qubit index")?);
                self.cursor.expect(Token::RBracket, "`]`")?;
                return Ok(Place::Indexed { register, index });
            }
            return self.qubit(scope);
        }
        let address = self.operand(scope, Width::WORD)?;
        let mut bit = 0;
        if self.cursor.eat(Token::Colon) {
            let (text, offset) = self.cursor.number("a bit index")?;
            bit = text.parse().map_err(|_| Failure { offset, kind: ReadErrorKind::BadNumber(text.to_owned()) })?;
        }
        self.cursor.expect(Token::RBracket, "`]`")?;
        Ok(Place::Memory { address, bit })
    }

    /// `(ANGLE)` in radians: a finite number. A literal that overflows to infinity (`1e999`) is refused here,
    /// because the writer has no spelling for an infinite angle and `read(write(m)) == m` must hold.
    fn angle(&mut self) -> Result<f64, Failure> {
        self.cursor.expect(Token::LParen, "`(` and an angle")?;
        let (text, offset) = self.cursor.number("an angle in radians")?;
        let angle = text
            .parse::<f64>()
            .ok()
            .filter(|angle| angle.is_finite())
            .ok_or_else(|| Failure { offset, kind: ReadErrorKind::BadNumber(text.to_owned()) })?;
        self.cursor.expect(Token::RParen, "`)`")?;
        Ok(angle)
    }

    /// `apply @f(ARGS) -> %out, %fault budget B`
    fn apply(&mut self, scope: &mut Scope<'s>) -> Result<Inst, Failure> {
        let callee = self.function_use()?;
        let args = self.arguments(scope)?;
        self.cursor.expect(Token::Arrow, "`->`")?;
        let out = self.slot_use(scope, "an output register")?;
        self.cursor.expect(Token::Comma, "`,`")?;
        let fault = self.slot_use(scope, "a fault register")?;
        self.cursor.expect(Token::Word("budget"), "`budget`")?;
        let budget = self.operand(scope, Width::WORD)?;
        Ok(Inst::Apply { callee, args, out, fault, budget })
    }

    /// `switch iN value, CASE: target, ..., default: target`
    fn switch(&mut self, scope: &mut Scope<'s>) -> Result<Terminator, Failure> {
        let width = self.int_type("an integer type")?;
        let value = self.operand(scope, width)?;
        let mut cases = Vec::new();
        loop {
            self.cursor.expect(Token::Comma, "`,` and a case")?;
            if self.cursor.eat(Token::Word("default")) {
                self.cursor.expect(Token::Colon, "`:`")?;
                return Ok(Terminator::Switch { width, value, cases, default: self.target(scope)? });
            }
            let case = self.literal(width)?.value();
            self.cursor.expect(Token::Colon, "`:`")?;
            cases.push(SwitchCase { value: case, target: self.target(scope)? });
        }
    }

    /// `ret`, `ret %value` or `ret INTEGER` (at the function's result width).
    fn ret(&mut self, scope: &mut Scope<'s>) -> Result<Terminator, Failure> {
        match (self.cursor.peek(), scope.result) {
            (None | Some(Token::Newline), _) => Ok(Terminator::Return(None)),
            (Some(Token::Number(_)), Some(Type::Int(width))) => {
                Ok(Terminator::Return(Some(self.operand(scope, width)?)))
            }
            (Some(Token::Local(_)), _) => Ok(Terminator::Return(Some(Operand::Slot(self.slot_use(scope, "a value")?)))),
            _ => Err(self.cursor.unexpected("a `%value`, or an integer when the function returns one")),
        }
    }

    /// `block` or `block(ARGS)`
    fn target(&mut self, scope: &mut Scope<'s>) -> Result<Target, Failure> {
        let offset = self.cursor.offset();
        let name = self.cursor.word("a block label")?;
        let block = BlockId(scope.blocks.mention(name, offset)?);
        let args = if self.cursor.peek() == Some(Token::LParen) { self.arguments(scope)? } else { Vec::new() };
        Ok(Target { block, args })
    }

    /// `(ARG, ...)`, each a `%value` or a typed constant `iN INTEGER`.
    fn arguments(&mut self, scope: &mut Scope<'s>) -> Result<Vec<Operand>, Failure> {
        self.cursor.expect(Token::LParen, "`(`")?;
        let mut args = Vec::new();
        if self.cursor.eat(Token::RParen) {
            return Ok(args);
        }
        loop {
            args.push(self.typed_operand(scope)?);
            if self.cursor.eat(Token::RParen) {
                return Ok(args);
            }
            self.cursor.expect(Token::Comma, "`,` or `)`")?;
        }
    }

    /// `%value` or `iN INTEGER`.
    fn typed_operand(&mut self, scope: &mut Scope<'s>) -> Result<Operand, Failure> {
        if let Some(Token::Local(_)) = self.cursor.peek() {
            return Ok(Operand::Slot(self.slot_use(scope, "a value")?));
        }
        let width = self.int_type("a `%value` or a typed constant such as `i32 7`")?;
        Ok(Operand::Const(self.literal(width)?))
    }

    /// `%value` or an `INTEGER` at `width`.
    fn operand(&mut self, scope: &mut Scope<'s>, width: Width) -> Result<Operand, Failure> {
        match self.cursor.peek() {
            Some(Token::Local(_)) => Ok(Operand::Slot(self.slot_use(scope, "a value")?)),
            Some(Token::Number(_)) => Ok(Operand::Const(self.literal(width)?)),
            _ => Err(self.cursor.unexpected("a `%value` or an integer")),
        }
    }

    /// `a, b`, both at `width`.
    fn operand_pair(&mut self, scope: &mut Scope<'s>, width: Width) -> Result<(Operand, Operand), Failure> {
        let lhs = self.operand(scope, width)?;
        self.cursor.expect(Token::Comma, "`,`")?;
        Ok((lhs, self.operand(scope, width)?))
    }

    /// `%register` or `%register[BIT]`; the bare form names qubit 0. A position below 2^32 is a register
    /// qubit; one from 2^32 to 2^64 - 1, past every register's width but possible on an `i64` handle whose
    /// width the validator cannot see, is a [`Place::Indexed`] by that constant (the fault `bad_range` when
    /// the gate runs).
    fn qubit(&mut self, scope: &mut Scope<'s>) -> Result<Place, Failure> {
        let register = self.slot_use(scope, "a qubit")?;
        if !self.cursor.eat(Token::LBracket) {
            return Ok(Place::Register(Qubit { register, bit: 0 }));
        }
        let (text, offset) = self.cursor.number("a qubit index")?;
        let position: u64 =
            text.parse().map_err(|_| Failure { offset, kind: ReadErrorKind::BadNumber(text.to_owned()) })?;
        self.cursor.expect(Token::RBracket, "`]`")?;
        Ok(match u32::try_from(position) {
            Ok(bit) => Place::Register(Qubit { register, bit }),
            Err(_) => Place::Indexed {
                register,
                index: Operand::Const(Const::new(Width::WORD, u128::from(position)).expect("a 64-bit position")),
            },
        })
    }

    /// `N` of `qalloc N`: from 1 to [`Qubits::MAX`].
    fn register_width(&mut self) -> Result<Qubits, Failure> {
        let (text, offset) = self.cursor.number("a number of qubits")?;
        let qubits = text.parse().ok().and_then(Qubits::new);
        qubits.ok_or_else(|| Failure { offset, kind: ReadErrorKind::BadNumber(text.to_owned()) })
    }

    fn slot_use(&mut self, scope: &mut Scope<'s>, expected: &'static str) -> Result<Slot, Failure> {
        let (name, offset) = self.cursor.local(expected)?;
        Ok(Slot(scope.slots.mention(name, offset)?))
    }

    fn function_use(&mut self) -> Result<FuncId, Failure> {
        let (name, offset) = self.cursor.global("a function")?;
        Ok(FuncId(self.functions.mention(name, offset)?))
    }
}

/// An error at the word `found`, read at `offset`.
fn unexpected_word(offset: usize, expected: &'static str, found: &str) -> Failure {
    Failure { offset, kind: ReadErrorKind::Unexpected { expected, found: format!("`{found}`") } }
}

/// The start of a statement: its optional `%name =` and its offset.
struct Line {
    dst: Option<Slot>,
    offset: usize,
}

impl Line {
    fn result(&self, mnemonic: &'static str) -> Result<Slot, Failure> {
        self.dst.ok_or(Failure { offset: self.offset, kind: ReadErrorKind::ResultRequired(mnemonic) })
    }

    fn no_result(&self, mnemonic: &'static str) -> Result<(), Failure> {
        match self.dst {
            None => Ok(()),
            Some(_) => Err(Failure { offset: self.offset, kind: ReadErrorKind::ResultNotAllowed(mnemonic) }),
        }
    }
}
