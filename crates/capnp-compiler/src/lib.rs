//! Bounded, lossless Cap'n Proto schema lexing.
//!
//! M22 follows the token and statement grammar in the lexer from pinned C++
//! commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`: identifiers, decoded string
//! and binary literals, numbers, operators, comma-delimited parenthesized and
//! bracketed token lists, semicolon statements, brace blocks, UTF-8 BOMs, and
//! post-declaration documentation comments. Every syntax node retains its
//! exact byte range and the tree owns the original source, so formatting and
//! comments are recoverable without re-encoding decoded values.
//!
//! Resource limits are checked before scanning and before growing bounded
//! collections. Nesting is capped before recursive descent. Invalid input
//! produces ranged diagnostics and recovery nodes instead of panicking.
//! Name resolution, schema IDs, layout, and semantic validation are M23-M25.

use std::fmt;
use std::sync::Arc;

/// A half-open UTF-8 byte range in the original source.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SourceRange {
    pub start: u32,
    pub end: u32,
}

impl SourceRange {
    fn from_usize(start: usize, end: usize) -> Self {
        Self {
            start: u32::try_from(start).unwrap_or(u32::MAX),
            end: u32::try_from(end).unwrap_or(u32::MAX),
        }
    }
}

/// A non-token source region retained by the lossless tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trivia {
    pub kind: TriviaKind,
    pub range: SourceRange,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TriviaKind {
    Whitespace,
    Comment,
    Utf8Bom,
}

/// One decoded token with its exact source extent.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Identifier(String),
    StringLiteral(String),
    BinaryLiteral(Vec<u8>),
    IntegerLiteral(u64),
    FloatLiteral(f64),
    Operator(String),
    Parenthesized(Vec<TokenSequence>),
    Bracketed(Vec<TokenSequence>),
    Invalid(String),
}

/// One comma-delimited item inside parentheses or brackets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenSequence {
    pub tokens: Vec<Token>,
}

/// A semicolon declaration or brace-delimited declaration block.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub tokens: Vec<Token>,
    pub body: StatementBody,
    pub doc_comment: Option<String>,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatementBody {
    Line,
    Block(Vec<Statement>),
    MissingTerminator,
}

/// A recoverable lexical or statement-structure problem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub range: SourceRange,
    pub message: String,
}

/// Hard resource bounds for parsing untrusted schemas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseLimits {
    pub max_source_bytes: usize,
    pub max_tokens: usize,
    pub max_statements: usize,
    pub max_nesting: usize,
    pub max_diagnostics: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 16 * 1024 * 1024,
            max_tokens: 1_000_000,
            max_statements: 250_000,
            max_nesting: 64,
            max_diagnostics: 128,
        }
    }
}

/// Deterministic work counters used by complexity regression tests.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ParseStats {
    pub advanced_bytes: usize,
    pub tokens: usize,
    pub statements: usize,
    pub maximum_nesting: usize,
}

/// Owned lossless syntax and any diagnostics recovered while producing it.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxTree {
    source: Arc<str>,
    pub statements: Vec<Statement>,
    pub trivia: Vec<Trivia>,
    pub diagnostics: Vec<Diagnostic>,
    pub stats: ParseStats,
    had_errors: bool,
}

impl SyntaxTree {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn source_text(&self, range: SourceRange) -> Option<&str> {
        self.source
            .get(usize::try_from(range.start).ok()?..usize::try_from(range.end).ok()?)
    }

    pub fn is_valid(&self) -> bool {
        !self.had_errors
    }
}

/// UTF-8 validation failure before lexical parsing begins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidUtf8 {
    pub valid_up_to: usize,
    pub error_len: Option<usize>,
}

impl fmt::Display for InvalidUtf8 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "schema source is not UTF-8 at byte {}",
            self.valid_up_to
        )
    }
}

impl std::error::Error for InvalidUtf8 {}

/// Parses UTF-8 bytes after validating the language's required encoding.
pub fn parse_schema_bytes(source: &[u8], limits: ParseLimits) -> Result<SyntaxTree, InvalidUtf8> {
    let source = std::str::from_utf8(source).map_err(|error| InvalidUtf8 {
        valid_up_to: error.valid_up_to(),
        error_len: error.error_len(),
    })?;
    Ok(parse_schema(Arc::<str>::from(source), limits))
}

/// Parses a schema into an owned lossless statement/token tree.
pub fn parse_schema(source: Arc<str>, limits: ParseLimits) -> SyntaxTree {
    Parser::new(source, limits).parse()
}

struct Parser {
    source: Arc<str>,
    cursor: usize,
    limits: ParseLimits,
    trivia: Vec<Trivia>,
    diagnostics: Vec<Diagnostic>,
    stats: ParseStats,
    exhausted: bool,
    had_errors: bool,
}

impl Parser {
    fn new(source: Arc<str>, limits: ParseLimits) -> Self {
        Self {
            source,
            cursor: 0,
            limits,
            trivia: Vec::new(),
            diagnostics: Vec::new(),
            stats: ParseStats::default(),
            exhausted: false,
            had_errors: false,
        }
    }

    fn parse(mut self) -> SyntaxTree {
        if self.source.len() > self.limits.max_source_bytes
            || self.source.len() > usize::try_from(u32::MAX).unwrap_or(usize::MAX)
        {
            self.diagnostic(
                0,
                self.source
                    .len()
                    .min(usize::try_from(u32::MAX).unwrap_or(usize::MAX)),
                "schema source exceeds the configured byte limit",
            );
            return SyntaxTree {
                source: self.source,
                statements: Vec::new(),
                trivia: self.trivia,
                diagnostics: self.diagnostics,
                stats: self.stats,
                had_errors: self.had_errors,
            };
        }
        self.skip_trivia();
        let statements = self.parse_statements(None, 0);
        SyntaxTree {
            source: self.source,
            statements,
            trivia: self.trivia,
            diagnostics: self.diagnostics,
            stats: self.stats,
            had_errors: self.had_errors,
        }
    }

    fn parse_statements(&mut self, closing: Option<u8>, depth: usize) -> Vec<Statement> {
        self.stats.maximum_nesting = self.stats.maximum_nesting.max(depth);
        let mut statements = Vec::new();
        loop {
            self.skip_trivia();
            if self.at_end() {
                if closing.is_some() {
                    self.diagnostic(self.cursor, self.cursor, "missing closing `}`");
                }
                break;
            }
            if self.peek() == closing {
                break;
            }
            if self.peek() == Some(b'}') {
                self.diagnostic(self.cursor, self.cursor + 1, "unmatched closing `}`");
                self.advance(1);
                continue;
            }
            if statements.len() >= self.limits.max_statements || self.exhausted {
                self.limit_diagnostic("statement limit exceeded");
                break;
            }
            let before = self.cursor;
            statements.push(self.parse_statement(depth));
            self.stats.statements += 1;
            if self.cursor == before {
                self.advance_char();
            }
        }
        statements
    }

    fn parse_statement(&mut self, depth: usize) -> Statement {
        let start = self.cursor;
        let tokens = self.parse_token_sequence(b";{}", depth);
        match self.peek() {
            Some(b';') => {
                self.advance(1);
                let doc_comment = self.take_doc_comment();
                Statement {
                    tokens,
                    body: StatementBody::Line,
                    doc_comment,
                    range: SourceRange::from_usize(start, self.cursor),
                }
            }
            Some(b'{') => {
                if depth >= self.limits.max_nesting {
                    self.diagnostic(self.cursor, self.cursor + 1, "nesting limit exceeded");
                    self.recover_balanced_braces();
                    return Statement {
                        tokens,
                        body: StatementBody::MissingTerminator,
                        doc_comment: None,
                        range: SourceRange::from_usize(start, self.cursor),
                    };
                }
                self.advance(1);
                let early_comment = self.take_doc_comment();
                let body = self.parse_statements(Some(b'}'), depth + 1);
                if self.peek() == Some(b'}') {
                    self.advance(1);
                }
                let late_comment = self.take_doc_comment();
                Statement {
                    tokens,
                    body: StatementBody::Block(body),
                    doc_comment: early_comment.or(late_comment),
                    range: SourceRange::from_usize(start, self.cursor),
                }
            }
            _ => {
                self.diagnostic(start, self.cursor, "statement must end with `;` or a block");
                Statement {
                    tokens,
                    body: StatementBody::MissingTerminator,
                    doc_comment: None,
                    range: SourceRange::from_usize(start, self.cursor),
                }
            }
        }
    }

    fn parse_token_sequence(&mut self, stops: &[u8], depth: usize) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia();
            if self.at_end() || self.peek().is_some_and(|value| stops.contains(&value)) {
                break;
            }
            if self.stats.tokens >= self.limits.max_tokens || self.exhausted {
                self.limit_diagnostic("token limit exceeded");
                break;
            }
            let before = self.cursor;
            if let Some(token) = self.parse_token(depth) {
                tokens.push(token);
                self.stats.tokens += 1;
            }
            if self.cursor == before {
                self.advance_char();
            }
        }
        tokens
    }

    fn parse_token(&mut self, depth: usize) -> Option<Token> {
        let start = self.cursor;
        let byte = self.peek()?;
        let kind = if is_identifier_start(byte) {
            self.advance(1);
            while self.peek().is_some_and(is_identifier_continue) {
                self.advance(1);
            }
            TokenKind::Identifier(self.source[start..self.cursor].to_owned())
        } else if byte == b'"' {
            TokenKind::StringLiteral(self.parse_string())
        } else if byte == b'`' {
            self.advance(1);
            let content = self.cursor;
            while self
                .peek()
                .is_some_and(|value| value != b'\r' && value != b'\n')
            {
                self.advance_char();
            }
            let mut value = self.source[content..self.cursor].to_owned();
            value.push('\n');
            TokenKind::StringLiteral(value)
        } else if self.starts_with("0x\"") {
            TokenKind::BinaryLiteral(self.parse_binary())
        } else if byte.is_ascii_digit() {
            self.parse_number()
        } else if is_operator(byte) {
            self.advance(1);
            while self.peek().is_some_and(is_operator) {
                self.advance(1);
            }
            TokenKind::Operator(self.source[start..self.cursor].to_owned())
        } else if byte == b'(' || byte == b'[' {
            let close = if byte == b'(' { b')' } else { b']' };
            if depth >= self.limits.max_nesting {
                self.diagnostic(start, start + 1, "nesting limit exceeded");
                self.advance(1);
                TokenKind::Invalid(self.source[start..self.cursor].to_owned())
            } else {
                self.advance(1);
                let items = self.parse_list(close, depth + 1);
                if self.peek() == Some(close) {
                    self.advance(1);
                } else {
                    self.diagnostic(start, self.cursor, "missing list closing delimiter");
                }
                if byte == b'(' {
                    TokenKind::Parenthesized(items)
                } else {
                    TokenKind::Bracketed(items)
                }
            }
        } else {
            self.advance_char();
            self.diagnostic(start, self.cursor, "unexpected character");
            TokenKind::Invalid(self.source[start..self.cursor].to_owned())
        };
        Some(Token {
            kind,
            range: SourceRange::from_usize(start, self.cursor),
        })
    }

    fn parse_list(&mut self, close: u8, depth: usize) -> Vec<TokenSequence> {
        self.stats.maximum_nesting = self.stats.maximum_nesting.max(depth);
        let mut items = Vec::new();
        self.skip_trivia();
        if self.peek() == Some(close) {
            return items;
        }
        loop {
            let tokens = self.parse_token_sequence(&[b',', close, b';', b'{', b'}'], depth);
            items.push(TokenSequence { tokens });
            self.skip_trivia();
            if self.peek() == Some(b',') {
                self.advance(1);
                self.skip_trivia();
                if self.peek() == Some(close) {
                    break;
                }
                continue;
            }
            break;
        }
        items
    }

    fn parse_string(&mut self) -> String {
        let start = self.cursor;
        self.advance(1);
        let mut output = String::new();
        let mut chunk = self.cursor;
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    output.push_str(&self.source[chunk..self.cursor]);
                    self.advance(1);
                    return output;
                }
                b'\\' => {
                    output.push_str(&self.source[chunk..self.cursor]);
                    self.advance(1);
                    let escape_start = self.cursor;
                    let Some(escape) = self.peek() else {
                        break;
                    };
                    self.advance(1);
                    match escape {
                        b'a' => output.push('\u{7}'),
                        b'b' => output.push('\u{8}'),
                        b'f' => output.push('\u{c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'v' => output.push('\u{b}'),
                        b'\\' => output.push('\\'),
                        b'\'' => output.push('\''),
                        b'"' => output.push('"'),
                        b'x' => {
                            let digits = self.take_hex_digits(2);
                            if digits.len() == 2 {
                                if let Ok(value) = u8::from_str_radix(digits, 16) {
                                    output.push(char::from(value));
                                }
                            } else {
                                self.diagnostic(
                                    escape_start,
                                    self.cursor,
                                    "hex escape requires two digits",
                                );
                            }
                        }
                        _ => {
                            self.diagnostic(escape_start, self.cursor, "unknown string escape");
                            output.push(char::from(escape));
                        }
                    }
                    chunk = self.cursor;
                }
                b'\r' | b'\n' => {
                    output.push_str(&self.source[chunk..self.cursor]);
                    self.diagnostic(start, self.cursor, "unterminated string literal");
                    return output;
                }
                _ => self.advance_char(),
            }
        }
        output.push_str(&self.source[chunk..self.cursor]);
        self.diagnostic(start, self.cursor, "unterminated string literal");
        output
    }

    fn parse_binary(&mut self) -> Vec<u8> {
        let start = self.cursor;
        self.advance(3);
        let mut digits = String::new();
        let mut closed = false;
        while let Some(byte) = self.peek() {
            if byte == b'"' {
                self.advance(1);
                closed = true;
                break;
            }
            if byte.is_ascii_hexdigit() {
                digits.push(char::from(byte));
                self.advance(1);
            } else if byte.is_ascii_whitespace() {
                self.advance(1);
            } else {
                self.diagnostic(self.cursor, self.cursor + 1, "invalid binary literal digit");
                self.advance_char();
            }
        }
        if !closed {
            self.diagnostic(start, self.cursor, "unterminated binary literal");
        }
        if digits.len() % 2 != 0 {
            self.diagnostic(
                start,
                self.cursor,
                "binary literal needs pairs of hex digits",
            );
        }
        digits
            .as_bytes()
            .chunks_exact(2)
            .filter_map(|pair| std::str::from_utf8(pair).ok())
            .filter_map(|pair| u8::from_str_radix(pair, 16).ok())
            .collect()
    }

    fn parse_number(&mut self) -> TokenKind {
        let start = self.cursor;
        self.advance(1);
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_'))
        {
            self.advance(1);
        }
        let raw = self.source[start..self.cursor].to_owned();
        let cleaned = raw.replace('_', "");
        let is_hex = cleaned.starts_with("0x") || cleaned.starts_with("0X");
        let is_float = !is_hex && cleaned.contains(['.', 'e', 'E']);
        if is_float {
            match cleaned.parse::<f64>() {
                Ok(value) => TokenKind::FloatLiteral(value),
                Err(_) => {
                    self.diagnostic(start, self.cursor, "invalid floating-point literal");
                    TokenKind::Invalid(raw)
                }
            }
        } else {
            let parsed = cleaned
                .strip_prefix("0x")
                .or_else(|| cleaned.strip_prefix("0X"))
                .map_or_else(
                    || cleaned.parse::<u64>(),
                    |digits| u64::from_str_radix(digits, 16),
                );
            match parsed {
                Ok(value) => TokenKind::IntegerLiteral(value),
                Err(_) => {
                    self.diagnostic(start, self.cursor, "invalid or overflowing integer literal");
                    TokenKind::Invalid(raw)
                }
            }
        }
    }

    fn take_hex_digits(&mut self, count: usize) -> &str {
        let start = self.cursor;
        for _ in 0..count {
            if self.peek().is_some_and(|byte| byte.is_ascii_hexdigit()) {
                self.advance(1);
            } else {
                break;
            }
        }
        &self.source[start..self.cursor]
    }

    fn take_doc_comment(&mut self) -> Option<String> {
        let checkpoint = self.cursor;
        self.skip_horizontal_whitespace();
        if self.peek() == Some(b'\r') || self.peek() == Some(b'\n') {
            self.consume_newline();
        }
        let mut output = String::new();
        let mut any = false;
        loop {
            self.skip_horizontal_whitespace();
            if self.peek() != Some(b'#') {
                break;
            }
            any = true;
            let comment_start = self.cursor;
            self.advance(1);
            if self.peek() == Some(b' ') {
                self.advance(1);
            }
            let text_start = self.cursor;
            while self
                .peek()
                .is_some_and(|byte| byte != b'\r' && byte != b'\n')
            {
                self.advance_char();
            }
            output.push_str(&self.source[text_start..self.cursor]);
            output.push('\n');
            let end = self.cursor;
            self.trivia.push(Trivia {
                kind: TriviaKind::Comment,
                range: SourceRange::from_usize(comment_start, end),
            });
            if self.peek() == Some(b'\r') || self.peek() == Some(b'\n') {
                self.consume_newline();
            } else {
                break;
            }
            let line_checkpoint = self.cursor;
            self.skip_horizontal_whitespace();
            if self.peek() != Some(b'#') {
                self.cursor = line_checkpoint;
                break;
            }
        }
        if any {
            Some(output)
        } else {
            self.cursor = checkpoint;
            None
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            let start = self.cursor;
            if self.starts_with("\u{feff}") {
                self.advance("\u{feff}".len());
                self.trivia.push(Trivia {
                    kind: TriviaKind::Utf8Bom,
                    range: SourceRange::from_usize(start, self.cursor),
                });
            } else if self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                    self.advance(1);
                }
                self.trivia.push(Trivia {
                    kind: TriviaKind::Whitespace,
                    range: SourceRange::from_usize(start, self.cursor),
                });
            } else if self.peek() == Some(b'#') {
                while self
                    .peek()
                    .is_some_and(|byte| byte != b'\r' && byte != b'\n')
                {
                    self.advance_char();
                }
                if self.peek() == Some(b'\r') || self.peek() == Some(b'\n') {
                    self.consume_newline();
                }
                self.trivia.push(Trivia {
                    kind: TriviaKind::Comment,
                    range: SourceRange::from_usize(start, self.cursor),
                });
            } else {
                break;
            }
        }
    }

    fn skip_horizontal_whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | 0x0b | 0x0c))
        {
            self.advance(1);
        }
    }

    fn consume_newline(&mut self) {
        if self.peek() == Some(b'\r') {
            self.advance(1);
            if self.peek() == Some(b'\n') {
                self.advance(1);
            }
        } else if self.peek() == Some(b'\n') {
            self.advance(1);
        }
    }

    fn recover_balanced_braces(&mut self) {
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            self.advance_char();
            match byte {
                b'{' => depth = depth.saturating_add(1),
                b'}' if depth == 0 => break,
                b'}' => depth -= 1,
                _ => {}
            }
        }
    }

    fn diagnostic(&mut self, start: usize, end: usize, message: &str) {
        self.had_errors = true;
        if self.diagnostics.len() < self.limits.max_diagnostics {
            self.diagnostics.push(Diagnostic {
                range: SourceRange::from_usize(start, end),
                message: message.to_owned(),
            });
        }
    }

    fn limit_diagnostic(&mut self, message: &str) {
        if !self.exhausted {
            self.diagnostic(self.cursor, self.cursor, message);
            self.exhausted = true;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.cursor).copied()
    }

    fn starts_with(&self, value: &str) -> bool {
        self.source[self.cursor..].starts_with(value)
    }

    fn at_end(&self) -> bool {
        self.cursor >= self.source.len()
    }

    fn advance(&mut self, bytes: usize) {
        self.cursor = self.cursor.saturating_add(bytes).min(self.source.len());
        self.stats.advanced_bytes = self.stats.advanced_bytes.saturating_add(bytes);
    }

    fn advance_char(&mut self) {
        if let Some(character) = self.source[self.cursor..].chars().next() {
            self.advance(character.len_utf8());
        }
    }
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_operator(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'$'
            | b'%'
            | b'&'
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'/'
            | b':'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'^'
            | b'|'
            | b'~'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMAS: &[(&str, &str)] = &[
        (
            "builder",
            include_str!("../../../conformance/schemas/builder-fixture.capnp"),
        ),
        (
            "evolution-v1",
            include_str!("../../../conformance/schemas/evolution-v1.capnp"),
        ),
        (
            "evolution-v2",
            include_str!("../../../conformance/schemas/evolution-v2.capnp"),
        ),
        (
            "evolution-v3",
            include_str!("../../../conformance/schemas/evolution-v3.capnp"),
        ),
        (
            "imports",
            include_str!("../../../conformance/schemas/import-fixture.capnp"),
        ),
        (
            "language",
            include_str!("../../../conformance/schemas/language-fixture.capnp"),
        ),
        (
            "orphans",
            include_str!("../../../conformance/schemas/orphan-fixture.capnp"),
        ),
        (
            "streaming",
            include_str!("../../../conformance/schemas/streaming-fixture.capnp"),
        ),
        (
            "wire",
            include_str!("../../../conformance/schemas/wire-fixture.capnp"),
        ),
        (
            "schema",
            include_str!(
                "../../../conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/schema.capnp"
            ),
        ),
        (
            "rpc",
            include_str!(
                "../../../conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/rpc.capnp"
            ),
        ),
        (
            "rpc-twoparty",
            include_str!(
                "../../../conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/rpc-twoparty.capnp"
            ),
        ),
        (
            "stream",
            include_str!(
                "../../../conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/stream.capnp"
            ),
        ),
        (
            "persistent",
            include_str!(
                "../../../conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/persistent.capnp"
            ),
        ),
    ];

    fn parse(source: &str) -> SyntaxTree {
        parse_schema(Arc::from(source), ParseLimits::default())
    }

    #[test]
    fn pinned_language_and_product_schemas_parse_losslessly() {
        for &(name, source) in SCHEMAS {
            let tree = parse(source);
            assert!(tree.is_valid(), "{name}: {:?}", tree.diagnostics);
            assert_eq!(tree.source(), source);
            assert!(tree.stats.tokens > 0, "{name}");
            assert!(tree.stats.advanced_bytes <= source.len() * 3 + 3, "{name}");
            check_ranges(&tree, &tree.statements);
        }
    }

    fn check_ranges(tree: &SyntaxTree, statements: &[Statement]) {
        for statement in statements {
            assert!(tree.source_text(statement.range).is_some());
            for token in &statement.tokens {
                check_token_range(tree, token);
            }
            if let StatementBody::Block(children) = &statement.body {
                check_ranges(tree, children);
            }
        }
    }

    fn check_token_range(tree: &SyntaxTree, token: &Token) {
        assert!(tree.source_text(token.range).is_some());
        match &token.kind {
            TokenKind::Parenthesized(items) | TokenKind::Bracketed(items) => {
                for item in items {
                    for token in &item.tokens {
                        check_token_range(tree, token);
                    }
                }
            }
            _ => {}
        }
    }

    #[test]
    fn pinned_lexer_shapes_ranges_literals_lists_bom_and_comments() {
        let tree = parse(
            "\u{feff}foo # ordinary\n bar (baz, [qux,]) 123 2.75 6e4 0x\"00cafeff\" \"x\\x20y\";",
        );
        assert!(tree.is_valid(), "{:?}", tree.diagnostics);
        let statement = &tree.statements[0];
        assert_eq!(statement.tokens.len(), 8);
        assert_eq!(tree.source_text(statement.tokens[0].range), Some("foo"));
        assert_eq!(tree.source_text(statement.tokens[1].range), Some("bar"));
        assert!(matches!(
            statement.tokens[2].kind,
            TokenKind::Parenthesized(_)
        ));
        assert_eq!(statement.tokens[3].kind, TokenKind::IntegerLiteral(123));
        assert_eq!(statement.tokens[4].kind, TokenKind::FloatLiteral(2.75));
        assert_eq!(statement.tokens[5].kind, TokenKind::FloatLiteral(60_000.0));
        assert_eq!(
            statement.tokens[6].kind,
            TokenKind::BinaryLiteral(vec![0, 0xca, 0xfe, 0xff])
        );
        assert_eq!(
            statement.tokens[7].kind,
            TokenKind::StringLiteral("x y".to_owned())
        );
        assert!(
            tree.trivia
                .iter()
                .any(|value| value.kind == TriviaKind::Utf8Bom)
        );
        assert!(
            tree.trivia
                .iter()
                .any(|value| value.kind == TriviaKind::Comment)
        );
    }

    #[test]
    fn upstream_post_declaration_doc_comment_rules_are_preserved() {
        let tree = parse(
            "foo;\n # bar baz\n  # qux corge\n\n# detached\nbar {# early\n baz;} # late ignored\nqux;",
        );
        assert!(tree.is_valid(), "{:?}", tree.diagnostics);
        assert_eq!(
            tree.statements[0].doc_comment.as_deref(),
            Some("bar baz\nqux corge\n")
        );
        assert_eq!(tree.statements[1].doc_comment.as_deref(), Some("early\n"));
        assert!(matches!(tree.statements[1].body, StatementBody::Block(_)));
        if let StatementBody::Block(children) = &tree.statements[1].body {
            assert_eq!(children.len(), 1);
        }
        assert_eq!(tree.statements[2].doc_comment, None);
    }

    #[test]
    fn malformed_input_recovers_with_exact_ranges() {
        let source = "@0xabc; broken [one, two; next @1 :Text; } final";
        let tree = parse(source);
        assert!(!tree.is_valid());
        assert!(tree.statements.len() >= 2);
        assert!(tree.diagnostics.iter().all(|diagnostic| {
            diagnostic.range.start <= diagnostic.range.end
                && usize::try_from(diagnostic.range.end).expect("u32") <= source.len()
        }));
        assert!(
            tree.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("closing"))
        );
        assert_eq!(tree.source(), source);
    }

    #[test]
    fn limits_and_utf8_fail_before_unbounded_work() {
        let limits = ParseLimits {
            max_source_bytes: 3,
            ..ParseLimits::default()
        };
        let tree = parse_schema(Arc::from("four"), limits);
        assert_eq!(tree.stats.advanced_bytes, 0);
        assert_eq!(tree.diagnostics.len(), 1);

        let silent = parse_schema(
            Arc::from("broken"),
            ParseLimits {
                max_diagnostics: 0,
                ..ParseLimits::default()
            },
        );
        assert!(!silent.is_valid());
        assert!(silent.diagnostics.is_empty());

        let error = parse_schema_bytes(&[b'a', 0xff, b'b'], ParseLimits::default())
            .expect_err("invalid UTF-8");
        assert_eq!(error.valid_up_to, 1);

        let nested = format!("{}{};", "(".repeat(100_000), ")".repeat(100_000));
        let tree = parse_schema(
            Arc::from(nested.as_str()),
            ParseLimits {
                max_nesting: 32,
                max_diagnostics: 8,
                ..ParseLimits::default()
            },
        );
        assert!(!tree.is_valid());
        assert!(tree.stats.maximum_nesting <= 32);
    }

    #[test]
    fn deterministic_arbitrary_bytes_never_panic_and_work_stays_linear() {
        let alphabet = b"abcXYZ019_@$()[]{},;# \\ \"\n\r\t\xff";
        let mut state = 0x1234_5678_9abc_def0u64;
        for length in 0..512usize {
            let mut bytes = Vec::with_capacity(length);
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                bytes.push(alphabet[(state as usize) % alphabet.len()]);
            }
            match parse_schema_bytes(&bytes, ParseLimits::default()) {
                Ok(tree) => {
                    assert!(tree.stats.advanced_bytes <= length * 4 + 4);
                    assert!(tree.diagnostics.len() <= ParseLimits::default().max_diagnostics);
                }
                Err(error) => assert!(error.valid_up_to <= length),
            }
        }
    }
}
