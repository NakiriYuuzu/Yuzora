//! Engine-neutral shapes: cells, columns, schema objects, edits.

use std::borrow::Cow;
use std::cmp::Ordering;

use serde::Serialize;

/// One value as the grid holds it. PostgreSQL rows arrive in the server's
/// text format, so their cells are `Text` and the column says whether it is
/// numeric; SQL Server and SQLite keep their native kinds.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(Box<str>),
    /// Raw bytes (shown as `0x…`, at most the first 64 bytes).
    Bytes(Box<[u8]>),
}

impl Cell {
    pub fn text(s: impl Into<Box<str>>) -> Self {
        Cell::Text(s.into())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Cell::Null)
    }

    /// What a cell shows; NULL shows as `NULL` (the grid dims it).
    pub fn display(&self) -> Cow<'_, str> {
        match self {
            Cell::Null => Cow::Borrowed("NULL"),
            Cell::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
            Cell::Int(i) => Cow::Owned(i.to_string()),
            Cell::Float(f) => Cow::Owned(format_float(*f)),
            Cell::Text(s) => Cow::Borrowed(s),
            Cell::Bytes(b) => Cow::Owned(hex(b)),
        }
    }

    /// What copying a cell puts on the clipboard: NULL copies as empty.
    pub fn copy_text(&self) -> Cow<'_, str> {
        match self {
            Cell::Null => Cow::Borrowed(""),
            other => other.display(),
        }
    }

    /// Sort order: NULL first, numbers by value (also numeric text), then text.
    pub fn compare(&self, other: &Cell, numeric: bool) -> Ordering {
        fn number(c: &Cell, numeric: bool) -> Option<f64> {
            match c {
                Cell::Int(i) => Some(*i as f64),
                Cell::Float(f) => Some(*f),
                Cell::Bool(b) => Some(*b as i64 as f64),
                Cell::Text(s) if numeric => s.trim().parse().ok(),
                _ => None,
            }
        }
        match (self, other) {
            (Cell::Null, Cell::Null) => Ordering::Equal,
            (Cell::Null, _) => Ordering::Less,
            (_, Cell::Null) => Ordering::Greater,
            _ => match (number(self, numeric), number(other, numeric)) {
                (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
                _ => self.display().cmp(&other.display()),
            },
        }
    }
}

fn format_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const SHOWN: usize = 64;
    let mut out = String::with_capacity(2 + bytes.len().min(SHOWN) * 2 + 1);
    out.push_str("0x");
    for b in bytes.iter().take(SHOWN) {
        out.push_str(&format!("{b:02X}"));
    }
    if bytes.len() > SHOWN {
        out.push('…');
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// The engine's type name (`int4`, `nvarchar`, `TEXT`), empty when unknown.
    pub type_name: String,
    /// Right-aligned, sorted by value.
    pub numeric: bool,
}

impl Column {
    pub fn new(name: impl Into<String>, type_name: impl Into<String>) -> Self {
        let type_name = type_name.into();
        Self { name: name.into(), numeric: is_numeric_type(&type_name), type_name }
    }
}

/// Integer, decimal and floating types across the three engines.
pub fn is_numeric_type(type_name: &str) -> bool {
    let t = type_name.trim().to_ascii_lowercase();
    let base = t.split('(').next().unwrap_or("").trim();
    matches!(
        base,
        "int2" | "int4" | "int8" | "smallint" | "integer" | "int" | "bigint" | "tinyint" | "numeric" | "decimal"
            | "float4" | "float8" | "real" | "double precision" | "float" | "money" | "smallmoney" | "oid"
            | "double" | "serial" | "bigserial"
    )
}

/// A schema-qualified object.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
pub struct ObjectRef {
    /// Empty for SQLite's `main` and engines without schemas.
    #[serde(default)]
    pub schema: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    Function,
    Procedure,
    Sequence,
    Trigger,
    Index,
    Type,
    Other,
}

/// One row of the sidebar's schema tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbObject {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
    /// Row estimate for tables (`None` when the engine has none cheaply).
    pub rows: Option<i64>,
    /// Anything else short worth showing (a function's arguments…).
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub default: Option<String>,
    /// 1-based position in the primary key.
    pub primary_key: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced: ObjectRef,
    pub referenced_columns: Vec<String>,
}

/// What the Structure tab shows, and what editing needs (the key).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    /// `CREATE …` text when the engine hands it out or it can be rebuilt.
    pub ddl: String,
}

impl TableInfo {
    /// Primary-key column names in key order.
    pub fn key_columns(&self) -> Vec<String> {
        let mut key: Vec<_> = self.columns.iter().filter_map(|c| c.primary_key.map(|p| (p, c.name.clone()))).collect();
        key.sort();
        key.into_iter().map(|(_, n)| n).collect()
    }
}

/// One row's write-back: find it by `key`, set `changes`.
#[derive(Clone, Debug, PartialEq)]
pub struct RowEdit {
    pub key: Vec<(String, Cell)>,
    pub changes: Vec<(String, Cell)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_display_and_copy() {
        assert_eq!(Cell::Null.display(), "NULL");
        assert_eq!(Cell::Null.copy_text(), "");
        assert_eq!(Cell::Float(2.0).display(), "2.0");
        assert_eq!(Cell::Bytes(vec![0xde, 0xad].into()).display(), "0xDEAD");
        assert!(Cell::Bytes(vec![0; 100].into()).display().ends_with('…'));
    }

    #[test]
    fn sorting_puts_null_first_and_numbers_by_value() {
        let mut v = vec![Cell::text("10"), Cell::Null, Cell::text("9")];
        v.sort_by(|a, b| a.compare(b, true));
        assert_eq!(v, vec![Cell::Null, Cell::text("9"), Cell::text("10")]);
        v.sort_by(|a, b| a.compare(b, false));
        assert_eq!(v, vec![Cell::Null, Cell::text("10"), Cell::text("9")]);
    }

    #[test]
    fn numeric_types_across_engines() {
        for t in ["int4", "numeric(10,2)", "BIGINT", "decimal", "float8", "money", "REAL"] {
            assert!(is_numeric_type(t), "{t}");
        }
        for t in ["text", "varchar(20)", "timestamptz", "uuid", "bit"] {
            assert!(!is_numeric_type(t), "{t}");
        }
    }
}
