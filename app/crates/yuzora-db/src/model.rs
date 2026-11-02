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
    /// Integers and decimal text compare exactly, never through `f64`.
    pub fn compare(&self, other: &Cell, numeric: bool) -> Ordering {
        match (self, other) {
            (Cell::Null, Cell::Null) => Ordering::Equal,
            (Cell::Null, _) => Ordering::Less,
            (_, Cell::Null) => Ordering::Greater,
            _ => match (Number::of(self, numeric), Number::of(other, numeric)) {
                (Some(a), Some(b)) => a.cmp(&b),
                _ => self.display().cmp(&other.display()),
            },
        }
    }
}

/// A cell's numeric value for sorting.
enum Number<'a> {
    Int(i64),
    /// Decimal text, compared digit by digit.
    Decimal(Decimal<'a>),
    /// A float, or text only a float reads (`1e20`, `Infinity`, `NaN`).
    Float(f64),
}

impl<'a> Number<'a> {
    fn of(cell: &'a Cell, numeric: bool) -> Option<Self> {
        match cell {
            Cell::Int(i) => Some(Number::Int(*i)),
            Cell::Bool(b) => Some(Number::Int(*b as i64)),
            Cell::Float(f) => Some(Number::Float(*f)),
            Cell::Text(s) if numeric => {
                Decimal::parse(s).map(Number::Decimal).or_else(|| s.trim().parse().ok().map(Number::Float))
            }
            _ => None,
        }
    }

    fn cmp(&self, other: &Number<'_>) -> Ordering {
        match (self, other) {
            (Number::Int(a), Number::Int(b)) => a.cmp(b),
            (Number::Decimal(a), Number::Decimal(b)) => a.cmp(b),
            (Number::Int(i), Number::Decimal(d)) => Decimal::parse(&i.to_string()).map_or(Ordering::Equal, |i| i.cmp(d)),
            (Number::Decimal(_), Number::Int(_)) => other.cmp(self).reverse(),
            (Number::Float(a), Number::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Number::Float(f), exact) => float_cmp(*f, exact),
            (exact, Number::Float(f)) => float_cmp(*f, exact).reverse(),
        }
    }
}

/// A float against an exact number: by the nearest float first, which can
/// only tie when the two are close; a tie is settled on the float's exact value.
fn float_cmp(f: f64, exact: &Number<'_>) -> Ordering {
    let text = match exact {
        Number::Int(i) => i.to_string(),
        Number::Decimal(d) => d.to_string(),
        Number::Float(g) => return f.partial_cmp(g).unwrap_or(Ordering::Equal),
    };
    let near: f64 = text.parse().unwrap_or(f64::NAN);
    match f.partial_cmp(&near) {
        Some(Ordering::Equal) if f.is_finite() => {
            // Every finite f64 is exact in 1074 decimal places.
            let f = format!("{f:.1074}");
            match (Decimal::parse(&f), Decimal::parse(&text)) {
                (Some(a), Some(b)) => a.cmp(&b),
                _ => Ordering::Equal,
            }
        }
        order => order.unwrap_or(Ordering::Equal),
    }
}

/// `-12.340` as sign, integer digits without leading zeros (`12`) and
/// fraction digits without trailing zeros (`34`).
#[derive(Clone, Copy)]
struct Decimal<'a> {
    negative: bool,
    int: &'a str,
    frac: &'a str,
}

impl<'a> Decimal<'a> {
    fn parse(s: &'a str) -> Option<Self> {
        let s = s.trim();
        let (negative, digits) = match s.as_bytes().first()? {
            b'-' => (true, &s[1..]),
            b'+' => (false, &s[1..]),
            _ => (false, s),
        };
        let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
        if (int.is_empty() && frac.is_empty()) || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        let (int, frac) = (int.trim_start_matches('0'), frac.trim_end_matches('0'));
        // -0 is 0.
        Some(Self { negative: negative && !(int.is_empty() && frac.is_empty()), int, frac })
    }

    fn cmp(&self, other: &Decimal<'_>) -> Ordering {
        let magnitude = || {
            self.int.len().cmp(&other.int.len()).then_with(|| self.int.cmp(other.int)).then_with(|| self.frac.cmp(other.frac))
        };
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => magnitude(),
            (true, true) => magnitude().reverse(),
        }
    }
}

impl std::fmt::Display for Decimal<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sign = if self.negative { "-" } else { "" };
        let int = if self.int.is_empty() { "0" } else { self.int };
        if self.frac.is_empty() { write!(f, "{sign}{int}") } else { write!(f, "{sign}{int}.{}", self.frac) }
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
    fn numbers_sort_exactly_past_what_a_float_holds() {
        use Ordering::*;
        let n = |s: &str| Cell::text(s);
        let cases = [
            // Integers past 2^53, as a bigint and as PostgreSQL's text.
            (Cell::Int(i64::MAX), Cell::Int(i64::MAX - 1), Greater),
            (n("9007199254740993"), n("9007199254740992"), Greater),
            (Cell::Int(9_007_199_254_740_993), n("9007199254740992"), Greater),
            // numeric(38, 23): equal as floats, not as decimals.
            (n("0.12345678901234567890123"), n("0.12345678901234567890122"), Greater),
            (n("-10.5"), n("-2"), Less),
            (n("-0.000"), n("0"), Equal),
            (n("1.50"), n("+1.5"), Equal),
            (n("007.25"), n("7.250"), Equal),
            (n("0.05"), n("0.5"), Less),
            // Against a float: its exact value decides a tie.
            (Cell::Int(9_007_199_254_740_993), Cell::Float(9_007_199_254_740_992.0), Greater),
            (n("0.1"), Cell::Float(0.1), Less),
            (Cell::Float(1.5), n("1.5"), Equal),
            (n("1e3"), n("999.9"), Greater),
            (n("-Infinity"), n("-99999999999999999999999"), Less),
        ];
        for (a, b, order) in cases {
            assert_eq!(a.compare(&b, true), order, "{a:?} vs {b:?}");
            assert_eq!(b.compare(&a, true), order.reverse(), "{b:?} vs {a:?}");
        }
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
