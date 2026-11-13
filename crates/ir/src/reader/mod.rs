//! The text reader: one pass over the tokens, every name resolved to a dense index as it is met.
//!
//! Names may be used before they are defined. Each gets an index on first mention, and the reader
//! reports any name still undefined at the end of its function or module. Functions and blocks are then
//! renumbered in the order they are defined, so that the writer reproduces the text's order and
//! `read(&write(&m)) == m` for every module read from text. The grammar is in `docs/SYNTAX.md`.

mod cursor;
mod error;
mod lexer;
mod literal;
mod names;
mod statement;

pub use error::{NameKind, ReadError, ReadErrorKind};

use crate::{Block, BlockId, Const, DataItem, DataSegment, FuncId, Function, Inst, Module, Param, Slot, Type, Width};
use cursor::Cursor;
use error::Failure;
use lexer::Token;
use names::Names;
use statement::Statement;

/// Reads a module from its text form. The result is not yet validated.
pub fn read(text: &str) -> Result<Module, ReadError> {
    let tokens =
        lexer::tokenize(text).map_err(|offset| Failure { offset, kind: ReadErrorKind::BadCharacter }.locate(text))?;
    let parser = Parser {
        cursor: Cursor::new(tokens, text.len()),
        functions: Names::new(NameKind::Function),
        defined: Vec::new(),
        memory_size: None,
        data: Vec::new(),
    };
    parser.module().map_err(|failure| failure.locate(text))
}

struct Parser<'s> {
    cursor: Cursor<'s>,
    functions: Names<'s>,
    /// The functions in the order they are defined; their calls still use mention indices.
    defined: Vec<Function>,
    memory_size: Option<u64>,
    data: Vec<DataSegment>,
}

/// The names local to the function being read.
struct Scope<'s> {
    slots: Names<'s>,
    blocks: Names<'s>,
    /// The function's result type, which gives `ret` constants their width.
    result: Option<Type>,
}

/// A block whose terminator has not been read yet.
struct OpenBlock<'s> {
    name: &'s str,
    params: Vec<Param>,
    insts: Vec<Inst>,
}

impl<'s> Parser<'s> {
    fn module(mut self) -> Result<Module, Failure> {
        loop {
            self.cursor.skip_blank_lines();
            match self.cursor.peek() {
                None => break,
                Some(Token::Word("memory")) => self.memory()?,
                Some(Token::Word("data")) => self.data_segment()?,
                Some(Token::Word("func")) => self.function()?,
                Some(_) => return Err(self.cursor.unexpected("`memory`, `data` or `func`")),
            }
        }
        let order = self.functions.definition_order()?;
        let mut module =
            Module { memory_size: self.memory_size.unwrap_or(0), data: self.data, functions: self.defined };
        renumber_functions(&mut module, &order);
        Ok(module)
    }

    /// `memory SIZE`
    fn memory(&mut self) -> Result<(), Failure> {
        let offset = self.cursor.offset();
        self.cursor.advance();
        let size = self.unsigned("a memory size in bytes")?;
        if self.memory_size.replace(size).is_some() {
            return Err(Failure { offset, kind: ReadErrorKind::DuplicateMemory });
        }
        self.cursor.end_of_line()
    }

    /// `data ADDRESS = ITEM, ITEM, ...`
    fn data_segment(&mut self) -> Result<(), Failure> {
        self.cursor.advance();
        let address = self.unsigned("an address")?;
        self.cursor.expect(Token::Equals, "`=`")?;
        let mut items = vec![self.data_item()?];
        while self.cursor.eat(Token::Comma) {
            items.push(self.data_item()?);
        }
        self.data.push(DataSegment { address, items });
        self.cursor.end_of_line()
    }

    /// `"string"`, `iN INTEGER` or `iN @function`.
    fn data_item(&mut self) -> Result<DataItem, Failure> {
        let offset = self.cursor.offset();
        if let Some(Token::Str(text)) = self.cursor.peek() {
            self.cursor.advance();
            let bytes = literal::decode_string(text).ok_or(Failure { offset, kind: ReadErrorKind::BadEscape })?;
            return Ok(DataItem::Bytes(bytes));
        }
        let width = self.int_type("a string or an integer type")?;
        if let Some(Token::Global(name)) = self.cursor.peek() {
            let offset = self.cursor.offset();
            self.cursor.advance();
            return Ok(DataItem::FuncAddr { width, func: FuncId(self.functions.mention(name, offset)?) });
        }
        Ok(DataItem::Int(self.literal(width)?))
    }

    /// `func @name(TYPE %param, ...) [-> TYPE] {`, the body, and `}`.
    fn function(&mut self) -> Result<(), Failure> {
        self.cursor.advance();
        let (name, offset) = self.cursor.global("a function name")?;
        self.functions.define(name, offset)?;
        let mut scope = Scope { slots: Names::new(NameKind::Value), blocks: Names::new(NameKind::Block), result: None };
        let params = self.params(&mut scope)?;
        if self.cursor.eat(Token::Arrow) {
            scope.result = Some(self.any_type("a result type")?);
        }
        self.cursor.expect(Token::LBrace, "`{`")?;
        self.cursor.end_of_line()?;
        let blocks = self.body(&mut scope)?;
        self.cursor.end_of_line()?;
        scope.slots.definition_order()?;
        self.defined.push(Function {
            name: name.to_owned(),
            params,
            result: scope.result,
            blocks,
            slot_names: scope.slots.into_names(),
        });
        Ok(())
    }

    /// `(TYPE %name, ...)`, each name defined as a slot.
    fn params(&mut self, scope: &mut Scope<'s>) -> Result<Vec<Param>, Failure> {
        self.cursor.expect(Token::LParen, "`(`")?;
        let mut params = Vec::new();
        if self.cursor.eat(Token::RParen) {
            return Ok(params);
        }
        loop {
            let ty = self.any_type("a parameter type")?;
            let (name, offset) = self.cursor.local("a parameter name")?;
            params.push(Param { slot: Slot(scope.slots.define(name, offset)?), ty });
            if self.cursor.eat(Token::RParen) {
                return Ok(params);
            }
            self.cursor.expect(Token::Comma, "`,` or `)`")?;
        }
    }

    /// Labels and statements up to and including the closing `}`. Blocks are returned in the order of
    /// their labels, and branch targets renumbered to match.
    fn body(&mut self, scope: &mut Scope<'s>) -> Result<Vec<Block>, Failure> {
        let mut blocks = Vec::new();
        let mut open: Option<OpenBlock<'s>> = None;
        loop {
            self.cursor.skip_blank_lines();
            match self.cursor.peek() {
                Some(Token::RBrace) => break,
                None => return Err(self.cursor.unexpected("`}`")),
                Some(_) if self.cursor.line_ends_with_colon() => {
                    if let Some(block) = &open {
                        let kind = ReadErrorKind::MissingTerminator(block.name.to_owned());
                        return Err(Failure { offset: self.cursor.offset(), kind });
                    }
                    open = Some(self.label(scope)?);
                }
                Some(_) => {
                    let Some(block) = open.as_mut() else {
                        return Err(Failure { offset: self.cursor.offset(), kind: ReadErrorKind::MissingLabel });
                    };
                    match self.statement(scope)? {
                        Statement::Inst(inst) => block.insts.push(inst),
                        Statement::Term(term) => {
                            if let Some(block) = open.take() {
                                blocks.push(Block {
                                    name: block.name.to_owned(),
                                    params: block.params,
                                    insts: block.insts,
                                    term,
                                });
                            }
                        }
                    }
                }
            }
        }
        if let Some(block) = open {
            let kind = ReadErrorKind::MissingTerminator(block.name.to_owned());
            return Err(Failure { offset: self.cursor.offset(), kind });
        }
        self.cursor.advance();
        let order = scope.blocks.definition_order()?;
        for block in &mut blocks {
            block.term.visit_targets_mut(|target| target.block = BlockId(order[target.block.index()]));
        }
        Ok(blocks)
    }

    /// `name:` or `name(TYPE %param, ...):`
    fn label(&mut self, scope: &mut Scope<'s>) -> Result<OpenBlock<'s>, Failure> {
        let offset = self.cursor.offset();
        let name = self.cursor.word("a block label")?;
        scope.blocks.define(name, offset)?;
        let params = if self.cursor.peek() == Some(Token::LParen) { self.params(scope)? } else { Vec::new() };
        self.cursor.expect(Token::Colon, "`:`")?;
        self.cursor.end_of_line()?;
        Ok(OpenBlock { name, params, insts: Vec::new() })
    }

    fn unsigned(&mut self, expected: &'static str) -> Result<u64, Failure> {
        let (text, offset) = self.cursor.number(expected)?;
        literal::parse_integer(text)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(Failure { offset, kind: ReadErrorKind::BadNumber(text.to_owned()) })
    }

    fn any_type(&mut self, expected: &'static str) -> Result<Type, Failure> {
        let offset = self.cursor.offset();
        let word = self.cursor.word(expected)?;
        literal::parse_type(word).ok_or(Failure { offset, kind: ReadErrorKind::BadType(word.to_owned()) })
    }

    fn int_type(&mut self, expected: &'static str) -> Result<Width, Failure> {
        let offset = self.cursor.offset();
        match self.any_type(expected)? {
            Type::Int(width) => Ok(width),
            ty @ Type::Qreg(_) => Err(Failure { offset, kind: ReadErrorKind::BadType(ty.to_string()) }),
        }
    }

    /// An integer literal at `width`.
    fn literal(&mut self, width: Width) -> Result<Const, Failure> {
        let (text, offset) = self.cursor.number("an integer")?;
        let value = literal::parse_integer(text)
            .ok_or_else(|| Failure { offset, kind: ReadErrorKind::BadNumber(text.to_owned()) })?;
        Const::from_literal(width, value).ok_or_else(|| Failure {
            offset,
            kind: ReadErrorKind::OutOfRange { literal: text.to_owned(), ty: Type::Int(width) },
        })
    }
}

/// Replaces every function index, a mention index while reading, by the function's definition index.
fn renumber_functions(module: &mut Module, order: &[u32]) {
    let renumber = |func: &mut FuncId| *func = FuncId(order[func.index()]);
    for item in module.data.iter_mut().flat_map(|segment| &mut segment.items) {
        if let DataItem::FuncAddr { func, .. } = item {
            renumber(func);
        }
    }
    let insts = module.functions.iter_mut().flat_map(|f| &mut f.blocks).flat_map(|block| &mut block.insts);
    for inst in insts {
        match inst {
            Inst::Call { callee: func, .. } | Inst::Apply { callee: func, .. } | Inst::FuncAddr { func, .. } => {
                renumber(func);
            }
            _ => {}
        }
    }
}
