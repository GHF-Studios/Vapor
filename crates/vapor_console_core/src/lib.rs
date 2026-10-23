//! Host-neutral console records, bounded view state, and editor language rules.
//!
//! This crate deliberately has no process, terminal, GUI, game, or persistence
//! access. Hosts adapt their own events and capabilities to these shared types.

mod diagnostic;

pub use diagnostic::{
    DiagnosticContext, DiagnosticOrigin, DiagnosticScope, DiagnosticSeverity, DiagnosticSpan,
    StructuredDiagnostic,
};

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Error returned when a quoted command argument is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandLineError {
    /// The line ended while an argument was quoted.
    UnterminatedQuote,
    /// The line ended immediately after an escape character.
    TrailingEscape,
}

/// Split a console command line into arguments while preserving quoted spaces.
///
/// Single and double quotes group text into one argument. A backslash escapes
/// the following character outside single quotes and inside double quotes.
pub fn tokenize_command_line(input: &str) -> Result<Vec<String>, CommandLineError> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;

    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }

        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => current.push(character),
            (Some('"'), '\\') => escaped = true,
            (Some(_), _) => current.push(character),
            (None, '\'') | (None, '"') => {
                quote = Some(character);
                started = true;
            }
            (None, '\\') => {
                escaped = true;
                started = true;
            }
            (None, character) if character.is_whitespace() => {
                if started {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, _) => {
                current.push(character);
                started = true;
            }
        }
    }

    if escaped {
        return Err(CommandLineError::TrailingEscape);
    }
    if quote.is_some() {
        return Err(CommandLineError::UnterminatedQuote);
    }
    if started {
        arguments.push(current);
    }
    Ok(arguments)
}

/// Severity shared by terminal, GUI, and in-game console adapters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsoleLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Success,
    Warn,
    Error,
    Input,
    System,
}

/// Visual or terminal style intent. Hosts map these roles to their own palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsoleStyleRole {
    Muted,
    Trace,
    Debug,
    Normal,
    Success,
    Warning,
    Error,
    Input,
    Metadata,
}

impl ConsoleLevel {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Success => "SUCCESS",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Input => "INPUT",
            Self::System => "SYSTEM",
        }
    }

    pub const fn style_role(self) -> ConsoleStyleRole {
        match self {
            Self::Trace => ConsoleStyleRole::Trace,
            Self::Debug => ConsoleStyleRole::Debug,
            Self::Info | Self::System => ConsoleStyleRole::Normal,
            Self::Success => ConsoleStyleRole::Success,
            Self::Warn => ConsoleStyleRole::Warning,
            Self::Error => ConsoleStyleRole::Error,
            Self::Input => ConsoleStyleRole::Input,
        }
    }
}

/// Transport origin or subsystem name. Kept open so hosts can add sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleSource(pub String);

/// A structured field that remains separate until a host renders the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleField {
    pub key: String,
    pub value: String,
}

/// One canonical event in console history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleRecord {
    pub timestamp_unix_ms: u64,
    pub level: ConsoleLevel,
    pub source: ConsoleSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub message: String,
    #[serde(default)]
    pub fields: Vec<ConsoleField>,
}

impl ConsoleRecord {
    pub fn new(
        timestamp_unix_ms: u64,
        level: ConsoleLevel,
        source: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            timestamp_unix_ms,
            level,
            source: ConsoleSource(source.into()),
            target: None,
            message: message.into(),
            fields: Vec::new(),
        }
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    pub fn field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push(ConsoleField {
            key: key.into(),
            value: value.into(),
        });
        self
    }
}

/// Remove terminal control sequences while preserving readable progress output.
pub fn plain_terminal_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            },
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                output.push('\n');
            }
            '\n' | '\t' => output.push(character),
            value if value.is_control() => {}
            value => output.push(value),
        }
    }
    output
}

/// Shared console view filters. Empty values mean no filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsoleFilter {
    pub levels: Vec<ConsoleLevel>,
    pub source: Option<String>,
    pub origin: Option<String>,
    pub composition: Option<String>,
    pub domain: Option<String>,
    pub scope: Option<String>,
    pub search: String,
}

impl ConsoleFilter {
    pub fn matches(&self, record: &ConsoleRecord) -> bool {
        if !self.levels.is_empty() && !self.levels.contains(&record.level) {
            return false;
        }
        if self
            .source
            .as_deref()
            .is_some_and(|source| !record.source.0.eq_ignore_ascii_case(source))
        {
            return false;
        }
        if self.origin.as_deref().is_some_and(|origin| {
            !record
                .fields
                .iter()
                .any(|field| field.key == "origin" && field.value.eq_ignore_ascii_case(origin))
        }) {
            return false;
        }
        if self.composition.as_deref().is_some_and(|composition| {
            !record.fields.iter().any(|field| {
                field.key == "composition" && field.value.eq_ignore_ascii_case(composition)
            })
        }) {
            return false;
        }
        if self.domain.as_deref().is_some_and(|domain| {
            !record.fields.iter().any(|field| {
                field.key == "domain" && field.value.eq_ignore_ascii_case(domain)
            })
        }) {
            return false;
        }
        if self.scope.as_deref().is_some_and(|scope| {
            !record.fields.iter().any(|field| {
                field.key == "scope" && field.value.eq_ignore_ascii_case(scope)
            })
        }) {
            return false;
        }
        let query = self.search.trim().to_lowercase();
        query.is_empty()
            || record.message.to_lowercase().contains(&query)
            || record.source.0.to_lowercase().contains(&query)
            || record
                .fields
                .iter()
                .any(|field| field.key == "origin" && field.value.to_lowercase().contains(&query))
            || record
                .target
                .as_ref()
                .is_some_and(|target| target.to_lowercase().contains(&query))
            || record.fields.iter().any(|field| {
                field.key.to_lowercase().contains(&query)
                    || field.value.to_lowercase().contains(&query)
            })
    }
}

/// Bounded scrollback shared across adapters. Clearing affects this view only.
#[derive(Debug, Clone)]
pub struct ConsoleHistory {
    capacity: usize,
    records: VecDeque<ConsoleRecord>,
    dropped: u64,
    revision: u64,
}

impl ConsoleHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            records: VecDeque::new(),
            dropped: 0,
            revision: 0,
        }
    }

    pub fn push(&mut self, record: ConsoleRecord) {
        if self.records.len() == self.capacity {
            self.records.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.records.push_back(record);
        self.revision = self.revision.saturating_add(1);
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.revision = self.revision.saturating_add(1);
    }

    pub fn records(&self) -> impl DoubleEndedIterator<Item = &ConsoleRecord> {
        self.records.iter()
    }

    /// Read a record by its current FIFO position without materializing scrollback.
    pub fn record_at(&self, index: usize) -> Option<&ConsoleRecord> {
        self.records.get(index)
    }

    /// Monotonic change marker for view caches derived from the record sequence.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn filtered<'a>(
        &'a self,
        filter: &'a ConsoleFilter,
    ) -> impl DoubleEndedIterator<Item = &'a ConsoleRecord> {
        self.records.iter().filter(|record| filter.matches(record))
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Bounded command prompt history; execution and permissions remain host-owned.
#[derive(Debug, Clone)]
pub struct PromptHistory {
    capacity: usize,
    commands: VecDeque<String>,
}

impl PromptHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            commands: VecDeque::new(),
        }
    }

    pub fn push(&mut self, command: impl Into<String>) {
        let command = command.into();
        if command.trim().is_empty() || self.commands.back() == Some(&command) {
            return;
        }
        if self.commands.len() == self.capacity {
            self.commands.pop_front();
        }
        self.commands.push_back(command);
    }

    pub fn commands(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.commands.iter().map(String::as_str)
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.commands.get(index).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl Default for PromptHistory {
    fn default() -> Self {
        Self::new(128)
    }
}

/// Token categories used by editor adapters. Ranges index the original UTF-8 source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxCategory {
    Keyword,
    Type,
    Literal,
    String,
    Number,
    Comment,
    Identifier,
    Punctuation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxToken {
    pub range: std::ops::Range<usize>,
    pub category: SyntaxCategory,
}

/// Shared lexical rules for Rhai snippets. Hosts map these categories to widgets.
pub struct RhaiLanguage;

impl RhaiLanguage {
    pub const fn keywords() -> &'static [&'static str] {
        RHAI_KEYWORDS
    }

    pub const fn types() -> &'static [&'static str] {
        RHAI_TYPES
    }

    pub const fn literals() -> &'static [&'static str] {
        RHAI_LITERALS
    }

    pub fn tokenize(source: &str) -> Vec<SyntaxToken> {
        let mut tokens = Vec::new();
        let mut cursor = 0;
        while cursor < source.len() {
            let tail = &source[cursor..];
            let character = tail
                .chars()
                .next()
                .expect("cursor remains on a char boundary");
            if character.is_whitespace() {
                cursor += character.len_utf8();
                continue;
            }
            let start = cursor;
            let (category, end) = if tail.starts_with("//") {
                let end = tail
                    .find('\n')
                    .map_or(source.len(), |newline| cursor + newline);
                (SyntaxCategory::Comment, end)
            } else if tail.starts_with("/*") {
                let end = tail
                    .find("*/")
                    .map_or(source.len(), |close| cursor + close + 2);
                (SyntaxCategory::Comment, end)
            } else if character == '"' || character == '\'' {
                (
                    SyntaxCategory::String,
                    scan_quoted(source, cursor, character),
                )
            } else if character.is_ascii_digit() {
                let mut end = cursor + character.len_utf8();
                for next in source[end..].chars() {
                    if next.is_ascii_alphanumeric() || matches!(next, '_' | '.') {
                        end += next.len_utf8();
                    } else {
                        break;
                    }
                }
                (SyntaxCategory::Number, end)
            } else if is_identifier_start(character) {
                let mut end = cursor + character.len_utf8();
                for next in source[end..].chars() {
                    if is_identifier_continue(next) {
                        end += next.len_utf8();
                    } else {
                        break;
                    }
                }
                let identifier = &source[start..end];
                let category = if RHAI_KEYWORDS.contains(&identifier) {
                    SyntaxCategory::Keyword
                } else if RHAI_TYPES.contains(&identifier) {
                    SyntaxCategory::Type
                } else if RHAI_LITERALS.contains(&identifier) {
                    SyntaxCategory::Literal
                } else {
                    SyntaxCategory::Identifier
                };
                (category, end)
            } else {
                (SyntaxCategory::Punctuation, cursor + character.len_utf8())
            };
            tokens.push(SyntaxToken {
                range: start..end,
                category,
            });
            cursor = end;
        }
        tokens
    }
}

const RHAI_KEYWORDS: &[&str] = &[
    "as", "break", "catch", "const", "continue", "else", "export", "fn", "for", "if", "import",
    "in", "let", "loop", "private", "return", "throw", "try", "while",
];
const RHAI_TYPES: &[&str] = &[
    "Array", "Blob", "bool", "char", "Dynamic", "FnPtr", "float", "int", "Map", "string", "Unit",
];
const RHAI_LITERALS: &[&str] = &["false", "this", "true"];

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn scan_quoted(source: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    let mut cursor = start + quote.len_utf8();
    while cursor < source.len() {
        let character = source[cursor..]
            .chars()
            .next()
            .expect("cursor remains on a char boundary");
        cursor += character.len_utf8();
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            break;
        }
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_output_removes_color_control_codes_and_keeps_lines() {
        assert_eq!(
            plain_terminal_text("\u{1b}[32mFinished\u{1b}[0m\r\nready\rworking"),
            "Finished\nready\nworking"
        );
    }

    #[test]
    fn console_history_is_bounded_and_clear_only_clears_the_view() {
        let mut history = ConsoleHistory::new(2);
        history.push(ConsoleRecord::new(1, ConsoleLevel::Info, "game", "one"));
        history.push(ConsoleRecord::new(2, ConsoleLevel::Warn, "game", "two"));
        history.push(ConsoleRecord::new(3, ConsoleLevel::Error, "vapor", "three"));
        assert_eq!(
            history
                .records()
                .map(|record| record.message.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
        assert_eq!(history.dropped_count(), 1);
        history.clear();
        assert_eq!(history.records().count(), 0);
        assert_eq!(history.dropped_count(), 1);
    }

    #[test]
    fn console_history_revision_tracks_push_and_clear_for_view_caches() {
        let mut history = ConsoleHistory::new(2);
        let initial = history.revision();
        history.push(ConsoleRecord::new(1, ConsoleLevel::Info, "test", "first"));
        let after_push = history.revision();
        assert!(after_push > initial);

        history.clear();
        assert!(history.revision() > after_push);
    }

    #[test]
    fn console_history_exposes_fifo_indexed_records_for_virtual_views() {
        let mut history = ConsoleHistory::new(2);
        history.push(ConsoleRecord::new(1, ConsoleLevel::Info, "test", "first"));
        history.push(ConsoleRecord::new(2, ConsoleLevel::Info, "test", "second"));
        history.push(ConsoleRecord::new(3, ConsoleLevel::Info, "test", "third"));

        assert_eq!(history.record_at(0).map(|record| record.message.as_str()), Some("second"));
        assert_eq!(history.record_at(1).map(|record| record.message.as_str()), Some("third"));
        assert!(history.record_at(2).is_none());
    }

    #[test]
    fn filters_match_structured_sources_fields_and_severity() {
        let record = ConsoleRecord::new(1, ConsoleLevel::Warn, "game", "contact lost")
            .target("ship")
            .field("sector", "luna")
            .field("origin", "studio")
            .field("composition", "loo-cast")
            .field("domain", "rust")
            .field("scope", "document");
        assert!(
            ConsoleFilter {
                levels: vec![ConsoleLevel::Warn],
                source: Some("GAME".to_owned()),
                origin: Some("STUDIO".to_owned()),
                composition: Some("LOO-CAST".to_owned()),
                domain: None,
                scope: None,
                search: "luna".to_owned(),
            }
            .matches(&record)
        );
        assert!(
            !ConsoleFilter {
                levels: vec![ConsoleLevel::Error],
                ..Default::default()
            }
            .matches(&record)
        );
        assert!(
            !ConsoleFilter {
                origin: Some("launcher".to_owned()),
                ..Default::default()
            }
            .matches(&record)
        );
        assert!(
            !ConsoleFilter {
                composition: Some("terminal-wheel".to_owned()),
                ..Default::default()
            }
            .matches(&record)
        );
        assert!(ConsoleFilter {
            domain: Some("RUST".to_owned()),
            ..Default::default()
        }.matches(&record));
        assert!(!ConsoleFilter {
            scope: Some("workspace".to_owned()),
            ..Default::default()
        }.matches(&record));
    }

    #[test]
    fn prompt_history_bounds_and_deduplicates_only_consecutive_commands() {
        let mut history = PromptHistory::new(2);
        history.push("help");
        history.push("help");
        history.push("status");
        history.push("help");
        assert_eq!(history.commands().collect::<Vec<_>>(), ["status", "help"]);
    }

    #[test]
    fn rhai_lexer_shares_language_categories_and_utf8_ranges() {
        let source = "let café = true; // script\nlet text = \"雪\\\"\";";
        let tokens = RhaiLanguage::tokenize(source);
        let token_text = |token: &SyntaxToken| &source[token.range.clone()];
        assert!(tokens.iter().any(|token| {
            token.category == SyntaxCategory::Keyword && token_text(token) == "let"
        }));
        assert!(tokens.iter().any(|token| {
            token.category == SyntaxCategory::Identifier && token_text(token) == "café"
        }));
        assert!(tokens.iter().any(|token| {
            token.category == SyntaxCategory::Literal && token_text(token) == "true"
        }));
        assert!(tokens.iter().any(|token| {
            token.category == SyntaxCategory::Comment && token_text(token).contains("script")
        }));
        assert!(tokens.iter().any(|token| {
            token.category == SyntaxCategory::String && token_text(token).contains("雪")
        }));
    }

    #[test]
    fn command_line_parser_preserves_quoted_arguments_and_rejects_unclosed_quotes() {
        assert_eq!(
            tokenize_command_line("workspace open \"/home/user/My Project\""),
            Ok(vec![
                "workspace".to_owned(),
                "open".to_owned(),
                "/home/user/My Project".to_owned(),
            ])
        );
        assert!(tokenize_command_line("workspace open \"/home/user/My Project").is_err());
    }
}
