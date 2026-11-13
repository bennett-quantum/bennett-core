//! A cursor over the token list, with the primitive expectations of the grammar.

use std::ops::Range;

use super::error::{Failure, ReadErrorKind};
use super::lexer::Token;

pub(super) struct Cursor<'s> {
    tokens: Vec<(Token<'s>, Range<usize>)>,
    pos: usize,
    end: usize,
}

impl<'s> Cursor<'s> {
    pub(super) fn new(tokens: Vec<(Token<'s>, Range<usize>)>, end: usize) -> Self {
        Cursor { tokens, pos: 0, end }
    }

    pub(super) fn peek(&self) -> Option<Token<'s>> {
        self.peek_nth(0)
    }

    pub(super) fn peek_nth(&self, n: usize) -> Option<Token<'s>> {
        self.tokens.get(self.pos + n).map(|(token, _)| *token)
    }

    /// The byte offset of the next token, or of the end of the text.
    pub(super) fn offset(&self) -> usize {
        self.tokens.get(self.pos).map_or(self.end, |(_, span)| span.start)
    }

    pub(super) fn advance(&mut self) -> Option<Token<'s>> {
        let token = self.peek();
        self.pos += usize::from(token.is_some());
        token
    }

    /// Consumes the next token if it is `token`.
    pub(super) fn eat(&mut self, token: Token<'_>) -> bool {
        let matches = self.peek() == Some(token);
        self.pos += usize::from(matches);
        matches
    }

    pub(super) fn expect(&mut self, token: Token<'_>, expected: &'static str) -> Result<(), Failure> {
        if self.eat(token) { Ok(()) } else { Err(self.unexpected(expected)) }
    }

    /// An error at the next token, saying what was expected instead.
    pub(super) fn unexpected(&self, expected: &'static str) -> Failure {
        let found = self.peek().map_or_else(|| "end of input".to_owned(), |token| token.describe());
        Failure { offset: self.offset(), kind: ReadErrorKind::Unexpected { expected, found } }
    }

    pub(super) fn word(&mut self, expected: &'static str) -> Result<&'s str, Failure> {
        match self.peek() {
            Some(Token::Word(word)) => {
                self.pos += 1;
                Ok(word)
            }
            _ => Err(self.unexpected(expected)),
        }
    }

    /// A `%name`, with its offset.
    pub(super) fn local(&mut self, expected: &'static str) -> Result<(&'s str, usize), Failure> {
        let offset = self.offset();
        match self.peek() {
            Some(Token::Local(name)) => {
                self.pos += 1;
                Ok((name, offset))
            }
            _ => Err(self.unexpected(expected)),
        }
    }

    /// A `@name`, with its offset.
    pub(super) fn global(&mut self, expected: &'static str) -> Result<(&'s str, usize), Failure> {
        let offset = self.offset();
        match self.peek() {
            Some(Token::Global(name)) => {
                self.pos += 1;
                Ok((name, offset))
            }
            _ => Err(self.unexpected(expected)),
        }
    }

    /// A number token's text, with its offset.
    pub(super) fn number(&mut self, expected: &'static str) -> Result<(&'s str, usize), Failure> {
        let offset = self.offset();
        match self.peek() {
            Some(Token::Number(text)) => {
                self.pos += 1;
                Ok((text, offset))
            }
            _ => Err(self.unexpected(expected)),
        }
    }

    /// The end of a statement: a newline or the end of the text.
    pub(super) fn end_of_line(&mut self) -> Result<(), Failure> {
        if self.peek().is_none() || self.eat(Token::Newline) { Ok(()) } else { Err(self.unexpected("end of line")) }
    }

    pub(super) fn skip_blank_lines(&mut self) {
        while self.eat(Token::Newline) {}
    }

    /// Whether the current line's last token is a colon, which makes the line a block label.
    pub(super) fn line_ends_with_colon(&self) -> bool {
        let line = self.tokens[self.pos..].iter().map(|(token, _)| *token).take_while(|t| *t != Token::Newline);
        line.last() == Some(Token::Colon)
    }
}
