//! SQL Server through Yuzora's patched tiberius (`app/vendor/tiberius-0.12.3-yuzora`).
//!
//! The protocol has an "attention" message that stops a running batch; the
//! driver does not send it. So stopping a query here means dropping the
//! connection: `cancel` ends the running `execute` and the socket with it
//! (the server abandons the batch), and the next call on this `MsConn`
//! quietly opens a new connection. The pool treats a connection without a
//! socket as closed, so it does not keep one around.

use std::sync::Arc;

use futures_util::future::{Either, select};
use futures_util::{FutureExt, TryStreamExt};
use tiberius::error::{Error as TdsError, TokenError};
use tiberius::numeric::Numeric;
use tiberius::{AuthMethod, Client, ColumnData, ColumnType, Config, EncryptionLevel, FromSql, QueryItem, Row, ToSql};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::{BATCH, ConnectSpec, RowSink};
use crate::config::{Engine, SslMode};
use crate::error::{DbError, DbResult};
use crate::model::{
    Cell, Column, ColumnInfo, DbObject, ForeignKeyInfo, IndexInfo, ObjectKind, ObjectRef, RowEdit, TableInfo,
};
use crate::sql::{self, Statement};

type Tds = Client<Compat<TcpStream>>;

pub struct MsConn {
    /// What to open again with after the socket was given up.
    spec: ConnectSpec,
    client: Option<Tds>,
    version: String,
    canceller: MsCanceller,
}

/// Ends the running `execute` (and the connection it ran on).
#[derive(Clone)]
pub struct MsCanceller {
    stop: Arc<watch::Sender<u64>>,
}

impl MsCanceller {
    pub async fn cancel(&self) {
        self.stop.send_modify(|n| *n += 1);
    }
}

impl MsConn {
    pub async fn connect(spec: &ConnectSpec) -> DbResult<Self> {
        let mut client = open(spec).await?;
        let version = scalar(&mut client, "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128))")
            .await
            .map_err(|e| DbError::Connect(e.to_string()))?;
        Ok(Self {
            spec: spec.clone(),
            client: Some(client),
            version: format!("SQL Server {version}"),
            canceller: MsCanceller { stop: Arc::new(watch::channel(0).0) },
        })
    }

    pub fn server_version(&self) -> String {
        self.version.clone()
    }

    /// There is no socket (a cancel or an early stop ended it).
    pub fn is_closed(&self) -> bool {
        self.client.is_none()
    }

    pub fn canceller(&self) -> MsCanceller {
        self.canceller.clone()
    }

    pub async fn databases(&mut self) -> DbResult<Vec<String>> {
        let rows = self
            .rows("SELECT name FROM sys.databases WHERE state = 0 AND HAS_DBACCESS(name) = 1 ORDER BY name", &[])
            .await?;
        rows.iter().map(|r| text(r, 0)).collect()
    }

    pub async fn objects(&mut self) -> DbResult<Vec<DbObject>> {
        let rows = self
            .rows(
                "SELECT s.name, o.name, RTRIM(o.type), \
                        CASE WHEN o.type = 'U' THEN (SELECT SUM(p.rows) FROM sys.partitions p \
                                                     WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)) END \
                 FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE o.is_ms_shipped = 0 AND o.type IN ('U', 'V', 'P', 'FN', 'IF', 'TF', 'SO')",
                &[],
            )
            .await?;
        let mut objects = Vec::with_capacity(rows.len());
        for r in &rows {
            let kind = match text(r, 2)?.as_str() {
                "U" => ObjectKind::Table,
                "V" => ObjectKind::View,
                "P" => ObjectKind::Procedure,
                "SO" => ObjectKind::Sequence,
                _ => ObjectKind::Function,
            };
            let rows = if kind == ObjectKind::Table { r.try_get::<i64, _>(3).map_err(query_error)? } else { None };
            objects.push(DbObject { schema: text(r, 0)?, name: text(r, 1)?, kind, rows, detail: String::new() });
        }
        sql::sort_objects(&mut objects);
        Ok(objects)
    }

    pub async fn describe(&mut self, object: &ObjectRef) -> DbResult<TableInfo> {
        let found = self
            .rows(
                "SELECT o.object_id, RTRIM(o.type) FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE s.name = COALESCE(NULLIF(@P1, N''), SCHEMA_NAME()) AND o.name = @P2 \
                   AND o.type IN ('U', 'V', 'P', 'FN', 'IF', 'TF')",
                &[&object.schema.as_str(), &object.name.as_str()],
            )
            .await?;
        let Some(found) = found.first() else {
            return Err(DbError::NotFound(format!(
                "There is no table or view {}",
                super::qualified(Engine::Mssql, object)
            )));
        };
        let id: i32 = found.try_get::<i32, _>(0).map_err(query_error)?.unwrap_or_default();
        let kind = text(found, 1)?;

        let rows = self
            .rows(
                "SELECT c.name, \
                        CASE WHEN t.name IN ('varchar', 'char', 'varbinary', 'binary') \
                               THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length AS varchar(10)) END + ')' \
                             WHEN t.name IN ('nvarchar', 'nchar') \
                               THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length / 2 AS varchar(10)) END + ')' \
                             WHEN t.name IN ('decimal', 'numeric') \
                               THEN t.name + '(' + CAST(c.precision AS varchar(10)) + ',' + CAST(c.scale AS varchar(10)) + ')' \
                             WHEN t.name IN ('datetime2', 'datetimeoffset', 'time') THEN t.name + '(' + CAST(c.scale AS varchar(10)) + ')' \
                             ELSE t.name END, \
                        c.is_nullable, dc.definition, cc.definition, \
                        CAST(ic.seed_value AS varchar(40)), CAST(ic.increment_value AS varchar(40)), pkc.key_ordinal \
                 FROM sys.columns c \
                 JOIN sys.types t ON t.user_type_id = c.user_type_id \
                 LEFT JOIN sys.default_constraints dc ON dc.parent_object_id = c.object_id AND dc.parent_column_id = c.column_id \
                 LEFT JOIN sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = c.column_id \
                 LEFT JOIN sys.identity_columns ic ON ic.object_id = c.object_id AND ic.column_id = c.column_id \
                 LEFT JOIN sys.indexes pk ON pk.object_id = c.object_id AND pk.is_primary_key = 1 \
                 LEFT JOIN sys.index_columns pkc ON pkc.object_id = pk.object_id AND pkc.index_id = pk.index_id \
                                                AND pkc.column_id = c.column_id AND pkc.is_included_column = 0 \
                 WHERE c.object_id = @P1 ORDER BY c.column_id",
                &[&id],
            )
            .await?;
        let mut columns = Vec::with_capacity(rows.len());
        for r in &rows {
            let (default, computed) = (opt_text(r, 3)?, opt_text(r, 4)?);
            columns.push(MsColumn {
                name: text(r, 0)?,
                type_name: text(r, 1)?,
                nullable: r.try_get::<bool, _>(2).map_err(query_error)?.unwrap_or(true),
                default,
                computed,
                identity: opt_text(r, 5)?.zip(opt_text(r, 6)?),
                key: r.try_get::<u8, _>(7).map_err(query_error)?.filter(|k| *k > 0).map(u32::from),
            });
        }

        let rows = self
            .rows(
                "SELECT i.name, i.is_unique, i.is_primary_key, i.is_unique_constraint, i.type, c.name, ic.is_descending_key \
                 FROM sys.indexes i \
                 JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0 \
                 JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
                 WHERE i.object_id = @P1 AND i.type > 0 AND ic.key_ordinal > 0 ORDER BY i.name, ic.key_ordinal",
                &[&id],
            )
            .await?;
        let mut indexes: Vec<MsIndex> = Vec::new();
        for r in &rows {
            let name = text(r, 0)?;
            if indexes.last().is_none_or(|i| i.name != name) {
                indexes.push(MsIndex {
                    name,
                    unique: r.try_get::<bool, _>(1).map_err(query_error)?.unwrap_or(false),
                    primary: r.try_get::<bool, _>(2).map_err(query_error)?.unwrap_or(false),
                    constraint: r.try_get::<bool, _>(3).map_err(query_error)?.unwrap_or(false),
                    clustered: r.try_get::<u8, _>(4).map_err(query_error)? == Some(1),
                    columns: Vec::new(),
                });
            }
            let descending = r.try_get::<bool, _>(6).map_err(query_error)?.unwrap_or(false);
            indexes.last_mut().expect("just pushed").columns.push((text(r, 5)?, descending));
        }

        let rows = self
            .rows(
                "SELECT fk.name, pc.name, rs.name, rt.name, rc.name \
                 FROM sys.foreign_keys fk \
                 JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
                 JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
                 JOIN sys.objects rt ON rt.object_id = fkc.referenced_object_id \
                 JOIN sys.schemas rs ON rs.schema_id = rt.schema_id \
                 JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
                 WHERE fk.parent_object_id = @P1 ORDER BY fk.name, fkc.constraint_column_id",
                &[&id],
            )
            .await?;
        let mut foreign_keys: Vec<ForeignKeyInfo> = Vec::new();
        for r in &rows {
            let name = text(r, 0)?;
            if foreign_keys.last().is_none_or(|f| f.name != name) {
                let referenced = ObjectRef { schema: text(r, 2)?, name: text(r, 3)? };
                foreign_keys.push(ForeignKeyInfo {
                    name,
                    columns: Vec::new(),
                    referenced,
                    referenced_columns: Vec::new(),
                });
            }
            let fk = foreign_keys.last_mut().expect("just pushed");
            fk.columns.push(text(r, 1)?);
            fk.referenced_columns.push(text(r, 4)?);
        }

        let ddl = if matches!(kind.as_str(), "U") {
            let rows = self
                .rows(
                    "SELECT name, definition FROM sys.check_constraints WHERE parent_object_id = @P1 ORDER BY name",
                    &[&id],
                )
                .await?;
            let checks = rows.iter().map(|r| Ok((text(r, 0)?, text(r, 1)?))).collect::<DbResult<Vec<_>>>()?;
            table_ddl(object, &columns, &indexes, &foreign_keys, &checks)
        } else {
            let rows = self.rows("SELECT OBJECT_DEFINITION(@P1)", &[&id]).await?;
            rows.first().map(|r| opt_text(r, 0)).transpose()?.flatten().map(|d| d.trim().to_owned()).unwrap_or_default()
        };

        Ok(TableInfo {
            columns: columns
                .iter()
                .map(|c| ColumnInfo {
                    name: c.name.clone(),
                    type_name: c.type_name.clone(),
                    nullable: c.nullable,
                    default: c.default.clone(),
                    primary_key: c.key,
                })
                .collect(),
            indexes: indexes
                .iter()
                .map(|i| IndexInfo {
                    name: i.name.clone(),
                    columns: i.columns.iter().map(|(c, _)| c.clone()).collect(),
                    unique: i.unique,
                    primary: i.primary,
                })
                .collect(),
            foreign_keys,
            ddl,
        })
    }

    pub async fn execute(&mut self, sql: &str, sink: &mut dyn RowSink) -> DbResult<()> {
        let batches = sql::split(sql, Engine::Mssql);
        // Subscribed before anything else: a cancel from here on counts.
        let mut stop = self.canceller.stop.subscribe();
        let client = self.take_client().await?;
        let work = Box::pin(run_batches(client, batches, sink));
        let cancelled = Box::pin(stop.changed());
        match select(work, cancelled).await {
            Either::Left(((client, flow), _)) => match flow {
                Flow::Finished => {
                    self.client = Some(client);
                    Ok(())
                }
                // The sink wants no more of a result that is still coming in:
                // the rest can only be dropped with the socket.
                Flow::Enough => Ok(()),
                Flow::Failed(error, usable) => {
                    self.client = usable.then_some(client);
                    Err(error)
                }
            },
            // The batch is dropped where it stands, and its socket with it.
            Either::Right(_) => Err(DbError::Cancelled),
        }
    }

    pub async fn apply_edits(&mut self, table: &ObjectRef, edits: &[RowEdit]) -> DbResult<u64> {
        let edits: Vec<&RowEdit> = edits.iter().filter(|e| !e.changes.is_empty()).collect();
        for edit in &edits {
            if edit.key.is_empty() {
                return Err(DbError::NotEditable(crate::i18n::t("edit.no_key", &[])));
            }
            if edit.key.iter().any(|(_, v)| v.is_null()) {
                return Err(DbError::NotEditable(crate::i18n::t("edit.null_key", &[])));
            }
        }
        let mut client = self.take_client().await?;
        match write_rows(&mut client, table, &edits).await {
            Ok(written) => {
                self.client = Some(client);
                Ok(written)
            }
            Err((error, usable)) => {
                // Whatever the failure, nothing stays half written; a connection
                // that can't even say so is let go.
                if usable {
                    let rolled_back = rollback(&mut client).await.is_ok();
                    self.client = rolled_back.then_some(client);
                }
                Err(error)
            }
        }
    }

    /// The socket, opened again if it was given up.
    async fn take_client(&mut self) -> DbResult<Tds> {
        match self.client.take() {
            Some(client) => Ok(client),
            None => open(&self.spec).await,
        }
    }

    /// `sql` with parameters; the rows of its first result.
    async fn rows(&mut self, sql: &str, params: &[&dyn ToSql]) -> DbResult<Vec<Row>> {
        let mut client = self.take_client().await?;
        match first_result(&mut client, sql, params).await {
            Ok(rows) => {
                self.client = Some(client);
                Ok(rows)
            }
            Err(e) => {
                if !fatal(&e) {
                    self.client = Some(client);
                }
                Err(query_error(e))
            }
        }
    }
}

async fn first_result(client: &mut Tds, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Row>, TdsError> {
    client.query(sql, params).await?.into_first_result().await
}

async fn rollback(client: &mut Tds) -> Result<(), TdsError> {
    batch(client, "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION").await
}

/// A batch whose results (there are none) are not wanted.
async fn batch(client: &mut Tds, sql: &str) -> Result<(), TdsError> {
    client.simple_query(sql).await?.into_results().await.map(drop)
}

// ---------------------------------------------------------------- connecting

async fn open(spec: &ConnectSpec) -> DbResult<Tds> {
    let cfg = &spec.config;
    let (host, port) = (spec.host(), spec.port());
    let mut config = Config::new();
    // The name the certificate is checked against, whatever the socket goes to.
    config.host(cfg.host.trim());
    config.port(port);
    if !cfg.instance.trim().is_empty() {
        // The driver can't ask the SQL Browser which port an instance has.
        if cfg.port.is_none() && spec.endpoint.is_none() {
            return Err(DbError::Config(crate::i18n::t("engine.instance_port", &[("instance", cfg.instance.trim().into())])));
        }
        config.instance_name(cfg.instance.trim());
    }
    if let Some(database) = spec.database() {
        config.database(database);
    }
    config.authentication(AuthMethod::sql_server(cfg.user.trim(), spec.password.as_deref().unwrap_or_default()));
    config.application_name("Yuzora");
    match cfg.ssl {
        SslMode::Disable => config.encryption(EncryptionLevel::NotSupported),
        SslMode::Prefer => {
            config.encryption(EncryptionLevel::Off);
            config.trust_cert();
        }
        SslMode::Require => {
            config.encryption(EncryptionLevel::Required);
            config.trust_cert();
        }
        // The driver always checks the name on the certificate as well.
        SslMode::VerifyCa | SslMode::VerifyFull => {
            config.encryption(EncryptionLevel::Required);
            if !cfg.ca_file.trim().is_empty() {
                config.trust_cert_ca(expand_home(cfg.ca_file.trim()));
            }
        }
    }

    let deadline = Instant::now() + spec.connect_timeout;
    let timed_out = || {
        DbError::Connect(crate::i18n::t(
            "engine.timeout",
            &[("s", spec.connect_timeout.as_secs().to_string()), ("target", format!("{host}:{port}"))],
        ))
    };
    let tcp = tokio::time::timeout_at(deadline, TcpStream::connect((host, port)))
        .await
        .map_err(|_| timed_out())?
        .map_err(|e| DbError::Connect(format!("{host}:{port}: {e}")))?;
    tcp.set_nodelay(true).map_err(|e| DbError::Connect(format!("{host}:{port}: {e}")))?;
    tokio::time::timeout_at(deadline, Client::connect(config, tcp.compat_write()))
        .await
        .map_err(|_| timed_out())?
        .map_err(|e| {
            let tls = |message: String| {
                DbError::Tls(crate::i18n::t(
                    "engine.tls_failed",
                    &[("mode", cfg.ssl.id().into()), ("target", format!("{host}:{port}")), ("error", message)],
                ))
            };
            match e {
                TdsError::Tls(message) => tls(message),
                TdsError::Server(token) => DbError::Connect(server_message(&token)),
                // The driver hands a handshake failure over as an I/O error.
                TdsError::Io { message, .. } if is_tls_failure(&message) => tls(message),
                TdsError::Io { message, .. } => DbError::Connect(format!("{host}:{port}: {message}")),
                other => DbError::Connect(format!("{host}:{port}: {other}")),
            }
        })
}

/// rustls' words for a handshake that went wrong.
fn is_tls_failure(message: &str) -> bool {
    ["certificate", "handshake", "alert", "peer is incompatible", "peer sent no"].iter().any(|w| message.contains(w))
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/").or_else(|| (path == "~").then_some("")) {
        Some(rest) => yuzora_core::paths::home_dir().join(rest).to_string_lossy().into_owned(),
        None => path.to_owned(),
    }
}

/// The first column of the first row.
async fn scalar(client: &mut Tds, sql: &str) -> Result<String, TdsError> {
    let row = client.simple_query(sql).await?.into_row().await?;
    Ok(row.and_then(|r| r.try_get::<&str, _>(0).ok().flatten().map(str::to_owned)).unwrap_or_default())
}

// ---------------------------------------------------------------- execute

/// How the batches ended.
enum Flow {
    Finished,
    /// The sink asked to stop while rows were still arriving.
    Enough,
    /// An error; whether the connection can still be used.
    Failed(DbError, bool),
}

async fn run_batches(mut client: Tds, batches: Vec<Statement>, sink: &mut dyn RowSink) -> (Tds, Flow) {
    for batch in &batches {
        match run_batch(&mut client, &batch.text, sink).await {
            Ok(true) => {}
            Ok(false) => return (client, Flow::Enough),
            Err((error, usable)) => return (client, Flow::Failed(error, usable)),
        }
    }
    (client, Flow::Finished)
}

/// One batch. `false`: the sink stopped it. The error says whether the
/// connection survives it.
async fn run_batch(client: &mut Tds, text: &str, sink: &mut dyn RowSink) -> Result<bool, (DbError, bool)> {
    let fail = |e: TdsError| (query_error(e.clone()), !fatal(&e));
    let mut stream = client.simple_query(text).await.map_err(fail)?;
    // The server's row counts come in wire order, one per statement that
    // reports one. A result set's own count (its row count) is not a change;
    // what lies between result sets is.
    let mut seen = 0;
    let mut open: Option<u64> = None;
    let mut told = false;
    let mut batch: Vec<Vec<Cell>> = Vec::new();
    loop {
        let item = match stream.try_next().now_or_never() {
            Some(item) => item,
            // Nothing more is waiting: pass on what is here, so a slow query
            // shows its rows as they come in.
            None => {
                if !flush(sink, &mut batch) {
                    return Ok(false);
                }
                stream.try_next().await
            }
        };
        let item = match item {
            Ok(item) => item,
            Err(e) => {
                flush(sink, &mut batch);
                return Err(fail(e));
            }
        };
        match item {
            Some(QueryItem::Metadata(meta)) => {
                if !flush(sink, &mut batch) {
                    return Ok(false);
                }
                settle(stream.rows_affected(), &mut seen, open.take(), sink, &mut told);
                let columns =
                    meta.columns().iter().map(|c| Column::new(c.name(), type_name(c.column_type()))).collect();
                sink.columns(columns);
                open = Some(0);
            }
            Some(QueryItem::Row(row)) => {
                let cells =
                    row.cells().map(|(_, data)| cell(data)).collect::<DbResult<Vec<_>>>().map_err(|e| (e, true))?;
                batch.push(cells);
                if let Some(n) = open.as_mut() {
                    *n += 1;
                }
                if batch.len() >= BATCH && !flush(sink, &mut batch) {
                    return Ok(false);
                }
            }
            None => break,
        }
    }
    if !flush(sink, &mut batch) {
        return Ok(false);
    }
    settle(stream.rows_affected(), &mut seen, open.take(), sink, &mut told);
    if !told {
        sink.done(None);
    }
    Ok(true)
}

/// Closes the open result set (if any) and reports what the statements since
/// then changed. `counts[*seen..]` are the DONE counts that came in since the
/// last call: the first belongs to the result set that is closing (when its
/// row count says so), the rest to statements that changed rows.
fn settle(counts: &[u64], seen: &mut usize, closing: Option<u64>, sink: &mut dyn RowSink, told: &mut bool) {
    let mut fresh = &counts[(*seen).min(counts.len())..];
    *seen = counts.len();
    if let Some(rows) = closing {
        sink.done(None);
        *told = true;
        if fresh.first() == Some(&rows) {
            fresh = &fresh[1..];
        }
    }
    if !fresh.is_empty() {
        sink.done(Some(fresh.iter().fold(0u64, |sum, n| sum.saturating_add(*n))));
        *told = true;
    }
}

fn flush(sink: &mut dyn RowSink, batch: &mut Vec<Vec<Cell>>) -> bool {
    batch.is_empty() || sink.rows(std::mem::take(batch))
}

// ---------------------------------------------------------------- values

fn cell(data: &ColumnData<'static>) -> DbResult<Cell> {
    fn shown<'a, T: FromSql<'a> + ToString>(data: &'a ColumnData<'static>) -> DbResult<Cell> {
        T::from_sql(data).map_err(query_error).map(|v| v.map_or(Cell::Null, |v| Cell::text(v.to_string())))
    }
    Ok(match data {
        ColumnData::U8(v) => v.map_or(Cell::Null, |n| Cell::Int(i64::from(n))),
        ColumnData::I16(v) => v.map_or(Cell::Null, |n| Cell::Int(i64::from(n))),
        ColumnData::I32(v) => v.map_or(Cell::Null, |n| Cell::Int(i64::from(n))),
        ColumnData::I64(v) => v.map_or(Cell::Null, Cell::Int),
        // 0.1f32 is 0.1, not 0.10000000149011612.
        ColumnData::F32(v) => v.map_or(Cell::Null, |f| Cell::Float(f.to_string().parse().unwrap_or(f64::from(f)))),
        ColumnData::F64(v) => v.map_or(Cell::Null, Cell::Float),
        ColumnData::Bit(v) => v.map_or(Cell::Null, Cell::Bool),
        ColumnData::String(v) => v.as_ref().map_or(Cell::Null, |s| Cell::text(s.as_ref())),
        ColumnData::Guid(v) => v.map_or(Cell::Null, |g| Cell::text(g.to_string().to_uppercase())),
        ColumnData::Binary(v) => v.as_ref().map_or(Cell::Null, |b| Cell::Bytes(b.as_ref().into())),
        ColumnData::Numeric(v) => v.map_or(Cell::Null, |n| Cell::text(numeric_text(n))),
        ColumnData::Xml(v) => v.as_ref().map_or(Cell::Null, |x| Cell::text(x.to_string())),
        // `datetime` counts in 1/300 s; SQL Server prints it rounded to the millisecond.
        ColumnData::DateTime(_) => match chrono::NaiveDateTime::from_sql(data).map_err(query_error)? {
            Some(at) => Cell::text(to_millis(at).to_string()),
            None => Cell::Null,
        },
        ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => shown::<chrono::NaiveDateTime>(data)?,
        ColumnData::Date(_) => shown::<chrono::NaiveDate>(data)?,
        ColumnData::Time(_) => shown::<chrono::NaiveTime>(data)?,
        // The offset it was stored with stays: 2024-02-29 12:34:56.123456 +08:00.
        ColumnData::DateTimeOffset(_) => shown::<chrono::DateTime<chrono::FixedOffset>>(data)?,
    })
}

/// `at` rounded to the nearest millisecond.
fn to_millis(at: chrono::NaiveDateTime) -> chrono::NaiveDateTime {
    use chrono::Timelike;
    let at = at + chrono::Duration::nanoseconds(500_000);
    at.with_nanosecond(at.nanosecond() / 1_000_000 * 1_000_000).unwrap_or(at)
}

/// A decimal with all its digits: `12345.600000` at scale 6, `-0.05`.
fn numeric_text(n: Numeric) -> String {
    let scale = usize::from(n.scale());
    let mut digits = n.value().unsigned_abs().to_string();
    if scale > 0 {
        if digits.len() <= scale {
            digits.insert_str(0, &"0".repeat(scale + 1 - digits.len()));
        }
        digits.insert(digits.len() - scale, '.');
    }
    if n.value() < 0 {
        digits.insert(0, '-');
    }
    digits
}

fn type_name(t: ColumnType) -> &'static str {
    match t {
        ColumnType::Null => "null",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 | ColumnType::Intn => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money => "money",
        ColumnType::Money4 => "smallmoney",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::Decimaln | ColumnType::Numericn => "decimal",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::BigVarBin => "varbinary",
        ColumnType::BigBinary => "binary",
        ColumnType::BigVarChar => "varchar",
        ColumnType::BigChar => "char",
        ColumnType::NVarchar => "nvarchar",
        ColumnType::NChar => "nchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::Text => "text",
        ColumnType::Image => "image",
        ColumnType::NText => "ntext",
        ColumnType::SSVariant => "sql_variant",
    }
}

// ---------------------------------------------------------------- errors

/// The connection can't be used after this one.
fn fatal(e: &TdsError) -> bool {
    !matches!(e, TdsError::Server(_) | TdsError::Conversion(_) | TdsError::ParseInt(_) | TdsError::BulkInput(_))
}

fn query_error(e: TdsError) -> DbError {
    match e {
        TdsError::Server(token) => DbError::Query(server_message(&token)),
        TdsError::Io { message, .. } => DbError::Query(crate::i18n::t("engine.lost", &[("error", message)])),
        other => DbError::Query(other.to_string()),
    }
}

/// `message (Msg 102, Level 15, State 1, Line 1)`, the way sqlcmd prints it.
fn server_message(t: &TokenError) -> String {
    let procedure = if t.procedure().is_empty() { String::new() } else { format!(", Procedure {}", t.procedure()) };
    format!(
        "{} (Msg {}, Level {}, State {}{procedure}, Line {})",
        t.message().trim(),
        t.code(),
        t.class(),
        t.state(),
        t.line()
    )
}

fn text(row: &Row, i: usize) -> DbResult<String> {
    Ok(opt_text(row, i)?.unwrap_or_default())
}

fn opt_text(row: &Row, i: usize) -> DbResult<Option<String>> {
    row.try_get::<&str, _>(i).map(|v| v.map(str::to_owned)).map_err(query_error)
}

// ---------------------------------------------------------------- write-back

/// A parameter of an `UPDATE`.
enum Param<'a> {
    Text(&'a str),
    Int(i64),
    Float(f64),
    Bool(bool),
    Bytes(&'a [u8]),
    Null,
}

static NO_TEXT: Option<&str> = None;

impl<'a> Param<'a> {
    fn of(cell: &'a Cell) -> Self {
        match cell {
            Cell::Null => Param::Null,
            Cell::Bool(b) => Param::Bool(*b),
            Cell::Int(i) => Param::Int(*i),
            Cell::Float(f) => Param::Float(*f),
            Cell::Text(s) => Param::Text(s),
            Cell::Bytes(b) => Param::Bytes(b),
        }
    }

    fn sql(&self) -> &dyn ToSql {
        match self {
            Param::Text(s) => s,
            Param::Int(i) => i,
            Param::Float(f) => f,
            Param::Bool(b) => b,
            Param::Bytes(b) => b,
            Param::Null => &NO_TEXT,
        }
    }
}

/// `UPDATE [s].[t] SET [a] = @P1 WHERE [k] = @P2; SELECT ROWCOUNT_BIG();`:
/// changes first, then the key. The count is the `UPDATE`'s own (a trigger's
/// writes do not add to it).
fn update_sql(table: &ObjectRef, edit: &RowEdit) -> String {
    let quote = |c: &str| super::quote_ident(Engine::Mssql, c);
    let mut n = 0;
    let mut assign = |(column, _): &(String, Cell)| {
        n += 1;
        format!("{} = @P{n}", quote(column))
    };
    let set: Vec<_> = edit.changes.iter().map(&mut assign).collect();
    let keys: Vec<_> = edit.key.iter().map(&mut assign).collect();
    format!(
        "UPDATE {} SET {} WHERE {}; SELECT ROWCOUNT_BIG();",
        super::qualified(Engine::Mssql, table),
        set.join(", "),
        keys.join(" AND ")
    )
}

/// Runs the edits in one transaction. On error, whether the connection is
/// still usable (so the caller can roll back).
async fn write_rows(client: &mut Tds, table: &ObjectRef, edits: &[&RowEdit]) -> Result<u64, (DbError, bool)> {
    let fail = |e: TdsError| (query_error(e.clone()), !fatal(&e));
    // The transaction commands are batches: run as an RPC (`execute`) the server
    // objects that the transaction count changed.
    batch(client, "BEGIN TRANSACTION").await.map_err(fail)?;
    let mut written = 0;
    for edit in edits {
        let params: Vec<Param<'_>> = edit.changes.iter().chain(&edit.key).map(|(_, c)| Param::of(c)).collect();
        let refs: Vec<&dyn ToSql> = params.iter().map(Param::sql).collect();
        let sql = update_sql(table, edit);
        let rows = client.query(sql, &refs).await.map_err(fail)?.into_first_result().await.map_err(fail)?;
        let changed = rows.first().and_then(|r| r.try_get::<i64, _>(0).ok().flatten()).unwrap_or(-1);
        if changed != 1 {
            return Err((DbError::Query(sql::key_mismatch(&edit.key, u64::try_from(changed).unwrap_or(0))), true));
        }
        written += 1;
    }
    batch(client, "COMMIT TRANSACTION").await.map_err(fail)?;
    Ok(written)
}

// ---------------------------------------------------------------- DDL

struct MsColumn {
    name: String,
    type_name: String,
    nullable: bool,
    default: Option<String>,
    /// The expression of a computed column.
    computed: Option<String>,
    /// Seed and increment of an identity column.
    identity: Option<(String, String)>,
    key: Option<u32>,
}

struct MsIndex {
    name: String,
    unique: bool,
    primary: bool,
    /// A `UNIQUE` constraint (an index that is one only by being named in the table).
    constraint: bool,
    clustered: bool,
    /// Key columns in order, with whether each is descending.
    columns: Vec<(String, bool)>,
}

/// A `CREATE TABLE` rebuilt from the catalog (columns, then keys and checks
/// and foreign keys, then the indexes that are not constraints). Not a dump:
/// filegroups, partitioning and index options are left out.
fn table_ddl(
    table: &ObjectRef,
    columns: &[MsColumn],
    indexes: &[MsIndex],
    foreign_keys: &[ForeignKeyInfo],
    checks: &[(String, String)],
) -> String {
    let q = |n: &str| super::quote_ident(Engine::Mssql, n);
    let list = |cols: &[String]| cols.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
    let key_list = |cols: &[(String, bool)]| {
        cols.iter()
            .map(|(c, desc)| format!("{}{}", q(c), if *desc { " DESC" } else { "" }))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut lines: Vec<String> = columns
        .iter()
        .map(|c| match &c.computed {
            Some(expression) => format!("  {} AS {expression}", q(&c.name)),
            None => {
                let mut line = format!("  {} {}", q(&c.name), c.type_name);
                if let Some((seed, step)) = &c.identity {
                    line.push_str(&format!(" IDENTITY({seed}, {step})"));
                }
                if let Some(default) = &c.default {
                    line.push_str(&format!(" DEFAULT {default}"));
                }
                line.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
                line
            }
        })
        .collect();
    let owned = |i: &&MsIndex| i.primary || i.constraint;
    for index in indexes.iter().filter(owned) {
        let kind = if index.primary { "PRIMARY KEY" } else { "UNIQUE" };
        let clustering = if index.clustered { "CLUSTERED" } else { "NONCLUSTERED" };
        lines.push(format!("  CONSTRAINT {} {kind} {clustering} ({})", q(&index.name), key_list(&index.columns)));
    }
    lines.extend(checks.iter().map(|(name, definition)| format!("  CONSTRAINT {} CHECK {definition}", q(name))));
    lines.extend(foreign_keys.iter().map(|f| {
        format!(
            "  CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
            q(&f.name),
            list(&f.columns),
            super::qualified(Engine::Mssql, &f.referenced),
            list(&f.referenced_columns)
        )
    }));
    let mut ddl = format!("CREATE TABLE {} (\n{}\n);", super::qualified(Engine::Mssql, table), lines.join(",\n"));
    for index in indexes.iter().filter(|i| !owned(i)) {
        ddl.push_str(&format!(
            "\n\nCREATE {}{} INDEX {} ON {} ({});",
            if index.unique { "UNIQUE " } else { "" },
            if index.clustered { "CLUSTERED" } else { "NONCLUSTERED" },
            q(&index.name),
            super::qualified(Engine::Mssql, table),
            key_list(&index.columns)
        ));
    }
    ddl
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(schema: &str, name: &str) -> ObjectRef {
        ObjectRef { schema: schema.into(), name: name.into() }
    }

    #[test]
    fn decimals_keep_every_digit_and_the_sign() {
        let n = |v: i128, s: u8| numeric_text(Numeric::new_with_scale(v, s));
        assert_eq!(n(12345678901234567890123456789012345678, 18), "12345678901234567890.123456789012345678");
        assert_eq!(n(-5, 2), "-0.05");
        assert_eq!(n(5, 2), "0.05");
        assert_eq!(n(-1234500, 4), "-123.4500");
        assert_eq!(n(0, 6), "0.000000");
        assert_eq!(n(42, 0), "42");
        assert_eq!(n(-42, 0), "-42");
    }

    #[test]
    fn values_keep_their_kinds() {
        use std::borrow::Cow;
        assert_eq!(cell(&ColumnData::I32(Some(7))).unwrap(), Cell::Int(7));
        assert_eq!(cell(&ColumnData::U8(Some(255))).unwrap(), Cell::Int(255));
        assert_eq!(cell(&ColumnData::I64(None)).unwrap(), Cell::Null);
        assert_eq!(cell(&ColumnData::F32(Some(0.1))).unwrap(), Cell::Float(0.1));
        assert_eq!(cell(&ColumnData::Bit(Some(true))).unwrap(), Cell::Bool(true));
        assert_eq!(cell(&ColumnData::String(Some(Cow::Borrowed("é")))).unwrap(), Cell::text("é"));
        assert_eq!(
            cell(&ColumnData::Binary(Some(Cow::Borrowed(&[1u8, 2][..])))).unwrap(),
            Cell::Bytes(vec![1, 2].into())
        );
        let guid = tiberius::Uuid::parse_str("6f9619ff-8b86-d011-b42d-00c04fc964ff").unwrap();
        assert_eq!(cell(&ColumnData::Guid(Some(guid))).unwrap(), Cell::text("6F9619FF-8B86-D011-B42D-00C04FC964FF"));
        assert_eq!(cell(&ColumnData::Numeric(Some(Numeric::new_with_scale(1250, 2)))).unwrap(), Cell::text("12.50"));
    }

    #[test]
    fn the_old_datetime_shows_milliseconds_like_sql_server() {
        let at =
            |ns: u32| chrono::NaiveDate::from_ymd_opt(2024, 2, 29).unwrap().and_hms_nano_opt(12, 34, 56, ns).unwrap();
        // 1/300 s steps: .123333333 prints .123, .126666667 prints .127, .9985 carries into the next second.
        assert_eq!(to_millis(at(123_333_333)).to_string(), "2024-02-29 12:34:56.123");
        assert_eq!(to_millis(at(126_666_667)).to_string(), "2024-02-29 12:34:56.127");
        assert_eq!(to_millis(at(999_666_667)).to_string(), "2024-02-29 12:34:57");
        assert_eq!(to_millis(at(0)).to_string(), "2024-02-29 12:34:56");
    }

    #[test]
    fn types_are_named_the_way_sql_server_does() {
        assert_eq!(type_name(ColumnType::Int4), "int");
        assert_eq!(type_name(ColumnType::NVarchar), "nvarchar");
        assert_eq!(type_name(ColumnType::Datetime2), "datetime2");
        assert!(crate::model::is_numeric_type(type_name(ColumnType::Int8)));
        assert!(crate::model::is_numeric_type(type_name(ColumnType::Numericn)));
        assert!(!crate::model::is_numeric_type(type_name(ColumnType::Guid)));
    }

    #[test]
    fn updates_set_the_changes_then_match_the_key_and_count_their_own_rows() {
        let edit = RowEdit {
            key: vec![("id".into(), Cell::Int(1)), ("a]b".into(), Cell::text("x"))],
            changes: vec![("name".into(), Cell::text("n")), ("age".into(), Cell::Null)],
        };
        assert_eq!(
            update_sql(&t("dbo", "people"), &edit),
            "UPDATE [dbo].[people] SET [name] = @P1, [age] = @P2 WHERE [id] = @P3 AND [a]]b] = @P4; SELECT ROWCOUNT_BIG();"
        );
    }

    #[test]
    fn counts_between_result_sets_are_what_changed() {
        #[derive(Default)]
        struct Log(Vec<String>);
        impl RowSink for Log {
            fn columns(&mut self, _: Vec<Column>) {}
            fn rows(&mut self, _: Vec<Vec<Cell>>) -> bool {
                true
            }
            fn done(&mut self, affected: Option<u64>) {
                self.0.push(format!("{affected:?}"));
            }
        }
        // UPDATE (3) ; SELECT of 2 rows (count 2) ; INSERT (1) ; INSERT (4) ; SELECT of 0 rows (count 0)
        let counts = [3, 2, 1, 4, 0];
        let (mut log, mut seen, mut told) = (Log::default(), 0, false);
        // The first result set starts after the UPDATE: only [3] is in so far.
        settle(&counts[..1], &mut seen, None, &mut log, &mut told);
        // The second starts after the INSERTs; the first closes with its own count (2).
        settle(&counts[..4], &mut seen, Some(2), &mut log, &mut told);
        // The end: the second closes (count 0).
        settle(&counts, &mut seen, Some(0), &mut log, &mut told);
        assert_eq!(log.0, ["Some(3)", "None", "Some(5)", "None"]);
        assert!(told);

        // NOCOUNT ON: no counts at all, and a bare `done(None)` is left to the caller.
        let (mut log, mut seen, mut told) = (Log::default(), 0, false);
        settle(&[], &mut seen, None, &mut log, &mut told);
        assert!(log.0.is_empty() && !told);
    }

    #[test]
    fn tables_are_rebuilt_from_the_catalog() {
        let col = |name: &str,
                   ty: &str,
                   nullable,
                   default: Option<&str>,
                   computed: Option<&str>,
                   identity: Option<(&str, &str)>| MsColumn {
            name: name.into(),
            type_name: ty.into(),
            nullable,
            default: default.map(str::to_owned),
            computed: computed.map(str::to_owned),
            identity: identity.map(|(a, b)| (a.to_owned(), b.to_owned())),
            key: None,
        };
        let columns = vec![
            col("id", "int", false, None, None, Some(("1", "1"))),
            col("label", "nvarchar(32)", false, Some("(N'x')"), None, None),
            col("twice", "int", true, None, Some("([id]*(2))"), None),
        ];
        let index = |name: &str, unique, primary, constraint, clustered, cols: &[(&str, bool)]| MsIndex {
            name: name.into(),
            unique,
            primary,
            constraint,
            clustered,
            columns: cols.iter().map(|(c, d)| ((*c).to_owned(), *d)).collect(),
        };
        let indexes = vec![
            index("PK_things", true, true, false, true, &[("id", false)]),
            index("IX_things_label", false, false, false, false, &[("label", true), ("id", false)]),
        ];
        let fks = vec![ForeignKeyInfo {
            name: "FK_things_other".into(),
            columns: vec!["id".into()],
            referenced: t("dbo", "other"),
            referenced_columns: vec!["id".into()],
        }];
        assert_eq!(
            table_ddl(&t("dbo", "things"), &columns, &indexes, &fks, &[("CK_things".into(), "([id]>(0))".into())]),
            "CREATE TABLE [dbo].[things] (\n\
             \x20 [id] int IDENTITY(1, 1) NOT NULL,\n\
             \x20 [label] nvarchar(32) DEFAULT (N'x') NOT NULL,\n\
             \x20 [twice] AS ([id]*(2)),\n\
             \x20 CONSTRAINT [PK_things] PRIMARY KEY CLUSTERED ([id]),\n\
             \x20 CONSTRAINT [CK_things] CHECK ([id]>(0)),\n\
             \x20 CONSTRAINT [FK_things_other] FOREIGN KEY ([id]) REFERENCES [dbo].[other] ([id])\n\
             );\n\nCREATE NONCLUSTERED INDEX [IX_things_label] ON [dbo].[things] ([label] DESC, [id]);"
        );
    }

    #[test]
    fn a_named_instance_without_its_port_is_explained() {
        let config = crate::config::ConnectionConfig {
            name: "x".into(),
            engine: Engine::Mssql,
            host: "lab".into(),
            user: "sa".into(),
            instance: "SQLEXPRESS".into(),
            ..Default::default()
        };
        let err = crate::runtime::block_on(open(&ConnectSpec::new(config, None))).err().unwrap();
        assert!(matches!(&err, DbError::Config(m) if m.contains("SQLEXPRESS") && m.contains("port")), "{err:?}");
    }

    #[test]
    fn a_dead_connection_is_told_from_a_failed_statement() {
        // A server's own error (TokenError has no public constructor) is checked in the integration tests.
        assert!(fatal(&TdsError::Utf8));
        assert!(fatal(&TdsError::Protocol("x".into())));
        assert!(!fatal(&TdsError::Conversion("x".into())));
    }
}
