//! PostgreSQL through tokio-postgres (rustls, ring).
//!
//! Rows are read in the server's text format (`simple_query_raw`), so
//! numeric, interval, inet, enums and arrays all show as the server prints
//! them and can be written back as typed. The column types, which the simple
//! protocol does not carry, come from preparing the statement first.

use std::borrow::Cow;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::BytesMut;
use futures_util::future::try_join5;
use futures_util::{FutureExt, StreamExt};
use tokio::time::Instant;
use tokio_postgres::error::{DbError as ServerError, ErrorPosition, SqlState};
use tokio_postgres::types::{Format, IsNull, Kind, ToSql, Type, to_sql_checked};
use tokio_postgres::{CancelToken, Client, SimpleColumn, SimpleQueryMessage, Statement};
use tokio_postgres_rustls::MakeRustlsConnect;

use super::{BATCH, ConnectSpec, RowSink, tls};
use crate::config::Engine;
use crate::error::{DbError, DbResult};
use crate::model::{
    Cell, Column, ColumnInfo, DbObject, ForeignKeyInfo, IndexInfo, ObjectKind, ObjectRef, RowEdit, TableInfo,
};
use crate::sql;

pub struct PgConn {
    client: Client,
    version: String,
    canceller: PgCanceller,
}

/// Asks the server to stop what the connection is running (a second, short
/// connection carries the request, over the same TLS settings).
#[derive(Clone)]
pub struct PgCanceller {
    token: CancelToken,
    tls: MakeRustlsConnect,
    /// A stop pressed by the person, as opposed to the sink saying enough:
    /// the server's "canceled" error then reads as [`DbError::Cancelled`].
    cancelled: Arc<AtomicBool>,
}

impl PgCanceller {
    pub async fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.signal().await;
    }

    async fn signal(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(5), self.token.cancel_query(self.tls.clone())).await;
    }
}

/// A failed attempt to open one database, with the server's SQLSTATE.
struct Attempt {
    error: DbError,
    code: Option<String>,
}

impl PgConn {
    pub async fn connect(spec: &ConnectSpec) -> DbResult<Self> {
        let tls = tls::connector(spec.config.ssl, &spec.config.ca_file)?;
        let user = spec.config.user.trim();
        // No database named: the usual maintenance ones, never a template
        // (this connection stays open as the working session, and a session in
        // template1 blocks CREATE DATABASE for everyone).
        let candidates: Vec<String> = match spec.database() {
            Some(db) => vec![db.to_owned()],
            None => {
                let mut list = vec!["postgres".to_owned()];
                if !["", "postgres", "template0", "template1"].contains(&user) {
                    list.push(user.to_owned());
                }
                list
            }
        };
        let deadline = Instant::now() + spec.connect_timeout;
        let mut tried = Vec::new();
        for (i, database) in candidates.iter().enumerate() {
            match open(spec, &tls, database, deadline).await {
                Ok(client) => return Self::ready(client, tls).await,
                Err(Attempt { error, code }) => {
                    // Only a database that can't be had is worth moving on
                    // from: missing, no CONNECT right, connections disallowed.
                    let unavailable = matches!(code.as_deref(), Some("3D000" | "42501" | "55000"));
                    if !unavailable || i + 1 == candidates.len() {
                        return Err(if tried.is_empty() { error } else { tried_all(tried, error) });
                    }
                    tried.push(format!("{database}: {error}"));
                }
            }
        }
        unreachable!("there is always one candidate database")
    }

    async fn ready(client: Client, tls: MakeRustlsConnect) -> DbResult<Self> {
        let row = client.query_one("SHOW server_version", &[]).await.map_err(|e| DbError::Connect(describe(&e)))?;
        let full: String = row.get(0);
        let version = format!("PostgreSQL {}", full.split_whitespace().next().unwrap_or(&full));
        let canceller = PgCanceller { token: client.cancel_token(), tls, cancelled: Arc::new(AtomicBool::new(false)) };
        Ok(Self { client, version, canceller })
    }

    pub fn server_version(&self) -> String {
        self.version.clone()
    }

    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    pub fn canceller(&self) -> PgCanceller {
        self.canceller.clone()
    }

    pub async fn databases(&mut self) -> DbResult<Vec<String>> {
        let rows = self
            .client
            .query(
                "SELECT datname::text FROM pg_database \
                 WHERE datallowconn AND NOT datistemplate AND has_database_privilege(datname, 'CONNECT') \
                 ORDER BY datname",
                &[],
            )
            .await
            .map_err(query_error)?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    pub async fn objects(&mut self) -> DbResult<Vec<DbObject>> {
        // `fn` and `proc` keep a function apart from a foreign table ('f').
        const USER_SCHEMA: &str = "n.nspname <> 'information_schema' AND n.nspname !~ '^pg_(catalog|toast|temp)' \
                                   AND has_schema_privilege(n.oid, 'USAGE')";
        let sql = format!(
            "SELECT n.nspname::text, c.relname::text, c.relkind::text, \
                    CASE WHEN c.relkind IN ('r', 'p', 'f') AND c.reltuples >= 0 THEN c.reltuples::float8 END, ''::text \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r', 'p', 'f', 'v', 'm', 'S') AND {USER_SCHEMA} \
             UNION ALL \
             SELECT n.nspname::text, p.proname::text, CASE p.prokind WHEN 'p' THEN 'proc' ELSE 'fn' END, NULL::float8, \
                    pg_get_function_identity_arguments(p.oid) \
             FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE p.prokind IN ('f', 'p') AND {USER_SCHEMA} \
               AND NOT EXISTS (SELECT 1 FROM pg_depend d \
                               WHERE d.classid = 'pg_proc'::regclass AND d.objid = p.oid AND d.deptype = 'e')"
        );
        let rows = self.client.query(sql.as_str(), &[]).await.map_err(query_error)?;
        let mut objects: Vec<DbObject> = rows
            .iter()
            .map(|r| {
                let kind = match r.get::<_, &str>(2) {
                    "r" | "p" | "f" => ObjectKind::Table,
                    "v" => ObjectKind::View,
                    "m" => ObjectKind::MaterializedView,
                    "S" => ObjectKind::Sequence,
                    "proc" => ObjectKind::Procedure,
                    _ => ObjectKind::Function,
                };
                DbObject {
                    schema: r.get(0),
                    name: r.get(1),
                    kind,
                    rows: r.get::<_, Option<f64>>(3).map(|n| n as i64),
                    detail: r.get(4),
                }
            })
            .collect();
        sql::sort_objects(&mut objects);
        Ok(objects)
    }

    pub async fn describe(&mut self, object: &ObjectRef) -> DbResult<TableInfo> {
        let c = &self.client;
        let found = c
            .query_opt(
                "SELECT c.oid, c.relkind::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE c.relname = $1::text AND c.relkind IN ('r', 'p', 'f', 'v', 'm') \
                   AND (n.nspname = $2::text OR ($2::text = '' AND pg_table_is_visible(c.oid)))",
                &[&object.name, &object.schema],
            )
            .await
            .map_err(query_error)?;
        let Some(found) = found else {
            return Err(DbError::NotFound(format!(
                "There is no table or view {}",
                super::qualified(Engine::Postgres, object)
            )));
        };
        let (oid, relkind): (u32, String) = (found.get(0), found.get(1));
        // The queries below are started together and awaited together: they
        // share the one connection, which pipelines them.
        let by_oid: &[&(dyn ToSql + Sync)] = &[&oid];

        let columns = c.query(
            "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
                    pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, a.attgenerated::text \
             FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum",
            by_oid,
        );
        let key = c.query(
            "SELECT a.attname::text, k.ord::int4 \
             FROM pg_index i \
             CROSS JOIN LATERAL unnest(i.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
             JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum \
             WHERE i.indrelid = $1 AND i.indisprimary",
            by_oid,
        );
        let indexes = c.query(
            "SELECT ic.relname::text, i.indisunique, i.indisprimary, \
                    ARRAY(SELECT pg_get_indexdef(i.indexrelid, k, true) FROM generate_series(1, i.indnkeyatts::int) AS k ORDER BY k), \
                    pg_get_indexdef(i.indexrelid), \
                    EXISTS (SELECT 1 FROM pg_constraint x \
                            WHERE x.conindid = i.indexrelid AND x.conrelid = i.indrelid AND x.contype IN ('p', 'u', 'x')) \
             FROM pg_index i JOIN pg_class ic ON ic.oid = i.indexrelid \
             WHERE i.indrelid = $1 ORDER BY ic.relname",
            by_oid,
        );
        let foreign_keys = c.query(
            "SELECT con.conname::text, \
                    ARRAY(SELECT a.attname::text FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
                          JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum ORDER BY k.ord), \
                    rn.nspname::text, rc.relname::text, \
                    ARRAY(SELECT a.attname::text FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord) \
                          JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum ORDER BY k.ord), \
                    pg_get_constraintdef(con.oid, true) \
             FROM pg_constraint con \
             JOIN pg_class rc ON rc.oid = con.confrelid JOIN pg_namespace rn ON rn.oid = rc.relnamespace \
             WHERE con.conrelid = $1 AND con.contype = 'f' ORDER BY con.conname",
            by_oid,
        );
        let constraints = c.query(
            "SELECT conname::text, pg_get_constraintdef(oid, true) FROM pg_constraint \
             WHERE conrelid = $1 AND contype IN ('p', 'u', 'c', 'x') \
             ORDER BY CASE contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'c' THEN 2 ELSE 3 END, conname",
            by_oid,
        );
        let (columns, key, indexes, foreign_keys, constraints) =
            try_join5(columns, key, indexes, foreign_keys, constraints).await.map_err(query_error)?;

        let key: HashMap<String, i32> = key.iter().map(|r| (r.get(0), r.get(1))).collect();
        let columns: Vec<PgColumn> = columns
            .iter()
            .map(|r| PgColumn {
                name: r.get(0),
                type_name: r.get(1),
                not_null: r.get(2),
                default: r.get(3),
                identity: r.get(4),
                generated: r.get(5),
            })
            .collect();
        let mut info = TableInfo {
            columns: columns
                .iter()
                .map(|c| ColumnInfo {
                    name: c.name.clone(),
                    type_name: c.type_name.clone(),
                    nullable: !c.not_null,
                    default: c.default.clone().filter(|_| c.generated.is_empty()),
                    primary_key: key.get(&c.name).and_then(|p| u32::try_from(*p).ok()),
                    // An identity BY DEFAULT may be written; ALWAYS only to DEFAULT.
                    generated: !c.generated.is_empty() || c.identity == "a",
                })
                .collect(),
            ..TableInfo::default()
        };
        let mut index_defs = Vec::new();
        for r in &indexes {
            info.indexes.push(IndexInfo { name: r.get(0), unique: r.get(1), primary: r.get(2), columns: r.get(3) });
            if !r.get::<_, bool>(5) {
                index_defs.push(r.get::<_, String>(4));
            }
        }
        let mut fk_defs = Vec::new();
        for r in &foreign_keys {
            let name: String = r.get(0);
            fk_defs.push((name.clone(), r.get::<_, String>(5)));
            info.foreign_keys.push(ForeignKeyInfo {
                name,
                columns: r.get(1),
                referenced: ObjectRef { schema: r.get(2), name: r.get(3) },
                referenced_columns: r.get(4),
            });
        }

        info.ddl = if matches!(relkind.as_str(), "v" | "m") {
            let def = c.query_one("SELECT pg_get_viewdef($1::oid, true)", by_oid).await.map_err(query_error)?;
            view_ddl(object, relkind == "m", &def.get::<_, String>(0), &index_defs)
        } else {
            let constraints: Vec<_> =
                constraints.iter().map(|r| (r.get::<_, String>(0), r.get::<_, String>(1))).collect();
            table_ddl(object, &columns, &constraints, &fk_defs, &index_defs)
        };
        Ok(info)
    }

    pub async fn execute(&mut self, sql: &str, sink: &mut dyn RowSink) -> DbResult<()> {
        self.canceller.cancelled.store(false, Ordering::SeqCst);
        for statement in sql::split(sql, Engine::Postgres) {
            // A stop pressed between two statements (or before the first) has
            // no query to land on.
            if self.canceller.cancelled.load(Ordering::SeqCst) || sink.cancelled() {
                return Err(DbError::Cancelled);
            }
            if !self.run(&statement.text, sink).await? {
                break;
            }
        }
        Ok(())
    }

    /// One statement. `false`: the sink had enough, the rest of the script is not run.
    async fn run(&self, text: &str, sink: &mut dyn RowSink) -> DbResult<bool> {
        // Preparing yields the column types. If it fails, the statement is
        // still tried as it is (a script the splitter cut wrongly may still
        // run); the prepare error stands in for the confusing "transaction is
        // aborted" the failed prepare itself leaves behind.
        let (prepared, prepare_error) = match self.client.prepare(text).await {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        let changes = sql::changes_rows(text);
        let stream = self.client.simple_query_raw(text).await.map_err(|e| self.failure(&e, prepare_error.as_ref()))?;
        let mut stream = std::pin::pin!(stream);
        let mut batch: Vec<Vec<Cell>> = Vec::new();
        let mut failed = None;
        let mut stopped = false;
        loop {
            let item = match stream.next().now_or_never() {
                Some(item) => item,
                // Nothing more is waiting: hand over what is here, so a slow
                // query shows its rows as they come in.
                None => {
                    if !flush(sink, &mut batch) {
                        stopped = true;
                        break;
                    }
                    stream.next().await
                }
            };
            match item {
                None => break,
                Some(Err(e)) => {
                    failed = Some(self.failure(&e, prepare_error.as_ref()));
                    break;
                }
                Some(Ok(SimpleQueryMessage::RowDescription(columns))) => {
                    sink.columns(column_list(&columns, prepared.as_ref()))
                }
                Some(Ok(SimpleQueryMessage::Row(row))) => {
                    match (0..row.len())
                        .map(|i| row.try_get(i).map(|v| v.map_or(Cell::Null, Cell::text)))
                        .collect::<Result<Vec<_>, _>>()
                    {
                        Ok(cells) => batch.push(cells),
                        Err(e) => {
                            failed = Some(query_error(e));
                            break;
                        }
                    }
                    if batch.len() >= BATCH && !flush(sink, &mut batch) {
                        stopped = true;
                        break;
                    }
                }
                Some(Ok(SimpleQueryMessage::CommandComplete(n))) => {
                    if !flush(sink, &mut batch) {
                        stopped = true;
                        break;
                    }
                    sink.done(changes.then_some(n));
                }
                Some(Ok(_)) => {}
            }
        }
        if let Some(error) = failed {
            flush(sink, &mut batch);
            return Err(error);
        }
        if stopped {
            // Ask the server to give up the rest, and read what is already on
            // the way (briefly) so the cancel can't land on the next statement.
            self.canceller.signal().await;
            let _ =
                tokio::time::timeout(Duration::from_secs(2), async { while stream.next().await.is_some() {} }).await;
            return Ok(false);
        }
        Ok(true)
    }

    /// A server error in the words the person can act on; the "canceled"
    /// after a stop is not an error but a stop.
    fn failure(&self, e: &tokio_postgres::Error, earlier: Option<&tokio_postgres::Error>) -> DbError {
        match e.code() {
            Some(code) if *code == SqlState::QUERY_CANCELED && self.canceller.cancelled.load(Ordering::SeqCst) => {
                DbError::Cancelled
            }
            Some(code) if *code == SqlState::IN_FAILED_SQL_TRANSACTION => query_failure(earlier.unwrap_or(e)),
            _ => query_failure(e),
        }
    }

    pub async fn apply_edits(&mut self, table: &ObjectRef, edits: &[RowEdit]) -> DbResult<u64> {
        self.canceller.cancelled.store(false, Ordering::SeqCst);
        // Inside a transaction the SQL tab left open, BEGIN is ignored and
        // COMMIT would commit the tab's own work too. Outside one, a simple
        // query is its own transaction: it started with the statement.
        match self.client.simple_query("SELECT now() = statement_timestamp()").await {
            Ok(messages) => {
                let alone = messages.iter().any(|m| matches!(m, SimpleQueryMessage::Row(r) if r.get(0) == Some("t")));
                if !alone {
                    return Err(sql::open_transaction());
                }
            }
            Err(e) if e.code() == Some(&SqlState::IN_FAILED_SQL_TRANSACTION) => return Err(sql::open_transaction()),
            Err(e) => return Err(query_error(e)),
        }
        let tx = self.client.transaction().await.map_err(query_error)?;
        let mut statements: HashMap<String, Statement> = HashMap::new();
        let mut written = 0;
        for edit in edits.iter().filter(|e| !e.changes.is_empty()) {
            if edit.key.is_empty() {
                return Err(DbError::NotEditable(crate::i18n::t("edit.no_key", &[])));
            }
            if edit.key.iter().any(|(_, v)| v.is_null()) {
                return Err(DbError::NotEditable(crate::i18n::t("edit.null_key", &[])));
            }
            let text = update_sql(table, edit);
            if !statements.contains_key(&text) {
                let prepared = tx.prepare(&text).await.map_err(query_error)?;
                statements.insert(text.clone(), prepared);
            }
            let params: Vec<Text<'_>> = edit.changes.iter().chain(&edit.key).map(|(_, c)| Text::of(c)).collect();
            let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as &(dyn ToSql + Sync)).collect();
            let changed = tx.execute(&statements[&text], &refs).await.map_err(query_error)?;
            if changed != 1 {
                let _ = tx.rollback().await;
                return Err(DbError::Query(sql::key_mismatch(&edit.key, changed)));
            }
            written += 1;
        }
        tx.commit().await.map_err(query_error)?;
        Ok(written)
    }
}

/// Opens one database: the connection's driver runs on the crate's runtime.
async fn open(
    spec: &ConnectSpec,
    tls: &MakeRustlsConnect,
    database: &str,
    deadline: Instant,
) -> Result<Client, Attempt> {
    let (host, port) = (spec.host(), spec.port());
    let mut cfg = tokio_postgres::Config::new();
    cfg.user(spec.config.user.trim()).dbname(database).application_name("Yuzora").port(port);
    cfg.ssl_mode(tls::pg_mode(spec.config.ssl));
    if let Some(password) = &spec.password {
        cfg.password(password);
    }
    // Through a tunnel the bytes go to its local end, but the certificate is
    // still the server's: the name to check stays the configured host.
    match spec.endpoint.as_ref().and_then(|(h, _)| h.parse::<IpAddr>().ok()) {
        Some(ip) => cfg.host(spec.config.host.trim()).hostaddr(ip),
        None => cfg.host(host),
    };
    let ssl = spec.config.ssl;
    let fail = |error: DbError, code: Option<String>| Attempt { error, code };
    match tokio::time::timeout_at(deadline, cfg.connect(tls.clone())).await {
        Err(_) => Err(fail(
            DbError::Connect(crate::i18n::t(
                "engine.timeout",
                &[("s", spec.connect_timeout.as_secs().to_string()), ("target", format!("{host}:{port}"))],
            )),
            None,
        )),
        Ok(Err(e)) => {
            if let Some(db) = e.as_db_error() {
                return Err(fail(DbError::Connect(server_message(db)), Some(db.code().code().to_owned())));
            }
            let text = describe(&e);
            Err(fail(
                if e.to_string().starts_with("error performing TLS handshake") {
                    // rustls refuses a CA certificate as the server's own, which is
                    // what a self-made `openssl req -x509` certificate usually is.
                    let mut error = text;
                    if error.contains("CaUsedAsEndEntity") {
                        error = format!("{error}\n{}", crate::i18n::t("engine.ca_as_server", &[]));
                    }
                    DbError::Tls(crate::i18n::t(
                        "engine.tls_failed",
                        &[("mode", ssl.id().into()), ("target", format!("{host}:{port}")), ("error", error)],
                    ))
                } else {
                    DbError::Connect(format!("{host}:{port}: {text}"))
                },
                None,
            ))
        }
        Ok(Ok((client, connection))) => {
            crate::runtime::handle().spawn(async move {
                let _ = connection.await;
            });
            Ok(client)
        }
    }
}

fn tried_all(tried: Vec<String>, last: DbError) -> DbError {
    DbError::Connect(format!(
        "{}\nThe databases tried: {}. Set a database in the connection to choose one.",
        last,
        tried.join("; ")
    ))
}

/// Passes `batch` on; `false` if the sink wants no more.
fn flush(sink: &mut dyn RowSink, batch: &mut Vec<Vec<Cell>>) -> bool {
    batch.is_empty() || sink.rows(std::mem::take(batch))
}

fn column_list(columns: &[SimpleColumn], prepared: Option<&Statement>) -> Vec<Column> {
    let types = prepared.map(Statement::columns).filter(|t| t.len() == columns.len());
    columns
        .iter()
        .enumerate()
        .map(|(i, c)| Column::new(c.name(), types.map(|t| type_name(t[i].type_())).unwrap_or_default()))
        .collect()
}

/// `int4`, `numeric`, `text[]`.
fn type_name(ty: &Type) -> String {
    match ty.kind() {
        Kind::Array(inner) => format!("{}[]", inner.name()),
        _ => ty.name().to_owned(),
    }
}

// ---------------------------------------------------------------- errors

fn query_error(e: tokio_postgres::Error) -> DbError {
    query_failure(&e)
}

fn query_failure(e: &tokio_postgres::Error) -> DbError {
    DbError::Query(describe(e))
}

/// What the server said, or else everything the error chain knows:
/// `tokio_postgres::Error` prints only a kind ("db error", "error connecting
/// to server") and keeps the cause behind `source()`.
fn describe(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => server_message(db),
        None if e.is_closed() => format!("The connection to the server was lost: {}", chain(e)),
        None => chain(e),
    }
}

/// `message (SQLSTATE)` with the detail, hint and position the server gave.
fn server_message(db: &ServerError) -> String {
    let mut out = format!("{} ({})", db.message(), db.code().code());
    if let Some(detail) = db.detail() {
        out.push_str(&format!("\nDETAIL: {detail}"));
    }
    if let Some(hint) = db.hint() {
        out.push_str(&format!("\nHINT: {hint}"));
    }
    if let Some(ErrorPosition::Original(at)) = db.position() {
        out.push_str(&format!("\nPOSITION: {at}"));
    }
    out
}

/// An error and its causes in one line, `a: b: c`.
fn chain(e: &(dyn StdError + 'static)) -> String {
    let mut out = e.to_string();
    let mut cause = e.source();
    while let Some(c) = cause {
        let text = c.to_string();
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        cause = c.source();
    }
    out
}

// ---------------------------------------------------------------- write-back

/// A parameter sent as text, whatever the column's type: the server parses it
/// as that type, and the value, which came out of the server as text, goes
/// back exactly as it was.
#[derive(Debug)]
struct Text<'a>(Option<Cow<'a, str>>);

impl<'a> Text<'a> {
    fn of(cell: &'a Cell) -> Self {
        Text(match cell {
            Cell::Null => None,
            Cell::Text(s) => Some(Cow::Borrowed(s)),
            Cell::Bool(b) => Some(Cow::Borrowed(if *b { "true" } else { "false" })),
            Cell::Int(i) => Some(Cow::Owned(i.to_string())),
            Cell::Float(f) => Some(Cow::Owned(f.to_string())),
            Cell::Bytes(b) => Some(Cow::Owned(b.iter().fold(String::from("\\x"), |mut s, byte| {
                s.push_str(&format!("{byte:02x}"));
                s
            }))),
        })
    }
}

impl ToSql for Text<'_> {
    fn to_sql(&self, _ty: &Type, out: &mut BytesMut) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
        match &self.0 {
            Some(text) => {
                out.extend_from_slice(text.as_bytes());
                Ok(IsNull::No)
            }
            None => Ok(IsNull::Yes),
        }
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }

    fn encode_format(&self, _ty: &Type) -> Format {
        Format::Text
    }

    to_sql_checked!();
}

/// `UPDATE "s"."t" SET "a" = $1 WHERE "k" = $2`: changes first, then the key.
fn update_sql(table: &ObjectRef, edit: &RowEdit) -> String {
    let quote = |c: &str| super::quote_ident(Engine::Postgres, c);
    let mut n = 0;
    let mut assign = |(column, _): &(String, Cell)| {
        n += 1;
        format!("{} = ${n}", quote(column))
    };
    let set: Vec<_> = edit.changes.iter().map(&mut assign).collect();
    let keys: Vec<_> = edit.key.iter().map(&mut assign).collect();
    format!("UPDATE {} SET {} WHERE {}", super::qualified(Engine::Postgres, table), set.join(", "), keys.join(" AND "))
}

// ---------------------------------------------------------------- DDL

struct PgColumn {
    name: String,
    type_name: String,
    not_null: bool,
    default: Option<String>,
    /// `a` always, `d` by default, empty: not an identity column.
    identity: String,
    /// `s` stored, empty: not generated.
    generated: String,
}

/// A `CREATE TABLE` rebuilt from the catalog (columns, then constraints,
/// then the indexes that no constraint owns). Not a dump: storage options,
/// partitioning and ownership are left out.
fn table_ddl(
    table: &ObjectRef,
    columns: &[PgColumn],
    constraints: &[(String, String)],
    foreign_keys: &[(String, String)],
    indexes: &[String],
) -> String {
    let q = |n: &str| super::quote_ident(Engine::Postgres, n);
    let mut lines: Vec<String> = columns
        .iter()
        .map(|c| {
            let mut line = format!("  {} {}", q(&c.name), c.type_name);
            match (c.identity.as_str(), c.generated.as_str(), &c.default) {
                ("a", ..) => line.push_str(" GENERATED ALWAYS AS IDENTITY"),
                ("d", ..) => line.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
                (_, "s", Some(expr)) => line.push_str(&format!(" GENERATED ALWAYS AS ({expr}) STORED")),
                (_, _, Some(expr)) => line.push_str(&format!(" DEFAULT {expr}")),
                _ => {}
            }
            if c.not_null {
                line.push_str(" NOT NULL");
            }
            line
        })
        .collect();
    lines.extend(constraints.iter().chain(foreign_keys).map(|(name, def)| format!("  CONSTRAINT {} {def}", q(name))));
    let mut ddl = format!("CREATE TABLE {} (\n{}\n);", super::qualified(Engine::Postgres, table), lines.join(",\n"));
    for index in indexes {
        ddl.push_str(&format!("\n\n{index};"));
    }
    ddl
}

fn view_ddl(view: &ObjectRef, materialized: bool, definition: &str, indexes: &[String]) -> String {
    let kind = if materialized { "MATERIALIZED VIEW" } else { "VIEW" };
    let body = definition.trim().trim_end_matches(';');
    let mut ddl = format!("CREATE {kind} {} AS\n{body};", super::qualified(Engine::Postgres, view));
    for index in indexes {
        ddl.push_str(&format!("\n\n{index};"));
    }
    ddl
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str) -> ObjectRef {
        ObjectRef { schema: "public".into(), name: name.into() }
    }

    #[test]
    fn parameters_are_text_for_any_column_type() {
        let mut out = BytesMut::new();
        let value = Cell::text("12345678901234567890.123456789012345678");
        let param = Text::of(&value);
        assert!(matches!(param.encode_format(&Type::NUMERIC), Format::Text));
        assert!(<Text<'_> as ToSql>::accepts(&Type::JSONB));
        assert!(matches!(param.to_sql(&Type::NUMERIC, &mut out).unwrap(), IsNull::No));
        assert_eq!(&out[..], b"12345678901234567890.123456789012345678");
        let mut out = BytesMut::new();
        assert!(matches!(Text::of(&Cell::Null).to_sql(&Type::INT4, &mut out).unwrap(), IsNull::Yes));
        assert!(out.is_empty());
    }

    #[test]
    fn other_cells_become_the_text_the_server_reads_back() {
        let text = |c: Cell| Text::of(&c).0.map(Cow::into_owned);
        assert_eq!(text(Cell::Int(-7)).as_deref(), Some("-7"));
        assert_eq!(text(Cell::Bool(true)).as_deref(), Some("true"));
        assert_eq!(text(Cell::Float(1.5)).as_deref(), Some("1.5"));
        assert_eq!(text(Cell::Bytes(vec![0x00, 0x01, 0xff].into())).as_deref(), Some("\\x0001ff"));
        assert_eq!(text(Cell::Null), None);
    }

    #[test]
    fn updates_set_the_changes_then_match_the_key() {
        let edit = RowEdit {
            key: vec![("id".into(), Cell::text("1")), ("tenant".into(), Cell::text("a"))],
            changes: vec![("na\"me".into(), Cell::text("n")), ("age".into(), Cell::Null)],
        };
        assert_eq!(
            update_sql(&t("people"), &edit),
            "UPDATE \"public\".\"people\" SET \"na\"\"me\" = $1, \"age\" = $2 WHERE \"id\" = $3 AND \"tenant\" = $4"
        );
    }

    #[test]
    fn types_print_with_array_brackets() {
        assert_eq!(type_name(&Type::INT4), "int4");
        assert_eq!(type_name(&Type::TEXT_ARRAY), "text[]");
        assert_eq!(type_name(&Type::NUMERIC), "numeric");
    }

    #[test]
    fn error_chains_keep_every_cause_once() {
        use std::io;
        #[derive(Debug)]
        struct Outer(io::Error);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("error connecting to server")
            }
        }
        impl StdError for Outer {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                Some(&self.0)
            }
        }
        let e = Outer(io::Error::new(io::ErrorKind::ConnectionRefused, "Connection refused (os error 61)"));
        assert_eq!(chain(&e), "error connecting to server: Connection refused (os error 61)");
    }

    #[test]
    fn tables_are_rebuilt_from_the_catalog() {
        let col = |name: &str, ty: &str, not_null, default: Option<&str>, identity: &str, generated: &str| PgColumn {
            name: name.into(),
            type_name: ty.into(),
            not_null,
            default: default.map(str::to_owned),
            identity: identity.into(),
            generated: generated.into(),
        };
        let columns = vec![
            col("id", "integer", true, None, "d", ""),
            col("label", "text", true, Some("'x'::text"), "", ""),
            col("twice", "integer", false, Some("(id * 2)"), "", "s"),
        ];
        let ddl = table_ddl(
            &t("things"),
            &columns,
            &[("things_pkey".into(), "PRIMARY KEY (id)".into())],
            &[("things_o".into(), "FOREIGN KEY (id) REFERENCES other(id)".into())],
            &["CREATE INDEX things_label ON public.things USING btree (label)".into()],
        );
        assert_eq!(
            ddl,
            "CREATE TABLE \"public\".\"things\" (\n\
             \x20 \"id\" integer GENERATED BY DEFAULT AS IDENTITY NOT NULL,\n\
             \x20 \"label\" text DEFAULT 'x'::text NOT NULL,\n\
             \x20 \"twice\" integer GENERATED ALWAYS AS ((id * 2)) STORED,\n\
             \x20 CONSTRAINT \"things_pkey\" PRIMARY KEY (id),\n\
             \x20 CONSTRAINT \"things_o\" FOREIGN KEY (id) REFERENCES other(id)\n\
             );\n\nCREATE INDEX things_label ON public.things USING btree (label);"
        );
        assert_eq!(
            view_ddl(&t("v"), true, " SELECT 1 AS one;", &[]),
            "CREATE MATERIALIZED VIEW \"public\".\"v\" AS\nSELECT 1 AS one;"
        );
    }
}
