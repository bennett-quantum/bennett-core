//! Tokens of the text form.

use std::ops::Range;

use logos::Logos;

/// A token. Spaces and `;` comments are skipped; newlines end statements.
#[derive(Logos, Clone, Copy, Debug, PartialEq)]
#[logos(skip r"[ \t\r]+")]
#[logos(skip r";[^\n]*")]
pub(super) enum Token<'s> {
    #[token("\n")]
    Newline,
    /// `%name`: a value; the payload omits the sigil.
    #[regex(r"%[A-Za-z0-9_.]+", |lex| &lex.slice()[1..])]
    Local(&'s str),
    /// `@name`: a function; the payload omits the sigil.
    #[regex(r"@[A-Za-z0-9_.]+", |lex| &lex.slice()[1..])]
    Global(&'s str),
    /// A keyword, mnemonic, type or block label.
    #[regex(r"[A-Za-z_][A-Za-z0-9_.]*", |lex| lex.slice())]
    Word(&'s str),
    /// An integer (decimal or `0x` hex) or a decimal floating-point number, optionally negative.
    #[regex(r"-?(0x[0-9a-fA-F]+|[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?)", |lex| lex.slice())]
    Number(&'s str),
    /// A string literal; the payload is the text between the quotes, escapes not yet decoded.
    #[regex(r#""([^"\\\n]|\\[^\n])*""#, |lex| { let s = lex.slice(); &s[1..s.len() - 1] })]
    Str(&'s str),
    #[token("=")]
    Equals,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("->")]
    Arrow,
}

impl Token<'_> {
    /// How the token reads in an error message.
    pub(super) fn describe(&self) -> String {
        match self {
            Token::Newline => "end of line".to_owned(),
            Token::Local(name) => format!("`%{name}`"),
            Token::Global(name) => format!("`@{name}`"),
            Token::Word(word) | Token::Number(word) => format!("`{word}`"),
            Token::Str(text) => format!("`\"{text}\"`"),
            Token::Equals => "`=`".to_owned(),
            Token::Comma => "`,`".to_owned(),
            Token::Colon => "`:`".to_owned(),
            Token::LParen => "`(`".to_owned(),
            Token::RParen => "`)`".to_owned(),
            Token::LBracket => "`[`".to_owned(),
            Token::RBracket => "`]`".to_owned(),
            Token::LBrace => "`{`".to_owned(),
            Token::RBrace => "`}`".to_owned(),
            Token::Arrow => "`->`".to_owned(),
        }
    }
}

/// Splits `text` into tokens with their byte ranges, or returns the offset of the first character that
/// starts no token.
pub(super) fn tokenize(text: &str) -> Result<Vec<(Token<'_>, Range<usize>)>, usize> {
    let mut lexer = Token::lexer(text);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next() {
        let span = lexer.span();
        match token {
            Ok(token) => tokens.push((token, span)),
            Err(()) => return Err(span.start),
        }
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<Token<'_>> {
        tokenize(text).map(|t| t.into_iter().map(|(token, _)| token).collect()).unwrap_or_default()
    }

    #[test]
    fn splits_a_statement() {
        assert_eq!(
            kinds("%x = add i32 %y, -1 ; note\n"),
            [
                Token::Local("x"),
                Token::Equals,
                Token::Word("add"),
                Token::Word("i32"),
                Token::Local("y"),
                Token::Comma,
                Token::Number("-1"),
                Token::Newline
            ]
        );
    }

    #[test]
    fn numbers_and_strings() {
        assert_eq!(kinds("0x1F 1.5e-3 -2"), [Token::Number("0x1F"), Token::Number("1.5e-3"), Token::Number("-2")]);
        assert_eq!(kinds(r#""a\"b""#), [Token::Str(r#"a\"b"#)]);
        assert_eq!(kinds("->"), [Token::Arrow]);
    }

    #[test]
    fn reports_the_offset_of_a_stray_character() {
        assert_eq!(tokenize("ret $").err(), Some(4));
    }
}
