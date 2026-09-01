//! The source front end for PhonoScript.
//!
//! This module deliberately stops at a source-faithful AST.  Name resolution,
//! domain typing, lowering, and execution belong to later stages so the GUI,
//! command-line interpreter, and embedded engine can share one language
//! implementation.  Source offsets are UTF-8 byte offsets; line and column are
//! one-based, and columns count Unicode scalar values rather than bytes.

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::Zero;

/// Prevent a short decimal spelling such as `1e2000000000` from forcing an
/// unbounded allocation while it is converted to an exact rational. Source
/// literals with more written digits remain naturally bounded by source size.
const MAX_EXACT_DECIMAL_EXPONENT: u32 = 10_000;

// ---------------------------------------------------------------------------
// Source locations and diagnostics

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourcePosition {
    pub byte: usize,
    pub line: usize,
    pub column: usize,
}

impl SourcePosition {
    pub const fn start() -> Self {
        Self {
            byte: 0,
            line: 1,
            column: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

impl Span {
    pub const fn empty(at: SourcePosition) -> Self {
        Self { start: at, end: at }
    }

    pub fn through(self, other: Self) -> Self {
        Self {
            start: self.start,
            end: other.end,
        }
    }

    pub fn byte_len(self) -> usize {
        self.end.byte.saturating_sub(self.start.byte)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    UnexpectedCharacter,
    UnterminatedString,
    InvalidEscape,
    UnterminatedBlockComment,
    InvalidNumber,
    ExpectedToken,
    ExpectedExpression,
    InvalidAssignmentTarget,
    TooManyParameters,
    TooManyArguments,
    UnclosedDelimiter,
    ExpectedStatementTerminator,
    ExpectedCommand,
    UnexpectedClosingDelimiter,
    InvalidModuleDeclaration,
}

impl DiagnosticCode {
    /// Stable public identifier suitable for editors and test fixtures.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnexpectedCharacter => "PSL0001",
            Self::UnterminatedString => "PSL0002",
            Self::InvalidEscape => "PSL0003",
            Self::UnterminatedBlockComment => "PSL0004",
            Self::InvalidNumber => "PSL0005",
            Self::ExpectedToken => "PSP0101",
            Self::ExpectedExpression => "PSP0102",
            Self::InvalidAssignmentTarget => "PSP0103",
            Self::TooManyParameters => "PSP0104",
            Self::TooManyArguments => "PSP0105",
            Self::UnclosedDelimiter => "PSP0106",
            Self::ExpectedStatementTerminator => "PSP0107",
            Self::ExpectedCommand => "PSP0108",
            Self::UnexpectedClosingDelimiter => "PSP0109",
            Self::InvalidModuleDeclaration => "PSP0110",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedSpan {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub message: String,
    pub primary: Span,
    pub related: Vec<RelatedSpan>,
    pub help: Option<String>,
}

impl Diagnostic {
    fn error(code: DiagnosticCode, message: impl Into<String>, primary: Span) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            primary,
            related: Vec::new(),
            help: None,
        }
    }

    fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    fn with_related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.related.push(RelatedSpan {
            span,
            message: message.into(),
        });
        self
    }
}

// ---------------------------------------------------------------------------
// Tokens and exact literals

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumericLiteral {
    Integer(BigInt),
    Rational(BigRational),
    /// A terminating decimal, optionally written with a base-ten exponent.
    /// `value` is exact; the remaining fields preserve the written notation.
    Decimal {
        value: BigRational,
        fractional_digits: u32,
        exponent: i32,
    },
}

impl NumericLiteral {
    pub fn exact_value(&self) -> BigRational {
        match self {
            Self::Integer(value) => BigRational::from_integer(value.clone()),
            Self::Rational(value) | Self::Decimal { value, .. } => value.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Dot,
    Colon,
    Semicolon,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    BangEqual,
    Equal,
    EqualEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    AndAnd,
    OrOr,
    Arrow,
    Identifier(String),
    Number(NumericLiteral),
    Text(String),
    Let,
    Var,
    Fn,
    If,
    Else,
    While,
    For,
    In,
    Return,
    True,
    False,
    Null,
    And,
    Or,
    Not,
    Command,
    Import,
    Export,
    From,
    As,
    Newline,
    Error,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenTag {
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Dot,
    Colon,
    Semicolon,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    BangEqual,
    Equal,
    EqualEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    AndAnd,
    OrOr,
    Arrow,
    Identifier,
    Number,
    Text,
    Let,
    Var,
    Fn,
    If,
    Else,
    While,
    For,
    In,
    Return,
    True,
    False,
    Null,
    And,
    Or,
    Not,
    Command,
    Import,
    Export,
    From,
    As,
    Newline,
    Error,
    Eof,
}

impl TokenKind {
    pub const fn tag(&self) -> TokenTag {
        match self {
            Self::LeftParen => TokenTag::LeftParen,
            Self::RightParen => TokenTag::RightParen,
            Self::LeftBrace => TokenTag::LeftBrace,
            Self::RightBrace => TokenTag::RightBrace,
            Self::LeftBracket => TokenTag::LeftBracket,
            Self::RightBracket => TokenTag::RightBracket,
            Self::Comma => TokenTag::Comma,
            Self::Dot => TokenTag::Dot,
            Self::Colon => TokenTag::Colon,
            Self::Semicolon => TokenTag::Semicolon,
            Self::Plus => TokenTag::Plus,
            Self::Minus => TokenTag::Minus,
            Self::Star => TokenTag::Star,
            Self::Slash => TokenTag::Slash,
            Self::Percent => TokenTag::Percent,
            Self::Bang => TokenTag::Bang,
            Self::BangEqual => TokenTag::BangEqual,
            Self::Equal => TokenTag::Equal,
            Self::EqualEqual => TokenTag::EqualEqual,
            Self::Greater => TokenTag::Greater,
            Self::GreaterEqual => TokenTag::GreaterEqual,
            Self::Less => TokenTag::Less,
            Self::LessEqual => TokenTag::LessEqual,
            Self::AndAnd => TokenTag::AndAnd,
            Self::OrOr => TokenTag::OrOr,
            Self::Arrow => TokenTag::Arrow,
            Self::Identifier(_) => TokenTag::Identifier,
            Self::Number(_) => TokenTag::Number,
            Self::Text(_) => TokenTag::Text,
            Self::Let => TokenTag::Let,
            Self::Var => TokenTag::Var,
            Self::Fn => TokenTag::Fn,
            Self::If => TokenTag::If,
            Self::Else => TokenTag::Else,
            Self::While => TokenTag::While,
            Self::For => TokenTag::For,
            Self::In => TokenTag::In,
            Self::Return => TokenTag::Return,
            Self::True => TokenTag::True,
            Self::False => TokenTag::False,
            Self::Null => TokenTag::Null,
            Self::And => TokenTag::And,
            Self::Or => TokenTag::Or,
            Self::Not => TokenTag::Not,
            Self::Command => TokenTag::Command,
            Self::Import => TokenTag::Import,
            Self::Export => TokenTag::Export,
            Self::From => TokenTag::From,
            Self::As => TokenTag::As,
            Self::Newline => TokenTag::Newline,
            Self::Error => TokenTag::Error,
            Self::Eof => TokenTag::Eof,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// Exact source spelling. Decoded string content lives in `TokenKind::Text`.
    pub lexeme: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ScanOutput {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ScanOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

pub fn scan(source: &str) -> ScanOutput {
    Scanner::new(source).scan()
}

struct Scanner<'source> {
    source: &'source str,
    cursor: usize,
    position: SourcePosition,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'source> Scanner<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            cursor: 0,
            position: SourcePosition::start(),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn scan(mut self) -> ScanOutput {
        while !self.is_at_end() {
            let start = self.position;
            let character = self.peek().expect("scanner cursor is on a UTF-8 boundary");
            match character {
                ' ' | '\t' | '\u{000b}' | '\u{000c}' => {
                    self.advance();
                }
                '\n' | '\r' => self.newline_token(),
                '#' => self.line_comment(),
                '/' if self.peek_n(1) == Some('/') => self.line_comment(),
                '/' if self.peek_n(1) == Some('*') => self.block_comment(start),
                '(' => self.single(TokenKind::LeftParen, start),
                ')' => self.single(TokenKind::RightParen, start),
                '{' => self.single(TokenKind::LeftBrace, start),
                '}' => self.single(TokenKind::RightBrace, start),
                '[' => self.single(TokenKind::LeftBracket, start),
                ']' => self.single(TokenKind::RightBracket, start),
                ',' => self.single(TokenKind::Comma, start),
                ':' => self.single(TokenKind::Colon, start),
                ';' => self.single(TokenKind::Semicolon, start),
                '+' => self.single(TokenKind::Plus, start),
                '*' => self.single(TokenKind::Star, start),
                '%' => self.single(TokenKind::Percent, start),
                '-' if self.peek_n(1) == Some('>') => {
                    self.advance();
                    self.advance();
                    self.push(TokenKind::Arrow, start);
                }
                '-' => self.single(TokenKind::Minus, start),
                '/' => self.single(TokenKind::Slash, start),
                '!' => self.one_or_two(TokenKind::Bang, TokenKind::BangEqual, '=', start),
                '=' => self.one_or_two(TokenKind::Equal, TokenKind::EqualEqual, '=', start),
                '>' => self.one_or_two(TokenKind::Greater, TokenKind::GreaterEqual, '=', start),
                '<' => self.one_or_two(TokenKind::Less, TokenKind::LessEqual, '=', start),
                '&' if self.peek_n(1) == Some('&') => {
                    self.advance();
                    self.advance();
                    self.push(TokenKind::AndAnd, start);
                }
                '|' if self.peek_n(1) == Some('|') => {
                    self.advance();
                    self.advance();
                    self.push(TokenKind::OrOr, start);
                }
                '.' if self.peek_n(1).is_some_and(|next| next.is_ascii_digit()) => {
                    self.advance();
                    self.number(start, true);
                }
                '.' => self.single(TokenKind::Dot, start),
                '\'' | '"' => self.string(start, character),
                value if value.is_ascii_digit() => self.number(start, false),
                value if is_identifier_start(value) => self.identifier(start),
                _ => {
                    self.advance();
                    let span = Span {
                        start,
                        end: self.position,
                    };
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::UnexpectedCharacter,
                            format!("unexpected character {character:?}"),
                            span,
                        )
                        .with_help("Remove the character or put it inside a quoted string."),
                    );
                    self.push_from_span(TokenKind::Error, span);
                }
            }
        }

        let at = self.position;
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            lexeme: String::new(),
            span: Span::empty(at),
        });
        ScanOutput {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn is_at_end(&self) -> bool {
        self.cursor >= self.source.len()
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.cursor..)?.chars().next()
    }

    fn peek_n(&self, offset: usize) -> Option<char> {
        self.source.get(self.cursor..)?.chars().nth(offset)
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.cursor += character.len_utf8();
        self.position.byte = self.cursor;
        if character == '\n' || character == '\r' {
            self.position.line += 1;
            self.position.column = 1;
        } else {
            self.position.column += 1;
        }
        Some(character)
    }

    fn consume_newline(&mut self) {
        if self.peek() == Some('\r') {
            self.advance();
            if self.peek() == Some('\n') {
                // CRLF is one logical line break, not two.
                self.cursor += '\n'.len_utf8();
                self.position.byte = self.cursor;
            }
        } else {
            self.advance();
        }
    }

    fn newline_token(&mut self) {
        let start = self.position;
        self.consume_newline();
        self.push(TokenKind::Newline, start);
    }

    fn single(&mut self, kind: TokenKind, start: SourcePosition) {
        self.advance();
        self.push(kind, start);
    }

    fn one_or_two(&mut self, one: TokenKind, two: TokenKind, second: char, start: SourcePosition) {
        self.advance();
        if self.peek() == Some(second) {
            self.advance();
            self.push(two, start);
        } else {
            self.push(one, start);
        }
    }

    fn push(&mut self, kind: TokenKind, start: SourcePosition) {
        self.push_from_span(
            kind,
            Span {
                start,
                end: self.position,
            },
        );
    }

    fn push_from_span(&mut self, kind: TokenKind, span: Span) {
        let lexeme = self
            .source
            .get(span.start.byte..span.end.byte)
            .unwrap_or_default()
            .to_owned();
        self.tokens.push(Token { kind, lexeme, span });
    }

    fn line_comment(&mut self) {
        while !self.is_at_end() && !matches!(self.peek(), Some('\n' | '\r')) {
            self.advance();
        }
    }

    fn block_comment(&mut self, opening: SourcePosition) {
        self.advance();
        self.advance();
        let mut depth = 1_usize;
        while !self.is_at_end() && depth > 0 {
            match (self.peek(), self.peek_n(1)) {
                (Some('/'), Some('*')) => {
                    self.advance();
                    self.advance();
                    depth += 1;
                }
                (Some('*'), Some('/')) => {
                    self.advance();
                    self.advance();
                    depth -= 1;
                }
                (Some('\n' | '\r'), _) => self.newline_token(),
                _ => {
                    self.advance();
                }
            }
        }
        if depth > 0 {
            let span = Span {
                start: opening,
                end: self.position,
            };
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::UnterminatedBlockComment,
                    "unterminated block comment",
                    span,
                )
                .with_help("Close the comment with */."),
            );
        }
    }

    fn string(&mut self, opening: SourcePosition, quote: char) {
        self.advance();
        let mut decoded = String::new();
        let mut terminated = false;
        while let Some(character) = self.peek() {
            if character == quote {
                self.advance();
                terminated = true;
                break;
            }
            if character != '\\' {
                if character == '\r' {
                    let crlf = self.peek_n(1) == Some('\n');
                    decoded.push('\r');
                    if crlf {
                        decoded.push('\n');
                    }
                    self.consume_newline();
                } else {
                    decoded.push(character);
                    self.advance();
                }
                continue;
            }

            let escape_start = self.position;
            self.advance();
            let Some(escaped) = self.peek() else {
                break;
            };
            match escaped {
                '\\' => {
                    decoded.push('\\');
                    self.advance();
                }
                '"' => {
                    decoded.push('"');
                    self.advance();
                }
                '\'' => {
                    decoded.push('\'');
                    self.advance();
                }
                'n' => {
                    decoded.push('\n');
                    self.advance();
                }
                'r' => {
                    decoded.push('\r');
                    self.advance();
                }
                't' => {
                    decoded.push('\t');
                    self.advance();
                }
                '0' => {
                    decoded.push('\0');
                    self.advance();
                }
                'u' => self.unicode_escape(escape_start, &mut decoded),
                invalid => {
                    self.advance();
                    let span = Span {
                        start: escape_start,
                        end: self.position,
                    };
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::InvalidEscape,
                            format!("unknown string escape \\{invalid}"),
                            span,
                        )
                        .with_help(r#"Use \\, \", \', \n, \r, \t, \0, or \u{...}."#),
                    );
                    decoded.push(invalid);
                }
            }
        }

        let span = Span {
            start: opening,
            end: self.position,
        };
        if terminated {
            self.push_from_span(TokenKind::Text(decoded), span);
        } else {
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::UnterminatedString,
                    "unterminated string literal",
                    span,
                )
                .with_help(format!("Close the string with {quote}.")),
            );
            self.push_from_span(TokenKind::Error, span);
        }
    }

    fn unicode_escape(&mut self, opening: SourcePosition, decoded: &mut String) {
        self.advance(); // u
        let mut valid = self.peek() == Some('{');
        if valid {
            self.advance();
        }
        let mut digits = String::new();
        while digits.len() < 6 && self.peek().is_some_and(|value| value.is_ascii_hexdigit()) {
            digits.push(self.advance().expect("peeked character exists"));
        }
        if digits.is_empty() || self.peek() != Some('}') {
            valid = false;
            while !self.is_at_end() && !matches!(self.peek(), Some('}' | '\n' | '\r' | '"' | '\''))
            {
                self.advance();
            }
        }
        if self.peek() == Some('}') {
            self.advance();
        }
        let scalar = u32::from_str_radix(&digits, 16)
            .ok()
            .and_then(char::from_u32);
        if valid && let Some(scalar) = scalar {
            decoded.push(scalar);
        } else {
            let span = Span {
                start: opening,
                end: self.position,
            };
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::InvalidEscape,
                    "invalid Unicode escape",
                    span,
                )
                .with_help("Use one to six hexadecimal digits, for example \\u{2598}."),
            );
            decoded.push('\u{fffd}');
        }
    }

    fn identifier(&mut self, start: SourcePosition) {
        self.advance();
        while self.peek().is_some_and(is_identifier_continue) {
            self.advance();
        }
        let spelling = &self.source[start.byte..self.position.byte];
        let kind = match spelling {
            "let" => TokenKind::Let,
            "var" => TokenKind::Var,
            "fn" => TokenKind::Fn,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "return" => TokenKind::Return,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "null" => TokenKind::Null,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            "command" => TokenKind::Command,
            "import" => TokenKind::Import,
            "export" => TokenKind::Export,
            "from" => TokenKind::From,
            "as" => TokenKind::As,
            _ => TokenKind::Identifier(spelling.to_owned()),
        };
        self.push(kind, start);
    }

    fn number(&mut self, start: SourcePosition, leading_dot: bool) {
        let mut malformed = false;
        malformed |= !self.digit_run();

        if !leading_dot
            && self.peek() == Some('/')
            && self.peek_n(1).is_some_and(|value| value.is_ascii_digit())
        {
            self.advance();
            malformed |= !self.digit_run();
            let span = Span {
                start,
                end: self.position,
            };
            let spelling = &self.source[span.start.byte..span.end.byte];
            let kind = parse_rational(spelling).filter(|_| !malformed).map_or_else(
                || {
                    self.invalid_number(span, "invalid rational literal");
                    TokenKind::Error
                },
                TokenKind::Number,
            );
            self.push_from_span(kind, span);
            return;
        }

        let mut decimal = leading_dot;
        if !leading_dot
            && self.peek() == Some('.')
            && self.peek_n(1).is_some_and(|value| value.is_ascii_digit())
        {
            decimal = true;
            self.advance();
            malformed |= !self.digit_run();
        }

        let mut exponent = false;
        if matches!(self.peek(), Some('e' | 'E'))
            && (self.peek_n(1).is_some_and(|value| value.is_ascii_digit())
                || matches!(self.peek_n(1), Some('+' | '-')))
        {
            exponent = true;
            decimal = true;
            self.advance();
            if matches!(self.peek(), Some('+' | '-')) {
                self.advance();
            }
            malformed |= !self.digit_run();
        }

        let span = Span {
            start,
            end: self.position,
        };
        let spelling = &self.source[span.start.byte..span.end.byte];
        let parsed = if decimal || exponent {
            parse_decimal(spelling)
        } else {
            parse_integer(spelling).map(NumericLiteral::Integer)
        };
        let kind = parsed.filter(|_| !malformed).map_or_else(
            || {
                self.invalid_number(span, "invalid exact numeric literal");
                TokenKind::Error
            },
            TokenKind::Number,
        );
        self.push_from_span(kind, span);
    }

    /// Consumes a digit/underscore run and returns whether its separators are valid.
    fn digit_run(&mut self) -> bool {
        let mut saw_digit = false;
        let mut previous_underscore = false;
        while let Some(character) = self.peek() {
            if character.is_ascii_digit() {
                saw_digit = true;
                previous_underscore = false;
                self.advance();
            } else if character == '_' {
                previous_underscore = true;
                self.advance();
            } else {
                break;
            }
        }
        saw_digit && !previous_underscore
    }

    fn invalid_number(&mut self, span: Span, message: &str) {
        self.diagnostics.push(
            Diagnostic::error(DiagnosticCode::InvalidNumber, message, span).with_help(
                "Use digits with internal '_' separators; rational denominators must be nonzero.",
            ),
        );
    }
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || character.is_alphanumeric() || is_combining_mark(character)
}

fn is_combining_mark(character: char) -> bool {
    matches!(
        character,
        '\u{0300}'..='\u{036f}'
            | '\u{1ab0}'..='\u{1aff}'
            | '\u{1dc0}'..='\u{1dff}'
            | '\u{20d0}'..='\u{20ff}'
            | '\u{fe20}'..='\u{fe2f}'
    )
}

fn clean_digits(spelling: &str) -> Option<String> {
    let mut clean = String::with_capacity(spelling.len());
    let mut previous_was_digit = false;
    for character in spelling.chars() {
        if character.is_ascii_digit() {
            clean.push(character);
            previous_was_digit = true;
        } else if character == '_' && previous_was_digit {
            previous_was_digit = false;
        } else {
            return None;
        }
    }
    previous_was_digit.then_some(clean)
}

fn parse_integer(spelling: &str) -> Option<BigInt> {
    BigInt::parse_bytes(clean_digits(spelling)?.as_bytes(), 10)
}

fn parse_rational(spelling: &str) -> Option<NumericLiteral> {
    let (numerator, denominator) = spelling.split_once('/')?;
    let numerator = parse_integer(numerator)?;
    let denominator = parse_integer(denominator)?;
    if denominator.is_zero() {
        return None;
    }
    Some(NumericLiteral::Rational(BigRational::new(
        numerator,
        denominator,
    )))
}

fn parse_decimal(spelling: &str) -> Option<NumericLiteral> {
    let clean = spelling.replace('_', "");
    let (mantissa, exponent) = clean
        .split_once(['e', 'E'])
        .map_or((clean.as_str(), 0_i32), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().ok().unwrap_or(i32::MIN))
        });
    if exponent == i32::MIN {
        return None;
    }
    if exponent.unsigned_abs() > MAX_EXACT_DECIMAL_EXPONENT {
        return None;
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.chars().all(|value| value.is_ascii_digit())
        || !fraction.chars().all(|value| value.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{}{}", if whole.is_empty() { "0" } else { whole }, fraction);
    let numerator = BigInt::parse_bytes(digits.as_bytes(), 10)?;
    let fractional_digits = u32::try_from(fraction.len()).ok()?;
    let mut denominator = BigInt::from(10_u8).pow(fractional_digits);
    let mut numerator = numerator;
    if exponent >= 0 {
        numerator *= BigInt::from(10_u8).pow(exponent.unsigned_abs());
    } else {
        denominator *= BigInt::from(10_u8).pow(exponent.unsigned_abs());
    }
    Some(NumericLiteral::Decimal {
        value: BigRational::new(numerator, denominator),
        fractional_digits,
        exponent,
    })
}

// ---------------------------------------------------------------------------
// Abstract syntax

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub statements: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementKind {
    Import {
        import_span: Span,
        path: String,
        path_span: Span,
        bindings: Vec<ImportBinding>,
    },
    Binding {
        exported: bool,
        export_span: Option<Span>,
        mutable: bool,
        name: String,
        name_span: Span,
        initializer: Option<Expression>,
    },
    Function {
        exported: bool,
        export_span: Option<Span>,
        name: String,
        name_span: Span,
        parameters: Vec<Parameter>,
        body: Vec<Statement>,
    },
    Block(Vec<Statement>),
    If {
        condition: Expression,
        then_branch: Vec<Statement>,
        else_branch: Option<Box<Statement>>,
    },
    While {
        condition: Expression,
        body: Vec<Statement>,
    },
    /// Collection iteration. Its later lowering can enforce a finite bound.
    For {
        binding: String,
        binding_span: Span,
        iterable: Expression,
        body: Vec<Statement>,
    },
    Return(Option<Expression>),
    Expression(Expression),
    /// Explicit bridge for the former line-oriented analytical commands.
    Command(Vec<Token>),
}

/// One explicitly selected immutable module binding. `imported` is the name
/// declared by the dependency; `local` is the name visible to this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    pub imported: String,
    pub imported_span: Span,
    pub local: String,
    pub local_span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionKind {
    Literal(Literal),
    Variable(String),
    List(Vec<Expression>),
    Record(Vec<RecordEntry>),
    Group(Box<Expression>),
    Unary {
        operator: UnaryOperator,
        operand: Box<Expression>,
    },
    Binary {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>,
    },
    Assignment {
        name: String,
        name_span: Span,
        value: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
    Index {
        collection: Box<Expression>,
        index: Box<Expression>,
    },
    Member {
        object: Box<Expression>,
        field: String,
        field_span: Span,
    },
}

/// One source-ordered record field. Keys are decoded text while `key_span`
/// retains the precise identifier or quoted spelling used for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordEntry {
    pub key: String,
    pub key_span: Span,
    pub value: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Number(NumericLiteral),
    Boolean(bool),
    Text(String),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Positive,
    Negate,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}

#[derive(Debug, Clone)]
pub struct FrontendOutput {
    pub tokens: Vec<Token>,
    pub program: Program,
    pub diagnostics: Vec<Diagnostic>,
}

impl FrontendOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

pub fn parse(source: &str) -> FrontendOutput {
    let scanned = scan(source);
    let mut parser = Parser::new(&scanned.tokens);
    let program = parser.program();
    let mut diagnostics = scanned.diagnostics;
    diagnostics.extend(parser.diagnostics);
    FrontendOutput {
        tokens: scanned.tokens,
        program,
        diagnostics,
    }
}

// ---------------------------------------------------------------------------
// Recursive-descent statements and precedence-climbing expressions

struct Parser<'tokens> {
    tokens: &'tokens [Token],
    current: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'tokens> Parser<'tokens> {
    fn new(tokens: &'tokens [Token]) -> Self {
        Self {
            tokens,
            current: 0,
            diagnostics: Vec::new(),
        }
    }

    fn program(&mut self) -> Program {
        let start = self.current().span.start;
        let mut statements = Vec::new();
        self.skip_separators();
        while !self.check(TokenTag::Eof) {
            if self.check(TokenTag::RightBrace) {
                let token = self.advance().clone();
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::UnexpectedClosingDelimiter,
                        "unexpected closing brace",
                        token.span,
                    )
                    .with_help("Remove this brace or add a matching opening brace."),
                );
                self.skip_separators();
                continue;
            }
            let checkpoint = self.current;
            match self.declaration() {
                Ok(statement) => statements.push(statement),
                Err(()) => {
                    self.synchronize();
                    if self.current == checkpoint && !self.check(TokenTag::Eof) {
                        self.advance();
                    }
                }
            }
            self.skip_separators();
        }
        let end = self.current().span.end;
        Program {
            statements,
            span: Span { start, end },
        }
    }

    fn declaration(&mut self) -> Result<Statement, ()> {
        if self.matches(&[TokenTag::Import]) {
            return self.import_declaration();
        }
        if self.matches(&[TokenTag::Export]) {
            let export_span = self.previous().span;
            if self.matches(&[TokenTag::Let]) {
                return self.binding(false, Some(export_span));
            }
            if self.matches(&[TokenTag::Fn]) {
                return self.function(Some(export_span));
            }
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::InvalidModuleDeclaration,
                    "export may only prefix an immutable let binding or a function",
                    self.current().span,
                )
                .with_related(export_span, "export declaration starts here")
                .with_help(
                    "Write `export let name = expression` or `export fn name(...) { ... }`.",
                ),
            );
            return Err(());
        }
        if self.matches(&[TokenTag::Let]) {
            return self.binding(false, None);
        }
        if self.matches(&[TokenTag::Var]) {
            return self.binding(true, None);
        }
        if self.matches(&[TokenTag::Fn]) {
            return self.function(None);
        }
        self.statement()
    }

    fn import_declaration(&mut self) -> Result<Statement, ()> {
        let import_span = self.previous().span;
        let left = self.consume(
            TokenTag::LeftBrace,
            "expected '{' after import",
            "List imported names inside braces.",
        )?;
        self.skip_soft_newlines();
        if self.check(TokenTag::RightBrace) {
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::InvalidModuleDeclaration,
                    "an import must select at least one exported name",
                    self.current().span,
                )
                .with_related(import_span, "import declaration starts here")
                .with_help("Add a name inside the braces, for example `import { evaluate } from \"./grammar.phont\"`."),
            );
            self.advance();
            return Err(());
        }

        let mut bindings = Vec::new();
        loop {
            let imported_token = self.consume(
                TokenTag::Identifier,
                "expected an exported name in the import list",
                "Import names are Unicode identifiers separated by commas.",
            )?;
            let (imported, imported_span) =
                identifier_parts(imported_token).expect("consumed identifier");
            let (local, local_span) = if self.matches(&[TokenTag::As]) {
                let local_token = self.consume(
                    TokenTag::Identifier,
                    "expected a local name after as",
                    "Write an identifier after as, for example `winner as selected_winner`.",
                )?;
                identifier_parts(local_token).expect("consumed identifier")
            } else {
                (imported.clone(), imported_span)
            };
            bindings.push(ImportBinding {
                imported,
                imported_span,
                local,
                local_span,
            });
            self.skip_soft_newlines();
            if !self.matches(&[TokenTag::Comma]) {
                break;
            }
            self.skip_soft_newlines();
            if self.check(TokenTag::RightBrace) {
                break;
            }
        }
        self.consume_closing(TokenTag::RightBrace, left.span, "import list")?;
        self.skip_soft_newlines();
        self.consume(
            TokenTag::From,
            "expected from after the import list",
            "Write `from` followed by a quoted relative .phont path.",
        )?;
        self.skip_soft_newlines();
        let path_token = self.consume(
            TokenTag::Text,
            "expected a quoted module path after from",
            "Module paths are quoted, for example \"./grammar.phont\".",
        )?;
        let TokenKind::Text(path) = &path_token.kind else {
            unreachable!("consumed text token")
        };
        let path = path.clone();
        let path_span = path_token.span;
        self.finish_statement();
        Ok(Statement {
            kind: StatementKind::Import {
                import_span,
                path,
                path_span,
                bindings,
            },
            span: import_span.through(path_span),
        })
    }

    fn binding(&mut self, mutable: bool, export_span: Option<Span>) -> Result<Statement, ()> {
        let opening = self.previous().span;
        let name_token = self.consume(
            TokenTag::Identifier,
            "expected a binding name",
            "Write a Unicode identifier after let or var.",
        )?;
        let (name, name_span) = identifier_parts(name_token).expect("consumed identifier");
        let initializer = if self.matches(&[TokenTag::Equal]) {
            Some(self.expression()?)
        } else {
            if !mutable {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::ExpectedToken,
                        "immutable let binding requires an initializer",
                        self.current().span,
                    )
                    .with_help(
                        "Add = followed by an expression, or use var for deferred assignment.",
                    ),
                );
            }
            None
        };
        let end = initializer
            .as_ref()
            .map_or(name_span, |expression| expression.span);
        self.finish_statement();
        Ok(Statement {
            kind: StatementKind::Binding {
                exported: export_span.is_some(),
                export_span,
                mutable,
                name,
                name_span,
                initializer,
            },
            span: export_span.unwrap_or(opening).through(end),
        })
    }

    fn function(&mut self, export_span: Option<Span>) -> Result<Statement, ()> {
        let opening = self.previous().span;
        let name_token = self.consume(
            TokenTag::Identifier,
            "expected a function name",
            "Write a Unicode identifier after fn.",
        )?;
        let (name, name_span) = identifier_parts(name_token).expect("consumed identifier");
        let left = self.consume(
            TokenTag::LeftParen,
            "expected '(' after the function name",
            "Function parameters are written inside parentheses.",
        )?;
        self.skip_soft_newlines();
        let mut parameters = Vec::new();
        if !self.check(TokenTag::RightParen) {
            loop {
                let parameter = self.consume(
                    TokenTag::Identifier,
                    "expected a parameter name",
                    "Separate parameter names with commas.",
                )?;
                let (parameter_name, parameter_span) =
                    identifier_parts(parameter).expect("consumed identifier");
                if parameters.len() >= 255 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::TooManyParameters,
                            "a function may have at most 255 parameters",
                            parameter_span,
                        )
                        .with_help("Pass a list or a domain record instead of more parameters."),
                    );
                }
                parameters.push(Parameter {
                    name: parameter_name,
                    span: parameter_span,
                });
                self.skip_soft_newlines();
                if !self.matches(&[TokenTag::Comma]) {
                    break;
                }
                self.skip_soft_newlines();
            }
        }
        self.consume_closing(TokenTag::RightParen, left.span, "function parameter list")?;
        self.skip_soft_newlines();
        let (body, close) = self.required_block("function body")?;
        Ok(Statement {
            kind: StatementKind::Function {
                exported: export_span.is_some(),
                export_span,
                name,
                name_span,
                parameters,
                body,
            },
            span: export_span.unwrap_or(opening).through(close),
        })
    }

    fn statement(&mut self) -> Result<Statement, ()> {
        if self.matches(&[TokenTag::If]) {
            return self.if_statement();
        }
        if self.matches(&[TokenTag::While]) {
            return self.while_statement();
        }
        if self.matches(&[TokenTag::For]) {
            return self.for_statement();
        }
        if self.matches(&[TokenTag::Return]) {
            return self.return_statement();
        }
        if self.matches(&[TokenTag::Command]) {
            return self.command_statement();
        }
        if self.matches(&[TokenTag::LeftBrace]) {
            let opening = self.previous().span;
            let (statements, close) = self.block(opening)?;
            return Ok(Statement {
                kind: StatementKind::Block(statements),
                span: opening.through(close),
            });
        }
        self.expression_statement()
    }

    fn if_statement(&mut self) -> Result<Statement, ()> {
        let opening = self.previous().span;
        let condition = self.condition_expression()?;
        self.skip_soft_newlines();
        let (then_branch, then_close) = self.required_block("if branch")?;
        self.skip_separators();
        let mut end = then_close;
        let else_branch = if self.matches(&[TokenTag::Else]) {
            if self.matches(&[TokenTag::If]) {
                let branch = self.if_statement()?;
                end = branch.span;
                Some(Box::new(branch))
            } else {
                self.skip_soft_newlines();
                let left = self.consume(
                    TokenTag::LeftBrace,
                    "expected '{' after else",
                    "Wrap the else branch in braces.",
                )?;
                let (body, close) = self.block(left.span)?;
                end = close;
                Some(Box::new(Statement {
                    kind: StatementKind::Block(body),
                    span: left.span.through(close),
                }))
            }
        } else {
            None
        };
        Ok(Statement {
            kind: StatementKind::If {
                condition,
                then_branch,
                else_branch,
            },
            span: opening.through(end),
        })
    }

    fn while_statement(&mut self) -> Result<Statement, ()> {
        let opening = self.previous().span;
        let condition = self.condition_expression()?;
        self.skip_soft_newlines();
        let (body, close) = self.required_block("while body")?;
        Ok(Statement {
            kind: StatementKind::While { condition, body },
            span: opening.through(close),
        })
    }

    fn for_statement(&mut self) -> Result<Statement, ()> {
        let opening = self.previous().span;
        let binding = self.consume(
            TokenTag::Identifier,
            "expected a loop binding after for",
            "Use the form: for candidate in candidates { ... }",
        )?;
        let (binding, binding_span) = identifier_parts(binding).expect("consumed identifier");
        self.consume(
            TokenTag::In,
            "expected 'in' after the loop binding",
            "PhonoScript collection loops use: for name in collection { ... }",
        )?;
        let iterable = self.expression()?;
        self.skip_soft_newlines();
        let (body, close) = self.required_block("for body")?;
        Ok(Statement {
            kind: StatementKind::For {
                binding,
                binding_span,
                iterable,
                body,
            },
            span: opening.through(close),
        })
    }

    fn return_statement(&mut self) -> Result<Statement, ()> {
        let opening = self.previous().span;
        let value = if self.at_statement_end() {
            None
        } else {
            Some(self.expression()?)
        };
        let end = value.as_ref().map_or(opening, |value| value.span);
        self.finish_statement();
        Ok(Statement {
            kind: StatementKind::Return(value),
            span: opening.through(end),
        })
    }

    fn command_statement(&mut self) -> Result<Statement, ()> {
        let opening = self.previous().span;
        let mut tokens = Vec::new();
        while !self.at_statement_end() {
            tokens.push(self.advance().clone());
        }
        if tokens.is_empty() {
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::ExpectedCommand,
                    "expected a command after the command keyword",
                    self.current().span,
                )
                .with_help("For example: command tableau add \"Final devoicing\""),
            );
        }
        let end = tokens.last().map_or(opening, |token| token.span);
        self.finish_statement();
        Ok(Statement {
            kind: StatementKind::Command(tokens),
            span: opening.through(end),
        })
    }

    fn expression_statement(&mut self) -> Result<Statement, ()> {
        let expression = self.expression()?;
        let span = expression.span;
        self.finish_statement();
        Ok(Statement {
            kind: StatementKind::Expression(expression),
            span,
        })
    }

    fn condition_expression(&mut self) -> Result<Expression, ()> {
        if self.matches(&[TokenTag::LeftParen]) {
            let opening = self.previous().span;
            self.skip_soft_newlines();
            let expression = self.expression()?;
            self.skip_soft_newlines();
            self.consume_closing(TokenTag::RightParen, opening, "condition")?;
            Ok(expression)
        } else {
            self.expression()
        }
    }

    fn required_block(&mut self, description: &str) -> Result<(Vec<Statement>, Span), ()> {
        let opening = self.consume(
            TokenTag::LeftBrace,
            format!("expected '{{' before {description}"),
            "Control-flow and function bodies are enclosed in braces.",
        )?;
        self.block(opening.span)
    }

    fn block(&mut self, opening: Span) -> Result<(Vec<Statement>, Span), ()> {
        let mut statements = Vec::new();
        self.skip_separators();
        while !self.check(TokenTag::RightBrace) && !self.check(TokenTag::Eof) {
            let checkpoint = self.current;
            match self.declaration() {
                Ok(statement) => statements.push(statement),
                Err(()) => {
                    self.synchronize();
                    if self.current == checkpoint
                        && !self.check(TokenTag::RightBrace)
                        && !self.check(TokenTag::Eof)
                    {
                        self.advance();
                    }
                }
            }
            self.skip_separators();
        }
        if self.check(TokenTag::Eof) {
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::UnclosedDelimiter,
                    "unterminated block",
                    self.current().span,
                )
                .with_related(opening, "this block starts here")
                .with_help("Add a closing '}'."),
            );
            return Err(());
        }
        let close = self.advance().span;
        Ok((statements, close))
    }

    fn expression(&mut self) -> Result<Expression, ()> {
        self.assignment()
    }

    fn assignment(&mut self) -> Result<Expression, ()> {
        let expression = self.or()?;
        if self.matches(&[TokenTag::Equal]) {
            let equals = self.previous().span;
            self.skip_soft_newlines();
            let value = self.assignment()?;
            if let ExpressionKind::Variable(name) = expression.kind {
                let span = expression.span.through(value.span);
                return Ok(Expression {
                    kind: ExpressionKind::Assignment {
                        name,
                        name_span: expression.span,
                        value: Box::new(value),
                    },
                    span,
                });
            }
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::InvalidAssignmentTarget,
                    "invalid assignment target",
                    expression.span,
                )
                .with_related(equals, "assignment operator is here")
                .with_help("Assign to a declared variable name."),
            );
        }
        Ok(expression)
    }

    fn or(&mut self) -> Result<Expression, ()> {
        self.left_associative(Self::and, &[TokenTag::Or, TokenTag::OrOr], |_| {
            BinaryOperator::Or
        })
    }

    fn and(&mut self) -> Result<Expression, ()> {
        self.left_associative(Self::equality, &[TokenTag::And, TokenTag::AndAnd], |_| {
            BinaryOperator::And
        })
    }

    fn equality(&mut self) -> Result<Expression, ()> {
        self.left_associative(
            Self::comparison,
            &[TokenTag::EqualEqual, TokenTag::BangEqual],
            |tag| match tag {
                TokenTag::EqualEqual => BinaryOperator::Equal,
                TokenTag::BangEqual => BinaryOperator::NotEqual,
                _ => unreachable!("operator set controls this match"),
            },
        )
    }

    fn comparison(&mut self) -> Result<Expression, ()> {
        self.left_associative(
            Self::term,
            &[
                TokenTag::Less,
                TokenTag::LessEqual,
                TokenTag::Greater,
                TokenTag::GreaterEqual,
            ],
            |tag| match tag {
                TokenTag::Less => BinaryOperator::Less,
                TokenTag::LessEqual => BinaryOperator::LessEqual,
                TokenTag::Greater => BinaryOperator::Greater,
                TokenTag::GreaterEqual => BinaryOperator::GreaterEqual,
                _ => unreachable!("operator set controls this match"),
            },
        )
    }

    fn term(&mut self) -> Result<Expression, ()> {
        self.left_associative(
            Self::factor,
            &[TokenTag::Plus, TokenTag::Minus],
            |tag| match tag {
                TokenTag::Plus => BinaryOperator::Add,
                TokenTag::Minus => BinaryOperator::Subtract,
                _ => unreachable!("operator set controls this match"),
            },
        )
    }

    fn factor(&mut self) -> Result<Expression, ()> {
        self.left_associative(
            Self::unary,
            &[TokenTag::Star, TokenTag::Slash, TokenTag::Percent],
            |tag| match tag {
                TokenTag::Star => BinaryOperator::Multiply,
                TokenTag::Slash => BinaryOperator::Divide,
                TokenTag::Percent => BinaryOperator::Remainder,
                _ => unreachable!("operator set controls this match"),
            },
        )
    }

    fn left_associative(
        &mut self,
        operand: fn(&mut Self) -> Result<Expression, ()>,
        operators: &[TokenTag],
        operator: fn(TokenTag) -> BinaryOperator,
    ) -> Result<Expression, ()> {
        let mut expression = operand(self)?;
        while operators.contains(&self.current().kind.tag()) {
            let tag = self.advance().kind.tag();
            self.skip_soft_newlines();
            let right = operand(self)?;
            let span = expression.span.through(right.span);
            expression = Expression {
                kind: ExpressionKind::Binary {
                    left: Box::new(expression),
                    operator: operator(tag),
                    right: Box::new(right),
                },
                span,
            };
        }
        Ok(expression)
    }

    fn unary(&mut self) -> Result<Expression, ()> {
        if self.matches(&[
            TokenTag::Bang,
            TokenTag::Not,
            TokenTag::Minus,
            TokenTag::Plus,
        ]) {
            let token = self.previous().clone();
            self.skip_soft_newlines();
            let operand = self.unary()?;
            let operator = match token.kind.tag() {
                TokenTag::Bang | TokenTag::Not => UnaryOperator::Not,
                TokenTag::Minus => UnaryOperator::Negate,
                TokenTag::Plus => UnaryOperator::Positive,
                _ => unreachable!("matched unary operator"),
            };
            return Ok(Expression {
                span: token.span.through(operand.span),
                kind: ExpressionKind::Unary {
                    operator,
                    operand: Box::new(operand),
                },
            });
        }
        self.call()
    }

    fn call(&mut self) -> Result<Expression, ()> {
        let mut expression = self.primary()?;
        loop {
            if self.matches(&[TokenTag::LeftParen]) {
                let opening = self.previous().span;
                self.skip_soft_newlines();
                let mut arguments = Vec::new();
                if !self.check(TokenTag::RightParen) {
                    loop {
                        let argument = self.expression()?;
                        if arguments.len() >= 255 {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    DiagnosticCode::TooManyArguments,
                                    "a call may have at most 255 arguments",
                                    argument.span,
                                )
                                .with_help("Pass a list or a domain record instead."),
                            );
                        }
                        arguments.push(argument);
                        self.skip_soft_newlines();
                        if !self.matches(&[TokenTag::Comma]) {
                            break;
                        }
                        self.skip_soft_newlines();
                        if self.check(TokenTag::RightParen) {
                            break;
                        }
                    }
                }
                let close = self.consume_closing(TokenTag::RightParen, opening, "argument list")?;
                let span = expression.span.through(close.span);
                expression = Expression {
                    kind: ExpressionKind::Call {
                        callee: Box::new(expression),
                        arguments,
                    },
                    span,
                };
            } else if self.matches(&[TokenTag::LeftBracket]) {
                let opening = self.previous().span;
                self.skip_soft_newlines();
                let index = self.expression()?;
                self.skip_soft_newlines();
                let close = self.consume_closing(TokenTag::RightBracket, opening, "index")?;
                let span = expression.span.through(close.span);
                expression = Expression {
                    kind: ExpressionKind::Index {
                        collection: Box::new(expression),
                        index: Box::new(index),
                    },
                    span,
                };
            } else if self.matches(&[TokenTag::Dot]) {
                let field_token = self
                    .consume(
                        TokenTag::Identifier,
                        "expected a field name after `.`",
                        "Use an identifier field name, or bracket notation for a quoted key.",
                    )?
                    .clone();
                let TokenKind::Identifier(field) = field_token.kind else {
                    unreachable!("identifier token was consumed")
                };
                let span = expression.span.through(field_token.span);
                expression = Expression {
                    kind: ExpressionKind::Member {
                        object: Box::new(expression),
                        field,
                        field_span: field_token.span,
                    },
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn primary(&mut self) -> Result<Expression, ()> {
        let token = self.advance().clone();
        let kind = match token.kind {
            TokenKind::Number(number) => ExpressionKind::Literal(Literal::Number(number)),
            TokenKind::Text(text) => ExpressionKind::Literal(Literal::Text(text)),
            TokenKind::True => ExpressionKind::Literal(Literal::Boolean(true)),
            TokenKind::False => ExpressionKind::Literal(Literal::Boolean(false)),
            TokenKind::Null => ExpressionKind::Literal(Literal::Null),
            TokenKind::Identifier(name) => ExpressionKind::Variable(name),
            TokenKind::LeftParen => {
                self.skip_soft_newlines();
                let expression = self.expression()?;
                self.skip_soft_newlines();
                let close = self.consume_closing(TokenTag::RightParen, token.span, "group")?;
                return Ok(Expression {
                    span: token.span.through(close.span),
                    kind: ExpressionKind::Group(Box::new(expression)),
                });
            }
            TokenKind::LeftBracket => {
                self.skip_soft_newlines();
                let mut values = Vec::new();
                if !self.check(TokenTag::RightBracket) {
                    loop {
                        values.push(self.expression()?);
                        self.skip_soft_newlines();
                        if !self.matches(&[TokenTag::Comma]) {
                            break;
                        }
                        self.skip_soft_newlines();
                        if self.check(TokenTag::RightBracket) {
                            break;
                        }
                    }
                }
                let close = self.consume_closing(TokenTag::RightBracket, token.span, "list")?;
                return Ok(Expression {
                    span: token.span.through(close.span),
                    kind: ExpressionKind::List(values),
                });
            }
            TokenKind::LeftBrace => {
                self.skip_soft_newlines();
                let mut entries = Vec::new();
                if !self.check(TokenTag::RightBrace) {
                    loop {
                        let key_token = self.advance().clone();
                        let key = match key_token.kind {
                            TokenKind::Identifier(key) | TokenKind::Text(key) => key,
                            _ => {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        DiagnosticCode::ExpectedToken,
                                        "expected a record key",
                                        key_token.span,
                                    )
                                    .with_help(
                                        "Use an identifier key or quoted text key before `:`.",
                                    ),
                                );
                                return Err(());
                            }
                        };
                        self.skip_soft_newlines();
                        if !self.matches(&[TokenTag::Colon]) {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    DiagnosticCode::ExpectedToken,
                                    "expected `:` after the record key",
                                    self.current().span,
                                )
                                .with_related(key_token.span, "record key starts here")
                                .with_help("Separate every record key from its value with `:`."),
                            );
                            return Err(());
                        }
                        self.skip_soft_newlines();
                        let value = self.expression()?;
                        entries.push(RecordEntry {
                            key,
                            key_span: key_token.span,
                            value,
                        });
                        self.skip_soft_newlines();
                        if !self.matches(&[TokenTag::Comma]) {
                            break;
                        }
                        self.skip_soft_newlines();
                        if self.check(TokenTag::RightBrace) {
                            break;
                        }
                    }
                }
                let close = self.consume_closing(TokenTag::RightBrace, token.span, "record")?;
                return Ok(Expression {
                    span: token.span.through(close.span),
                    kind: ExpressionKind::Record(entries),
                });
            }
            _ => {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::ExpectedExpression,
                        "expected an expression",
                        token.span,
                    )
                    .with_help(
                        "Use a literal, name, list, record, grouped expression, or function call.",
                    ),
                );
                return Err(());
            }
        };
        Ok(Expression {
            kind,
            span: token.span,
        })
    }

    fn finish_statement(&mut self) {
        if self.matches(&[TokenTag::Semicolon]) {
            self.skip_soft_newlines();
            return;
        }
        if self.matches(&[TokenTag::Newline]) {
            self.skip_separators();
            return;
        }
        if self.check(TokenTag::RightBrace) || self.check(TokenTag::Eof) {
            return;
        }
        self.diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::ExpectedStatementTerminator,
                "expected a newline or ';' after the statement",
                self.current().span,
            )
            .with_help("Put each statement on its own line or separate statements with ';'."),
        );
    }

    fn consume(
        &mut self,
        tag: TokenTag,
        message: impl Into<String>,
        help: impl Into<String>,
    ) -> Result<&'tokens Token, ()> {
        if self.check(tag) {
            return Ok(self.advance());
        }
        self.diagnostics.push(
            Diagnostic::error(DiagnosticCode::ExpectedToken, message, self.current().span)
                .with_help(help),
        );
        Err(())
    }

    fn consume_closing(
        &mut self,
        tag: TokenTag,
        opening: Span,
        description: &str,
    ) -> Result<&'tokens Token, ()> {
        if self.check(tag) {
            return Ok(self.advance());
        }
        self.diagnostics.push(
            Diagnostic::error(
                DiagnosticCode::UnclosedDelimiter,
                format!("unterminated {description}"),
                self.current().span,
            )
            .with_related(opening, "opening delimiter is here")
            .with_help("Add the matching closing delimiter."),
        );
        Err(())
    }

    fn at_statement_end(&self) -> bool {
        matches!(
            self.current().kind.tag(),
            TokenTag::Newline | TokenTag::Semicolon | TokenTag::RightBrace | TokenTag::Eof
        )
    }

    fn skip_soft_newlines(&mut self) {
        while self.matches(&[TokenTag::Newline]) {}
    }

    fn skip_separators(&mut self) {
        while self.matches(&[TokenTag::Newline, TokenTag::Semicolon]) {}
    }

    fn synchronize(&mut self) {
        while !self.check(TokenTag::Eof) {
            if self.current > 0
                && matches!(
                    self.previous().kind.tag(),
                    TokenTag::Newline | TokenTag::Semicolon
                )
            {
                return;
            }
            if matches!(
                self.current().kind.tag(),
                TokenTag::Let
                    | TokenTag::Var
                    | TokenTag::Fn
                    | TokenTag::If
                    | TokenTag::While
                    | TokenTag::For
                    | TokenTag::Return
                    | TokenTag::Command
                    | TokenTag::Import
                    | TokenTag::Export
                    | TokenTag::RightBrace
            ) {
                return;
            }
            self.advance();
        }
    }

    fn matches(&mut self, tags: &[TokenTag]) -> bool {
        if tags.iter().any(|tag| self.check(*tag)) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, tag: TokenTag) -> bool {
        self.current().kind.tag() == tag
    }

    fn advance(&mut self) -> &'tokens Token {
        let token = self.current();
        if token.kind.tag() != TokenTag::Eof {
            self.current += 1;
        }
        token
    }

    fn current(&self) -> &'tokens Token {
        // The scanner always appends EOF. This fallback also makes the parser
        // robust when embedded callers construct a truncated token array.
        self.tokens
            .get(self.current)
            .or_else(|| self.tokens.last())
            .expect("parser requires at least an EOF token")
    }

    fn previous(&self) -> &'tokens Token {
        &self.tokens[self.current.saturating_sub(1)]
    }
}

fn identifier_parts(token: &Token) -> Option<(String, Span)> {
    match &token.kind {
        TokenKind::Identifier(name) => Some((name.clone(), token.span)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn error_codes(output: &FrontendOutput) -> Vec<DiagnosticCode> {
        output.diagnostics.iter().map(|item| item.code).collect()
    }

    #[test]
    fn scanner_tracks_utf8_bytes_and_scalar_columns() {
        let scanned = scan("ʃa = \"t͡ʃ\"\n");
        assert!(scanned.diagnostics.is_empty(), "{:?}", scanned.diagnostics);
        let first = &scanned.tokens[0];
        assert_eq!(first.lexeme, "ʃa");
        assert_eq!(first.span.start.byte, 0);
        assert_eq!(first.span.end.byte, "ʃa".len());
        assert_eq!(first.span.start.column, 1);
        assert_eq!(first.span.end.column, 3);
        let text = scanned
            .tokens
            .iter()
            .find(|token| token.kind.tag() == TokenTag::Text)
            .expect("text token");
        assert_eq!(text.kind, TokenKind::Text("t͡ʃ".to_owned()));
        assert_eq!(text.span.start.line, 1);
    }

    #[test]
    fn scanner_keeps_combining_marks_with_unicode_identifiers() {
        let scanned = scan("t͡ʃ = ã\n");
        assert!(scanned.diagnostics.is_empty(), "{:?}", scanned.diagnostics);
        assert_eq!(
            scanned.tokens[0].kind,
            TokenKind::Identifier("t͡ʃ".to_owned())
        );
        assert_eq!(
            scanned.tokens[2].kind,
            TokenKind::Identifier("ã".to_owned())
        );
    }

    #[test]
    fn scanner_emits_logical_newlines_for_lf_crlf_and_cr() {
        let scanned = scan("a\nb\r\nc\rd");
        let newlines: Vec<_> = scanned
            .tokens
            .iter()
            .filter(|token| token.kind.tag() == TokenTag::Newline)
            .collect();
        assert_eq!(newlines.len(), 3);
        assert_eq!(newlines[1].lexeme, "\r\n");
        assert_eq!(scanned.tokens.last().unwrap().kind.tag(), TokenTag::Eof);
        assert_eq!(scanned.tokens.last().unwrap().span.start.line, 4);
    }

    #[test]
    fn scanner_handles_nested_comments_without_losing_statement_boundaries() {
        let scanned = scan("let a = 1 /* outer\n /* inner */ done */\nlet b = 2 # note\n");
        assert!(scanned.diagnostics.is_empty(), "{:?}", scanned.diagnostics);
        assert_eq!(
            scanned
                .tokens
                .iter()
                .filter(|token| token.kind.tag() == TokenTag::Newline)
                .count(),
            3
        );
        assert!(!scanned.tokens.iter().any(|token| token.lexeme == "outer"));
    }

    #[test]
    fn scanner_decodes_strings_and_unicode_escapes() {
        let scanned = scan(r#""line\n\t\u{2598}\\\"" 'ə\''"#);
        assert!(scanned.diagnostics.is_empty(), "{:?}", scanned.diagnostics);
        assert_eq!(
            scanned.tokens[0].kind,
            TokenKind::Text("line\n\t▘\\\"".to_owned())
        );
        assert_eq!(scanned.tokens[1].kind, TokenKind::Text("ə'".to_owned()));
    }

    #[test]
    fn multiline_string_counts_crlf_as_one_source_line() {
        let scanned = scan("\"a\r\nb\"\nnext");
        assert!(scanned.diagnostics.is_empty(), "{:?}", scanned.diagnostics);
        assert_eq!(scanned.tokens[0].kind, TokenKind::Text("a\r\nb".to_owned()));
        let next = scanned
            .tokens
            .iter()
            .find(|token| token.lexeme == "next")
            .expect("identifier after string");
        assert_eq!(next.span.start.line, 3);
    }

    #[test]
    fn scanner_reports_structured_lexical_errors() {
        let scanned = scan("\"bad\\q\" /* never closed");
        let codes: Vec<_> = scanned.diagnostics.iter().map(|item| item.code).collect();
        assert_eq!(
            codes,
            vec![
                DiagnosticCode::InvalidEscape,
                DiagnosticCode::UnterminatedBlockComment
            ]
        );
        assert!(scanned.diagnostics.iter().all(|item| item.help.is_some()));
    }

    #[test]
    fn scanner_recognizes_module_keywords() {
        let scanned =
            scan("import { value as local } from \"./core.phont\"\nexport let result = local\n");
        assert!(scanned.diagnostics.is_empty(), "{:?}", scanned.diagnostics);
        let tags: Vec<_> = scanned
            .tokens
            .iter()
            .map(|token| token.kind.tag())
            .collect();
        assert!(tags.contains(&TokenTag::Import));
        assert!(tags.contains(&TokenTag::As));
        assert!(tags.contains(&TokenTag::From));
        assert!(tags.contains(&TokenTag::Export));
    }

    #[test]
    fn parser_preserves_selective_import_alias_and_export_spans() {
        let source = r#"import { solve, compare as cmp } from "./core.phont"
export let answer = 42
export fn twice(x) { return x + x }
"#;
        let output = parse(source);
        assert!(!output.has_errors(), "{:?}", output.diagnostics);
        assert_eq!(output.program.statements.len(), 3);

        let StatementKind::Import {
            import_span,
            path,
            path_span,
            bindings,
        } = &output.program.statements[0].kind
        else {
            panic!("selective import expected");
        };
        assert_eq!(
            &source[import_span.start.byte..import_span.end.byte],
            "import"
        );
        assert_eq!(path, "./core.phont");
        assert_eq!(
            &source[path_span.start.byte..path_span.end.byte],
            "\"./core.phont\""
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].imported, "solve");
        assert_eq!(bindings[0].local, "solve");
        assert_eq!(bindings[1].imported, "compare");
        assert_eq!(bindings[1].local, "cmp");
        assert_eq!(
            &source[bindings[1].local_span.start.byte..bindings[1].local_span.end.byte],
            "cmp"
        );

        let StatementKind::Binding {
            exported,
            export_span,
            mutable,
            ..
        } = &output.program.statements[1].kind
        else {
            panic!("exported let expected");
        };
        assert!(*exported);
        assert!(!mutable);
        assert_eq!(
            &source[export_span.unwrap().start.byte..export_span.unwrap().end.byte],
            "export"
        );
        let StatementKind::Function {
            exported,
            export_span,
            ..
        } = &output.program.statements[2].kind
        else {
            panic!("exported function expected");
        };
        assert!(*exported);
        assert_eq!(
            &source[export_span.unwrap().start.byte..export_span.unwrap().end.byte],
            "export"
        );
    }

    #[test]
    fn parser_rejects_empty_malformed_and_mutable_exports() {
        for source in [
            "import {} from \"./core.phont\"\n",
            "import { value } from core\n",
            "export var value = 1\n",
            "export command evaluate\n",
        ] {
            let output = parse(source);
            assert!(output.has_errors(), "source unexpectedly parsed: {source}");
        }
        let mutable = parse("export var value = 1\n");
        assert!(error_codes(&mutable).contains(&DiagnosticCode::InvalidModuleDeclaration));
        let empty = parse("import {} from \"./core.phont\"\n");
        assert!(error_codes(&empty).contains(&DiagnosticCode::InvalidModuleDeclaration));
    }

    #[test]
    fn numeric_literals_are_exact_and_preserve_notation() {
        let scanned = scan("123456789012345678901234567890 6/8 0.125 12.5e-2 .75");
        assert!(scanned.diagnostics.is_empty(), "{:?}", scanned.diagnostics);
        let values: Vec<_> = scanned
            .tokens
            .iter()
            .filter_map(|token| match &token.kind {
                TokenKind::Number(number) => Some(number.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(values.len(), 5);
        assert_eq!(
            values[1].exact_value(),
            BigRational::new(BigInt::from(3), BigInt::from(4))
        );
        assert_eq!(
            values[2].exact_value(),
            BigRational::new(BigInt::from(1), BigInt::from(8))
        );
        assert_eq!(
            values[3].exact_value(),
            BigRational::new(BigInt::from(1), BigInt::from(8))
        );
        assert_eq!(
            values[4].exact_value(),
            BigRational::new(BigInt::from(3), BigInt::from(4))
        );
    }

    #[test]
    fn zero_denominator_is_a_lexical_refusal() {
        let scanned = scan("1/0");
        assert_eq!(scanned.diagnostics.len(), 1);
        assert_eq!(scanned.diagnostics[0].code, DiagnosticCode::InvalidNumber);
        assert_eq!(scanned.tokens[0].kind, TokenKind::Error);
    }

    #[test]
    fn decimal_exponent_has_an_explicit_allocation_bound() {
        let accepted = scan("1e10000");
        assert!(accepted.diagnostics.is_empty());
        let refused = scan("1e10001");
        assert_eq!(refused.diagnostics[0].code, DiagnosticCode::InvalidNumber);
        assert_eq!(refused.tokens[0].kind, TokenKind::Error);
    }

    #[test]
    fn parser_respects_arithmetic_and_logical_precedence() {
        let output = parse("let result = 1 + 2 * 3 == 7 and not false\n");
        assert!(!output.has_errors(), "{:?}", output.diagnostics);
        let StatementKind::Binding {
            initializer: Some(expression),
            ..
        } = &output.program.statements[0].kind
        else {
            panic!("expected initialized binding");
        };
        let ExpressionKind::Binary {
            operator: BinaryOperator::And,
            left,
            right,
        } = &expression.kind
        else {
            panic!("expected logical and at root: {expression:?}");
        };
        assert!(matches!(
            left.kind,
            ExpressionKind::Binary {
                operator: BinaryOperator::Equal,
                ..
            }
        ));
        assert!(matches!(
            right.kind,
            ExpressionKind::Unary {
                operator: UnaryOperator::Not,
                ..
            }
        ));
    }

    #[test]
    fn parser_accepts_lexical_state_control_flow_functions_and_calls() {
        let source = r#"
let constraints = ["MAX", "DEP", "IDENT"]
var seen = 0
fn count(items) {
    var total = 0
    for item in items {
        total = total + 1
    }
    return total
}
if count(constraints) == 3 {
    while seen < 1 {
        seen = seen + 1
    }
} else {
    seen = -1
}
"#;
        let output = parse(source);
        assert!(!output.has_errors(), "{:?}", output.diagnostics);
        assert_eq!(output.program.statements.len(), 4);
        assert!(matches!(
            output.program.statements[2].kind,
            StatementKind::Function { .. }
        ));
        assert!(matches!(
            output.program.statements[3].kind,
            StatementKind::If { .. }
        ));
    }

    #[test]
    fn parser_supports_multiline_lists_calls_and_indexing() {
        let output = parse("let selected = choose(\n  [\"a\", \"b\",],\n  weights[0],\n)\n");
        assert!(!output.has_errors(), "{:?}", output.diagnostics);
        let StatementKind::Binding {
            initializer: Some(expression),
            ..
        } = &output.program.statements[0].kind
        else {
            panic!("binding expected");
        };
        assert!(matches!(expression.kind, ExpressionKind::Call { .. }));
    }

    #[test]
    fn parser_supports_chained_record_member_access() {
        let source = "let value = result.statistics.retained_candidates\n";
        let output = parse(source);
        assert!(!output.has_errors(), "{:?}", output.diagnostics);
        let StatementKind::Binding {
            initializer: Some(expression),
            ..
        } = &output.program.statements[0].kind
        else {
            panic!("member binding expected");
        };
        let ExpressionKind::Member {
            object,
            field,
            field_span,
        } = &expression.kind
        else {
            panic!("outer member expected: {expression:?}");
        };
        assert_eq!(field, "retained_candidates");
        assert_eq!(
            &source[field_span.start.byte..field_span.end.byte],
            "retained_candidates"
        );
        assert!(matches!(object.kind, ExpressionKind::Member { .. }));

        let malformed = parse("let value = result.\nlet later = 1\n");
        assert!(malformed.has_errors());
        assert!(malformed.program.statements.iter().any(|statement| {
            matches!(
                &statement.kind,
                StatementKind::Binding { name, .. } if name == "later"
            )
        }));
    }

    #[test]
    fn scanner_and_parser_support_null_and_source_ordered_record_literals() {
        let source = "let r = {\n  σ: null,\n  \"text key\": 1/3,\n  nested: { ok: true },\n}\n";
        let output = parse(source);
        assert!(!output.has_errors(), "{:?}", output.diagnostics);
        assert!(
            output
                .tokens
                .iter()
                .any(|token| token.kind.tag() == TokenTag::Null && token.lexeme == "null")
        );
        let StatementKind::Binding {
            initializer: Some(expression),
            ..
        } = &output.program.statements[0].kind
        else {
            panic!("record binding expected");
        };
        let ExpressionKind::Record(entries) = &expression.kind else {
            panic!("record literal expected: {expression:?}");
        };
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["σ", "text key", "nested"]
        );
        assert_eq!(
            &source[entries[0].key_span.start.byte..entries[0].key_span.end.byte],
            "σ"
        );
        assert_eq!(entries[0].key_span.start.column, 3);
        assert_eq!(entries[0].key_span.end.column, 4);
        assert!(matches!(
            entries[0].value.kind,
            ExpressionKind::Literal(Literal::Null)
        ));
        assert!(matches!(entries[2].value.kind, ExpressionKind::Record(_)));
    }

    #[test]
    fn malformed_records_keep_structured_diagnostics_and_statement_recovery() {
        let missing_colon = parse("let bad = { key 1 }\nlet later = null\n");
        let diagnostic = missing_colon
            .diagnostics
            .iter()
            .find(|item| item.code == DiagnosticCode::ExpectedToken)
            .expect("missing colon diagnostic");
        assert!(diagnostic.message.contains("`:`"));
        assert_eq!(diagnostic.related.len(), 1);
        assert!(missing_colon.program.statements.iter().any(|statement| {
            matches!(
                &statement.kind,
                StatementKind::Binding { name, .. } if name == "later"
            )
        }));

        let unclosed = parse("let bad = { key: null\n");
        let diagnostic = unclosed
            .diagnostics
            .iter()
            .find(|item| item.code == DiagnosticCode::UnclosedDelimiter)
            .expect("unclosed record diagnostic");
        assert_eq!(diagnostic.related.len(), 1);
        assert_eq!(diagnostic.related[0].message, "opening delimiter is here");
    }

    #[test]
    fn assignment_is_right_associative() {
        let output = parse("var a\nvar b\na = b = 2\n");
        assert!(!output.has_errors(), "{:?}", output.diagnostics);
        let StatementKind::Expression(expression) = &output.program.statements[2].kind else {
            panic!("expression statement expected");
        };
        let ExpressionKind::Assignment { value, .. } = &expression.kind else {
            panic!("outer assignment expected");
        };
        assert!(matches!(value.kind, ExpressionKind::Assignment { .. }));
    }

    #[test]
    fn parser_rejects_invalid_assignment_targets() {
        let output = parse("(a + b) = 4\n");
        assert!(error_codes(&output).contains(&DiagnosticCode::InvalidAssignmentTarget));
    }

    #[test]
    fn command_bridge_preserves_typed_tokens_and_source_spelling() {
        let output = parse("command tableau add \"Final devoicing\" 3/4\n");
        assert!(!output.has_errors(), "{:?}", output.diagnostics);
        let StatementKind::Command(tokens) = &output.program.statements[0].kind else {
            panic!("command statement expected");
        };
        assert_eq!(tokens[0].lexeme, "tableau");
        assert_eq!(
            tokens[2].kind,
            TokenKind::Text("Final devoicing".to_owned())
        );
        assert!(matches!(tokens[3].kind, TokenKind::Number(_)));
    }

    #[test]
    fn parser_reports_multiple_errors_and_recovers_at_statement_boundaries() {
        let output = parse("let = 1\nvar good = 2\nreturn )\nvar later = 3\n");
        assert!(output.has_errors());
        assert!(output.diagnostics.len() >= 2, "{:?}", output.diagnostics);
        assert!(output.program.statements.iter().any(|statement| {
            matches!(
                &statement.kind,
                StatementKind::Binding { name, .. } if name == "good"
            )
        }));
        assert!(output.program.statements.iter().any(|statement| {
            matches!(
                &statement.kind,
                StatementKind::Binding { name, .. } if name == "later"
            )
        }));
    }

    #[test]
    fn unclosed_delimiters_report_opening_and_repair() {
        let output = parse("fn evaluate(x) {\n  return [x, 1\n");
        let diagnostic = output
            .diagnostics
            .iter()
            .find(|item| item.code == DiagnosticCode::UnclosedDelimiter)
            .expect("unclosed delimiter diagnostic");
        assert!(!diagnostic.related.is_empty());
        assert!(diagnostic.help.is_some());
    }

    proptest! {
        #[test]
        fn scanner_never_panics_and_spans_stay_on_utf8_boundaries(source in any::<String>()) {
            let output = scan(&source);
            let mut previous_end = 0_usize;
            for token in &output.tokens {
                prop_assert!(token.span.start.byte <= token.span.end.byte);
                prop_assert!(token.span.start.byte >= previous_end);
                prop_assert!(token.span.end.byte <= source.len());
                prop_assert!(source.is_char_boundary(token.span.start.byte));
                prop_assert!(source.is_char_boundary(token.span.end.byte));
                prop_assert_eq!(
                    token.lexeme.as_str(),
                    &source[token.span.start.byte..token.span.end.byte]
                );
                previous_end = token.span.end.byte;
            }
        }

        #[test]
        fn exact_rational_scanning_normalizes_without_rounding(
            numerator in 0_u64..1_000_000,
            denominator in 1_u64..1_000_000,
        ) {
            let source = format!("{numerator}/{denominator}");
            let output = scan(&source);
            prop_assert!(output.diagnostics.is_empty());
            let TokenKind::Number(number) = &output.tokens[0].kind else {
                return Err(TestCaseError::fail("numeric token expected"));
            };
            prop_assert_eq!(
                number.exact_value(),
                BigRational::new(BigInt::from(numerator), BigInt::from(denominator))
            );
        }

        #[test]
        fn parser_never_panics_on_arbitrary_unicode(source in any::<String>()) {
            let _ = parse(&source);
        }
    }
}
