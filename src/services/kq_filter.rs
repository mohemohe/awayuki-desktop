// SPDX-License-Identifier: MIT
//
// KQ syntax and compatibility semantics are derived from Krile StarryEyes
// (Copyright (c) 2013 Karno and StarryEyes contributors, MIT License),
// revision a2c4c9b68287c9058d82a15cd28c6615863a626f. This is an idiomatic,
// provider-neutral reimplementation for Awayuki; no Cadena code is copied.

use std::collections::HashSet;
use std::fmt;
use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::db::models::{DbAccount, DbStatus, DbStatusViewerState};

pub(crate) const ENGINE_ID: &str = "kq";

const DEFAULT_MAX_QUERY_BYTES: usize = 32 * 1024;
const DEFAULT_MAX_TOKENS: usize = 4_096;
const DEFAULT_MAX_DEPTH: usize = 64;
const DEFAULT_MAX_SET_ITEMS: usize = 1_024;
const DEFAULT_MAX_SOURCES: usize = 64;
const DEFAULT_MAX_SOURCE_ARGUMENTS: usize = 64;
const DEFAULT_MAX_REGEX_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[allow(dead_code)]
    fn range(self) -> Range<usize> {
        self.start..self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueryLimits {
    pub max_query_bytes: usize,
    pub max_tokens: usize,
    pub max_depth: usize,
    pub max_set_items: usize,
    pub max_sources: usize,
    pub max_source_arguments: usize,
    pub max_regex_bytes: usize,
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            max_query_bytes: DEFAULT_MAX_QUERY_BYTES,
            max_tokens: DEFAULT_MAX_TOKENS,
            max_depth: DEFAULT_MAX_DEPTH,
            max_set_items: DEFAULT_MAX_SET_ITEMS,
            max_sources: DEFAULT_MAX_SOURCES,
            max_source_arguments: DEFAULT_MAX_SOURCE_ARGUMENTS,
            max_regex_bytes: DEFAULT_MAX_REGEX_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompileError {
    message: String,
    span: Span,
    line: usize,
    column: usize,
}

impl CompileError {
    fn at(input: &str, span: Span, message: impl Into<String>) -> Self {
        let mut bounded_start = span.start.min(input.len());
        while !input.is_char_boundary(bounded_start) {
            bounded_start = bounded_start.saturating_sub(1);
        }
        let prefix = &input[..bounded_start];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
        let column = input[line_start..bounded_start].chars().count() + 1;
        let mut bounded_end = span.end.min(input.len());
        while bounded_end > bounded_start && !input.is_char_boundary(bounded_end) {
            bounded_end = bounded_end.saturating_sub(1);
        }
        Self {
            message: message.into(),
            span: Span::new(bounded_start, bounded_end.max(bounded_start)),
            line,
            column,
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    #[allow(dead_code)]
    pub(crate) fn span(&self) -> Range<usize> {
        self.span.range()
    }

    pub(crate) fn offset(&self) -> usize {
        self.span.start
    }

    pub(crate) fn line(&self) -> usize {
        self.line
    }

    pub(crate) fn column(&self) -> usize {
        self.column
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at line {}, column {}",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for CompileError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SourceKind {
    Local,
    Home,
    List,
    Mentions,
    Direct,
    Search,
    Track,
    Conversation,
    User,
    Public,
    LocalPublic,
    Hashtag,
    Bookmarks,
    Favourites,
}

impl SourceKind {
    #[allow(dead_code)]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Home => "home",
            Self::List => "list",
            Self::Mentions => "mentions",
            Self::Direct => "direct",
            Self::Search => "search",
            Self::Track => "track",
            Self::Conversation => "conversation",
            Self::User => "user",
            Self::Public => "public",
            Self::LocalPublic => "local_public",
            Self::Hashtag => "hashtag",
            Self::Bookmarks => "bookmarks",
            Self::Favourites => "favourites",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceSpec {
    pub kind: SourceKind,
    pub arguments: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct StatusKey {
    pub server_domain: String,
    pub id: String,
}

impl StatusKey {
    pub(crate) fn new(server_domain: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            server_domain: server_domain.into(),
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TimelineMembership {
    pub timeline_type: String,
    pub account_acct: String,
    pub parameter: Option<String>,
}

impl TimelineMembership {
    pub(crate) fn new(
        timeline_type: impl Into<String>,
        account_acct: impl Into<String>,
        parameter: Option<String>,
    ) -> Self {
        Self {
            timeline_type: timeline_type.into(),
            account_acct: account_acct.into(),
            parameter,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoginAccountIdentity {
    pub acct: String,
    pub server_domain: String,
    pub account_id: String,
    pub display_name: String,
    pub server_kind: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StatusView<'a> {
    pub status: &'a DbStatus,
    pub account: Option<&'a DbAccount>,
}

impl<'a> StatusView<'a> {
    pub(crate) fn new(status: &'a DbStatus, account: Option<&'a DbAccount>) -> Self {
        Self { status, account }
    }
}

#[derive(Debug)]
pub(crate) struct EvaluationContext<'a> {
    pub wrapper: StatusView<'a>,
    /// The displayed/effective status. For a non-boost this is the wrapper;
    /// for an unresolved boost it must be None (never fall back to wrapper).
    pub effective: Option<StatusView<'a>>,
    pub quote: Option<StatusView<'a>>,
    pub login_accounts: &'a [LoginAccountIdentity],
    pub memberships: &'a [TimelineMembership],
    pub viewer_states: &'a [DbStatusViewerState],
    pub conversation_keys: &'a [StatusKey],
}

impl<'a> EvaluationContext<'a> {
    pub(crate) fn new(wrapper: StatusView<'a>, effective: Option<StatusView<'a>>) -> Self {
        Self {
            wrapper,
            effective,
            quote: None,
            login_accounts: &[],
            memberships: &[],
            viewer_states: &[],
            conversation_keys: &[],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct QueryRequirements {
    pub effective_status: bool,
    pub quote_status: bool,
    pub memberships: bool,
    pub viewer_states: bool,
    pub login_accounts: bool,
    pub conversations: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum SqlPrefilterValue {
    Text(String),
    Integer(i64),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SqlPrefilter {
    clause: String,
    bindings: Vec<SqlPrefilterValue>,
}

impl SqlPrefilter {
    pub(crate) fn clause(&self) -> &str {
        &self.clause
    }

    pub(crate) fn bindings(&self) -> &[SqlPrefilterValue] {
        &self.bindings
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.clause.is_empty()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledQuery {
    sources: Vec<SourceSpec>,
    predicate: Expr,
    sql_prefilter: SqlPrefilter,
    requirements: QueryRequirements,
    conversation_ids: Vec<String>,
}

impl CompiledQuery {
    pub(crate) fn sources(&self) -> &[SourceSpec] {
        &self.sources
    }

    pub(crate) fn sql_prefilter(&self) -> &SqlPrefilter {
        &self.sql_prefilter
    }

    pub(crate) fn requirements(&self) -> QueryRequirements {
        self.requirements
    }

    pub(crate) fn conversation_ids(&self) -> &[String] {
        &self.conversation_ids
    }
}

#[derive(Debug, Default)]
pub(crate) struct Evaluator(super::kq_evaluator::Evaluator);

impl Evaluator {
    pub(crate) fn new() -> Self {
        Self(super::kq_evaluator::Evaluator::new())
    }

    pub(crate) fn matches(
        &mut self,
        query: &CompiledQuery,
        context: &EvaluationContext<'_>,
    ) -> bool {
        self.0.matches(&query.sources, &query.predicate, context)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValueTypes(pub(crate) u8);

impl ValueTypes {
    pub(crate) const BOOL: Self = Self(1);
    pub(crate) const NUMBER: Self = Self(2);
    pub(crate) const TEXT: Self = Self(4);
    pub(crate) const SET: Self = Self(8);
    pub(crate) const IDENTITY: Self = Self(16);

    fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for ValueTypes {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Expr {
    pub(crate) kind: ExprKind,
    pub(crate) types: ValueTypes,
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub(crate) enum ExprKind {
    Bool(bool),
    Number(i64),
    Text(String),
    Identity(String),
    Set(Vec<Expr>),
    Field(Field),
    Unary(UnaryOp, Box<Expr>),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        literal_regex: Option<Regex>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnaryOp {
    Not,
    Negate,
    Caseful,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Or,
    And,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Regex,
    StartsWith,
    EndsWith,
    Contains,
    In,
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Text,
    RawContent,
    Id,
    Uri,
    Url,
    Application,
    DirectMessage,
    Boost,
    InReplyTo,
    ReplyAccountId,
    IsReply,
    Mentions,
    FavouritesCount,
    ReblogsCount,
    RepliesCount,
    Visibility,
    Language,
    SpoilerText,
    HasSpoiler,
    Sensitive,
    HasMedia,
    MediaCount,
    MediaTypes,
    MediaDescriptions,
    HasImage,
    HasVideo,
    HasAudio,
    HasPoll,
    PollId,
    PollExpired,
    PollMultiple,
    PollVotesCount,
    PollVotersCount,
    PollOptionsCount,
    PollOptions,
    PollExpiresAt,
    HasCard,
    HasQuote,
    Edited,
    EditedAt,
    Domain,
    Hashtags,
    OurAccounts,
    IsPublic,
    IsUnlisted,
    IsPrivate,
    AuthorId,
    AuthorUsername,
    AuthorAcct,
    AuthorDisplayName,
    AuthorNote,
    AuthorLocked,
    AuthorBot,
    AuthorFollowers,
    AuthorFollowing,
    AuthorStatuses,
    AuthorDomain,
    BoosterId,
    BoosterUsername,
    BoosterAcct,
    BoosterDisplayName,
    BoosterNote,
    BoosterLocked,
    BoosterBot,
    BoosterFollowers,
    BoosterFollowing,
    BoosterStatuses,
    BoosterDomain,
    QuoteId,
    QuoteUrl,
    QuoteText,
    QuoteAuthorAcct,
    ViewerFavourited,
    ViewerReblogged,
    ViewerMuted,
    ViewerBookmarked,
    ViewerPinned,
}

impl Field {
    fn value_types(self) -> ValueTypes {
        match self {
            Self::Id
            | Self::InReplyTo
            | Self::ReplyAccountId
            | Self::PollId
            | Self::AuthorId
            | Self::BoosterId
            | Self::QuoteId => ValueTypes::IDENTITY,
            Self::DirectMessage
            | Self::Boost
            | Self::IsReply
            | Self::HasSpoiler
            | Self::Sensitive
            | Self::HasMedia
            | Self::HasImage
            | Self::HasVideo
            | Self::HasAudio
            | Self::HasPoll
            | Self::PollExpired
            | Self::PollMultiple
            | Self::HasCard
            | Self::HasQuote
            | Self::Edited
            | Self::IsPublic
            | Self::IsUnlisted
            | Self::IsPrivate
            | Self::AuthorLocked
            | Self::AuthorBot
            | Self::BoosterLocked
            | Self::BoosterBot
            | Self::ViewerFavourited
            | Self::ViewerReblogged
            | Self::ViewerMuted
            | Self::ViewerBookmarked
            | Self::ViewerPinned => ValueTypes::BOOL,
            Self::FavouritesCount
            | Self::ReblogsCount
            | Self::RepliesCount
            | Self::MediaCount
            | Self::PollVotesCount
            | Self::PollVotersCount
            | Self::PollOptionsCount
            | Self::AuthorFollowers
            | Self::AuthorFollowing
            | Self::AuthorStatuses
            | Self::BoosterFollowers
            | Self::BoosterFollowing
            | Self::BoosterStatuses => ValueTypes::NUMBER,
            Self::Mentions
            | Self::MediaTypes
            | Self::MediaDescriptions
            | Self::Hashtags
            | Self::OurAccounts
            | Self::PollOptions => ValueTypes::SET,
            _ => ValueTypes::TEXT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Identifier(String),
    String(String),
    Account(String),
    OpaqueId(String),
    Number(i64),
    MinMagnitude,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Comma,
    Dot,
    Colon,
    Plus,
    Minus,
    Star,
    Slash,
    Bang,
    Ampersand,
    Pipe,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    ArrowRight,
    ArrowLeft,
    End,
}

struct Lexer<'a> {
    input: &'a str,
    position: usize,
    limits: QueryLimits,
    tokens: Vec<Token>,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str, limits: QueryLimits) -> Self {
        Self {
            input,
            position: 0,
            limits,
            tokens: Vec::new(),
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, CompileError> {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
                continue;
            }
            let start = self.position;
            match ch {
                '"' => {
                    let (value, end) = self.quoted(start)?;
                    self.push(TokenKind::String(value), start, end)?;
                }
                '@' => self.account(start)?,
                '#' => self.opaque_id(start)?,
                '0'..='9' => self.number(start)?,
                '(' => self.single(TokenKind::LeftParen)?,
                ')' => self.single(TokenKind::RightParen)?,
                '[' => self.single(TokenKind::LeftBracket)?,
                ']' => self.single(TokenKind::RightBracket)?,
                ',' => self.single(TokenKind::Comma)?,
                '.' => self.single(TokenKind::Dot)?,
                ':' => self.single(TokenKind::Colon)?,
                '+' => self.single(TokenKind::Plus)?,
                '*' => self.single(TokenKind::Star)?,
                '/' => self.single(TokenKind::Slash)?,
                '&' => {
                    self.advance();
                    if self.peek() == Some('&') {
                        self.advance();
                    }
                    self.push(TokenKind::Ampersand, start, self.position)?;
                }
                '|' => {
                    self.advance();
                    if self.peek() == Some('|') {
                        self.advance();
                    }
                    self.push(TokenKind::Pipe, start, self.position)?;
                }
                '=' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                    }
                    self.push(TokenKind::Equal, start, self.position)?;
                }
                '!' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        self.push(TokenKind::NotEqual, start, self.position)?;
                    } else {
                        self.push(TokenKind::Bang, start, self.position)?;
                    }
                }
                '-' => {
                    self.advance();
                    if self.peek() == Some('>') {
                        self.advance();
                        self.push(TokenKind::ArrowRight, start, self.position)?;
                    } else {
                        self.push(TokenKind::Minus, start, self.position)?;
                    }
                }
                '<' => {
                    self.advance();
                    let kind = match self.peek() {
                        Some('=') => {
                            self.advance();
                            TokenKind::LessEqual
                        }
                        Some('-') => {
                            self.advance();
                            TokenKind::ArrowLeft
                        }
                        _ => TokenKind::Less,
                    };
                    self.push(kind, start, self.position)?;
                }
                '>' => {
                    self.advance();
                    let kind = if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::GreaterEqual
                    } else {
                        TokenKind::Greater
                    };
                    self.push(kind, start, self.position)?;
                }
                _ if is_identifier_start(ch) => self.identifier(start)?,
                _ => {
                    self.advance();
                    return Err(CompileError::at(
                        self.input,
                        Span::new(start, self.position),
                        "unexpected character",
                    ));
                }
            }
        }
        self.push(TokenKind::End, self.position, self.position)?;
        Ok(self.tokens)
    }

    fn peek(&self) -> Option<char> {
        self.input[self.position..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.position += ch.len_utf8();
        Some(ch)
    }

    fn single(&mut self, kind: TokenKind) -> Result<(), CompileError> {
        let start = self.position;
        self.advance();
        self.push(kind, start, self.position)
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) -> Result<(), CompileError> {
        if self.tokens.len() >= self.limits.max_tokens {
            return Err(CompileError::at(
                self.input,
                Span::new(start, end),
                "query token limit exceeded",
            ));
        }
        self.tokens.push(Token {
            kind,
            span: Span::new(start, end),
        });
        Ok(())
    }

    fn quoted(&mut self, start: usize) -> Result<(String, usize), CompileError> {
        debug_assert_eq!(self.peek(), Some('"'));
        self.advance();
        let mut value = String::new();
        loop {
            let Some(ch) = self.advance() else {
                return Err(CompileError::at(
                    self.input,
                    Span::new(start, self.position),
                    "unterminated string literal",
                ));
            };
            match ch {
                '"' => return Ok((value, self.position)),
                '\\' => {
                    let Some(escaped) = self.advance() else {
                        return Err(CompileError::at(
                            self.input,
                            Span::new(start, self.position),
                            "unterminated string escape",
                        ));
                    };
                    if matches!(escaped, '"' | '\\') {
                        value.push(escaped);
                    } else {
                        value.push('\\');
                        value.push(escaped);
                    }
                }
                _ => value.push(ch),
            }
        }
    }

    fn account(&mut self, start: usize) -> Result<(), CompileError> {
        self.advance();
        if self.peek() == Some('"') {
            let quote_start = self.position;
            let (value, end) = self.quoted(quote_start)?;
            if value.is_empty() {
                return Err(CompileError::at(
                    self.input,
                    Span::new(start, end),
                    "empty account literal",
                ));
            }
            return self.push(TokenKind::Account(value), start, end);
        }
        let value_start = self.position;
        while let Some(ch) = self.peek() {
            if is_literal_delimiter(ch) || ch == ':' {
                break;
            }
            self.advance();
        }
        if self.position == value_start {
            return Err(CompileError::at(
                self.input,
                Span::new(start, self.position),
                "empty account literal",
            ));
        }
        self.push(
            TokenKind::Account(self.input[value_start..self.position].to_string()),
            start,
            self.position,
        )
    }

    fn opaque_id(&mut self, start: usize) -> Result<(), CompileError> {
        self.advance();
        if self.peek() == Some('"') {
            let quote_start = self.position;
            let (value, end) = self.quoted(quote_start)?;
            if value.is_empty() {
                return Err(CompileError::at(
                    self.input,
                    Span::new(start, end),
                    "empty identifier literal",
                ));
            }
            return self.push(TokenKind::OpaqueId(value), start, end);
        }
        let value_start = self.position;
        while let Some(ch) = self.peek() {
            if is_literal_delimiter(ch) {
                break;
            }
            self.advance();
        }
        if self.position == value_start {
            return Err(CompileError::at(
                self.input,
                Span::new(start, self.position),
                "empty identifier literal",
            ));
        }
        self.push(
            TokenKind::OpaqueId(self.input[value_start..self.position].to_string()),
            start,
            self.position,
        )
    }

    fn number(&mut self, start: usize) -> Result<(), CompileError> {
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            self.advance();
        }
        let magnitude = self.input[start..self.position]
            .parse::<u64>()
            .map_err(|_| {
                CompileError::at(
                    self.input,
                    Span::new(start, self.position),
                    "integer literal is outside the signed 64-bit range",
                )
            })?;
        let kind = if magnitude <= i64::MAX as u64 {
            TokenKind::Number(magnitude as i64)
        } else if magnitude == (i64::MAX as u64) + 1 {
            TokenKind::MinMagnitude
        } else {
            return Err(CompileError::at(
                self.input,
                Span::new(start, self.position),
                "integer literal is outside the signed 64-bit range",
            ));
        };
        self.push(kind, start, self.position)
    }

    fn identifier(&mut self, start: usize) -> Result<(), CompileError> {
        self.advance();
        while self.peek().is_some_and(is_identifier_continue) {
            self.advance();
        }
        self.push(
            TokenKind::Identifier(self.input[start..self.position].to_string()),
            start,
            self.position,
        )
    }
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

fn is_literal_delimiter(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '(' | ')' | '[' | ']' | ',' | '&' | '|' | '=' | '!' | '<' | '>' | '+' | '*' | '/'
        )
}

pub(crate) fn compile_query(query: &str) -> Result<CompiledQuery, CompileError> {
    compile_query_with_limits(query, QueryLimits::default())
}

pub(crate) fn compile_query_with_limits(
    query: &str,
    limits: QueryLimits,
) -> Result<CompiledQuery, CompileError> {
    if query.len() > limits.max_query_bytes {
        return Err(CompileError::at(
            query,
            Span::new(limits.max_query_bytes, limits.max_query_bytes),
            "query byte limit exceeded",
        ));
    }
    let tokens = Lexer::new(query, limits).tokenize()?;
    Parser::new(query, tokens, limits).parse()
}

struct Parser<'a> {
    input: &'a str,
    tokens: Vec<Token>,
    position: usize,
    limits: QueryLimits,
    source_argument_count: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, tokens: Vec<Token>, limits: QueryLimits) -> Self {
        Self {
            input,
            tokens,
            position: 0,
            limits,
            source_argument_count: 0,
        }
    }

    fn parse(mut self) -> Result<CompiledQuery, CompileError> {
        let had_from = self.take_keyword("from").is_some();
        let sources = if had_from {
            self.parse_sources()?
        } else if self.check_keyword("where") {
            vec![SourceSpec {
                kind: SourceKind::Local,
                arguments: Vec::new(),
                span: Span::new(0, 0),
            }]
        } else {
            return Err(self.error_current("query must start with from or where"));
        };

        if sources.is_empty() {
            return Err(self.error_current("from requires at least one source"));
        }
        let predicate = if self.take_keyword("where").is_some() {
            if self.at_end() {
                if had_from {
                    Expr {
                        kind: ExprKind::Bool(true),
                        types: ValueTypes::BOOL,
                        span: self.current().span,
                    }
                } else {
                    return Err(self.error_current("where requires a predicate"));
                }
            } else {
                self.parse_or(0)?
            }
        } else {
            Expr {
                kind: ExprKind::Bool(true),
                types: ValueTypes::BOOL,
                span: self.current().span,
            }
        };
        if !predicate.types.intersects(ValueTypes::BOOL) {
            return Err(CompileError::at(
                self.input,
                predicate.span,
                "query predicate must produce a boolean value",
            ));
        }
        if !self.at_end() {
            return Err(self.error_current("unexpected token after query"));
        }
        let requirements = query_requirements(&sources, &predicate);
        let conversation_ids = sources
            .iter()
            .filter(|source| source.kind == SourceKind::Conversation)
            .flat_map(|source| source.arguments.iter().cloned())
            .collect();
        Ok(CompiledQuery {
            sources,
            predicate,
            sql_prefilter: SqlPrefilter::default(),
            requirements,
            conversation_ids,
        })
    }

    fn parse_sources(&mut self) -> Result<Vec<SourceSpec>, CompileError> {
        let mut sources = Vec::new();
        while !self.at_end() && !self.check_keyword("where") {
            let mut parsed = self.parse_source()?;
            sources.append(&mut parsed);
            if sources.len() > self.limits.max_sources {
                return Err(self.error_current("source limit exceeded"));
            }
            if self.take_simple(SimpleToken::Comma).is_some() {
                if self.at_end() || self.check_keyword("where") {
                    break;
                }
            } else if !self.at_end() && !self.check_keyword("where") {
                return Err(self.error_current("sources must be separated by commas"));
            }
        }
        let mut seen = HashSet::new();
        sources.retain(|source| seen.insert((source.kind, source.arguments.clone())));
        Ok(sources)
    }

    fn parse_source(&mut self) -> Result<Vec<SourceSpec>, CompileError> {
        let token = self.advance().clone();
        let source_name = match &token.kind {
            TokenKind::Identifier(name) => name.to_ascii_lowercase(),
            TokenKind::Star => "*".to_string(),
            _ => {
                return Err(CompileError::at(
                    self.input,
                    token.span,
                    "source name expected",
                ))
            }
        };
        let kind = resolve_source_kind(&source_name)
            .ok_or_else(|| CompileError::at(self.input, token.span, "unsupported source"))?;
        let mut arguments = Vec::new();
        if self.take_simple(SimpleToken::Colon).is_some() {
            arguments.push(self.parse_source_argument()?);
            while self.check_simple(SimpleToken::Comma) && self.next_is_source_argument() {
                self.advance();
                arguments.push(self.parse_source_argument()?);
            }
        } else if self.take_simple(SimpleToken::LeftParen).is_some() {
            if !self.check_simple(SimpleToken::RightParen) {
                loop {
                    arguments.push(self.parse_source_argument()?);
                    if self.take_simple(SimpleToken::Comma).is_none() {
                        break;
                    }
                    if self.check_simple(SimpleToken::RightParen) {
                        break;
                    }
                }
            }
            self.expect_simple(
                SimpleToken::RightParen,
                "source call is missing a closing parenthesis",
            )?;
        }

        if source_requires_argument(kind) && arguments.is_empty() {
            return Err(CompileError::at(
                self.input,
                token.span,
                "source requires an argument",
            ));
        }
        if kind == SourceKind::Local && !arguments.is_empty() {
            return Err(CompileError::at(
                self.input,
                token.span,
                "named local timelines are not available in Awayuki",
            ));
        }
        if arguments.is_empty() {
            return Ok(vec![SourceSpec {
                kind,
                arguments,
                span: token.span,
            }]);
        }
        if arguments.iter().any(|argument| argument.trim().is_empty()) {
            return Err(CompileError::at(
                self.input,
                token.span,
                "source argument cannot be empty",
            ));
        }
        if kind == SourceKind::User && arguments.iter().any(|argument| argument.trim() == "*") {
            return Err(CompileError::at(
                self.input,
                token.span,
                "user source does not accept a wildcard argument",
            ));
        }
        let mut normalized = arguments
            .into_iter()
            .map(|argument| normalize_source_argument(kind, argument))
            .collect::<Vec<_>>();
        if source_requires_argument(kind) && normalized.iter().any(String::is_empty) {
            return Err(CompileError::at(
                self.input,
                token.span,
                "source argument cannot be empty",
            ));
        }
        if source_accepts_account_wildcard(kind)
            && normalized.iter().any(|argument| argument == "*")
        {
            normalized.clear();
        }
        if normalized.is_empty() {
            return Ok(vec![SourceSpec {
                kind,
                arguments: Vec::new(),
                span: token.span,
            }]);
        }
        Ok(normalized
            .into_iter()
            .map(|argument| SourceSpec {
                kind,
                arguments: vec![argument],
                span: token.span,
            })
            .collect())
    }

    fn parse_source_argument(&mut self) -> Result<String, CompileError> {
        self.source_argument_count = self.source_argument_count.saturating_add(1);
        if self.source_argument_count > self.limits.max_source_arguments {
            return Err(self.error_current("source argument limit exceeded"));
        }
        let token = self.advance().clone();
        match token.kind {
            TokenKind::String(value)
            | TokenKind::Account(value)
            | TokenKind::OpaqueId(value)
            | TokenKind::Identifier(value) => Ok(value),
            TokenKind::Number(value) => Ok(value.to_string()),
            TokenKind::Star => Ok("*".to_string()),
            _ => Err(CompileError::at(
                self.input,
                token.span,
                "source argument must be a string, account, or identifier",
            )),
        }
    }

    fn next_is_source_argument(&self) -> bool {
        self.tokens.get(self.position + 1).is_some_and(|token| {
            matches!(
                token.kind,
                TokenKind::String(_)
                    | TokenKind::Account(_)
                    | TokenKind::OpaqueId(_)
                    | TokenKind::Number(_)
                    | TokenKind::Star
            )
        })
    }

    fn guard_depth(&self, depth: usize) -> Result<(), CompileError> {
        if depth > self.limits.max_depth {
            return Err(self.error_current("query nesting limit exceeded"));
        }
        Ok(())
    }

    fn current(&self) -> &Token {
        &self.tokens[self.position.min(self.tokens.len().saturating_sub(1))]
    }

    fn advance(&mut self) -> &Token {
        let index = self.position;
        if !self.at_end() {
            self.position += 1;
        }
        &self.tokens[index]
    }

    fn at_end(&self) -> bool {
        matches!(self.current().kind, TokenKind::End)
    }

    fn check_keyword(&self, keyword: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Identifier(value) if value.eq_ignore_ascii_case(keyword))
    }

    fn take_keyword(&mut self, keyword: &str) -> Option<Token> {
        self.check_keyword(keyword).then(|| self.advance().clone())
    }

    fn check_simple(&self, expected: SimpleToken) -> bool {
        expected.matches(&self.current().kind)
    }

    fn take_simple(&mut self, expected: SimpleToken) -> Option<Token> {
        self.check_simple(expected).then(|| self.advance().clone())
    }

    fn expect_simple(
        &mut self,
        expected: SimpleToken,
        message: &'static str,
    ) -> Result<Token, CompileError> {
        self.take_simple(expected)
            .ok_or_else(|| self.error_current(message))
    }

    fn error_current(&self, message: &'static str) -> CompileError {
        CompileError::at(self.input, self.current().span, message)
    }
}

#[derive(Debug, Clone, Copy)]
enum SimpleToken {
    LeftParen,
    RightParen,
    RightBracket,
    Comma,
    Dot,
    Colon,
    Plus,
    Minus,
    Star,
    Slash,
    Bang,
    Ampersand,
    Pipe,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    ArrowRight,
    ArrowLeft,
}

impl SimpleToken {
    fn matches(self, token: &TokenKind) -> bool {
        matches!(
            (self, token),
            (Self::LeftParen, TokenKind::LeftParen)
                | (Self::RightParen, TokenKind::RightParen)
                | (Self::RightBracket, TokenKind::RightBracket)
                | (Self::Comma, TokenKind::Comma)
                | (Self::Dot, TokenKind::Dot)
                | (Self::Colon, TokenKind::Colon)
                | (Self::Plus, TokenKind::Plus)
                | (Self::Minus, TokenKind::Minus)
                | (Self::Star, TokenKind::Star)
                | (Self::Slash, TokenKind::Slash)
                | (Self::Bang, TokenKind::Bang)
                | (Self::Ampersand, TokenKind::Ampersand)
                | (Self::Pipe, TokenKind::Pipe)
                | (Self::Equal, TokenKind::Equal)
                | (Self::NotEqual, TokenKind::NotEqual)
                | (Self::Less, TokenKind::Less)
                | (Self::LessEqual, TokenKind::LessEqual)
                | (Self::Greater, TokenKind::Greater)
                | (Self::GreaterEqual, TokenKind::GreaterEqual)
                | (Self::ArrowRight, TokenKind::ArrowRight)
                | (Self::ArrowLeft, TokenKind::ArrowLeft)
        )
    }
}

fn resolve_source_kind(name: &str) -> Option<SourceKind> {
    Some(match name {
        "*" | "all" | "local" => SourceKind::Local,
        "home" => SourceKind::Home,
        "list" => SourceKind::List,
        "mention" | "mentions" | "reply" | "replies" => SourceKind::Mentions,
        "message" | "messages" | "dm" | "dms" | "direct" => SourceKind::Direct,
        "search" | "find" => SourceKind::Search,
        "track" | "stream" => SourceKind::Track,
        "conv" | "conversation" | "talk" | "tree" => SourceKind::Conversation,
        "user" => SourceKind::User,
        "public" | "federated" => SourceKind::Public,
        "local_public" | "localpublic" => SourceKind::LocalPublic,
        "hashtag" | "tag" => SourceKind::Hashtag,
        "bookmark" | "bookmarks" | "bookmarked" => SourceKind::Bookmarks,
        "favourite" | "favourites" | "favorite" | "favorites" | "favs" => SourceKind::Favourites,
        _ => return None,
    })
}

fn source_requires_argument(kind: SourceKind) -> bool {
    matches!(
        kind,
        SourceKind::List
            | SourceKind::Search
            | SourceKind::Track
            | SourceKind::Conversation
            | SourceKind::User
            | SourceKind::Hashtag
    )
}

fn source_accepts_account_wildcard(kind: SourceKind) -> bool {
    matches!(
        kind,
        SourceKind::Home
            | SourceKind::Mentions
            | SourceKind::Direct
            | SourceKind::Public
            | SourceKind::LocalPublic
            | SourceKind::Bookmarks
            | SourceKind::Favourites
    )
}

fn normalize_source_argument(kind: SourceKind, argument: String) -> String {
    let trimmed = argument.trim();
    match kind {
        SourceKind::Hashtag => trimmed.trim_start_matches('#').to_string(),
        SourceKind::Home
        | SourceKind::Mentions
        | SourceKind::Direct
        | SourceKind::Public
        | SourceKind::LocalPublic
        | SourceKind::Bookmarks
        | SourceKind::Favourites
        | SourceKind::User => trimmed.strip_prefix('@').unwrap_or(trimmed).to_string(),
        _ => trimmed.to_string(),
    }
}

impl Parser<'_> {
    fn parse_or(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        let first = self.parse_and(depth)?;
        let mut tails = Vec::new();
        while let Some(operator) = self
            .take_simple(SimpleToken::Pipe)
            .or_else(|| self.take_keyword("or"))
        {
            tails.push((operator.span, self.parse_and(depth)?));
        }
        self.right_fold(first, tails, |_| BinaryOp::Or)
    }

    fn parse_and(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        let first = self.parse_equality(depth)?;
        let mut tails = Vec::new();
        while let Some(operator) = self
            .take_simple(SimpleToken::Ampersand)
            .or_else(|| self.take_keyword("and"))
        {
            tails.push((operator.span, self.parse_equality(depth)?));
        }
        self.right_fold(first, tails, |_| BinaryOp::And)
    }

    fn parse_equality(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        let first = self.parse_comparison(depth)?;
        let mut tails = Vec::new();
        loop {
            let operator = if let Some(token) = self.take_simple(SimpleToken::Equal) {
                Some((BinaryOp::Equal, token))
            } else {
                self.take_simple(SimpleToken::NotEqual)
                    .map(|token| (BinaryOp::NotEqual, token))
            };
            let Some((operator, token)) = operator else {
                break;
            };
            tails.push((operator, token.span, self.parse_comparison(depth)?));
        }
        self.right_fold_ops(first, tails)
    }

    fn parse_comparison(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        let first = self.parse_match_operator(depth)?;
        let mut tails = Vec::new();
        loop {
            let operator = [
                (SimpleToken::Less, BinaryOp::Less),
                (SimpleToken::LessEqual, BinaryOp::LessEqual),
                (SimpleToken::Greater, BinaryOp::Greater),
                (SimpleToken::GreaterEqual, BinaryOp::GreaterEqual),
            ]
            .into_iter()
            .find_map(|(simple, op)| self.take_simple(simple).map(|token| (op, token)));
            let Some((operator, token)) = operator else {
                break;
            };
            tails.push((operator, token.span, self.parse_match_operator(depth)?));
        }
        self.right_fold_ops(first, tails)
    }

    fn parse_match_operator(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        let first = self.parse_additive(depth)?;
        let mut tails = Vec::new();
        loop {
            let operator = if let Some(token) = self
                .take_keyword("regex")
                .or_else(|| self.take_keyword("match"))
            {
                Some((BinaryOp::Regex, token))
            } else if let Some(token) = self
                .take_keyword("startswith")
                .or_else(|| self.take_keyword("startwith"))
            {
                Some((BinaryOp::StartsWith, token))
            } else if let Some(token) = self
                .take_keyword("endswith")
                .or_else(|| self.take_keyword("endwith"))
            {
                Some((BinaryOp::EndsWith, token))
            } else if let Some(token) = self
                .take_keyword("contains")
                .or_else(|| self.take_simple(SimpleToken::ArrowRight))
            {
                Some((BinaryOp::Contains, token))
            } else {
                self.take_keyword("in")
                    .or_else(|| self.take_simple(SimpleToken::ArrowLeft))
                    .map(|token| (BinaryOp::In, token))
            };
            let Some((operator, token)) = operator else {
                break;
            };
            tails.push((operator, token.span, self.parse_additive(depth)?));
        }
        self.right_fold_ops(first, tails)
    }

    fn parse_additive(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        let first = self.parse_multiplicative(depth)?;
        let mut tails = Vec::new();
        loop {
            let operator = if let Some(token) = self.take_simple(SimpleToken::Plus) {
                Some((BinaryOp::Add, token))
            } else {
                self.take_simple(SimpleToken::Minus)
                    .map(|token| (BinaryOp::Subtract, token))
            };
            let Some((operator, token)) = operator else {
                break;
            };
            tails.push((operator, token.span, self.parse_multiplicative(depth)?));
        }
        self.right_fold_ops(first, tails)
    }

    fn parse_multiplicative(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        let first = self.parse_unary(depth)?;
        let mut tails = Vec::new();
        loop {
            let operator = if let Some(token) = self.take_simple(SimpleToken::Star) {
                Some((BinaryOp::Multiply, token))
            } else {
                self.take_simple(SimpleToken::Slash)
                    .map(|token| (BinaryOp::Divide, token))
            };
            let Some((operator, token)) = operator else {
                break;
            };
            tails.push((operator, token.span, self.parse_unary(depth)?));
        }
        self.right_fold_ops(first, tails)
    }

    fn parse_unary(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        if let Some(token) = self
            .take_simple(SimpleToken::Bang)
            .or_else(|| self.take_keyword("not"))
        {
            let operand = self.parse_unary(depth + 1)?;
            return self.make_unary(UnaryOp::Not, operand, token.span);
        }
        if let Some(token) = self.take_simple(SimpleToken::Minus) {
            if matches!(self.current().kind, TokenKind::MinMagnitude) {
                let magnitude = self.advance().clone();
                return Ok(Expr {
                    kind: ExprKind::Number(i64::MIN),
                    types: ValueTypes::NUMBER,
                    span: Span::new(token.span.start, magnitude.span.end),
                });
            }
            let operand = self.parse_unary(depth + 1)?;
            return self.make_unary(UnaryOp::Negate, operand, token.span);
        }
        if let Some(token) = self.take_keyword("caseful") {
            let operand = self.parse_unary(depth + 1)?;
            return self.make_unary(UnaryOp::Caseful, operand, token.span);
        }
        self.parse_primary(depth)
    }

    fn parse_primary(&mut self, depth: usize) -> Result<Expr, CompileError> {
        self.guard_depth(depth)?;
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Number(value) => Ok(Expr {
                kind: ExprKind::Number(value),
                types: ValueTypes::NUMBER,
                span: token.span,
            }),
            TokenKind::MinMagnitude => Err(CompileError::at(
                self.input,
                token.span,
                "integer literal is outside the signed 64-bit range",
            )),
            TokenKind::String(value) | TokenKind::Account(value) => Ok(Expr {
                kind: ExprKind::Text(value),
                types: ValueTypes::TEXT,
                span: token.span,
            }),
            TokenKind::OpaqueId(value) => Ok(Expr {
                kind: ExprKind::Identity(value),
                types: ValueTypes::IDENTITY,
                span: token.span,
            }),
            TokenKind::Star => Ok(Expr {
                kind: ExprKind::Field(Field::OurAccounts),
                types: ValueTypes::SET,
                span: token.span,
            }),
            TokenKind::Identifier(first) => self.parse_field_or_boolean(first, token.span),
            TokenKind::LeftParen => {
                if self.take_simple(SimpleToken::RightParen).is_some() {
                    return Ok(Expr {
                        kind: ExprKind::Bool(true),
                        types: ValueTypes::BOOL,
                        span: token.span,
                    });
                }
                let expression = self.parse_or(depth + 1)?;
                let close = self.expect_simple(
                    SimpleToken::RightParen,
                    "group is missing a closing parenthesis",
                )?;
                Ok(Expr {
                    span: Span::new(token.span.start, close.span.end),
                    ..expression
                })
            }
            TokenKind::LeftBracket => self.parse_set(token.span, depth + 1),
            TokenKind::End => Err(CompileError::at(
                self.input,
                token.span,
                "expression expected",
            )),
            _ => Err(CompileError::at(
                self.input,
                token.span,
                "value or field expected",
            )),
        }
    }

    fn parse_set(&mut self, open: Span, depth: usize) -> Result<Expr, CompileError> {
        let mut values = Vec::new();
        if !self.check_simple(SimpleToken::RightBracket) {
            loop {
                if values.len() >= self.limits.max_set_items {
                    return Err(self.error_current("set item limit exceeded"));
                }
                let value = self.parse_or(depth)?;
                if !value
                    .types
                    .intersects(ValueTypes::NUMBER | ValueTypes::TEXT | ValueTypes::IDENTITY)
                {
                    return Err(CompileError::at(
                        self.input,
                        value.span,
                        "set items must be scalar values",
                    ));
                }
                values.push(value);
                if self.take_simple(SimpleToken::Comma).is_none() {
                    break;
                }
                if self.check_simple(SimpleToken::RightBracket) {
                    break;
                }
            }
        }
        let close = self.expect_simple(
            SimpleToken::RightBracket,
            "set is missing a closing bracket",
        )?;
        Ok(Expr {
            kind: ExprKind::Set(values),
            types: ValueTypes::SET,
            span: Span::new(open.start, close.span.end),
        })
    }

    fn parse_field_or_boolean(&mut self, first: String, start: Span) -> Result<Expr, CompileError> {
        if first.eq_ignore_ascii_case("true") || first.eq_ignore_ascii_case("false") {
            return Ok(Expr {
                kind: ExprKind::Bool(first.eq_ignore_ascii_case("true")),
                types: ValueTypes::BOOL,
                span: start,
            });
        }
        let mut parts = vec![first];
        let mut end = start.end;
        while self.take_simple(SimpleToken::Dot).is_some() {
            let token = self.advance().clone();
            let TokenKind::Identifier(part) = token.kind else {
                return Err(CompileError::at(
                    self.input,
                    token.span,
                    "field component expected after dot",
                ));
            };
            parts.push(part);
            end = token.span.end;
        }
        let field = resolve_field(&parts).ok_or_else(|| {
            CompileError::at(
                self.input,
                Span::new(start.start, end),
                "field is unsupported or unavailable",
            )
        })?;
        Ok(Expr {
            kind: ExprKind::Field(field),
            types: field.value_types(),
            span: Span::new(start.start, end),
        })
    }

    fn right_fold(
        &self,
        first: Expr,
        tails: Vec<(Span, Expr)>,
        operator: impl Fn(Span) -> BinaryOp,
    ) -> Result<Expr, CompileError> {
        let tails = tails
            .into_iter()
            .map(|(span, expression)| (operator(span), span, expression))
            .collect();
        self.right_fold_ops(first, tails)
    }

    fn right_fold_ops(
        &self,
        first: Expr,
        tails: Vec<(BinaryOp, Span, Expr)>,
    ) -> Result<Expr, CompileError> {
        if tails.is_empty() {
            return Ok(first);
        }
        if tails.len() >= self.limits.max_depth {
            return Err(CompileError::at(
                self.input,
                tails[self.limits.max_depth.saturating_sub(1)].1,
                "expression depth limit exceeded",
            ));
        }
        let mut operands = Vec::with_capacity(tails.len() + 1);
        let mut operators = Vec::with_capacity(tails.len());
        operands.push(first);
        for (operator, span, operand) in tails {
            operators.push((operator, span));
            operands.push(operand);
        }
        let mut right = operands.pop().expect("tail has a right operand");
        while let Some((operator, span)) = operators.pop() {
            let left = operands.pop().expect("operator has a left operand");
            right = self.make_binary(operator, left, right, span)?;
        }
        Ok(right)
    }

    fn make_unary(
        &self,
        op: UnaryOp,
        operand: Expr,
        operator_span: Span,
    ) -> Result<Expr, CompileError> {
        let required = match op {
            UnaryOp::Not => ValueTypes::BOOL,
            UnaryOp::Negate => ValueTypes::NUMBER,
            UnaryOp::Caseful => ValueTypes::TEXT,
        };
        if !operand.types.intersects(required) {
            return Err(CompileError::at(
                self.input,
                operator_span,
                "unary operator cannot be applied to this value type",
            ));
        }
        let span = Span::new(operator_span.start, operand.span.end);
        Ok(Expr {
            kind: ExprKind::Unary(op, Box::new(operand)),
            types: required,
            span,
        })
    }

    fn make_binary(
        &self,
        op: BinaryOp,
        left: Expr,
        right: Expr,
        operator_span: Span,
    ) -> Result<Expr, CompileError> {
        let result_types = binary_result_types(op, left.types, right.types).ok_or_else(|| {
            CompileError::at(
                self.input,
                operator_span,
                "binary operator cannot be applied to these value types",
            )
        })?;
        let literal_regex = if op == BinaryOp::Regex {
            match &right.kind {
                ExprKind::Text(pattern) => {
                    if pattern.len() > self.limits.max_regex_bytes {
                        return Err(CompileError::at(
                            self.input,
                            right.span,
                            "regular expression limit exceeded",
                        ));
                    }
                    Some(
                        RegexBuilder::new(pattern)
                            .case_insensitive(false)
                            .size_limit(self.limits.max_regex_bytes.saturating_mul(64))
                            .dfa_size_limit(self.limits.max_regex_bytes.saturating_mul(256))
                            .nest_limit(self.limits.max_depth as u32)
                            .build()
                            .map_err(|_| {
                                CompileError::at(
                                    self.input,
                                    right.span,
                                    "invalid regular expression",
                                )
                            })?,
                    )
                }
                _ => {
                    return Err(CompileError::at(
                        self.input,
                        right.span,
                        "regular expression pattern must be a string literal",
                    ))
                }
            }
        } else {
            None
        };
        let span = Span::new(left.span.start, right.span.end);
        Ok(Expr {
            kind: ExprKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                literal_regex,
            },
            types: result_types,
            span,
        })
    }
}

fn binary_result_types(op: BinaryOp, left: ValueTypes, right: ValueTypes) -> Option<ValueTypes> {
    let same = ValueTypes(left.0 & right.0);
    match op {
        BinaryOp::Or | BinaryOp::And
            if left.intersects(ValueTypes::BOOL) && right.intersects(ValueTypes::BOOL) =>
        {
            Some(ValueTypes::BOOL)
        }
        BinaryOp::Equal | BinaryOp::NotEqual
            if same.intersects(
                ValueTypes::BOOL | ValueTypes::NUMBER | ValueTypes::TEXT | ValueTypes::IDENTITY,
            ) || (left.intersects(ValueTypes::IDENTITY)
                && right.intersects(ValueTypes::TEXT | ValueTypes::NUMBER))
                || (right.intersects(ValueTypes::IDENTITY)
                    && left.intersects(ValueTypes::TEXT | ValueTypes::NUMBER)) =>
        {
            Some(ValueTypes::BOOL)
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual
            if left.intersects(ValueTypes::NUMBER) && right.intersects(ValueTypes::NUMBER) =>
        {
            Some(ValueTypes::BOOL)
        }
        BinaryOp::Regex | BinaryOp::StartsWith | BinaryOp::EndsWith
            if left.intersects(ValueTypes::TEXT) && right.intersects(ValueTypes::TEXT) =>
        {
            Some(ValueTypes::BOOL)
        }
        BinaryOp::Contains
            if (left.intersects(ValueTypes::TEXT) && right.intersects(ValueTypes::TEXT))
                || (left.intersects(ValueTypes::SET)
                    && right.intersects(
                        ValueTypes::TEXT
                            | ValueTypes::NUMBER
                            | ValueTypes::IDENTITY
                            | ValueTypes::SET,
                    )) =>
        {
            Some(ValueTypes::BOOL)
        }
        BinaryOp::In
            if right.intersects(ValueTypes::SET)
                && left.intersects(
                    ValueTypes::TEXT | ValueTypes::NUMBER | ValueTypes::IDENTITY | ValueTypes::SET,
                ) =>
        {
            Some(ValueTypes::BOOL)
        }
        BinaryOp::Add
            if same.intersects(ValueTypes::NUMBER | ValueTypes::TEXT | ValueTypes::SET) =>
        {
            Some(same)
        }
        BinaryOp::Subtract if same.intersects(ValueTypes::NUMBER | ValueTypes::SET) => Some(same),
        BinaryOp::Multiply if same.intersects(ValueTypes::NUMBER | ValueTypes::SET) => Some(same),
        BinaryOp::Divide
            if left.intersects(ValueTypes::NUMBER) && right.intersects(ValueTypes::NUMBER) =>
        {
            Some(ValueTypes::NUMBER)
        }
        _ => None,
    }
}

fn resolve_field(parts: &[String]) -> Option<Field> {
    let lower = parts
        .iter()
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let path = lower.iter().map(String::as_str).collect::<Vec<_>>();
    Some(match path.as_slice() {
        ["text"] | ["body"] | ["content"] => Field::Text,
        ["raw_content"] | ["raw"] => Field::RawContent,
        ["id"] => Field::Id,
        ["uri"] => Field::Uri,
        ["url"] => Field::Url,
        ["via"]
        | ["source"]
        | ["client"]
        | ["from"]
        | ["application"]
        | ["application_name"]
        | ["application", "name"] => Field::Application,
        ["direct_message"]
        | ["directmessage"]
        | ["isdirectmessage"]
        | ["is_direct_message"]
        | ["dm"]
        | ["isdm"]
        | ["is_dm"]
        | ["message"]
        | ["ismessage"]
        | ["is_message"]
        | ["is_direct"]
        | ["direct"] => Field::DirectMessage,
        ["retweet"]
        | ["rt"]
        | ["isretweet"]
        | ["is_retweet"]
        | ["reblog"]
        | ["isreblog"]
        | ["is_reblog"]
        | ["boost"]
        | ["isboost"]
        | ["is_boost"]
        | ["renote"]
        | ["isrenote"]
        | ["is_renote"] => Field::Boost,
        ["in_reply_to"]
        | ["in_reply_to_id"]
        | ["inreplyto"]
        | ["replyto"]
        | ["reply_to"]
        | ["reply", "id"] => Field::InReplyTo,
        ["in_reply_to_account"] | ["in_reply_to_account_id"] | ["reply", "account_id"] => {
            Field::ReplyAccountId
        }
        ["is_reply"] | ["reply"] => Field::IsReply,
        ["to"] | ["mention"] | ["mentions"] => Field::Mentions,
        ["favs"]
        | ["favourite"]
        | ["favourites"]
        | ["favorite"]
        | ["favorites"]
        | ["favourer"]
        | ["favourers"]
        | ["favorer"]
        | ["favorers"]
        | ["like"]
        | ["likes"]
        | ["fav_count"]
        | ["favourites_count"]
        | ["favorites_count"]
        | ["likes_count"]
        | ["reactions_count"] => Field::FavouritesCount,
        ["retweets"]
        | ["rts"]
        | ["reblogs"]
        | ["boosts"]
        | ["renotes"]
        | ["reposts"]
        | ["retweeters"]
        | ["reblogs_count"]
        | ["boosts_count"]
        | ["renotes_count"]
        | ["reposts_count"] => Field::ReblogsCount,
        ["replies"] | ["replies_count"] => Field::RepliesCount,
        ["visibility"] => Field::Visibility,
        ["public"] | ["is_public"] => Field::IsPublic,
        ["unlisted"] | ["is_unlisted"] => Field::IsUnlisted,
        ["private"] | ["is_private"] | ["followers_only"] => Field::IsPrivate,
        ["lang"] | ["language"] => Field::Language,
        ["cw"] | ["spoiler"] | ["spoiler_text"] => Field::SpoilerText,
        ["has_cw"] | ["has_spoiler"] => Field::HasSpoiler,
        ["sensitive"] => Field::Sensitive,
        ["media"] | ["has_media"] => Field::HasMedia,
        ["media_count"] | ["media", "count"] => Field::MediaCount,
        ["media_types"] | ["media", "types"] => Field::MediaTypes,
        ["media_descriptions"] | ["media", "descriptions"] => Field::MediaDescriptions,
        ["has_image"] | ["media", "has_image"] => Field::HasImage,
        ["has_video"] | ["media", "has_video"] => Field::HasVideo,
        ["has_audio"] | ["media", "has_audio"] => Field::HasAudio,
        ["poll"] | ["has_poll"] => Field::HasPoll,
        ["poll", "id"] => Field::PollId,
        ["poll", "expired"] => Field::PollExpired,
        ["poll", "multiple"] => Field::PollMultiple,
        ["poll", "votes_count"] => Field::PollVotesCount,
        ["poll", "voters_count"] => Field::PollVotersCount,
        ["poll", "options_count"] => Field::PollOptionsCount,
        ["poll", "options"] => Field::PollOptions,
        ["poll", "expires_at"] => Field::PollExpiresAt,
        ["card"] | ["has_card"] => Field::HasCard,
        ["quote"] | ["has_quote"] | ["isquote"] | ["is_quote"] => Field::HasQuote,
        ["quote", "id"] => Field::QuoteId,
        ["quote", "url"] => Field::QuoteUrl,
        ["quote", "text"] => Field::QuoteText,
        ["quote", "user"]
        | ["quote", "author"]
        | ["quote", "user", "acct"]
        | ["quote", "author", "acct"] => Field::QuoteAuthorAcct,
        ["edited"] | ["is_edited"] => Field::Edited,
        ["edited_at"] => Field::EditedAt,
        ["domain"] | ["server_domain"] | ["host"] => Field::Domain,
        ["hashtag"] | ["hashtags"] | ["tag"] | ["tags"] => Field::Hashtags,
        ["we"] | ["our"] | ["us"] => Field::OurAccounts,

        ["user"]
        | ["author"]
        | ["user", "acct"]
        | ["author", "acct"]
        | ["user", "screen_name"]
        | ["user", "screenname"] => Field::AuthorAcct,
        ["author", "username"] => Field::AuthorUsername,
        // StarryEyes' legacy `user.username` maps to display name; the
        // provider-neutral local handle is `author.username`.
        ["user", "username"]
        | ["user", "name"]
        | ["user", "display_name"]
        | ["author", "name"]
        | ["author", "display_name"] => Field::AuthorDisplayName,
        ["user", "id"] | ["author", "id"] => Field::AuthorId,
        ["user", "description"]
        | ["user", "desc"]
        | ["user", "bio"]
        | ["user", "note"]
        | ["author", "description"]
        | ["author", "desc"]
        | ["author", "bio"]
        | ["author", "note"] => Field::AuthorNote,
        ["user", "protected"]
        | ["user", "isprotected"]
        | ["user", "is_protected"]
        | ["user", "locked"]
        | ["author", "protected"]
        | ["author", "is_protected"]
        | ["author", "locked"] => Field::AuthorLocked,
        ["user", "bot"] | ["user", "is_bot"] | ["author", "bot"] | ["author", "is_bot"] => {
            Field::AuthorBot
        }
        ["user", "follower"]
        | ["user", "followers"]
        | ["user", "followerscount"]
        | ["user", "follower_count"]
        | ["user", "followers_count"]
        | ["author", "followers"]
        | ["author", "followers_count"] => Field::AuthorFollowers,
        ["user", "follow"]
        | ["user", "following"]
        | ["user", "followings"]
        | ["user", "followingcount"]
        | ["user", "followingscount"]
        | ["user", "following_count"]
        | ["user", "followings_count"]
        | ["user", "friend"]
        | ["user", "friends"]
        | ["user", "friendscount"]
        | ["user", "friend_count"]
        | ["user", "friends_count"]
        | ["author", "following"]
        | ["author", "following_count"] => Field::AuthorFollowing,
        ["user", "status"]
        | ["user", "statuses"]
        | ["user", "statuscount"]
        | ["user", "statusescount"]
        | ["user", "status_count"]
        | ["user", "statuses_count"]
        | ["author", "statuses"]
        | ["author", "statuses_count"] => Field::AuthorStatuses,
        ["user", "domain"]
        | ["user", "server_domain"]
        | ["author", "domain"]
        | ["author", "server_domain"] => Field::AuthorDomain,

        ["retweeter"]
        | ["reblogger"]
        | ["booster"]
        | ["renoter"]
        | ["retweeter", "acct"]
        | ["reblogger", "acct"]
        | ["booster", "acct"]
        | ["retweeter", "screen_name"]
        | ["retweeter", "screenname"]
        | ["reblogger", "screen_name"]
        | ["reblogger", "screenname"]
        | ["booster", "screen_name"]
        | ["booster", "screenname"] => Field::BoosterAcct,
        ["booster", "username"] | ["reblogger", "username"] => Field::BoosterUsername,
        ["retweeter", "username"]
        | ["retweeter", "name"]
        | ["retweeter", "display_name"]
        | ["booster", "name"]
        | ["booster", "display_name"]
        | ["reblogger", "name"]
        | ["reblogger", "display_name"] => Field::BoosterDisplayName,
        ["retweeter", "id"] | ["reblogger", "id"] | ["booster", "id"] => Field::BoosterId,
        ["retweeter", "description"]
        | ["retweeter", "desc"]
        | ["retweeter", "bio"]
        | ["retweeter", "note"]
        | ["reblogger", "description"]
        | ["reblogger", "desc"]
        | ["reblogger", "bio"]
        | ["reblogger", "note"]
        | ["booster", "description"]
        | ["booster", "desc"]
        | ["booster", "bio"]
        | ["booster", "note"] => Field::BoosterNote,
        ["retweeter", "protected"]
        | ["retweeter", "isprotected"]
        | ["retweeter", "is_protected"]
        | ["retweeter", "locked"]
        | ["reblogger", "protected"]
        | ["reblogger", "is_protected"]
        | ["reblogger", "locked"]
        | ["booster", "protected"]
        | ["booster", "is_protected"]
        | ["booster", "locked"] => Field::BoosterLocked,
        ["retweeter", "bot"]
        | ["retweeter", "is_bot"]
        | ["reblogger", "bot"]
        | ["reblogger", "is_bot"]
        | ["booster", "bot"]
        | ["booster", "is_bot"] => Field::BoosterBot,
        ["retweeter", "follower"]
        | ["retweeter", "followers"]
        | ["retweeter", "followerscount"]
        | ["retweeter", "follower_count"]
        | ["retweeter", "followers_count"]
        | ["reblogger", "follower"]
        | ["reblogger", "followers"]
        | ["reblogger", "followerscount"]
        | ["reblogger", "followers_count"]
        | ["booster", "follower"]
        | ["booster", "followers"]
        | ["booster", "followerscount"]
        | ["booster", "followers_count"] => Field::BoosterFollowers,
        ["retweeter", "follow"]
        | ["retweeter", "following"]
        | ["retweeter", "followings"]
        | ["retweeter", "followingcount"]
        | ["retweeter", "followingscount"]
        | ["retweeter", "following_count"]
        | ["retweeter", "followings_count"]
        | ["retweeter", "friend"]
        | ["retweeter", "friends"]
        | ["retweeter", "friendscount"]
        | ["retweeter", "friend_count"]
        | ["retweeter", "friends_count"]
        | ["reblogger", "follow"]
        | ["reblogger", "following"]
        | ["reblogger", "followings"]
        | ["reblogger", "followingcount"]
        | ["reblogger", "following_count"]
        | ["reblogger", "followings_count"]
        | ["reblogger", "friend"]
        | ["reblogger", "friends"]
        | ["booster", "follow"]
        | ["booster", "following"]
        | ["booster", "followings"]
        | ["booster", "followingcount"]
        | ["booster", "following_count"]
        | ["booster", "followings_count"]
        | ["booster", "friend"]
        | ["booster", "friends"] => Field::BoosterFollowing,
        ["retweeter", "status"]
        | ["retweeter", "statuses"]
        | ["retweeter", "statuscount"]
        | ["retweeter", "statusescount"]
        | ["retweeter", "status_count"]
        | ["retweeter", "statuses_count"]
        | ["reblogger", "statuses"]
        | ["reblogger", "statuses_count"]
        | ["booster", "statuses"]
        | ["booster", "statuses_count"] => Field::BoosterStatuses,
        ["retweeter", "domain"]
        | ["retweeter", "server_domain"]
        | ["reblogger", "domain"]
        | ["reblogger", "server_domain"]
        | ["booster", "domain"]
        | ["booster", "server_domain"] => Field::BoosterDomain,

        ["viewer", "favourited"] | ["viewer", "favorited"] => Field::ViewerFavourited,
        ["viewer", "reblogged"] | ["viewer", "boosted"] | ["viewer", "renoted"] => {
            Field::ViewerReblogged
        }
        ["viewer", "muted"] => Field::ViewerMuted,
        ["viewer", "bookmarked"] => Field::ViewerBookmarked,
        ["viewer", "pinned"] => Field::ViewerPinned,

        // Intentionally unavailable: Twitter-only relationship/list fields,
        // non-persisted profile flags, protocol, quote resolution state, and
        // viewer-specific poll fields stored without account scope.
        _ => return None,
    })
}

fn query_requirements(sources: &[SourceSpec], predicate: &Expr) -> QueryRequirements {
    let mut requirements = QueryRequirements {
        effective_status: sources.iter().any(|source| {
            matches!(
                source.kind,
                SourceKind::User | SourceKind::Bookmarks | SourceKind::Favourites
            )
        }),
        memberships: sources.iter().any(|source| {
            !matches!(
                source.kind,
                SourceKind::Local | SourceKind::User | SourceKind::Conversation
            )
        }),
        viewer_states: sources
            .iter()
            .any(|source| matches!(source.kind, SourceKind::Bookmarks | SourceKind::Favourites)),
        login_accounts: sources.iter().any(|source| {
            matches!(
                source.kind,
                SourceKind::Home
                    | SourceKind::List
                    | SourceKind::Mentions
                    | SourceKind::Direct
                    | SourceKind::Public
                    | SourceKind::LocalPublic
                    | SourceKind::Hashtag
                    | SourceKind::Bookmarks
                    | SourceKind::Favourites
            )
        }),
        conversations: sources
            .iter()
            .any(|source| source.kind == SourceKind::Conversation),
        ..QueryRequirements::default()
    };
    visit_fields(predicate, &mut |field| match field {
        Field::BoosterId
        | Field::BoosterUsername
        | Field::BoosterAcct
        | Field::BoosterDisplayName
        | Field::BoosterNote
        | Field::BoosterLocked
        | Field::BoosterBot
        | Field::BoosterFollowers
        | Field::BoosterFollowing
        | Field::BoosterStatuses
        | Field::BoosterDomain
        | Field::Boost => {}
        Field::QuoteId | Field::QuoteUrl | Field::QuoteText | Field::QuoteAuthorAcct => {
            requirements.quote_status = true;
        }
        Field::ViewerFavourited
        | Field::ViewerReblogged
        | Field::ViewerMuted
        | Field::ViewerBookmarked
        | Field::ViewerPinned => {
            requirements.effective_status = true;
            requirements.viewer_states = true;
            requirements.memberships = true;
        }
        Field::OurAccounts => requirements.login_accounts = true,
        _ => requirements.effective_status = true,
    });
    requirements
}

fn visit_fields(expression: &Expr, visitor: &mut impl FnMut(Field)) {
    match &expression.kind {
        ExprKind::Field(field) => visitor(*field),
        ExprKind::Set(values) => {
            for value in values {
                visit_fields(value, visitor);
            }
        }
        ExprKind::Unary(_, operand) => visit_fields(operand, visitor),
        ExprKind::Binary { left, right, .. } => {
            visit_fields(left, visitor);
            visit_fields(right, visitor);
        }
        ExprKind::Bool(_) | ExprKind::Number(_) | ExprKind::Text(_) | ExprKind::Identity(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(query: &str) -> CompiledQuery {
        compile_query(query).unwrap_or_else(|error| panic!("{query:?}: {error}"))
    }

    #[test]
    fn source_aliases_are_case_insensitive_and_federated_is_public() {
        let cases = [
            ("*", SourceKind::Local),
            ("ALL", SourceKind::Local),
            ("local", SourceKind::Local),
            ("home", SourceKind::Home),
            ("list:\"1\"", SourceKind::List),
            ("mention", SourceKind::Mentions),
            ("replies", SourceKind::Mentions),
            ("messages", SourceKind::Direct),
            ("DM", SourceKind::Direct),
            ("search:\"x\"", SourceKind::Search),
            ("find:\"x\"", SourceKind::Search),
            ("track:\"x\"", SourceKind::Track),
            ("stream:\"x\"", SourceKind::Track),
            ("conv:\"opaque\"", SourceKind::Conversation),
            ("tree:\"opaque\"", SourceKind::Conversation),
            ("user:\"alice@example.test\"", SourceKind::User),
            ("public", SourceKind::Public),
            ("federated", SourceKind::Public),
            ("localpublic", SourceKind::LocalPublic),
            ("tag:\"rust\"", SourceKind::Hashtag),
            ("bookmarked", SourceKind::Bookmarks),
            ("favorites", SourceKind::Favourites),
        ];
        for (source, expected) in cases {
            let query = compile(&format!("from {source} where ()"));
            assert_eq!(query.sources()[0].kind, expected, "{source}");
        }
    }

    #[test]
    fn source_colon_call_multi_argument_and_normalization_are_supported() {
        let query = compile(
            "from HOME:@alice-smith@sub.example.social, home(\"bob@example.test\", \"carol@example.test\"), hashtag:\"#Rust\", list:\"alice@example.test/list-1\" where ()",
        );
        assert_eq!(query.sources().len(), 5);
        assert_eq!(
            query.sources()[0].arguments,
            ["alice-smith@sub.example.social"]
        );
        assert_eq!(query.sources()[1].arguments, ["bob@example.test"]);
        assert_eq!(query.sources()[2].arguments, ["carol@example.test"]);
        assert_eq!(query.sources()[3].arguments, ["Rust"]);
        assert_eq!(query.sources()[4].arguments, ["alice@example.test/list-1"]);

        let expanded = compile("from home:\"alice\",\"bob\" where ()");
        assert_eq!(expanded.sources().len(), 2);
        assert_eq!(expanded.sources()[0].arguments, ["alice"]);
        assert_eq!(expanded.sources()[1].arguments, ["bob"]);

        let wildcard = compile("from home:* where ()");
        assert!(wildcard.sources()[0].arguments.is_empty());
        let wildcard_dominates = compile("from home(\"alice\", *) where ()");
        assert_eq!(wildcard_dominates.sources().len(), 1);
        assert!(wildcard_dominates.sources()[0].arguments.is_empty());

        let duplicate = compile("from local, local, home:\"alice\", home:\"alice\" where ()");
        assert_eq!(duplicate.sources().len(), 2);

        for required in ["list", "search", "track", "conversation", "user", "hashtag"] {
            assert!(compile_query(&format!("from {required}:\"\" where ()")).is_err());
            assert!(compile_query(&format!("from {required}:\"   \" where ()")).is_err());
        }
    }

    #[test]
    fn full_and_quoted_fediverse_literals_stay_atomic() {
        let query = compile(
            "where user == @alice-smith@sub.example.social | user == @\"alice@host.example:8443\" | id == #\"did:plc:AbC/opaque\"",
        );
        let mut literals = Vec::new();
        collect_literals(&query.predicate, &mut literals);
        assert!(literals.contains(&"alice-smith@sub.example.social".to_string()));
        assert!(literals.contains(&"alice@host.example:8443".to_string()));
        assert!(literals.contains(&"did:plc:AbC/opaque".to_string()));
    }

    fn collect_literals(expression: &Expr, output: &mut Vec<String>) {
        match &expression.kind {
            ExprKind::Text(value) | ExprKind::Identity(value) => output.push(value.clone()),
            ExprKind::Set(values) => {
                for value in values {
                    collect_literals(value, output);
                }
            }
            ExprKind::Unary(_, value) => collect_literals(value, output),
            ExprKind::Binary { left, right, .. } => {
                collect_literals(left, output);
                collect_literals(right, output);
            }
            ExprKind::Bool(_) | ExprKind::Number(_) | ExprKind::Field(_) => {}
        }
    }

    #[test]
    fn strings_only_unescape_quote_and_backslash() {
        let query = compile(r#"where text == "a\"b\\c\n""#);
        let ExprKind::Binary { right, .. } = &query.predicate.kind else {
            panic!("equality expected")
        };
        assert!(matches!(&right.kind, ExprKind::Text(value) if value == "a\"b\\c\\n"));
        assert!(compile_query(r#"where text == "trailing\"#).is_err());
    }

    #[test]
    fn arithmetic_and_every_binary_level_are_right_associative() {
        let query = compile("where 10 - 3 - 2 == 9");
        let ExprKind::Binary {
            op: BinaryOp::Equal,
            left,
            ..
        } = &query.predicate.kind
        else {
            panic!("equality expected")
        };
        let ExprKind::Binary {
            op: BinaryOp::Subtract,
            right,
            ..
        } = &left.kind
        else {
            panic!("subtraction expected")
        };
        assert!(matches!(
            right.kind,
            ExprKind::Binary {
                op: BinaryOp::Subtract,
                ..
            }
        ));

        let query = compile("where 20 / 5 / 2 == 10");
        let ExprKind::Binary { left, .. } = &query.predicate.kind else {
            panic!("equality expected")
        };
        let ExprKind::Binary { right, .. } = &left.kind else {
            panic!("division expected")
        };
        assert!(matches!(
            right.kind,
            ExprKind::Binary {
                op: BinaryOp::Divide,
                ..
            }
        ));
    }

    #[test]
    fn match_operators_share_one_precedence_level_and_legacy_aliases_parse() {
        assert!(compile_query(
            "where text contains \"x\" startswith \"y\" regex \"z\" endswith \"q\""
        )
        .is_err());
        compile("where text startwith \"x\" | text endwith \"y\"");
        compile("where @alice@example.test in [@alice@example.test]");
        assert!(compile_query("where \"a\" in \"abc\"").is_err());
    }

    #[test]
    fn empty_query_rules_and_legacy_trailing_delimiters_are_explicit() {
        assert!(compile_query("").is_err());
        assert!(compile_query("   \n").is_err());
        assert!(compile_query("where").is_err());
        compile("from local where");
        compile("from local,");
        compile("where [1, 2,] contains 2");
        compile("from home(\"alice\",) where ()");
    }

    #[test]
    fn aliases_resolve_to_the_expected_provider_neutral_fields() {
        let cases: &[(&[&str], Field)] = &[
            (&["body"], Field::Text),
            (&["raw"], Field::RawContent),
            (&["from"], Field::Application),
            (&["application_name"], Field::Application),
            (&["isdm"], Field::DirectMessage),
            (&["is_direct_message"], Field::DirectMessage),
            (&["isretweet"], Field::Boost),
            (&["is_reblog"], Field::Boost),
            (&["replyto"], Field::InReplyTo),
            (&["reply", "account_id"], Field::ReplyAccountId),
            (&["favorers"], Field::FavouritesCount),
            (&["reactions_count"], Field::FavouritesCount),
            (&["retweeters"], Field::ReblogsCount),
            (&["reposts_count"], Field::ReblogsCount),
            (&["followers_only"], Field::IsPrivate),
            (&["media", "descriptions"], Field::MediaDescriptions),
            (&["poll", "options_count"], Field::PollOptionsCount),
            (&["is_quote"], Field::HasQuote),
            (&["tags"], Field::Hashtags),
            (&["user", "screenname"], Field::AuthorAcct),
            (&["user", "username"], Field::AuthorDisplayName),
            (&["author", "username"], Field::AuthorUsername),
            (&["user", "isprotected"], Field::AuthorLocked),
            (&["user", "bio"], Field::AuthorNote),
            (&["user", "friendscount"], Field::AuthorFollowing),
            (&["user", "statusescount"], Field::AuthorStatuses),
            (&["booster", "screen_name"], Field::BoosterAcct),
            (&["retweeter", "username"], Field::BoosterDisplayName),
            (&["booster", "username"], Field::BoosterUsername),
            (&["retweeter", "is_protected"], Field::BoosterLocked),
            (&["viewer", "bookmarked"], Field::ViewerBookmarked),
        ];
        for (path, expected) in cases {
            let owned = path
                .iter()
                .map(|part| (*part).to_string())
                .collect::<Vec<_>>();
            assert_eq!(resolve_field(&owned), Some(*expected), "{path:?}");
        }
    }

    #[test]
    fn unavailable_twitter_and_unscoped_viewer_fields_are_rejected_statically() {
        for field in [
            "protocol",
            "user.verified",
            "user.translator",
            "user.contributors_enabled",
            "user.geo_enabled",
            "user.listed",
            "poll.voted",
            "poll.own_votes",
            "quote.state",
            "list.owner.slug",
        ] {
            let error = compile_query(&format!("where {field} == true")).unwrap_err();
            assert_eq!(
                error.message(),
                "field is unsupported or unavailable",
                "{field}"
            );
        }
    }

    #[test]
    fn regex_is_literal_bounded_case_sensitive_and_does_not_leak_query_text() {
        compile("where text regex \"^[A-Z]+$\"");
        assert!(compile_query("where text regex user.name").is_err());
        let secret = "super-secret-pattern";
        let error = compile_query(&format!("where text regex \"({secret}\"")).unwrap_err();
        assert_eq!(error.message(), "invalid regular expression");
        assert!(!error.to_string().contains(secret));

        let limits = QueryLimits {
            max_regex_bytes: 3,
            ..QueryLimits::default()
        };
        assert!(compile_query_with_limits("where text regex \"abcd\"", limits).is_err());
    }

    #[test]
    fn opaque_id_type_only_coerces_in_identity_contexts() {
        compile("where id == 123");
        compile("where id == \"123\"");
        compile("where id == #\"did:plc:AbC\"");
        compile("where id in [#\"a\", #\"b\"]");
        assert!(compile_query("where text == 123").is_err());
        assert!(compile_query("where id + \"suffix\" == \"x\"").is_err());
        assert!(compile_query("where caseful id == #\"a\"").is_err());
        assert!(compile_query("where id regex \".*\"").is_err());
        assert!(compile_query("where id startswith \"prefix\"").is_err());
        assert!(compile_query("where id < 2").is_err());
    }

    #[test]
    fn integer_literals_cover_the_complete_signed_i64_range() {
        compile("where -9223372036854775808 == -9223372036854775808");
        compile("where 9223372036854775807 == 9223372036854775807");
        assert!(compile_query("where 9223372036854775808 == 0").is_err());
        assert!(compile_query("where -9223372036854775809 == 0").is_err());
    }

    #[test]
    fn utf8_error_offsets_and_all_compile_budgets_are_bounded() {
        let limits = QueryLimits {
            max_query_bytes: 2,
            ..QueryLimits::default()
        };
        let input = "あwhere";
        let error = compile_query_with_limits(input, limits).unwrap_err();
        assert!(input.is_char_boundary(error.offset()));
        assert!(input.is_char_boundary(error.span().end));

        let token_limits = QueryLimits {
            max_tokens: 3,
            ..QueryLimits::default()
        };
        assert!(compile_query_with_limits("where true & true", token_limits).is_err());

        let set_limits = QueryLimits {
            max_set_items: 2,
            ..QueryLimits::default()
        };
        assert!(compile_query_with_limits("where [1,2,3] contains 1", set_limits).is_err());

        let source_limits = QueryLimits {
            max_sources: 2,
            max_source_arguments: 10,
            ..QueryLimits::default()
        };
        assert!(compile_query_with_limits(
            "from home:\"a\", home:\"b\", home:\"c\" where ()",
            source_limits
        )
        .is_err());

        let argument_limits = QueryLimits {
            max_source_arguments: 2,
            ..QueryLimits::default()
        };
        assert!(compile_query_with_limits(
            "from home(\"a\",\"b\",\"c\") where ()",
            argument_limits
        )
        .is_err());

        let nesting =
            "(".repeat(DEFAULT_MAX_DEPTH + 1) + "true" + &")".repeat(DEFAULT_MAX_DEPTH + 1);
        assert!(compile_query(&format!("where {nesting}")).is_err());

        let chain = std::iter::repeat_n("true", DEFAULT_MAX_DEPTH + 1)
            .collect::<Vec<_>>()
            .join(" & ");
        assert!(compile_query(&format!("where {chain}")).is_err());
    }

    #[test]
    fn requirements_only_request_expensive_context_when_referenced() {
        let text = compile("where text contains \"x\"").requirements();
        assert!(text.effective_status);
        assert!(!text.quote_status);
        assert!(!text.viewer_states);
        assert!(!text.login_accounts);

        let home = compile("from home where viewer.favourited").requirements();
        assert!(home.memberships);
        assert!(home.login_accounts);
        assert!(home.viewer_states);

        let quote = compile("where quote.text contains \"x\"").requirements();
        assert!(quote.quote_status);

        let conversation_query = compile("from conversation:\"opaque-id\"");
        assert!(conversation_query.requirements().conversations);
        assert_eq!(conversation_ids(&conversation_query), ["opaque-id"]);
    }

    fn conversation_ids(query: &CompiledQuery) -> &[String] {
        query.conversation_ids()
    }

    #[test]
    fn sql_prefilter_is_conservative_and_bind_only() {
        let query = compile("where text contains \"x' OR 1=1 --\"");
        assert!(query.sql_prefilter().is_empty());
        assert!(!query.sql_prefilter().clause().contains("OR 1=1"));
        assert!(query.sql_prefilter().bindings().is_empty());
    }

    fn test_status(id: &str, account_id: &str, content: &str) -> DbStatus {
        DbStatus {
            id: id.to_string(),
            server_domain: "example.test".to_string(),
            uri: format!("https://example.test/statuses/{id}"),
            url: Some(format!("https://example.test/@user/{id}")),
            created_at: "2026-08-09T00:00:00Z".to_string(),
            edited_at: None,
            account_id: account_id.to_string(),
            content: content.to_string(),
            visibility: "public".to_string(),
            sensitive: false,
            spoiler_text: String::new(),
            reblogs_count: 7,
            favourites_count: 11,
            replies_count: 3,
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            reblog_of_id: None,
            language: Some("en".to_string()),
            pinned: None,
            favourited: None,
            reblogged: None,
            muted: None,
            bookmarked: None,
            poll_json: None,
            card_json: None,
            application_json: Some(r#"{"name":"Awayuki"}"#.to_string()),
            mentions_json: Some("[]".to_string()),
            tags_json: Some("[]".to_string()),
            emojis_json: Some("[]".to_string()),
            media_attachments_json: Some("[]".to_string()),
            fetched_at: "2026-08-09T00:00:00Z".to_string(),
            quote_id: None,
            quote_original_url: None,
        }
    }

    fn test_account(id: &str, username: &str, display_name: &str) -> DbAccount {
        DbAccount {
            id: id.to_string(),
            server_domain: "example.test".to_string(),
            username: username.to_string(),
            acct: username.to_string(),
            display_name: display_name.to_string(),
            note: "<p>Profile <b>note</b></p>".to_string(),
            avatar: String::new(),
            avatar_static: String::new(),
            header: String::new(),
            locked: false,
            bot: false,
            followers_count: 101,
            following_count: 51,
            statuses_count: 201,
            created_at: "2026-08-09T00:00:00Z".to_string(),
            fetched_at: "2026-08-09T00:00:00Z".to_string(),
            fields_json: None,
            emojis_json: None,
        }
    }

    fn login(acct: &str, account_id: &str) -> LoginAccountIdentity {
        LoginAccountIdentity {
            acct: acct.to_string(),
            server_domain: "example.test".to_string(),
            account_id: account_id.to_string(),
            display_name: acct.to_string(),
            server_kind: "mastodon".to_string(),
            is_active: true,
        }
    }

    fn viewer_state(acct: &str, status_id: &str, favourited: bool) -> DbStatusViewerState {
        DbStatusViewerState {
            login_account_acct: acct.to_string(),
            status_id: status_id.to_string(),
            server_domain: "example.test".to_string(),
            favourited: Some(favourited),
            reblogged: Some(false),
            muted: Some(false),
            bookmarked: Some(false),
            pinned: Some(false),
            updated_at: "2026-08-09T00:00:00Z".to_string(),
        }
    }

    fn matches(query: &str, context: &EvaluationContext<'_>) -> bool {
        Evaluator::new().matches(&compile(query), context)
    }

    #[test]
    fn evaluator_preserves_right_associative_arithmetic_and_set_operators() {
        let status = test_status("1", "author-id", "<p>Hello</p>");
        let account = test_account("author-id", "author", "Author");
        let view = StatusView::new(&status, Some(&account));
        let context = EvaluationContext::new(view, Some(view));
        assert!(matches("where 10 - 3 - 2 == 9", &context));
        assert!(matches("where 20 / 5 / 2 == 10", &context));
        assert!(matches("where ([1,2] + [2,3]) contains 3", &context));
        assert!(matches("where ([1,2,3] - [2]) contains 3", &context));
        assert!(matches("where ([1,2] * [2,3]) contains 2", &context));
    }

    #[test]
    fn displayed_text_is_plain_but_raw_content_is_explicit() {
        let status = test_status(
            "AbC",
            "author-id",
            "<p>Hello <b>world</b> &amp; friends</p>",
        );
        let account = test_account("author-id", "author", "Author");
        let view = StatusView::new(&status, Some(&account));
        let context = EvaluationContext::new(view, Some(view));
        assert!(matches(
            "where text contains \"hello world & friends\"",
            &context
        ));
        assert!(!matches("where text contains \"<b>\"", &context));
        assert!(matches("where raw_content contains \"<b>\"", &context));
        assert!(matches(
            "where user.note contains \"profile note\"",
            &context
        ));
        assert!(!matches("where caseful text contains \"hello\"", &context));
        assert!(matches("where text regex \"Hello.*friends\"", &context));
        assert!(!matches("where text regex \"hello.*friends\"", &context));
    }

    #[test]
    fn boost_wrapper_and_effective_original_have_distinct_semantics() {
        let mut wrapper_status = test_status("wrapper", "booster-id", "<p>Wrapper</p>");
        wrapper_status.reblog_of_id = Some("original".to_string());
        let booster = test_account("booster-id", "booster", "Booster");
        let original_status = test_status("original", "author-id", "<p>Original text</p>");
        let author = test_account("author-id", "author", "Author");
        let wrapper = StatusView::new(&wrapper_status, Some(&booster));
        let original = StatusView::new(&original_status, Some(&author));
        let context = EvaluationContext::new(wrapper, Some(original));

        assert!(matches(
            "where boost & user == @author@example.test & booster == @booster@example.test & text contains \"original\"",
            &context
        ));
        assert!(matches(
            "from user:\"booster@example.test\" where user == @author@example.test",
            &context
        ));
        assert!(!matches(
            "from user:\"author@example.test\" where ()",
            &context
        ));

        let unresolved = EvaluationContext::new(wrapper, None);
        assert!(matches("where boost", &unresolved));
        assert!(matches(
            "where booster == @booster@example.test",
            &unresolved
        ));
        assert!(!matches("where text != \"anything\"", &unresolved));
        assert!(!matches("where !(text == \"anything\")", &unresolved));
        assert!(!matches("where user != @someone@example.test", &unresolved));
    }

    #[test]
    fn opaque_ids_are_case_sensitive_and_decimal_compatible() {
        let status = test_status("AbC", "author-id", "<p>Text</p>");
        let account = test_account("author-id", "author", "Author");
        let view = StatusView::new(&status, Some(&account));
        let context = EvaluationContext::new(view, Some(view));
        assert!(matches("where id == #AbC", &context));
        assert!(!matches("where id == #abc", &context));
        assert!(matches("where id == \"AbC\"", &context));

        let numeric = test_status("123", "author-id", "<p>Text</p>");
        let numeric_view = StatusView::new(&numeric, Some(&account));
        let numeric_context = EvaluationContext::new(numeric_view, Some(numeric_view));
        assert!(matches("where id == 123", &numeric_context));
    }

    #[test]
    fn source_or_predicate_and_and_viewer_branch_scopes_are_authoritative() {
        let status = test_status("post", "author-id", "<p>Text</p>");
        let account = test_account("author-id", "author", "Author");
        let view = StatusView::new(&status, Some(&account));
        let logins = vec![
            login("alice@example.test", "alice-id"),
            login("bob@example.test", "bob-id"),
        ];
        let memberships = vec![
            TimelineMembership::new("home", "alice@example.test", None),
            TimelineMembership::new("home", "bob@example.test", None),
            TimelineMembership::new("list", "alice@example.test", Some("list-1".to_string())),
            TimelineMembership::new("list", "bob@example.test", Some("list-1".to_string())),
            TimelineMembership::new("hashtag", "alice@example.test", Some("rust".to_string())),
            TimelineMembership::new("hashtag", "bob@example.test", Some("rust".to_string())),
        ];
        let states = vec![
            viewer_state("alice@example.test", "post", false),
            viewer_state("bob@example.test", "post", true),
        ];
        let mut context = EvaluationContext::new(view, Some(view));
        context.login_accounts = &logins;
        context.memberships = &memberships;
        context.viewer_states = &states;

        assert!(!matches(
            "from home:\"alice@example.test\" where viewer.favourited",
            &context
        ));
        assert!(matches(
            "from home:\"bob@example.test\" where viewer.favourited",
            &context
        ));
        assert!(!matches("from home where viewer.favourited", &context));
        assert!(matches("from home where ()", &context));
        assert!(matches(
            "from home:\"alice@example.test\",\"bob@example.test\" where viewer.favourited",
            &context
        ));
        assert!(!matches(
            "from list:\"list-1\" where viewer.favourited",
            &context
        ));
        assert!(matches(
            "from list:\"bob@example.test/list-1\" where viewer.favourited",
            &context
        ));
        assert!(!matches(
            "from hashtag:\"rust\" where viewer.favourited",
            &context
        ));

        let mut bookmarked = states.clone();
        bookmarked[1].bookmarked = Some(true);
        context.viewer_states = &bookmarked;
        assert!(matches("from bookmarks where viewer.bookmarked", &context));
    }

    #[test]
    fn conversation_and_public_status_extensions_use_only_available_cache_data() {
        let mut status = test_status("post", "author-id", "<p>Text</p>");
        status.visibility = "unlisted".to_string();
        status.sensitive = true;
        status.edited_at = Some("2026-08-09T01:00:00Z".to_string());
        let account = test_account("author-id", "author", "Author");
        let view = StatusView::new(&status, Some(&account));
        let keys = vec![StatusKey::new("example.test", "post")];
        let mut context = EvaluationContext::new(view, Some(view));
        context.conversation_keys = &keys;
        assert!(matches(
            "from conversation:\"root\" where unlisted & sensitive & edited & favs == 11 & rts == 7 & replies == 3 & domain == \"example.test\"",
            &context
        ));
    }
}
