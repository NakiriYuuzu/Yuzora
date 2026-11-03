//! `SqlEditor`: gpui-base's code editor with a small SQL highlighter of our
//! own (keywords, types, strings, numbers, comments), until L5 brings a
//! shared one.
//!
//! The script names an editor by id and asks for its text with
//! [`text`] (everything, the selection, the statement under the cursor);
//! ⌘↵ / Ctrl+Enter is sent as `"run"` through [`actions`] instead of
//! inserting a line.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex};

use gpui::{
    AnyElement, App, AppContext, Context, Entity, HighlightStyle, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, Styled, Window, div,
};
use gpui_base::input::{
    Editor, EditorState, Enter, FoldRange, HighlightStyleResolver, InputBase, InputEdit, InputEditorStyle,
    InputHighlighter, Rope,
};
use serde::Serialize;
use serde_json::Value;

use crate::config::Engine;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Text {
    pub text: String,
    pub selection: String,
    /// `None`: where the statement under the cursor ends depends on the
    /// session ([`crate::sql::runnable_statement_at`]); it is not run alone.
    pub statement: Option<String>,
}

struct Registered {
    state: Entity<EditorState>,
    engine: Engine,
    /// Text a script set before the editor drew again (setting needs a window).
    pending: Option<String>,
}

thread_local! {
    /// Editors by id, kept while their pane lives (a pane in another tab
    /// keeps its text). [`release`] drops them.
    static EDITORS: RefCell<HashMap<String, Registered>> = RefCell::new(HashMap::new());
}

/// Drops the editors whose id starts with `prefix` (`"<owner>:"`).
pub fn release(prefix: &str) {
    EDITORS.with(|all| all.borrow_mut().retain(|id, _| !id.starts_with(prefix)));
    ACTIONS.lock().unwrap_or_else(|e| e.into_inner()).retain(|id, _| !id.starts_with(prefix));
}

type Actions = (async_channel::Sender<String>, async_channel::Receiver<String>);

static ACTIONS: LazyLock<Mutex<HashMap<String, Actions>>> = LazyLock::new(Default::default);

fn channel(id: &str) -> Actions {
    ACTIONS.lock().unwrap_or_else(|e| e.into_inner()).entry(id.to_owned()).or_insert_with(async_channel::unbounded).clone()
}

/// The next action of editor `id` (`"run"`).
pub fn actions(id: &str) -> async_channel::Receiver<String> {
    channel(id).1
}

/// The element for `SqlEditor(id, {engine, value})`.
pub fn element(id: &str, props: &Value, window: &mut Window, cx: &mut App) -> AnyElement {
    let engine = match props.get("engine").and_then(Value::as_str) {
        Some("mssql") => Engine::Mssql,
        Some("sqlite") => Engine::Sqlite,
        _ => Engine::Postgres,
    };
    let initial = props.get("value").and_then(Value::as_str).unwrap_or("").to_owned();
    let readonly = props.get("readonly").and_then(Value::as_bool).unwrap_or(false);
    let existing = EDITORS.with(|all| {
        let mut all = all.borrow_mut();
        let entry = all.get_mut(id)?;
        entry.engine = engine;
        Some((entry.state.clone(), entry.pending.take()))
    });
    let state = match existing {
        Some((state, pending)) => {
            if let Some(text) = pending {
                state.update(cx, |s, cx| s.set_value(text, window, cx));
            }
            state
        }
        None => {
            let state = cx.new(|cx| {
                let mut state =
                    EditorState::new(window, cx).language("sql").line_number(!readonly).folding(false).soft_wrap(false);
                state.set_editor_style(InputEditorStyle { highlight_styles: Arc::new(SqlColors), ..Default::default() });
                state.set_highlighter_factory(
                    Rc::new(|_| Some(Box::new(SqlHighlighter::default()) as Box<dyn InputHighlighter>)),
                    cx,
                );
                state.set_readonly(readonly, cx);
                state
            });
            state.update(cx, |s, cx| s.set_value(initial, window, cx));
            EDITORS.with(|all| {
                all.borrow_mut().insert(id.to_owned(), Registered { state: state.clone(), engine, pending: None })
            });
            state
        }
    };
    let typography = &gpui_base::Theme::global(cx).tokens.typography;
    let (font, size) = (typography.mono.clone(), typography.mono_md.size);
    let sender = channel(id).0;
    let focus_target = state.clone();
    div()
        .size_full()
        .min_h_0()
        .flex()
        // ⌘↵ runs rather than inserting a line: catch it before the editor does.
        .capture_action(move |action: &Enter, _: &mut Window, cx: &mut App| {
            if action.secondary {
                let _ = sender.try_send("run".into());
                cx.stop_propagation();
            }
        })
        .child(
            InputBase::new(SharedString::from(format!("yuzora-db-editor-frame:{id}")))
                .flex()
                .items_start()
                .w_full()
                .h_full()
                .min_h_0()
                .font_family(font)
                .text_size(size)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    focus_target.update(cx, |s, cx| s.focus(window, cx));
                })
                .child(Editor::new(&state)),
        )
        .into_any_element()
}

fn live(id: &str) -> Option<(Entity<EditorState>, Engine)> {
    EDITORS.with(|all| all.borrow().get(id).map(|r| (r.state.clone(), r.engine)))
}

/// What editor `id` holds now (`None` when it isn't on screen).
pub fn text(id: &str) -> Option<Text> {
    let (state, engine) = live(id)?;
    gpui_shell::with_current_app(|cx| {
        let s = state.read(cx);
        let text = s.value().to_string();
        let cursor = s.cursor();
        let statement = crate::sql::runnable_statement_at(&text, cursor, engine);
        Text { selection: s.selected_value().to_string(), statement, text }
    })
}

/// Replaces editor `id`'s text (applied the next time it draws). An editor
/// not made yet starts with its `value` prop instead.
pub fn set_text(id: &str, text: &str) {
    EDITORS.with(|all| {
        if let Some(r) = all.borrow_mut().get_mut(id) {
            r.pending = Some(text.to_owned());
        }
    });
    yuzora_core::js::request_refresh();
}

// ---------------------------------------------------------------- highlighting

/// Keywords, types and functions, uppercase.
const KEYWORDS: &[&str] = &[
    "ADD", "ALL", "ALTER", "AND", "ANY", "AS", "ASC", "BEGIN", "BETWEEN", "BY", "CASCADE", "CASE", "CAST", "CHECK",
    "COLUMN", "COMMIT", "CONSTRAINT", "CREATE", "CROSS", "CURRENT_DATE", "CURRENT_TIMESTAMP", "DATABASE", "DECLARE",
    "DEFAULT", "DELETE", "DESC", "DISTINCT", "DO", "DROP", "ELSE", "END", "EXCEPT", "EXEC", "EXECUTE", "EXISTS",
    "EXPLAIN", "FETCH", "FOREIGN", "FROM", "FULL", "FUNCTION", "GO", "GRANT", "GROUP", "HAVING", "IF", "ILIKE", "IN",
    "INDEX", "INNER", "INSERT", "INTERSECT", "INTO", "IS", "JOIN", "KEY", "LEFT", "LIKE", "LIMIT", "MATERIALIZED",
    "MERGE", "NOT", "NULL", "OFFSET", "ON", "OR", "ORDER", "OUTER", "OUTPUT", "OVER", "PARTITION", "PRIMARY",
    "PROCEDURE", "REFERENCES", "RENAME", "REPLACE", "RETURNING", "RETURNS", "REVOKE", "RIGHT", "ROLLBACK", "SCHEMA",
    "SELECT", "SEQUENCE", "SET", "TABLE", "THEN", "TO", "TOP", "TRANSACTION", "TRIGGER", "TRUNCATE", "UNION",
    "UNIQUE", "UPDATE", "USING", "VALUES", "VIEW", "WHEN", "WHERE", "WINDOW", "WITH",
];

const TYPES: &[&str] = &[
    "BIGINT", "BIGSERIAL", "BINARY", "BIT", "BLOB", "BOOL", "BOOLEAN", "BYTEA", "CHAR", "CHARACTER", "DATE",
    "DATETIME", "DATETIME2", "DATETIMEOFFSET", "DECIMAL", "DOUBLE", "FLOAT", "FLOAT4", "FLOAT8", "INT", "INT2", "INT4",
    "INT8", "INTEGER", "INTERVAL", "JSON", "JSONB", "MONEY", "NCHAR", "NUMERIC", "NVARCHAR", "REAL", "SERIAL",
    "SMALLINT", "TEXT", "TIME", "TIMESTAMP", "TIMESTAMPTZ", "TIMETZ", "TINYINT", "UNIQUEIDENTIFIER", "UUID",
    "VARBINARY", "VARCHAR", "XML",
];

const CONSTANTS: &[&str] = &["TRUE", "FALSE"];

/// Splits SQL into highlighted runs: (byte range, semantic name).
pub fn tokens(text: &str) -> Vec<(Range<usize>, &'static str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let start = i;
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            out.push((start..i, "comment"));
        } else if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            out.push((start..i, "comment"));
        } else if b == b'\'' || ((b == b'N' || b == b'E' || b == b'n' || b == b'e') && bytes.get(i + 1) == Some(&b'\'')) {
            if b != b'\'' {
                i += 1;
            }
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if bytes.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push((start..i, "string"));
        } else if b == b'$' && let Some(end) = dollar_quote_end(text, i) {
            i = end;
            out.push((start..i, "string"));
        } else if b.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            out.push((start..i, "number"));
        } else if b.is_ascii_alphabetic() || b == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = text[start..i].to_ascii_uppercase();
            let next = text[i..].trim_start().starts_with('(');
            let name = if KEYWORDS.binary_search(&word.as_str()).is_ok() {
                Some("keyword")
            } else if TYPES.binary_search(&word.as_str()).is_ok() {
                Some("type")
            } else if CONSTANTS.contains(&word.as_str()) {
                Some("boolean")
            } else if next {
                Some("function")
            } else {
                None
            };
            if let Some(name) = name {
                out.push((start..i, name));
            }
        } else {
            // One whole character (it may be several bytes).
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// `$tag$ … $tag$` starting at `i`: where it ends.
fn dollar_quote_end(text: &str, i: usize) -> Option<usize> {
    let rest = &text[i + 1..];
    let tag_len = rest.find('$')?;
    let tag = &rest[..tag_len];
    if !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || tag.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let fence = format!("${tag}$");
    let body = i + fence.len();
    Some(text[body..].find(&fence).map_or(text.len(), |e| body + e + fence.len()))
}

#[derive(Default)]
struct SqlHighlighter {
    runs: Vec<(Range<usize>, &'static str)>,
}

impl InputHighlighter for SqlHighlighter {
    fn language(&self) -> SharedString {
        "sql".into()
    }

    fn update(&mut self, _: Option<InputEdit>, text: &Rope, _: bool, _: &mut Window, _: &mut Context<EditorState>) {
        self.runs = tokens(&text.to_string());
    }

    fn styles(&self, range: &Range<usize>, resolver: &dyn HighlightStyleResolver) -> Vec<(Range<usize>, HighlightStyle)> {
        let first = self.runs.partition_point(|(r, _)| r.end <= range.start);
        let mut out = Vec::new();
        let mut cursor = range.start;
        for (r, name) in &self.runs[first..] {
            if r.start >= range.end {
                break;
            }
            let (start, end) = (r.start.max(range.start), r.end.min(range.end));
            if start >= end || end <= cursor {
                continue;
            }
            if cursor < start {
                out.push((cursor..start, HighlightStyle::default()));
            }
            out.push((start..end, resolver.style(name).unwrap_or_default()));
            cursor = end;
        }
        if cursor < range.end {
            out.push((cursor..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

/// The design's SQL colors (p4): tokens.json `sql`, its light part while a light theme shows.
struct SqlColors;

/// A highlight's color for a dark (`false`) or light (`true`) theme.
fn sql_color(light: bool, name: &str) -> Option<gpui::Hsla> {
    static SQL: LazyLock<[Value; 2]> = LazyLock::new(|| [false, true].map(|l| yuzora_core::tokens::for_theme(l)["sql"].clone()));
    let key = match name {
        "type" | "boolean" => "type",
        "keyword" | "string" | "number" | "comment" | "function" => name,
        _ => return None,
    };
    crate::grid::hex(SQL[usize::from(light)][key].as_str()?)
}

impl HighlightStyleResolver for SqlColors {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        Some(HighlightStyle { color: Some(sql_color(yuzora_core::tokens::light(), name)?), ..Default::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(text: &str) -> Vec<(&str, &'static str)> {
        tokens(text).into_iter().map(|(r, n)| (&text[r], n)).collect()
    }

    #[test]
    fn word_lists_are_sorted_for_binary_search() {
        assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]));
        assert!(TYPES.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn highlights_keywords_types_strings_and_comments() {
        assert_eq!(
            named("ALTER TABLE agent_sessions ADD COLUMN seen_at timestamptz;"),
            [("ALTER", "keyword"), ("TABLE", "keyword"), ("ADD", "keyword"), ("COLUMN", "keyword"), ("timestamptz", "type")]
        );
        assert_eq!(
            named("select count(*) from t where s in ('blocked', 'it''s') -- done\n/* x */ 42"),
            [
                ("select", "keyword"),
                ("count", "function"),
                ("from", "keyword"),
                ("where", "keyword"),
                ("in", "keyword"),
                ("'blocked'", "string"),
                ("'it''s'", "string"),
                ("-- done", "comment"),
                ("/* x */", "comment"),
                ("42", "number"),
            ]
        );
        assert_eq!(named("$$ select 1 $$ N'中文'"), [("$$ select 1 $$", "string"), ("N'中文'", "string")]);
    }

    #[test]
    fn sql_colors_follow_the_theme() {
        let hex = |s| crate::grid::hex(s);
        // Dark keeps the design's colors; light takes tokens.json's light part.
        assert_eq!(sql_color(false, "keyword"), hex("#c678dd"));
        assert_eq!(sql_color(true, "keyword"), hex("#9333c4"));
        assert_eq!(sql_color(true, "boolean"), sql_color(true, "type"));
        for light in [false, true] {
            for name in ["keyword", "type", "boolean", "string", "number", "comment", "function"] {
                assert!(sql_color(light, name).is_some(), "light={light}: {name}");
            }
            assert_eq!(sql_color(light, "punctuation"), None);
        }
    }
}
