//! Yuzora L3（資料庫）: Databases: connections, schema trees, data grid, SQL editor (PostgreSQL, MSSQL, SQLite).
//!
//! Owned by lane L3. Scripts reach it as `import { … } from "yuzora:db"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].
//!
//! - [`config`]: saved connections (settings `db` section + credential store).
//! - [`engine`]: PostgreSQL, SQL Server and SQLite behind one face.
//! - [`session`]: live connections per owner, queries as jobs, write-back.
//! - [`results`]: result sets the native grid reads directly.
//! - [`grid`], [`editor`]: the `DataGrid` and `SqlEditor` native elements.

pub mod config;
pub mod editor;
pub mod engine;
pub mod error;
pub mod grid;
pub mod history;
pub mod i18n;
pub mod model;
pub mod results;
pub mod runtime;
pub mod session;
pub mod sql;

use std::future::Future;

use serde::Serialize;
use serde_json::{Value, json};
use yuzora_core::host::{arg, error as host_error, from_host, to_host};
use yuzora_core::{HostArguments, HostError, HostModule, HostValue};

use crate::config::ConnectionConfig;
use crate::error::DbError;
use crate::model::ObjectRef;

/// The import specifier: `yuzora:db`.
pub const NAME: &str = "yuzora:db";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
import { NativeElement, HostValue } from "gpui-kit";

export type Engine = "postgres" | "mssql" | "sqlite";
/** `require` encrypts without checking the certificate; `verify-*` check it. */
export type SslMode = "disable" | "prefer" | "require" | "verify-ca" | "verify-full";
export interface Connection {
  id: string; name: string; engine: Engine;
  host: string; port: number | null; user: string; database: string;
  ssl: SslMode; caFile: string; instance: string;
  /** SQLite file (`~` is home) and read-only. */
  path: string; readOnly: boolean;
  savePassword: boolean;
  /** Through an SSH host (L4). Saved, refused until the SSH lane provides tunnels. */
  ssh: { host: string } | null;
}
export interface ConnectionStatus {
  state: "idle" | "connecting" | "connected" | "error";
  /** `PostgreSQL 16.4`, once connected. */
  version: string;
  error: string | null;
}
export type ObjectKind = "table" | "view" | "materialized_view" | "function" | "procedure" | "sequence" | "trigger" | "index" | "type" | "other";
export interface DbObject { schema: string; name: string; kind: ObjectKind; rows: number | null; detail: string }
export interface ObjectRef { schema: string; name: string }
export interface ColumnInfo { name: string; typeName: string; nullable: boolean; default: string | null; primaryKey: number | null; generated: boolean }
export interface TableInfo {
  columns: ColumnInfo[];
  indexes: { name: string; columns: string[]; unique: boolean; primary: boolean }[];
  foreignKeys: { name: string; columns: string[]; referenced: ObjectRef; referencedColumns: string[] }[];
  ddl: string;
}
export interface ResultSummary {
  id: number; statement: string; columns: number; rows: number;
  /** `count(*)` of a table's rows, when known. */
  total: number | null;
  complete: boolean;
  /** Stopped at the row cap (`db.maxRows`, default 100000). */
  truncated: boolean;
  affected: number | null; error: string | null; elapsedMs: number;
  editable: boolean; notEditable: string | null; edits: number;
  /** The edits are being written back right now. */
  writing: boolean;
}
export interface JobState {
  id: number; running: boolean; version: number; elapsedMs: number;
  error: string | null; cancelled: boolean; results: ResultSummary[];
}
export interface HistoryEntry { sql: string; at: number; elapsedMs: number; ok: boolean }

/** The yuzora-db crate's version (Yuzora's). */
export function version(): string;
/** The engines, with their labels and default ports. */
export function engines(): { id: Engine; label: string; defaultPort: number | null }[];
/** Saved connections with what the sidebar shows (`target`) and their live status. */
export function connections(): (Connection & { target: string; label: string; status: ConnectionStatus })[];
/** Adds (no id) or replaces a connection. `password`: set it, or leave the stored one when null. */
export function saveConnection(config: Partial<Connection>, password?: string | null): Promise<Connection>;
export function deleteConnection(id: string): Promise<null>;
/** Connects once with these settings (the stored password when none is given). */
export function testConnection(config: Partial<Connection>, password?: string | null): Promise<{ version: string; elapsedMs: number }>;
/** Opens the sidebar's connection; resolves with its status. */
export function connect(id: string, database?: string | null): Promise<ConnectionStatus>;
/** Closes every connection to it. */
export function disconnect(id: string): null;
export function databases(id: string): Promise<string[]>;
/** Tables, views and other objects of a database (`null`: the connection's own). */
export function objects(id: string, database: string | null): Promise<DbObject[]>;
/** Columns, indexes, foreign keys and DDL of one object. */
export function describe(id: string, database: string | null, object: ObjectRef): Promise<TableInfo>;
/** The description of the table that job `job` (from `openTable`) is opening. It is read once, beside the rows,
 * for that open and no other: ask for it with the job instead of with `describe`. Rejects when the job is gone. */
export function tableInfo(job: number): Promise<TableInfo>;
/** Starts `sql` (one statement or a script) on `owner`'s connection; returns the job id. */
export function query(id: string, database: string | null, owner: string, sql: string): number;
/** Starts reading a table's rows (editable when it has a primary key); returns the job id. */
export function openTable(id: string, database: string | null, owner: string, table: ObjectRef, order?: { column: string; desc: boolean } | null): number;
/** A job's state once it changed past `seen` (at once when it already has). */
export function job(id: number, seen?: number): Promise<JobState>;
export function cancel(job: number): null;
/** Stops the write-back of a result that is being written (the edits stay pending); no-op otherwise. */
export function stopWrite(result: number): null;
/** A result's summary (null when released). */
export function result(id: number): ResultSummary | null;
/** Closes `owner`'s connections and drops its results (a pane closing). */
export function release(owner: string): null;
export function history(id: string): HistoryEntry[];
export function clearHistory(id: string): null;
/** The SQL editor `id`: its whole text, the selection, and the statement under the cursor
 * (`null` when where it ends depends on the session: PostgreSQL's standard_conforming_strings). */
export function editorText(id: string): { text: string; selection: string; statement: string | null } | null;
export function setEditorText(id: string, text: string): null;
/** Waits for the editor's next action: `"run"` (⌘↵ / Ctrl+Enter). */
export function editorAction(id: string): Promise<string>;
/** A result's rows in the native grid (kept until `release(owner)` for ids `"<owner>:…"`). props: `{ result: number | null, colors?: Record<string, string> }` (theme hex colors: foreground, muted, dim, divider, header, hover, selected, accent, editedRow, warning, error, button, buttonText, buttonBorder, background). */
export const DataGrid: { new(id: string, props: HostValue): NativeElement };
/** A SQL editor with highlighting. props: `{ engine?: Engine, value?: string, readonly?: boolean }`; `value` is its text when first made (then `setEditorText`). Kept until `release(owner)` for ids `"<owner>:…"`. */
export const SqlEditor: { new(id: string, props: HostValue): NativeElement };
"#;

fn json<T: Serialize>(value: &T) -> HostValue {
    to_host(&serde_json::to_value(value).unwrap_or(Value::Null))
}

fn db_error(e: DbError) -> HostError {
    host_error(e)
}

/// Runs `work` on the crate's runtime and hands the script its JSON.
fn spawn<T, F>(work: F) -> impl Future<Output = Result<HostValue, HostError>> + Send + 'static
where
    T: Serialize + Send + 'static,
    F: Future<Output = Result<T, DbError>> + Send + 'static,
{
    let done = runtime::run(work);
    async move { done.await.map(|v| json(&v)).map_err(db_error) }
}

fn string(a: &HostArguments, i: usize) -> Result<String, HostError> {
    Ok(a.string(i)?.to_owned())
}

/// Argument `i` as an optional string (`null`, missing and `""` are none).
fn opt_string(a: &HostArguments, i: usize) -> Option<String> {
    a.get(i).and_then(HostValue::as_str).map(str::to_owned).filter(|s| !s.is_empty())
}

fn object_ref(a: &HostArguments, i: usize) -> Result<ObjectRef, HostError> {
    serde_json::from_value(arg(a, i)).map_err(|e| host_error(format!("argument {} is not {{schema, name}}: {e}", i + 1)))
}

fn connection_config(a: &HostArguments, i: usize) -> Result<ConnectionConfig, HostError> {
    let mut v = arg(a, i);
    // JS may send `port: ""` from an empty field.
    if v.get("port").is_some_and(|p| p.as_str() == Some("")) {
        v["port"] = Value::Null;
    }
    serde_json::from_value(v).map_err(|e| host_error(format!("not a connection: {e}")))
}

fn connections_json() -> HostValue {
    let list: Vec<Value> = config::load()
        .into_iter()
        .map(|c| {
            let mut v = serde_json::to_value(&c).unwrap_or(Value::Null);
            v["target"] = json!(c.target());
            v["label"] = json!(c.engine.label());
            v["status"] = serde_json::to_value(session::status(&c.id)).unwrap_or(Value::Null);
            v
        })
        .collect();
    to_host(&Value::Array(list))
}

/// The `yuzora:db` module.
pub fn host_module() -> HostModule {
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(HostValue::Str(yuzora_core::VERSION.into())))
        .function("engines", |_| {
            let list: Vec<Value> = [config::Engine::Postgres, config::Engine::Mssql, config::Engine::Sqlite]
                .into_iter()
                .map(|e| json!({ "id": e.id(), "label": e.label(), "defaultPort": e.default_port() }))
                .collect();
            Ok(to_host(&Value::Array(list)))
        })
        .function("connections", |_| Ok(connections_json()))
        .async_function("saveConnection", |a| {
            let config = connection_config(a, 0)?;
            let password = a.get(1).and_then(HostValue::as_str).map(str::to_owned);
            Ok(spawn(async move {
                let before = config::get(&config.id).ok();
                let saved = config::save(config, password.as_deref())?;
                // Connecting differently: reconnect on next use. A rename
                // leaves open connections (and their running work) alone.
                if password.is_some() || before.is_none_or(|b| !b.connects_like(&saved)) {
                    session::disconnect(&saved.id);
                }
                Ok(saved)
            }))
        })
        .async_function("deleteConnection", |a| {
            let id = string(a, 0)?;
            Ok(spawn(async move {
                session::disconnect(&id);
                crate::history::clear(&id);
                config::delete(&id).map(|_| Value::Null)
            }))
        })
        .async_function("testConnection", |a| {
            let config = connection_config(a, 0)?;
            let password = a.get(1).and_then(HostValue::as_str).map(str::to_owned);
            Ok(spawn(async move {
                let (version, elapsed) = session::test(config, password).await?;
                Ok(json!({ "version": version, "elapsedMs": elapsed }))
            }))
        })
        .async_function("connect", |a| {
            let id = string(a, 0)?;
            let database = opt_string(a, 1);
            Ok(spawn(async move {
                session::lease(&id, database.as_deref(), session::META).await?;
                Ok(session::status(&id))
            }))
        })
        .function("disconnect", |a| {
            session::disconnect(a.string(0)?);
            Ok(HostValue::Null)
        })
        .async_function("databases", |a| {
            let id = string(a, 0)?;
            Ok(spawn(async move { session::databases(&id).await }))
        })
        .async_function("objects", |a| {
            let id = string(a, 0)?;
            let database = opt_string(a, 1);
            Ok(spawn(async move { session::objects(&id, database.as_deref()).await }))
        })
        .async_function("describe", |a| {
            let id = string(a, 0)?;
            let database = opt_string(a, 1);
            let object = object_ref(a, 2)?;
            Ok(spawn(async move { session::describe(&id, database.as_deref(), &object).await }))
        })
        .async_function("tableInfo", |a| {
            let job = a.integer(0)? as u64;
            Ok(spawn(async move { session::table_info(job).await }))
        })
        .function("query", |a| {
            let job = session::start_query(a.string(0)?, opt_string(a, 1).as_deref(), a.string(2)?, a.string(3)?);
            Ok(HostValue::Number(job as f64))
        })
        .function("openTable", |a| {
            let table = object_ref(a, 3)?;
            let order = match arg(a, 4) {
                Value::Object(o) => o
                    .get("column")
                    .and_then(Value::as_str)
                    .map(|c| (c.to_owned(), o.get("desc").and_then(Value::as_bool).unwrap_or(false))),
                _ => None,
            };
            let job = session::open_table(a.string(0)?, opt_string(a, 1).as_deref(), a.string(2)?, &table, order);
            Ok(HostValue::Number(job as f64))
        })
        .async_function("job", |a| {
            let id = a.integer(0)? as u64;
            let seen = a.get(1).and_then(HostValue::as_number).unwrap_or(0.0) as u64;
            Ok(spawn(async move { session::job_after(id, seen).await }))
        })
        .function("cancel", |a| {
            session::cancel(a.integer(0)? as u64);
            Ok(HostValue::Null)
        })
        .function("stopWrite", |a| {
            session::stop_write(a.integer(0)? as u64);
            Ok(HostValue::Null)
        })
        .function("result", |a| {
            let id = a.integer(0)? as u64;
            Ok(results::get(id).map_or(HostValue::Null, |set| json(&results::lock(&set).summary())))
        })
        .function("release", |a| {
            let owner = a.string(0)?;
            session::release_owner(owner);
            grid::release(&format!("{owner}:"));
            editor::release(&format!("{owner}:"));
            Ok(HostValue::Null)
        })
        .function("history", |a| Ok(json(&history::list(a.string(0)?))))
        .function("clearHistory", |a| {
            history::clear(a.string(0)?);
            Ok(HostValue::Null)
        })
        .function("editorText", |a| Ok(editor::text(a.string(0)?).map_or(HostValue::Null, |t| json(&t))))
        .function("setEditorText", |a| {
            editor::set_text(a.string(0)?, a.string(1)?);
            Ok(HostValue::Null)
        })
        .async_function("editorAction", |a| {
            let actions = editor::actions(a.string(0)?);
            Ok(async move {
                match actions.recv().await {
                    Ok(action) => Ok(HostValue::Str(action)),
                    // The editor went away: never resolve, as a dropped call would.
                    Err(_) => std::future::pending().await,
                }
            })
        })
        .component("DataGrid", |args, window, cx| grid::element(args.id(), &from_host(args.props()), window, cx))
        .component("SqlEditor", |args, window, cx| {
            let props = from_host(args.props());
            editor::element(args.id(), &props, window, cx)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_validates_and_reports_its_version() {
        let m = host_module();
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:db");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(yuzora_core::VERSION.into()));
    }

    #[test]
    fn engines_list_their_default_ports() {
        let m = host_module();
        let v = from_host(&m.call("engines", &HostArguments::new([])).unwrap());
        assert_eq!(v[0], json!({ "id": "postgres", "label": "PostgreSQL", "defaultPort": 5432 }));
        assert_eq!(v[2]["defaultPort"], Value::Null);
    }

    #[test]
    fn a_missing_result_reads_as_null() {
        let m = host_module();
        assert_eq!(m.call("result", &HostArguments::new([HostValue::Number(9e9)])).unwrap(), HostValue::Null);
    }
}
