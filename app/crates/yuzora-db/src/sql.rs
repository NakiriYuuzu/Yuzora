//! STUB: SQL text helpers (statement splitting), replaced by the implementation.

use std::ops::Range;

use crate::config::Engine;

/// One statement of a script: its text (trimmed, without the separator) and
/// where it sits in the source (byte range).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub text: String,
    pub range: Range<usize>,
}

/// Splits `sql` into statements for `engine`.
pub fn split(sql: &str, _engine: Engine) -> Vec<Statement> {
    let text = sql.trim();
    if text.is_empty() { Vec::new() } else { vec![Statement { text: text.to_owned(), range: 0..sql.len() }] }
}

/// The statement under `offset` (between two, the one before).
pub fn statement_at(sql: &str, offset: usize, engine: Engine) -> Option<Statement> {
    let all = split(sql, engine);
    all.iter().find(|s| s.range.contains(&offset)).or_else(|| all.iter().rev().find(|s| s.range.end <= offset)).cloned()
}
