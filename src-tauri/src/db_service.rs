// FEAT-1 Database + F2 network backends: connection registry + query commands.
//
// The registry maps a connId to an `Arc<DbHandle>`, one variant per engine:
// SQLite (synchronous rusqlite behind a std Mutex), PostgreSQL (tokio-postgres,
// natively async), and MSSQL (tiberius behind a tokio Mutex, `&mut self` API).
// Query commands are `async fn`: they clone the `Arc` out of the registry lock
// and then run the engine call *outside* the lock — the std Mutex is never held
// across an `.await` (SQLite work is offloaded to `spawn_blocking`). Every engine
// serialises into the same `columns + rows + kind` serde contract so the whole
// front-end (dbStore, DatabasePanel) is engine-agnostic.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::TryStreamExt;
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use rust_decimal::Decimal;
use tiberius::{AuthMethod, ColumnData, Config as MssqlConfig, FromSql};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio_postgres::types::Type as PgType;
use tokio_postgres::Row as PgRow;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

/// The MSSQL client type: tiberius over a tokio TcpStream via the compat shim.
type MssqlClient = tiberius::Client<Compat<TcpStream>>;

/// One PostgreSQL client plus the spawned driver task. tokio-postgres splits a
/// connection into a `Client` (issues queries, `&self`) and a `Connection`
/// future that must be polled to drive the socket; we spawn the latter and abort
/// it on close.
pub struct PgConn {
    client: tokio_postgres::Client,
    conn_task: tauri::async_runtime::JoinHandle<()>,
}

/// An open database, one variant per engine.
///
/// `Send + Sync`: SQLite's `Connection` is `Send` but `!Sync`, so the inner
/// `Mutex` makes it shareable; tokio-postgres `Client` is already `Send + Sync`;
/// tiberius `Client` is `Send` and wrapped in a tokio `Mutex` (its query API is
/// `&mut self`). The outer `Arc<DbHandle>` is therefore freely cloneable across
/// tasks.
// Always stored behind `Arc<DbHandle>` (see `DbState`), so the on-stack size of
// the largest variant is never paid — boxing a variant would only add a
// redundant indirection under the Arc.
#[allow(clippy::large_enum_variant)]
pub enum DbHandle {
    Sqlite(Mutex<Connection>),
    Postgres(PgConn),
    Mssql(AsyncMutex<MssqlClient>),
}

/// connId → open handle. The outer `Mutex` makes the registry `Send + Sync` for
/// Tauri managed state; it is only ever held long enough to clone an `Arc` out.
pub struct DbState(pub Mutex<HashMap<String, Arc<DbHandle>>>);

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

fn next_conn_id() -> String {
    format!("db-{}", NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed))
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpenResult {
    pub conn_id: String,
}

#[derive(serde::Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub name: String,
    /// "table" or "view".
    pub kind: String,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct ColumnInfo {
    pub name: String,
    /// Declared column type (may be empty for untyped columns).
    #[serde(rename = "type")]
    pub col_type: String,
    pub notnull: bool,
    pub pk: bool,
}

#[derive(serde::Serialize, Debug)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum QueryResult {
    Select {
        columns: Vec<String>,
        rows: Vec<Vec<serde_json::Value>>,
        truncated: bool,
    },
    Execute {
        affected_rows: usize,
    },
}

pub const DEFAULT_MAX_ROWS: usize = 500;

/// Connection descriptor from the front-end. Passwords arrive in-flight only and
/// are never persisted anywhere in this module.
#[derive(serde::Deserialize, Debug)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbOpenConfig {
    Sqlite {
        path: String,
    },
    Postgres {
        host: String,
        port: u16,
        database: String,
        user: String,
        password: String,
        ssl: bool,
    },
    Mssql {
        host: String,
        port: u16,
        database: String,
        user: String,
        password: String,
        trust_cert: bool,
    },
}

// ---------------------------------------------------------------------------
// SQLite (unchanged MVP behaviour, run on the blocking pool)
// ---------------------------------------------------------------------------

/// Double-quote a SQLite identifier, escaping embedded quotes ("" ) so a table
/// name can never break out of the quotes / inject SQL. `PRAGMA table_info` and
/// the click-to-query builder both need this because they interpolate the name.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// One SQLite value → JSON. NULL/INTEGER/REAL/TEXT map to native JSON; BLOB is
/// replaced by a "<blob N bytes>" marker string (never serialise raw bytes).
fn value_to_json(v: ValueRef<'_>) -> serde_json::Value {
    match v {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(n) => serde_json::json!(n),
        ValueRef::Real(f) => serde_json::json!(f),
        ValueRef::Text(bytes) => serde_json::json!(String::from_utf8_lossy(bytes)),
        ValueRef::Blob(bytes) => serde_json::json!(format!("<blob {} bytes>", bytes.len())),
    }
}

/// user tables + views from sqlite_master (internal sqlite_* names excluded).
pub fn list_tables(conn: &Connection) -> Result<Vec<TableInfo>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT name, type FROM sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
             ORDER BY type, name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(TableInfo {
                name: row.get(0)?,
                kind: row.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// `PRAGMA table_info(<table>)`. The table name can't be bound as a parameter,
/// so it is passed through `quote_ident` to stay injection-safe.
pub fn table_columns(conn: &Connection, table: &str) -> Result<Vec<ColumnInfo>, String> {
    let sql = format!("PRAGMA table_info({})", quote_ident(table));
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    // PRAGMA table_info columns: cid, name, type, notnull, dflt_value, pk.
    let rows = stmt
        .query_map([], |row| {
            Ok(ColumnInfo {
                name: row.get(1)?,
                col_type: row.get(2)?,
                notnull: row.get::<_, i64>(3)? != 0,
                pk: row.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Run one SQL statement. A prepared statement with columns (SELECT / PRAGMA /
/// RETURNING) yields a `Select` result capped at `max_rows` (with `truncated`
/// set when more rows exist); anything else runs via `execute` and reports the
/// affected row count. SQL errors are returned verbatim as the Err string.
pub fn run_query(conn: &Connection, sql: &str, max_rows: usize) -> Result<QueryResult, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let col_count = stmt.column_count();
    if col_count == 0 {
        let affected = stmt.execute([]).map_err(|e| e.to_string())?;
        return Ok(QueryResult::Execute {
            affected_rows: affected,
        });
    }
    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut out: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if out.len() >= max_rows {
            // One more row exists beyond the cap → mark truncated and stop.
            truncated = true;
            break;
        }
        let mut vals = Vec::with_capacity(col_count);
        for i in 0..col_count {
            vals.push(value_to_json(row.get_ref(i).map_err(|e| e.to_string())?));
        }
        out.push(vals);
    }
    Ok(QueryResult::Select {
        columns,
        rows: out,
        truncated,
    })
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

/// How a Postgres column value is decoded into JSON, chosen from its `Type`.
/// Split out as a pure classifier so the type→decode mapping is unit-testable
/// without a live connection.
#[derive(Debug, PartialEq, Eq)]
enum PgColKind {
    Bool,
    I16,
    I32,
    I64,
    F32,
    F64,
    /// Kept as a lossless string (numeric/decimal precision would be lost as f64).
    Numeric,
    Text,
    Uuid,
    Timestamp,
    TimestampTz,
    Date,
    Time,
    Json,
    /// Replaced by a "<blob N bytes>" marker.
    Bytea,
    /// Anything else: best-effort text, else a "<typename>" marker.
    Fallback,
}

fn classify_pg_type(ty: &PgType) -> PgColKind {
    if *ty == PgType::BOOL {
        PgColKind::Bool
    } else if *ty == PgType::INT2 {
        PgColKind::I16
    } else if *ty == PgType::INT4 {
        PgColKind::I32
    } else if *ty == PgType::INT8 {
        PgColKind::I64
    } else if *ty == PgType::FLOAT4 {
        PgColKind::F32
    } else if *ty == PgType::FLOAT8 {
        PgColKind::F64
    } else if *ty == PgType::NUMERIC {
        PgColKind::Numeric
    } else if *ty == PgType::VARCHAR
        || *ty == PgType::TEXT
        || *ty == PgType::BPCHAR
        || *ty == PgType::NAME
    {
        // NOTE: the internal "char" type (PgType::CHAR, OID 18) is deliberately
        // NOT here — String::accepts rejects it, so it decodes via the Fallback
        // PgText wrapper below (which special-cases its single-byte layout).
        PgColKind::Text
    } else if *ty == PgType::UUID {
        PgColKind::Uuid
    } else if *ty == PgType::TIMESTAMP {
        PgColKind::Timestamp
    } else if *ty == PgType::TIMESTAMPTZ {
        PgColKind::TimestampTz
    } else if *ty == PgType::DATE {
        PgColKind::Date
    } else if *ty == PgType::TIME {
        PgColKind::Time
    } else if *ty == PgType::JSON || *ty == PgType::JSONB {
        PgColKind::Json
    } else if *ty == PgType::BYTEA {
        PgColKind::Bytea
    } else {
        PgColKind::Fallback
    }
}

/// Decode column `idx` of `row` (of type `ty`) into JSON. NULL → JSON null; a
/// decode failure on a classified type degrades to null rather than aborting the
/// whole result.
fn pg_cell<T, F>(row: &PgRow, idx: usize, f: F) -> serde_json::Value
where
    T: for<'a> tokio_postgres::types::FromSql<'a>,
    F: FnOnce(T) -> serde_json::Value,
{
    let got: Result<Option<T>, _> = row.try_get(idx);
    match got {
        Ok(Some(v)) => f(v),
        Ok(None) => serde_json::Value::Null,
        Err(_) => serde_json::Value::Null,
    }
}

fn pg_value_to_json(row: &PgRow, idx: usize, ty: &PgType) -> serde_json::Value {
    use serde_json::{json, Value};
    match classify_pg_type(ty) {
        PgColKind::Bool => pg_cell::<bool, _>(row, idx, Value::Bool),
        PgColKind::I16 => pg_cell::<i16, _>(row, idx, |v| json!(v)),
        PgColKind::I32 => pg_cell::<i32, _>(row, idx, |v| json!(v)),
        PgColKind::I64 => pg_cell::<i64, _>(row, idx, |v| json!(v)),
        PgColKind::F32 => pg_cell::<f32, _>(row, idx, |v| json!(v)),
        PgColKind::F64 => pg_cell::<f64, _>(row, idx, |v| json!(v)),
        PgColKind::Numeric => pg_cell::<Decimal, _>(row, idx, |v| Value::String(v.to_string())),
        PgColKind::Text => pg_cell::<String, _>(row, idx, Value::String),
        PgColKind::Uuid => pg_cell::<uuid::Uuid, _>(row, idx, |v| Value::String(v.to_string())),
        PgColKind::Timestamp => {
            pg_cell::<chrono::NaiveDateTime, _>(row, idx, |v| Value::String(v.to_string()))
        }
        PgColKind::TimestampTz => {
            pg_cell::<chrono::DateTime<chrono::Utc>, _>(row, idx, |v| Value::String(v.to_rfc3339()))
        }
        PgColKind::Date => {
            pg_cell::<chrono::NaiveDate, _>(row, idx, |v| Value::String(v.to_string()))
        }
        PgColKind::Time => {
            pg_cell::<chrono::NaiveTime, _>(row, idx, |v| Value::String(v.to_string()))
        }
        PgColKind::Json => pg_cell::<serde_json::Value, _>(row, idx, |v| v),
        PgColKind::Bytea => pg_cell::<Vec<u8>, _>(row, idx, |v| {
            Value::String(format!("<blob {} bytes>", v.len()))
        }),
        PgColKind::Fallback => pg_cell::<PgText, _>(row, idx, |v| v.0),
    }
}

/// Best-effort text decoder that accepts EVERY Postgres type. tokio-postgres
/// checks `FromSql::accepts` before reading a value, and `String::accepts` rejects
/// types such as the internal "char" (OID 18), INET, arrays, interval and enums —
/// returning WrongType even when the value is NULL. Accepting everything lets a
/// NULL decode as `Ok(None)` (→ JSON null) instead of being mislabelled, while
/// non-NULL bytes render as lossy UTF-8 text or a "<typename>" marker.
struct PgText(serde_json::Value);

impl<'a> tokio_postgres::types::FromSql<'a> for PgText {
    fn from_sql(
        ty: &PgType,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        // The internal "char" type is a single signed byte, not UTF-8 text.
        if *ty == PgType::CHAR {
            let ch = raw
                .first()
                .map(|&b| (b as char).to_string())
                .unwrap_or_default();
            return Ok(PgText(serde_json::Value::String(ch)));
        }
        let text = match std::str::from_utf8(raw) {
            Ok(s) => s.to_string(),
            Err(_) => format!("<{}>", ty.name()),
        };
        Ok(PgText(serde_json::Value::String(text)))
    }

    fn accepts(_ty: &PgType) -> bool {
        true
    }
}

/// Build a rustls TLS connector for Postgres, reusing the process's ring crypto
/// provider (explicit, so a second installed provider can't make it ambiguous)
/// and the webpki root store.
fn pg_tls() -> Result<tokio_postgres_rustls::MakeRustlsConnect, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

async fn pg_open(
    host: String,
    port: u16,
    database: String,
    user: String,
    password: String,
    ssl: bool,
) -> Result<DbHandle, String> {
    let mut cfg = tokio_postgres::Config::new();
    cfg.host(&host)
        .port(port)
        .dbname(&database)
        .user(&user)
        .password(&password);

    // The Connection future's concrete type differs per TLS choice, but it is
    // consumed (spawned) inside each branch so both yield the same (Client, task).
    let (client, conn_task) = if ssl {
        let (client, connection) = cfg
            .connect(pg_tls()?)
            .await
            .map_err(|e| format!("cannot connect to postgres: {e}"))?;
        let task = tauri::async_runtime::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("postgres connection error: {e}");
            }
        });
        (client, task)
    } else {
        let (client, connection) = cfg
            .connect(tokio_postgres::NoTls)
            .await
            .map_err(|e| format!("cannot connect to postgres: {e}"))?;
        let task = tauri::async_runtime::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("postgres connection error: {e}");
            }
        });
        (client, task)
    };
    Ok(DbHandle::Postgres(PgConn { client, conn_task }))
}

async fn pg_list_tables(client: &tokio_postgres::Client) -> Result<Vec<TableInfo>, String> {
    let rows = client
        .query(
            "SELECT table_name, table_type FROM information_schema.tables \
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
             ORDER BY table_type, table_name",
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let table_type: String = r.get(1);
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                "view"
            } else {
                "table"
            };
            TableInfo {
                name,
                kind: kind.to_string(),
            }
        })
        .collect())
}

async fn pg_table_columns(
    client: &tokio_postgres::Client,
    table: &str,
) -> Result<Vec<ColumnInfo>, String> {
    let rows = client
        .query(
            "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
             WHERE table_name = $1 ORDER BY ordinal_position",
            &[&table],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let data_type: String = r.get(1);
            let is_nullable: String = r.get(2);
            ColumnInfo {
                name,
                col_type: data_type,
                notnull: is_nullable.eq_ignore_ascii_case("NO"),
                pk: false,
            }
        })
        .collect())
}

async fn pg_run_query(
    client: &tokio_postgres::Client,
    sql: &str,
    max_rows: usize,
) -> Result<QueryResult, String> {
    let stmt = client.prepare(sql).await.map_err(|e| e.to_string())?;
    // A statement with no result columns (INSERT/UPDATE/DDL) reports affected rows.
    if stmt.columns().is_empty() {
        let affected = client
            .execute(&stmt, &[])
            .await
            .map_err(|e| e.to_string())?;
        return Ok(QueryResult::Execute {
            affected_rows: affected as usize,
        });
    }
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let col_types: Vec<PgType> = stmt.columns().iter().map(|c| c.type_().clone()).collect();

    // Stream rows so an unbounded SELECT stops at the cap instead of buffering the
    // whole table (mirrors the SQLite path). An empty typed params list satisfies
    // query_raw's BorrowToSql/ExactSizeIterator bounds.
    let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    let row_stream = client
        .query_raw(&stmt, params)
        .await
        .map_err(|e| e.to_string())?;
    futures_util::pin_mut!(row_stream);
    let mut out: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = row_stream.try_next().await.map_err(|e| e.to_string())? {
        if out.len() >= max_rows {
            truncated = true;
            break;
        }
        let mut vals = Vec::with_capacity(columns.len());
        for (i, ty) in col_types.iter().enumerate() {
            vals.push(pg_value_to_json(&row, i, ty));
        }
        out.push(vals);
    }
    Ok(QueryResult::Select {
        columns,
        rows: out,
        truncated,
    })
}

// ---------------------------------------------------------------------------
// MSSQL
// ---------------------------------------------------------------------------

/// One tiberius `ColumnData` → JSON, mirroring the SQLite/PG mappings: integers
/// and floats stay numeric, bit → bool, decimals/uuid/xml/date-time → string,
/// binary → a "<blob N bytes>" marker, NULL → JSON null. Pure, so every branch
/// is unit-testable by constructing the variant directly.
fn mssql_value_to_json(data: &ColumnData<'static>) -> serde_json::Value {
    use serde_json::{json, Value};
    match data {
        ColumnData::U8(v) => v.as_ref().map_or(Value::Null, |n| json!(n)),
        ColumnData::I16(v) => v.as_ref().map_or(Value::Null, |n| json!(n)),
        ColumnData::I32(v) => v.as_ref().map_or(Value::Null, |n| json!(n)),
        ColumnData::I64(v) => v.as_ref().map_or(Value::Null, |n| json!(n)),
        ColumnData::F32(v) => v.as_ref().map_or(Value::Null, |n| json!(n)),
        ColumnData::F64(v) => v.as_ref().map_or(Value::Null, |n| json!(n)),
        ColumnData::Bit(v) => v.as_ref().map_or(Value::Null, |b| Value::Bool(*b)),
        ColumnData::String(v) => v
            .as_ref()
            .map_or(Value::Null, |s| Value::String(s.to_string())),
        ColumnData::Guid(v) => v
            .as_ref()
            .map_or(Value::Null, |g| Value::String(g.to_string())),
        ColumnData::Numeric(v) => v
            .as_ref()
            .map_or(Value::Null, |n| Value::String(n.to_string())),
        ColumnData::Binary(v) => v.as_ref().map_or(Value::Null, |b| {
            Value::String(format!("<blob {} bytes>", b.len()))
        }),
        ColumnData::Xml(v) => v
            .as_ref()
            .map_or(Value::Null, |x| Value::String(x.to_string())),
        // Date/time: reuse tiberius's chrono FromSql conversions for a readable
        // string; a conversion failure degrades to null.
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            chrono::NaiveDateTime::from_sql(data)
                .ok()
                .flatten()
                .map_or(Value::Null, |dt| Value::String(dt.to_string()))
        }
        ColumnData::Date(_) => chrono::NaiveDate::from_sql(data)
            .ok()
            .flatten()
            .map_or(Value::Null, |d| Value::String(d.to_string())),
        ColumnData::Time(_) => chrono::NaiveTime::from_sql(data)
            .ok()
            .flatten()
            .map_or(Value::Null, |t| Value::String(t.to_string())),
        ColumnData::DateTimeOffset(_) => chrono::DateTime::<chrono::Utc>::from_sql(data)
            .ok()
            .flatten()
            .map_or(Value::Null, |dt| Value::String(dt.to_rfc3339())),
    }
}

async fn mssql_connect(
    host: String,
    port: u16,
    database: String,
    user: String,
    password: String,
    trust_cert: bool,
) -> Result<MssqlClient, String> {
    let mut config = MssqlConfig::new();
    config.host(&host);
    config.port(port);
    config.database(&database);
    config.authentication(AuthMethod::sql_server(&user, &password));
    if trust_cert {
        config.trust_cert();
    }
    let tcp = TcpStream::connect(config.get_addr())
        .await
        .map_err(|e| format!("cannot connect to mssql {host}:{port}: {e}"))?;
    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
    tiberius::Client::connect(config, tcp.compat_write())
        .await
        .map_err(|e| format!("cannot connect to mssql: {e}"))
}

async fn mssql_list_tables(client: &mut MssqlClient) -> Result<Vec<TableInfo>, String> {
    let stream = client
        .query(
            "SELECT table_name, table_type FROM information_schema.tables \
             ORDER BY table_type, table_name",
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;
    let rows = stream
        .into_first_result()
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| {
            let name: &str = r.get(0).unwrap_or_default();
            let table_type: &str = r.get(1).unwrap_or_default();
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                "view"
            } else {
                "table"
            };
            TableInfo {
                name: name.to_string(),
                kind: kind.to_string(),
            }
        })
        .collect())
}

async fn mssql_table_columns(
    client: &mut MssqlClient,
    table: &str,
) -> Result<Vec<ColumnInfo>, String> {
    let table_owned = table.to_string();
    let params: &[&dyn tiberius::ToSql] = &[&table_owned];
    let stream = client
        .query(
            "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
             WHERE table_name = @P1 ORDER BY ordinal_position",
            params,
        )
        .await
        .map_err(|e| e.to_string())?;
    let rows = stream
        .into_first_result()
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| {
            let name: &str = r.get(0).unwrap_or_default();
            let data_type: &str = r.get(1).unwrap_or_default();
            let is_nullable: &str = r.get(2).unwrap_or_default();
            ColumnInfo {
                name: name.to_string(),
                col_type: data_type.to_string(),
                notnull: is_nullable.eq_ignore_ascii_case("NO"),
                pk: false,
            }
        })
        .collect())
}

async fn mssql_run_query(
    client: &mut MssqlClient,
    sql: &str,
    max_rows: usize,
) -> Result<QueryResult, String> {
    let mut stream = client.query(sql, &[]).await.map_err(|e| e.to_string())?;
    // No result-set columns → a command (INSERT/UPDATE/DDL). tiberius's query
    // stream doesn't surface the DONE row count here, so report 0; the `execute`
    // kind still lets the front-end refresh the table list after DDL.
    let columns: Vec<String> = match stream.columns().await.map_err(|e| e.to_string())? {
        Some(cols) => cols.iter().map(|c| c.name().to_string()).collect(),
        None => Vec::new(),
    };
    if columns.is_empty() {
        return Ok(QueryResult::Execute { affected_rows: 0 });
    }
    let mut row_stream = stream.into_row_stream();
    let mut out: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = row_stream.try_next().await.map_err(|e| e.to_string())? {
        if out.len() >= max_rows {
            truncated = true;
            break;
        }
        let vals: Vec<serde_json::Value> = row
            .into_iter()
            .map(|cell| mssql_value_to_json(&cell))
            .collect();
        out.push(vals);
    }
    Ok(QueryResult::Select {
        columns,
        rows: out,
        truncated,
    })
}

// ---------------------------------------------------------------------------
// Registry + commands
// ---------------------------------------------------------------------------

/// Clone the handle registered under `conn_id` out of the registry. The lock is
/// released before the caller `.await`s the engine call.
fn get_handle(state: &DbState, conn_id: &str) -> Result<Arc<DbHandle>, String> {
    state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .get(conn_id)
        .cloned()
        .ok_or_else(|| format!("no database connection: {conn_id}"))
}

/// Run a blocking SQLite closure against a handle on the blocking pool, so the
/// std Mutex is never locked on an async worker thread.
async fn on_sqlite<T, F>(handle: Arc<DbHandle>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || match &*handle {
        DbHandle::Sqlite(conn) => {
            let conn = conn.lock().map_err(|e| e.to_string())?;
            f(&conn)
        }
        _ => Err("connection is not sqlite".to_string()),
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn db_open(
    state: tauri::State<'_, DbState>,
    config: DbOpenConfig,
) -> Result<OpenResult, String> {
    let handle = match config {
        DbOpenConfig::Sqlite { path } => {
            let conn = tauri::async_runtime::spawn_blocking(move || Connection::open(&path))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| format!("cannot open database: {e}"))?;
            DbHandle::Sqlite(Mutex::new(conn))
        }
        DbOpenConfig::Postgres {
            host,
            port,
            database,
            user,
            password,
            ssl,
        } => pg_open(host, port, database, user, password, ssl).await?,
        DbOpenConfig::Mssql {
            host,
            port,
            database,
            user,
            password,
            trust_cert,
        } => {
            let client = mssql_connect(host, port, database, user, password, trust_cert).await?;
            DbHandle::Mssql(AsyncMutex::new(client))
        }
    };
    let conn_id = next_conn_id();
    state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .insert(conn_id.clone(), Arc::new(handle));
    Ok(OpenResult { conn_id })
}

#[tauri::command]
pub async fn db_close(state: tauri::State<'_, DbState>, conn_id: String) -> Result<(), String> {
    // Removing an unknown id is a no-op. Dropping the handle closes SQLite/MSSQL;
    // for Postgres, abort the driver task so it stops promptly (dropping the last
    // Client would end it too, but a concurrent query could still hold a clone).
    let removed = state.0.lock().map_err(|e| e.to_string())?.remove(&conn_id);
    if let Some(handle) = removed {
        if let DbHandle::Postgres(pg) = &*handle {
            pg.conn_task.abort();
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn db_list_tables(
    state: tauri::State<'_, DbState>,
    conn_id: String,
) -> Result<Vec<TableInfo>, String> {
    let handle = get_handle(&state, &conn_id)?;
    match &*handle {
        DbHandle::Sqlite(_) => on_sqlite(handle.clone(), list_tables).await,
        DbHandle::Postgres(pg) => pg_list_tables(&pg.client).await,
        DbHandle::Mssql(m) => mssql_list_tables(&mut *m.lock().await).await,
    }
}

#[tauri::command]
pub async fn db_table_columns(
    state: tauri::State<'_, DbState>,
    conn_id: String,
    table: String,
) -> Result<Vec<ColumnInfo>, String> {
    let handle = get_handle(&state, &conn_id)?;
    match &*handle {
        DbHandle::Sqlite(_) => {
            let t = table.clone();
            on_sqlite(handle.clone(), move |conn| table_columns(conn, &t)).await
        }
        DbHandle::Postgres(pg) => pg_table_columns(&pg.client, &table).await,
        DbHandle::Mssql(m) => mssql_table_columns(&mut *m.lock().await, &table).await,
    }
}

#[tauri::command]
pub async fn db_query(
    state: tauri::State<'_, DbState>,
    conn_id: String,
    sql: String,
    max_rows: Option<usize>,
) -> Result<QueryResult, String> {
    let cap = max_rows.unwrap_or(DEFAULT_MAX_ROWS);
    let handle = get_handle(&state, &conn_id)?;
    match &*handle {
        DbHandle::Sqlite(_) => {
            let s = sql.clone();
            on_sqlite(handle.clone(), move |conn| run_query(conn, &s, cap)).await
        }
        DbHandle::Postgres(pg) => pg_run_query(&pg.client, &sql, cap).await,
        DbHandle::Mssql(m) => mssql_run_query(&mut *m.lock().await, &sql, cap).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn list_tables_reports_tables_and_views_with_kind() {
        let conn = mem();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER);\
             CREATE VIEW v AS SELECT id FROM t;",
        )
        .unwrap();
        let tables = list_tables(&conn).unwrap();
        // sqlite_% internal tables are excluded; ordered by type then name.
        let by_name: Vec<_> = tables
            .iter()
            .map(|t| (t.name.as_str(), t.kind.as_str()))
            .collect();
        assert!(by_name.contains(&("t", "table")));
        assert!(by_name.contains(&("v", "view")));
    }

    #[test]
    fn select_serialises_native_and_blob_and_null_types() {
        let conn = mem();
        conn.execute_batch(
            "CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n INTEGER);\
             INSERT INTO t VALUES (42, 3.5, 'hi', x'0102030405', NULL);",
        )
        .unwrap();
        let result = run_query(&conn, "SELECT i, r, s, b, n FROM t", DEFAULT_MAX_ROWS).unwrap();
        match result {
            QueryResult::Select {
                columns,
                rows,
                truncated,
            } => {
                assert_eq!(columns, vec!["i", "r", "s", "b", "n"]);
                assert!(!truncated);
                assert_eq!(rows.len(), 1);
                let row = &rows[0];
                assert_eq!(row[0], serde_json::json!(42));
                assert_eq!(row[1], serde_json::json!(3.5));
                assert_eq!(row[2], serde_json::json!("hi"));
                assert_eq!(row[3], serde_json::json!("<blob 5 bytes>"));
                assert_eq!(row[4], serde_json::Value::Null);
            }
            other => panic!(
                "expected Select, got {}",
                serde_json::to_value(other).unwrap()
            ),
        }
    }

    #[test]
    fn select_truncates_at_max_rows() {
        let conn = mem();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER);\
             INSERT INTO t VALUES (1),(2),(3),(4),(5);",
        )
        .unwrap();
        let result = run_query(&conn, "SELECT id FROM t ORDER BY id", 2).unwrap();
        match result {
            QueryResult::Select {
                rows, truncated, ..
            } => {
                assert_eq!(rows.len(), 2);
                assert!(truncated);
            }
            other => panic!(
                "expected Select, got {}",
                serde_json::to_value(other).unwrap()
            ),
        }
    }

    #[test]
    fn select_exactly_at_cap_is_not_truncated() {
        let conn = mem();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER);\
             INSERT INTO t VALUES (1),(2);",
        )
        .unwrap();
        let result = run_query(&conn, "SELECT id FROM t", 2).unwrap();
        match result {
            QueryResult::Select {
                rows, truncated, ..
            } => {
                assert_eq!(rows.len(), 2);
                assert!(!truncated);
            }
            other => panic!(
                "expected Select, got {}",
                serde_json::to_value(other).unwrap()
            ),
        }
    }

    #[test]
    fn execute_reports_affected_rows() {
        let conn = mem();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER);\
             INSERT INTO t VALUES (1),(2),(3);",
        )
        .unwrap();
        let result = run_query(&conn, "UPDATE t SET id = id + 1", DEFAULT_MAX_ROWS).unwrap();
        match result {
            QueryResult::Execute { affected_rows } => assert_eq!(affected_rows, 3),
            other => panic!(
                "expected Execute, got {}",
                serde_json::to_value(other).unwrap()
            ),
        }
    }

    #[test]
    fn sql_error_is_returned_verbatim() {
        let conn = mem();
        let err = run_query(&conn, "SELECT * FROM nope", DEFAULT_MAX_ROWS).unwrap_err();
        assert!(err.contains("nope"), "unexpected error: {err}");
    }

    #[test]
    fn quote_ident_escapes_embedded_quotes() {
        assert_eq!(quote_ident("plain"), "\"plain\"");
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn table_columns_handles_quoted_name_and_blocks_injection() {
        let conn = mem();
        // Table whose name itself contains a double quote: only correct escaping
        // makes PRAGMA table_info find it.
        conn.execute_batch("CREATE TABLE \"a\"\"b\" (x INTEGER, y TEXT NOT NULL);")
            .unwrap();
        conn.execute_batch("CREATE TABLE victim (id INTEGER);")
            .unwrap();

        let cols = table_columns(&conn, "a\"b").unwrap();
        let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["x", "y"]);
        assert!(cols[1].notnull);

        // An injection attempt in the table name must not execute the DROP: it is
        // quoted into a single (nonexistent) identifier, yielding no columns, and
        // the victim table survives.
        let inject = table_columns(&conn, "x\"); DROP TABLE victim; --").unwrap();
        assert!(inject.is_empty());
        let still_there =
            run_query(&conn, "SELECT count(*) FROM victim", DEFAULT_MAX_ROWS).unwrap();
        match still_there {
            QueryResult::Select { rows, .. } => assert_eq!(rows[0][0], serde_json::json!(0)),
            other => panic!(
                "expected Select, got {}",
                serde_json::to_value(other).unwrap()
            ),
        }
    }

    #[test]
    fn open_close_registry_roundtrip() {
        let state = DbState(Mutex::new(HashMap::new()));
        // Open an in-memory db directly into the registry to exercise get_handle.
        let conn_id = next_conn_id();
        state.0.lock().unwrap().insert(
            conn_id.clone(),
            Arc::new(DbHandle::Sqlite(Mutex::new(
                Connection::open_in_memory().unwrap(),
            ))),
        );
        let handle = get_handle(&state, &conn_id).unwrap();
        match &*handle {
            DbHandle::Sqlite(conn) => {
                conn.lock()
                    .unwrap()
                    .execute_batch("CREATE TABLE t (id INTEGER);")
                    .unwrap();
                let tables = list_tables(&conn.lock().unwrap()).unwrap();
                assert_eq!(tables.len(), 1);
            }
            _ => panic!("expected sqlite handle"),
        }
        // Unknown id → readable error (Arc<DbHandle> isn't Debug, so read the Err
        // side directly rather than via unwrap_err).
        assert!(get_handle(&state, "db-999")
            .err()
            .unwrap()
            .contains("no database connection"));
    }

    #[test]
    fn classify_pg_type_maps_common_types() {
        assert_eq!(classify_pg_type(&PgType::BOOL), PgColKind::Bool);
        assert_eq!(classify_pg_type(&PgType::INT2), PgColKind::I16);
        assert_eq!(classify_pg_type(&PgType::INT4), PgColKind::I32);
        assert_eq!(classify_pg_type(&PgType::INT8), PgColKind::I64);
        assert_eq!(classify_pg_type(&PgType::FLOAT4), PgColKind::F32);
        assert_eq!(classify_pg_type(&PgType::FLOAT8), PgColKind::F64);
        assert_eq!(classify_pg_type(&PgType::NUMERIC), PgColKind::Numeric);
        assert_eq!(classify_pg_type(&PgType::VARCHAR), PgColKind::Text);
        assert_eq!(classify_pg_type(&PgType::TEXT), PgColKind::Text);
        assert_eq!(classify_pg_type(&PgType::BPCHAR), PgColKind::Text);
        assert_eq!(classify_pg_type(&PgType::UUID), PgColKind::Uuid);
        assert_eq!(classify_pg_type(&PgType::TIMESTAMP), PgColKind::Timestamp);
        assert_eq!(
            classify_pg_type(&PgType::TIMESTAMPTZ),
            PgColKind::TimestampTz
        );
        assert_eq!(classify_pg_type(&PgType::DATE), PgColKind::Date);
        assert_eq!(classify_pg_type(&PgType::TIME), PgColKind::Time);
        assert_eq!(classify_pg_type(&PgType::JSON), PgColKind::Json);
        assert_eq!(classify_pg_type(&PgType::JSONB), PgColKind::Json);
        assert_eq!(classify_pg_type(&PgType::BYTEA), PgColKind::Bytea);
        // An unmapped type (INET) falls through to the text/marker fallback.
        assert_eq!(classify_pg_type(&PgType::INET), PgColKind::Fallback);
    }

    #[test]
    fn mssql_value_to_json_maps_scalars() {
        use serde_json::{json, Value};
        assert_eq!(mssql_value_to_json(&ColumnData::I32(Some(42))), json!(42));
        assert_eq!(mssql_value_to_json(&ColumnData::I64(Some(9))), json!(9));
        assert_eq!(mssql_value_to_json(&ColumnData::F64(Some(3.5))), json!(3.5));
        assert_eq!(
            mssql_value_to_json(&ColumnData::Bit(Some(true))),
            Value::Bool(true)
        );
        assert_eq!(
            mssql_value_to_json(&ColumnData::String(Some("hi".into()))),
            json!("hi")
        );
        assert_eq!(
            mssql_value_to_json(&ColumnData::Binary(Some(vec![1, 2, 3].into()))),
            json!("<blob 3 bytes>")
        );
    }

    #[test]
    fn mssql_value_to_json_maps_nulls_to_json_null() {
        use serde_json::Value;
        assert_eq!(mssql_value_to_json(&ColumnData::I32(None)), Value::Null);
        assert_eq!(mssql_value_to_json(&ColumnData::Bit(None)), Value::Null);
        assert_eq!(mssql_value_to_json(&ColumnData::String(None)), Value::Null);
        assert_eq!(mssql_value_to_json(&ColumnData::F64(None)), Value::Null);
    }

    #[test]
    fn mssql_value_to_json_maps_guid_to_string() {
        let uuid = uuid::Uuid::nil();
        let out = mssql_value_to_json(&ColumnData::Guid(Some(uuid)));
        assert_eq!(
            out,
            serde_json::json!("00000000-0000-0000-0000-000000000000")
        );
    }
}
