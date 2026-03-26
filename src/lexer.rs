//! Lexer / tokenizer for the v6c C compiler.
//!
//! Converts a source string into a sequence of [`Token`]s.  Each token
//! carries its [`TokenKind`], the source text that produced it, and the
//! line/column where it starts.
//!
//! The lexer handles:
//! - All C89 keywords plus the `__stack` / `__global` compiler extensions
//! - Integer constants (decimal, hex `0x`, octal `0`) with optional `U`/`L`
//!   suffixes
//! - Character and string literals with full escape-sequence support
//! - Single-line (`//`) and multi-line (`/* */`) comments
//! - Preprocessor directives (`#include`, `#define`, …) – recognised as
//!   single tokens for later processing by the preprocessor module

use std::fmt;

// ---------------------------------------------------------------------------
// Token kind
// ---------------------------------------------------------------------------

/// Every distinct class of token the lexer can produce.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TokenKind {
    // -- Keywords --------------------------------------------------------
    Auto,
    Break,
    Case,
    Char,
    Const,
    Continue,
    Default,
    Do,
    Double,
    Else,
    Enum,
    Extern,
    Float,
    For,
    Goto,
    If,
    Int,
    Long,
    Register,
    Return,
    Short,
    Signed,
    Sizeof,
    Static,
    Struct,
    Switch,
    Typedef,
    Union,
    Unsigned,
    Void,
    Volatile,
    While,

    // Compiler extensions
    /// `__stack` – mark a function as using a stack frame.
    Stack,
    /// `__global` – mark a variable/function as globally allocated.
    Global,

    // -- Literals --------------------------------------------------------
    /// Identifier (variable / function / type name).
    Ident,
    /// Integer constant (decimal, hex, or octal) with optional suffix.
    IntLiteral,
    /// Character literal, e.g. `'a'` or `'\n'`.
    CharLiteral,
    /// String literal, e.g. `"hello\n"`.
    StringLiteral,

    // -- Operators -------------------------------------------------------
    Plus,         // +
    Minus,        // -
    Star,         // *
    Slash,        // /
    Percent,      // %
    Amp,          // &
    Pipe,         // |
    Caret,        // ^
    Tilde,        // ~
    Bang,         // !
    Assign,       // =
    Lt,           // <
    Gt,           // >
    Dot,          // .

    // Two-character operators
    PlusPlus,     // ++
    MinusMinus,   // --
    Arrow,        // ->
    LtLt,        // <<
    GtGt,        // >>
    EqEq,        // ==
    BangEq,      // !=
    LtEq,        // <=
    GtEq,        // >=
    AmpAmp,      // &&
    PipePipe,    // ||

    // Compound assignment
    PlusEq,      // +=
    MinusEq,     // -=
    StarEq,      // *=
    SlashEq,     // /=
    PercentEq,   // %=
    AmpEq,       // &=
    PipeEq,      // |=
    CaretEq,     // ^=
    LtLtEq,     // <<=
    GtGtEq,     // >>=

    // -- Punctuation -----------------------------------------------------
    LParen,      // (
    RParen,      // )
    LBrace,      // {
    RBrace,      // }
    LBracket,    // [
    RBracket,    // ]
    Semicolon,   // ;
    Comma,       // ,
    Colon,       // :
    Question,    // ?
    Ellipsis,    // ...
    Hash,        // # (used for stringise in macros)

    // -- Preprocessor ----------------------------------------------------
    /// A complete preprocessor line: `#include <stdio.h>`, `#define X 1`, …
    /// The value field contains the full directive text *after* the `#`.
    PreprocDirective,

    // -- Sentinel --------------------------------------------------------
    /// Marks the end of input.
    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Auto => "auto",
            Self::Break => "break",
            Self::Case => "case",
            Self::Char => "char",
            Self::Const => "const",
            Self::Continue => "continue",
            Self::Default => "default",
            Self::Do => "do",
            Self::Double => "double",
            Self::Else => "else",
            Self::Enum => "enum",
            Self::Extern => "extern",
            Self::Float => "float",
            Self::For => "for",
            Self::Goto => "goto",
            Self::If => "if",
            Self::Int => "int",
            Self::Long => "long",
            Self::Register => "register",
            Self::Return => "return",
            Self::Short => "short",
            Self::Signed => "signed",
            Self::Sizeof => "sizeof",
            Self::Static => "static",
            Self::Struct => "struct",
            Self::Switch => "switch",
            Self::Typedef => "typedef",
            Self::Union => "union",
            Self::Unsigned => "unsigned",
            Self::Void => "void",
            Self::Volatile => "volatile",
            Self::While => "while",
            Self::Stack => "__stack",
            Self::Global => "__global",
            Self::Ident => "identifier",
            Self::IntLiteral => "integer literal",
            Self::CharLiteral => "character literal",
            Self::StringLiteral => "string literal",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::Slash => "/",
            Self::Percent => "%",
            Self::Amp => "&",
            Self::Pipe => "|",
            Self::Caret => "^",
            Self::Tilde => "~",
            Self::Bang => "!",
            Self::Assign => "=",
            Self::Lt => "<",
            Self::Gt => ">",
            Self::Dot => ".",
            Self::PlusPlus => "++",
            Self::MinusMinus => "--",
            Self::Arrow => "->",
            Self::LtLt => "<<",
            Self::GtGt => ">>",
            Self::EqEq => "==",
            Self::BangEq => "!=",
            Self::LtEq => "<=",
            Self::GtEq => ">=",
            Self::AmpAmp => "&&",
            Self::PipePipe => "||",
            Self::PlusEq => "+=",
            Self::MinusEq => "-=",
            Self::StarEq => "*=",
            Self::SlashEq => "/=",
            Self::PercentEq => "%=",
            Self::AmpEq => "&=",
            Self::PipeEq => "|=",
            Self::CaretEq => "^=",
            Self::LtLtEq => "<<=",
            Self::GtGtEq => ">>=",
            Self::LParen => "(",
            Self::RParen => ")",
            Self::LBrace => "{",
            Self::RBrace => "}",
            Self::LBracket => "[",
            Self::RBracket => "]",
            Self::Semicolon => ";",
            Self::Comma => ",",
            Self::Colon => ":",
            Self::Question => "?",
            Self::Ellipsis => "...",
            Self::Hash => "#",
            Self::PreprocDirective => "preprocessor directive",
            Self::Eof => "end of file",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

/// A single token produced by the lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// What kind of token this is.
    pub kind: TokenKind,
    /// The source text that produced this token (or the processed value for
    /// string/char literals).
    pub value: String,
    /// 1-based line number where the token starts.
    pub line: u32,
    /// 1-based column (byte offset within the line) where the token starts.
    pub column: u32,
}

impl Token {
    pub fn new(kind: TokenKind, value: impl Into<String>, line: u32, column: u32) -> Self {
        Self {
            kind,
            value: value.into(),
            line,
            column,
        }
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {} `{}`", self.line, self.column, self.kind, self.value)
    }
}

// ---------------------------------------------------------------------------
// Lexer error
// ---------------------------------------------------------------------------

/// An error encountered while tokenizing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub message: String,
    pub line: u32,
    pub column: u32,
}

impl LexError {
    fn new(message: impl Into<String>, line: u32, column: u32) -> Self {
        Self {
            message: message.into(),
            line,
            column,
        }
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: error: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for LexError {}

// ---------------------------------------------------------------------------
// Keyword lookup
// ---------------------------------------------------------------------------

fn keyword_kind(word: &str) -> Option<TokenKind> {
    match word {
        "auto" => Some(TokenKind::Auto),
        "break" => Some(TokenKind::Break),
        "case" => Some(TokenKind::Case),
        "char" => Some(TokenKind::Char),
        "const" => Some(TokenKind::Const),
        "continue" => Some(TokenKind::Continue),
        "default" => Some(TokenKind::Default),
        "do" => Some(TokenKind::Do),
        "double" => Some(TokenKind::Double),
        "else" => Some(TokenKind::Else),
        "enum" => Some(TokenKind::Enum),
        "extern" => Some(TokenKind::Extern),
        "float" => Some(TokenKind::Float),
        "for" => Some(TokenKind::For),
        "goto" => Some(TokenKind::Goto),
        "if" => Some(TokenKind::If),
        "int" => Some(TokenKind::Int),
        "long" => Some(TokenKind::Long),
        "register" => Some(TokenKind::Register),
        "return" => Some(TokenKind::Return),
        "short" => Some(TokenKind::Short),
        "signed" => Some(TokenKind::Signed),
        "sizeof" => Some(TokenKind::Sizeof),
        "static" => Some(TokenKind::Static),
        "struct" => Some(TokenKind::Struct),
        "switch" => Some(TokenKind::Switch),
        "typedef" => Some(TokenKind::Typedef),
        "union" => Some(TokenKind::Union),
        "unsigned" => Some(TokenKind::Unsigned),
        "void" => Some(TokenKind::Void),
        "volatile" => Some(TokenKind::Volatile),
        "while" => Some(TokenKind::While),
        "__stack" => Some(TokenKind::Stack),
        "__global" => Some(TokenKind::Global),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

/// Tokenizes a source string into a sequence of [`Token`]s.
pub struct Lexer<'src> {
    /// Source text as bytes for efficient random access.
    src: &'src [u8],
    /// Current byte offset into `src`.
    pos: usize,
    /// Current 1-based line number.
    line: u32,
    /// Current 1-based column number (byte offset within the line).
    column: u32,
    /// Whether we are at the logical start of a line (for `#` detection).
    at_line_start: bool,
}

impl<'src> Lexer<'src> {
    /// Create a new lexer over the given source text.
    pub fn new(source: &'src str) -> Self {
        Self {
            src: source.as_bytes(),
            pos: 0,
            line: 1,
            column: 1,
            at_line_start: true,
        }
    }

    // -- Helpers ---------------------------------------------------------

    /// Peek at the current byte without advancing.
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    /// Peek at the byte `offset` positions ahead of `pos`.
    fn peek_ahead(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    /// Advance one byte and return it.
    fn advance(&mut self) -> Option<u8> {
        let ch = self.src.get(self.pos).copied()?;
        self.pos += 1;
        if ch == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    /// True when all input has been consumed.
    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// Return an error anchored at the given position.
    fn error(&self, msg: impl Into<String>, line: u32, col: u32) -> LexError {
        LexError::new(msg, line, col)
    }

    // -- Whitespace & comments ------------------------------------------

    /// Skip whitespace and comments, returning `true` if any newlines were
    /// crossed (used for `at_line_start` tracking).
    fn skip_whitespace_and_comments(&mut self) -> Result<(), LexError> {
        loop {
            // Whitespace
            match self.peek() {
                Some(b' ' | b'\t' | b'\r') => {
                    self.advance();
                    continue;
                }
                Some(b'\n') => {
                    self.advance();
                    self.at_line_start = true;
                    continue;
                }
                _ => {}
            }

            // Line comment
            if self.peek() == Some(b'/') && self.peek_ahead(1) == Some(b'/') {
                self.advance(); // /
                self.advance(); // /
                while let Some(ch) = self.peek() {
                    if ch == b'\n' {
                        break;
                    }
                    self.advance();
                }
                continue;
            }

            // Block comment
            if self.peek() == Some(b'/') && self.peek_ahead(1) == Some(b'*') {
                let start_line = self.line;
                let start_col = self.column;
                self.advance(); // /
                self.advance(); // *
                loop {
                    match self.advance() {
                        Some(b'*') if self.peek() == Some(b'/') => {
                            self.advance(); // /
                            break;
                        }
                        Some(_) => {}
                        None => {
                            return Err(self.error(
                                "unterminated block comment",
                                start_line,
                                start_col,
                            ));
                        }
                    }
                }
                continue;
            }

            break;
        }
        Ok(())
    }

    // -- Number literals ------------------------------------------------

    fn lex_number(&mut self, start_line: u32, start_col: u32) -> Result<Token, LexError> {
        let start = self.pos;

        // Check for hex or octal prefix.
        if self.peek() == Some(b'0') {
            match self.peek_ahead(1) {
                Some(b'x' | b'X') => return self.lex_hex(start_line, start_col),
                Some(b'0'..=b'9') => return self.lex_octal(start_line, start_col),
                _ => {
                    // Could be just `0` or `0` followed by suffix.
                }
            }
        }

        // Decimal digits.
        while let Some(b'0'..=b'9') = self.peek() {
            self.advance();
        }

        // Optional suffix.
        self.eat_int_suffix();

        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        Ok(Token::new(TokenKind::IntLiteral, text, start_line, start_col))
    }

    fn lex_hex(&mut self, start_line: u32, start_col: u32) -> Result<Token, LexError> {
        let start = self.pos;
        self.advance(); // 0
        self.advance(); // x/X

        if !matches!(self.peek(), Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F')) {
            return Err(self.error(
                "expected hex digit after '0x'",
                start_line,
                start_col,
            ));
        }

        while matches!(self.peek(), Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F')) {
            self.advance();
        }

        self.eat_int_suffix();

        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        Ok(Token::new(TokenKind::IntLiteral, text, start_line, start_col))
    }

    fn lex_octal(&mut self, start_line: u32, start_col: u32) -> Result<Token, LexError> {
        let start = self.pos;
        self.advance(); // leading 0

        while matches!(self.peek(), Some(b'0'..=b'7')) {
            self.advance();
        }

        // Catch invalid octal digits like 08.
        if matches!(self.peek(), Some(b'8' | b'9')) {
            return Err(self.error(
                format!("invalid digit '{}' in octal constant", self.peek().unwrap() as char),
                self.line,
                self.column,
            ));
        }

        self.eat_int_suffix();

        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        Ok(Token::new(TokenKind::IntLiteral, text, start_line, start_col))
    }

    /// Consume optional `u`/`U`, `l`/`L`, or `ul`/`UL` suffixes.
    fn eat_int_suffix(&mut self) {
        match self.peek() {
            Some(b'u' | b'U') => {
                self.advance();
                if matches!(self.peek(), Some(b'l' | b'L')) {
                    self.advance();
                }
            }
            Some(b'l' | b'L') => {
                self.advance();
                if matches!(self.peek(), Some(b'u' | b'U')) {
                    self.advance();
                }
            }
            _ => {}
        }
    }

    // -- Character & string literals ------------------------------------

    /// Parse an escape sequence starting *after* the backslash.
    /// Returns the decoded byte value.
    fn parse_escape(&mut self) -> Result<u8, LexError> {
        let esc_line = self.line;
        let esc_col = self.column;

        match self.advance() {
            Some(b'n') => Ok(b'\n'),
            Some(b't') => Ok(b'\t'),
            Some(b'r') => Ok(b'\r'),
            Some(b'0') => {
                // Could be \0 or \0NN (octal).
                if matches!(self.peek(), Some(b'0'..=b'7')) {
                    self.parse_octal_escape(0)
                } else {
                    Ok(0)
                }
            }
            Some(b'\\') => Ok(b'\\'),
            Some(b'\'') => Ok(b'\''),
            Some(b'"') => Ok(b'"'),
            Some(b'a') => Ok(0x07), // bell
            Some(b'b') => Ok(0x08), // backspace
            Some(b'f') => Ok(0x0C), // form feed
            Some(b'v') => Ok(0x0B), // vertical tab
            Some(b'?') => Ok(b'?'),
            Some(b'x') => self.parse_hex_escape(),
            Some(ch @ b'1'..=b'7') => self.parse_octal_escape(ch - b'0'),
            Some(ch) => Err(self.error(
                format!("unknown escape sequence '\\{}'", ch as char),
                esc_line,
                esc_col,
            )),
            None => Err(self.error("unexpected end of file in escape sequence", esc_line, esc_col)),
        }
    }

    /// Parse `\xNN` – one or two hex digits.
    fn parse_hex_escape(&mut self) -> Result<u8, LexError> {
        let loc_line = self.line;
        let loc_col = self.column;

        let mut value: u8 = 0;
        let mut digits = 0u32;

        while digits < 2 {
            match self.peek() {
                Some(ch @ b'0'..=b'9') => {
                    value = value.wrapping_mul(16).wrapping_add(ch - b'0');
                    self.advance();
                    digits += 1;
                }
                Some(ch @ b'a'..=b'f') => {
                    value = value.wrapping_mul(16).wrapping_add(ch - b'a' + 10);
                    self.advance();
                    digits += 1;
                }
                Some(ch @ b'A'..=b'F') => {
                    value = value.wrapping_mul(16).wrapping_add(ch - b'A' + 10);
                    self.advance();
                    digits += 1;
                }
                _ => break,
            }
        }

        if digits == 0 {
            return Err(self.error("expected hex digit after '\\x'", loc_line, loc_col));
        }
        Ok(value)
    }

    /// Parse octal escape digits.  `first` is the already-consumed first
    /// octal digit value (0–7).  We consume up to two more octal digits.
    fn parse_octal_escape(&mut self, first: u8) -> Result<u8, LexError> {
        let mut value = first as u16;
        let mut digits = 1u32;

        while digits < 3 {
            match self.peek() {
                Some(ch @ b'0'..=b'7') => {
                    value = value * 8 + (ch - b'0') as u16;
                    self.advance();
                    digits += 1;
                }
                _ => break,
            }
        }

        if value > 255 {
            // This is technically reachable with e.g. \777 but we cap at u8.
            Ok(value as u8)
        } else {
            Ok(value as u8)
        }
    }

    fn lex_char_literal(&mut self, start_line: u32, start_col: u32) -> Result<Token, LexError> {
        self.advance(); // opening '

        let ch = match self.peek() {
            Some(b'\\') => {
                self.advance(); // backslash
                self.parse_escape()?
            }
            Some(b'\'') => {
                return Err(self.error("empty character literal", start_line, start_col));
            }
            Some(b'\n') | None => {
                return Err(self.error("unterminated character literal", start_line, start_col));
            }
            Some(_) => self.advance().unwrap(),
        };

        match self.advance() {
            Some(b'\'') => {}
            _ => {
                return Err(self.error(
                    "unterminated character literal (missing closing ')",
                    start_line,
                    start_col,
                ));
            }
        }

        // Store the numeric value as the token value string.
        Ok(Token::new(
            TokenKind::CharLiteral,
            format!("{}", ch),
            start_line,
            start_col,
        ))
    }

    fn lex_string_literal(&mut self, start_line: u32, start_col: u32) -> Result<Token, LexError> {
        self.advance(); // opening "
        let mut value = Vec::new();

        loop {
            match self.peek() {
                Some(b'"') => {
                    self.advance();
                    break;
                }
                Some(b'\\') => {
                    self.advance(); // backslash
                    let ch = self.parse_escape()?;
                    value.push(ch);
                }
                Some(b'\n') | None => {
                    return Err(self.error(
                        "unterminated string literal",
                        start_line,
                        start_col,
                    ));
                }
                Some(_) => {
                    value.push(self.advance().unwrap());
                }
            }
        }

        let s = String::from_utf8_lossy(&value).into_owned();
        Ok(Token::new(TokenKind::StringLiteral, s, start_line, start_col))
    }

    // -- Identifiers & keywords -----------------------------------------

    fn lex_ident_or_keyword(
        &mut self,
        start_line: u32,
        start_col: u32,
    ) -> Token {
        let start = self.pos;

        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                self.advance();
            } else {
                break;
            }
        }

        let word = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        let kind = keyword_kind(word).unwrap_or(TokenKind::Ident);
        Token::new(kind, word, start_line, start_col)
    }

    // -- Preprocessor directives ----------------------------------------

    fn lex_preproc_directive(
        &mut self,
        start_line: u32,
        start_col: u32,
    ) -> Token {
        self.advance(); // #

        // Skip optional whitespace between # and directive name.
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.advance();
        }

        // Collect everything until end of line (respecting backslash
        // continuation).
        let text_start = self.pos;
        loop {
            match self.peek() {
                Some(b'\n') | None => break,
                Some(b'\\') if self.peek_ahead(1) == Some(b'\n') => {
                    // Line continuation – consume both characters and keep going.
                    self.advance(); // backslash
                    self.advance(); // newline
                }
                _ => {
                    self.advance();
                }
            }
        }

        let text = std::str::from_utf8(&self.src[text_start..self.pos])
            .unwrap()
            .trim_end()
            .to_string();

        Token::new(TokenKind::PreprocDirective, text, start_line, start_col)
    }

    // -- Operators & punctuation ----------------------------------------

    /// Consume a one-, two-, or three-character operator/punctuation token.
    fn lex_operator(
        &mut self,
        start_line: u32,
        start_col: u32,
    ) -> Result<Token, LexError> {
        let ch = self.advance().unwrap();
        let next = self.peek();

        let (kind, value) = match ch {
            b'+' => match next {
                Some(b'+') => { self.advance(); (TokenKind::PlusPlus, "++") }
                Some(b'=') => { self.advance(); (TokenKind::PlusEq, "+=") }
                _ => (TokenKind::Plus, "+"),
            },
            b'-' => match next {
                Some(b'-') => { self.advance(); (TokenKind::MinusMinus, "--") }
                Some(b'>') => { self.advance(); (TokenKind::Arrow, "->") }
                Some(b'=') => { self.advance(); (TokenKind::MinusEq, "-=") }
                _ => (TokenKind::Minus, "-"),
            },
            b'*' => match next {
                Some(b'=') => { self.advance(); (TokenKind::StarEq, "*=") }
                _ => (TokenKind::Star, "*"),
            },
            b'/' => match next {
                Some(b'=') => { self.advance(); (TokenKind::SlashEq, "/=") }
                _ => (TokenKind::Slash, "/"),
            },
            b'%' => match next {
                Some(b'=') => { self.advance(); (TokenKind::PercentEq, "%=") }
                _ => (TokenKind::Percent, "%"),
            },
            b'&' => match next {
                Some(b'&') => { self.advance(); (TokenKind::AmpAmp, "&&") }
                Some(b'=') => { self.advance(); (TokenKind::AmpEq, "&=") }
                _ => (TokenKind::Amp, "&"),
            },
            b'|' => match next {
                Some(b'|') => { self.advance(); (TokenKind::PipePipe, "||") }
                Some(b'=') => { self.advance(); (TokenKind::PipeEq, "|=") }
                _ => (TokenKind::Pipe, "|"),
            },
            b'^' => match next {
                Some(b'=') => { self.advance(); (TokenKind::CaretEq, "^=") }
                _ => (TokenKind::Caret, "^"),
            },
            b'~' => (TokenKind::Tilde, "~"),
            b'!' => match next {
                Some(b'=') => { self.advance(); (TokenKind::BangEq, "!=") }
                _ => (TokenKind::Bang, "!"),
            },
            b'=' => match next {
                Some(b'=') => { self.advance(); (TokenKind::EqEq, "==") }
                _ => (TokenKind::Assign, "="),
            },
            b'<' => match next {
                Some(b'<') => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        (TokenKind::LtLtEq, "<<=")
                    } else {
                        (TokenKind::LtLt, "<<")
                    }
                }
                Some(b'=') => { self.advance(); (TokenKind::LtEq, "<=") }
                _ => (TokenKind::Lt, "<"),
            },
            b'>' => match next {
                Some(b'>') => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        (TokenKind::GtGtEq, ">>=")
                    } else {
                        (TokenKind::GtGt, ">>")
                    }
                }
                Some(b'=') => { self.advance(); (TokenKind::GtEq, ">=") }
                _ => (TokenKind::Gt, ">"),
            },
            b'.' => {
                if self.peek() == Some(b'.') && self.peek_ahead(1) == Some(b'.') {
                    self.advance();
                    self.advance();
                    (TokenKind::Ellipsis, "...")
                } else {
                    (TokenKind::Dot, ".")
                }
            }
            b'(' => (TokenKind::LParen, "("),
            b')' => (TokenKind::RParen, ")"),
            b'{' => (TokenKind::LBrace, "{"),
            b'}' => (TokenKind::RBrace, "}"),
            b'[' => (TokenKind::LBracket, "["),
            b']' => (TokenKind::RBracket, "]"),
            b';' => (TokenKind::Semicolon, ";"),
            b',' => (TokenKind::Comma, ","),
            b':' => (TokenKind::Colon, ":"),
            b'?' => (TokenKind::Question, "?"),
            b'#' => (TokenKind::Hash, "#"),
            _ => {
                return Err(self.error(
                    format!("unexpected character '{}'", ch as char),
                    start_line,
                    start_col,
                ));
            }
        };

        Ok(Token::new(kind, value, start_line, start_col))
    }

    // -- Main entry point -----------------------------------------------

    /// Produce the next token from the input.
    pub fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace_and_comments()?;

        if self.at_end() {
            return Ok(Token::new(TokenKind::Eof, "", self.line, self.column));
        }

        let start_line = self.line;
        let start_col = self.column;
        let was_line_start = self.at_line_start;

        // Any non-whitespace token clears the line-start flag.
        self.at_line_start = false;

        let ch = self.peek().unwrap();

        // Preprocessor directive: # at the start of a line.
        if ch == b'#' && was_line_start {
            return Ok(self.lex_preproc_directive(start_line, start_col));
        }

        // Identifier or keyword.
        if ch.is_ascii_alphabetic() || ch == b'_' {
            return Ok(self.lex_ident_or_keyword(start_line, start_col));
        }

        // Number literal.
        if ch.is_ascii_digit() {
            return self.lex_number(start_line, start_col);
        }

        // Character literal.
        if ch == b'\'' {
            return self.lex_char_literal(start_line, start_col);
        }

        // String literal.
        if ch == b'"' {
            return self.lex_string_literal(start_line, start_col);
        }

        // Operators and punctuation.
        self.lex_operator(start_line, start_col)
    }
}

// ---------------------------------------------------------------------------
// Public convenience function
// ---------------------------------------------------------------------------

/// Tokenize an entire source string, returning the vector of tokens
/// (including a trailing [`TokenKind::Eof`]).
pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();

    loop {
        let tok = lexer.next_token()?;
        let is_eof = tok.kind == TokenKind::Eof;
        tokens.push(tok);
        if is_eof {
            break;
        }
    }

    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: tokenize and strip the trailing Eof.
    fn toks(src: &str) -> Vec<Token> {
        let mut v = tokenize(src).expect("tokenize failed");
        assert_eq!(v.last().unwrap().kind, TokenKind::Eof);
        v.pop(); // remove Eof
        v
    }

    /// Helper: tokenize and return just the kinds.
    fn kinds(src: &str) -> Vec<TokenKind> {
        toks(src).into_iter().map(|t| t.kind).collect()
    }

    // -- Keywords -------------------------------------------------------

    #[test]
    fn keywords() {
        let src = "auto break case char const continue default do double else \
                   enum extern float for goto if int long register return \
                   short signed sizeof static struct switch typedef union \
                   unsigned void volatile while __stack __global";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Auto,
                TokenKind::Break,
                TokenKind::Case,
                TokenKind::Char,
                TokenKind::Const,
                TokenKind::Continue,
                TokenKind::Default,
                TokenKind::Do,
                TokenKind::Double,
                TokenKind::Else,
                TokenKind::Enum,
                TokenKind::Extern,
                TokenKind::Float,
                TokenKind::For,
                TokenKind::Goto,
                TokenKind::If,
                TokenKind::Int,
                TokenKind::Long,
                TokenKind::Register,
                TokenKind::Return,
                TokenKind::Short,
                TokenKind::Signed,
                TokenKind::Sizeof,
                TokenKind::Static,
                TokenKind::Struct,
                TokenKind::Switch,
                TokenKind::Typedef,
                TokenKind::Union,
                TokenKind::Unsigned,
                TokenKind::Void,
                TokenKind::Volatile,
                TokenKind::While,
                TokenKind::Stack,
                TokenKind::Global,
            ]
        );
    }

    // -- Identifiers ----------------------------------------------------

    #[test]
    fn identifiers() {
        let t = toks("foo _bar baz123 _0");
        assert!(t.iter().all(|t| t.kind == TokenKind::Ident));
        assert_eq!(t[0].value, "foo");
        assert_eq!(t[1].value, "_bar");
        assert_eq!(t[2].value, "baz123");
        assert_eq!(t[3].value, "_0");
    }

    #[test]
    fn keyword_prefix_is_ident() {
        // `integer` should be an Ident, not Int + Ident.
        let t = toks("integer");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, TokenKind::Ident);
        assert_eq!(t[0].value, "integer");
    }

    // -- Integer literals -----------------------------------------------

    #[test]
    fn decimal_literals() {
        let t = toks("0 42 12345");
        assert_eq!(t.len(), 3);
        assert!(t.iter().all(|t| t.kind == TokenKind::IntLiteral));
        assert_eq!(t[0].value, "0");
        assert_eq!(t[1].value, "42");
        assert_eq!(t[2].value, "12345");
    }

    #[test]
    fn hex_literals() {
        let t = toks("0x0 0xFF 0X1A2b");
        assert_eq!(t.len(), 3);
        assert!(t.iter().all(|t| t.kind == TokenKind::IntLiteral));
        assert_eq!(t[0].value, "0x0");
        assert_eq!(t[1].value, "0xFF");
        assert_eq!(t[2].value, "0X1A2b");
    }

    #[test]
    fn octal_literals() {
        let t = toks("017 0377");
        assert_eq!(t.len(), 2);
        assert!(t.iter().all(|t| t.kind == TokenKind::IntLiteral));
        assert_eq!(t[0].value, "017");
        assert_eq!(t[1].value, "0377");
    }

    #[test]
    fn int_suffixes() {
        let t = toks("42U 42L 42UL 42LU 0xFFu 0xFFl");
        assert_eq!(t.len(), 6);
        assert!(t.iter().all(|t| t.kind == TokenKind::IntLiteral));
        assert_eq!(t[0].value, "42U");
        assert_eq!(t[1].value, "42L");
        assert_eq!(t[2].value, "42UL");
        assert_eq!(t[3].value, "42LU");
        assert_eq!(t[4].value, "0xFFu");
        assert_eq!(t[5].value, "0xFFl");
    }

    #[test]
    fn hex_missing_digits() {
        let err = tokenize("0x ").unwrap_err();
        assert!(err.message.contains("hex digit"));
    }

    #[test]
    fn octal_invalid_digit() {
        let err = tokenize("08").unwrap_err();
        assert!(err.message.contains("invalid digit"));
    }

    // -- Character literals ---------------------------------------------

    #[test]
    fn char_literals_simple() {
        let t = toks("'a' 'Z' '0'");
        assert_eq!(t.len(), 3);
        assert!(t.iter().all(|t| t.kind == TokenKind::CharLiteral));
        assert_eq!(t[0].value, "97");   // 'a'
        assert_eq!(t[1].value, "90");   // 'Z'
        assert_eq!(t[2].value, "48");   // '0'
    }

    #[test]
    fn char_escape_sequences() {
        let t = toks(r"'\n' '\t' '\r' '\0' '\\' '\''");
        assert_eq!(t.len(), 6);
        assert_eq!(t[0].value, "10");   // \n
        assert_eq!(t[1].value, "9");    // \t
        assert_eq!(t[2].value, "13");   // \r
        assert_eq!(t[3].value, "0");    // \0
        assert_eq!(t[4].value, "92");   // \\
        assert_eq!(t[5].value, "39");   // \'
    }

    #[test]
    fn char_hex_escape() {
        let t = toks(r"'\x41' '\xFF'");
        assert_eq!(t[0].value, "65");   // 0x41 = 'A'
        assert_eq!(t[1].value, "255");  // 0xFF
    }

    #[test]
    fn char_octal_escape() {
        let t = toks(r"'\101' '\177'");
        assert_eq!(t[0].value, "65");   // 0101 = 'A'
        assert_eq!(t[1].value, "127");  // 0177
    }

    #[test]
    fn empty_char_literal() {
        let err = tokenize("''").unwrap_err();
        assert!(err.message.contains("empty character"));
    }

    #[test]
    fn unterminated_char_literal() {
        let err = tokenize("'a").unwrap_err();
        assert!(err.message.contains("unterminated character"));
    }

    // -- String literals ------------------------------------------------

    #[test]
    fn string_literal_simple() {
        let t = toks(r#""hello""#);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, TokenKind::StringLiteral);
        assert_eq!(t[0].value, "hello");
    }

    #[test]
    fn string_escape_sequences() {
        let t = toks(r#""a\nb\tc\0d""#);
        assert_eq!(t[0].kind, TokenKind::StringLiteral);
        assert_eq!(t[0].value, "a\nb\tc\0d");
    }

    #[test]
    fn string_hex_escape() {
        let t = toks(r#""\x48\x49""#);
        assert_eq!(t[0].value, "HI"); // 0x48='H', 0x49='I'
    }

    #[test]
    fn unterminated_string() {
        let err = tokenize("\"hello\n\"").unwrap_err();
        assert!(err.message.contains("unterminated string"));
    }

    // -- Operators & punctuation ----------------------------------------

    #[test]
    fn single_char_operators() {
        let k = kinds("+ - * / % & | ^ ~ ! = < > .");
        assert_eq!(
            k,
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Amp,
                TokenKind::Pipe,
                TokenKind::Caret,
                TokenKind::Tilde,
                TokenKind::Bang,
                TokenKind::Assign,
                TokenKind::Lt,
                TokenKind::Gt,
                TokenKind::Dot,
            ]
        );
    }

    #[test]
    fn two_char_operators() {
        let k = kinds("++ -- -> << >> == != <= >= && ||");
        assert_eq!(
            k,
            vec![
                TokenKind::PlusPlus,
                TokenKind::MinusMinus,
                TokenKind::Arrow,
                TokenKind::LtLt,
                TokenKind::GtGt,
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
            ]
        );
    }

    #[test]
    fn compound_assignment() {
        let k = kinds("+= -= *= /= %= &= |= ^= <<= >>=");
        assert_eq!(
            k,
            vec![
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarEq,
                TokenKind::SlashEq,
                TokenKind::PercentEq,
                TokenKind::AmpEq,
                TokenKind::PipeEq,
                TokenKind::CaretEq,
                TokenKind::LtLtEq,
                TokenKind::GtGtEq,
            ]
        );
    }

    #[test]
    fn punctuation() {
        let k = kinds("( ) { } [ ] ; , : ?");
        assert_eq!(
            k,
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Semicolon,
                TokenKind::Comma,
                TokenKind::Colon,
                TokenKind::Question,
            ]
        );
    }

    #[test]
    fn ellipsis() {
        let k = kinds("...");
        assert_eq!(k, vec![TokenKind::Ellipsis]);
    }

    // -- Comments -------------------------------------------------------

    #[test]
    fn line_comment() {
        let t = toks("a // comment\nb");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].value, "a");
        assert_eq!(t[1].value, "b");
    }

    #[test]
    fn block_comment() {
        let t = toks("a /* block\ncomment */ b");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].value, "a");
        assert_eq!(t[1].value, "b");
    }

    #[test]
    fn unterminated_block_comment() {
        let err = tokenize("/* oops").unwrap_err();
        assert!(err.message.contains("unterminated block comment"));
    }

    // -- Preprocessor ---------------------------------------------------

    #[test]
    fn preproc_include() {
        let t = toks("#include <stdio.h>\nint x;");
        assert_eq!(t[0].kind, TokenKind::PreprocDirective);
        assert_eq!(t[0].value, "include <stdio.h>");
        assert_eq!(t[1].kind, TokenKind::Int);
    }

    #[test]
    fn preproc_define() {
        let t = toks("#define FOO 42\n");
        assert_eq!(t[0].kind, TokenKind::PreprocDirective);
        assert_eq!(t[0].value, "define FOO 42");
    }

    #[test]
    fn preproc_line_continuation() {
        let t = toks("#define MULTI \\\nline\nint x;");
        assert_eq!(t[0].kind, TokenKind::PreprocDirective);
        assert!(t[0].value.contains("MULTI"));
        assert!(t[0].value.contains("line"));
    }

    #[test]
    fn hash_not_at_line_start() {
        // A `#` that is *not* at the start of a line is just a Hash token.
        let t = toks("a #");
        assert_eq!(t[1].kind, TokenKind::Hash);
    }

    // -- Position tracking ----------------------------------------------

    #[test]
    fn line_and_column() {
        let t = toks("int\n  x");
        assert_eq!(t[0].line, 1);
        assert_eq!(t[0].column, 1);
        assert_eq!(t[1].line, 2);
        assert_eq!(t[1].column, 3);
    }

    // -- Compound expressions -------------------------------------------

    #[test]
    fn function_call_tokens() {
        let k = kinds("foo(1, 2)");
        assert_eq!(
            k,
            vec![
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::IntLiteral,
                TokenKind::Comma,
                TokenKind::IntLiteral,
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn pointer_dereference_arrow() {
        let k = kinds("p->field");
        assert_eq!(
            k,
            vec![TokenKind::Ident, TokenKind::Arrow, TokenKind::Ident]
        );
    }

    #[test]
    fn complex_expression() {
        let k = kinds("x = a + b * c;");
        assert_eq!(
            k,
            vec![
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Ident,
                TokenKind::Plus,
                TokenKind::Ident,
                TokenKind::Star,
                TokenKind::Ident,
                TokenKind::Semicolon,
            ]
        );
    }

    #[test]
    fn ternary_expression() {
        let k = kinds("a ? b : c");
        assert_eq!(
            k,
            vec![
                TokenKind::Ident,
                TokenKind::Question,
                TokenKind::Ident,
                TokenKind::Colon,
                TokenKind::Ident,
            ]
        );
    }

    // -- Full mini-program ----------------------------------------------

    #[test]
    fn mini_program() {
        let src = "\
#include <stdio.h>

int main(void) {
    int x = 42;
    if (x > 0) {
        return x;
    }
    return 0;
}
";
        let tokens = tokenize(src).unwrap();
        // Just verify it doesn't error and has reasonable length.
        assert!(tokens.len() > 20);
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
    }

    // -- Display impls --------------------------------------------------

    #[test]
    fn token_display() {
        let tok = Token::new(TokenKind::Int, "int", 1, 1);
        let s = format!("{}", tok);
        assert!(s.contains("int"));
        assert!(s.contains("1:1"));
    }

    #[test]
    fn lex_error_display() {
        let e = LexError::new("bad char", 3, 7);
        let s = format!("{}", e);
        assert!(s.contains("3:7"));
        assert!(s.contains("bad char"));
    }

    // -- Edge cases -----------------------------------------------------

    #[test]
    fn empty_input() {
        let t = tokenize("").unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, TokenKind::Eof);
    }

    #[test]
    fn only_whitespace() {
        let t = tokenize("   \n\t\n  ").unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, TokenKind::Eof);
    }

    #[test]
    fn adjacent_operators() {
        // Ensure maximal-munch: `++` not `+ +`.
        let k = kinds("a+++b");
        // This should be: a ++ + b
        assert_eq!(
            k,
            vec![
                TokenKind::Ident,
                TokenKind::PlusPlus,
                TokenKind::Plus,
                TokenKind::Ident,
            ]
        );
    }

    #[test]
    fn shift_assign_vs_shift_then_assign() {
        // <<= should be one token, not << then =.
        let k = kinds("a <<= 1");
        assert_eq!(
            k,
            vec![
                TokenKind::Ident,
                TokenKind::LtLtEq,
                TokenKind::IntLiteral,
            ]
        );
    }

    #[test]
    fn string_with_escaped_quote() {
        let t = toks(r#""say \"hi\"""#);
        assert_eq!(t[0].value, "say \"hi\"");
    }

    #[test]
    fn zero_literal() {
        let t = toks("0");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, TokenKind::IntLiteral);
        assert_eq!(t[0].value, "0");
    }

    #[test]
    fn char_literal_bell_and_friends() {
        let t = toks(r"'\a' '\b' '\f' '\v' '\?'");
        assert_eq!(t[0].value, "7");    // bell
        assert_eq!(t[1].value, "8");    // backspace
        assert_eq!(t[2].value, "12");   // form feed
        assert_eq!(t[3].value, "11");   // vertical tab
        assert_eq!(t[4].value, "63");   // ?
    }

    #[test]
    fn sizeof_keyword() {
        let k = kinds("sizeof(int)");
        assert_eq!(
            k,
            vec![TokenKind::Sizeof, TokenKind::LParen, TokenKind::Int, TokenKind::RParen]
        );
    }

    #[test]
    fn array_subscript() {
        let k = kinds("arr[i]");
        assert_eq!(
            k,
            vec![TokenKind::Ident, TokenKind::LBracket, TokenKind::Ident, TokenKind::RBracket]
        );
    }
}
