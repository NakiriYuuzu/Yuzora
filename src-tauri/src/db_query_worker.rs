//! Bounded same-binary helper for network result decoding.
//!
//! PostgreSQL and MSSQL drivers allocate a complete protocol row/frame before
//! application conversion. That allocation is confined to this disposable
//! child. The parent only reads length-prefixed frames that already respect
//! the field/row ceilings.
//!
//! Residual: Unix helpers use a sampled resident-set watchdog; non-macOS Unix
//! also keeps a larger `RLIMIT_AS` fail-safe so normal driver virtual mappings
//! do not consume the actual resident-memory budget. Windows uses a job-object
//! process limit. If a platform guard cannot be installed, the helper fails
//! closed before driver or network decode. The watchdog has a short sampling
//! window rather than a kernel-enforced resident pre-allocation ceiling.

use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex as AsyncMutex;
use zeroize::Zeroize;

use crate::db_result_session::{DEFAULT_PROCESS_BYTES, DEFAULT_ROW_BYTES};
use crate::db_service::{
    ColumnInfo, DatabaseError, DatabaseErrorEngine, DatabaseObjectKind, DbValue,
    PostgresInsecureException, PostgresTransportMode, Retryability, TableInfo,
};

pub const WORKER_ENV: &str = "YUZORA_DB_QUERY_WORKER";
pub const WORKER_BIN_ENV: &str = "YUZORA_DB_QUERY_WORKER_BIN";
pub const MAX_HELPER_FRAME_BYTES: usize = DEFAULT_ROW_BYTES
    .saturating_mul(3)
    .saturating_add(64 * 1024);
/// OS ceiling for the disposable helper. This is the result-process budget plus
/// headroom for the binary, TLS, and driver bookkeeping. Hostile multi-gigabyte
/// values still trip the helper; ordinary connect/query stays inside it.
pub const HELPER_MEMORY_BYTES: u64 =
    (DEFAULT_PROCESS_BYTES as u64).saturating_add(256 * 1024 * 1024);
const HELPER_IO_TIMEOUT: Duration = Duration::from_secs(120);
const HELPER_REAP_TIMEOUT: Duration = Duration::from_secs(2);
// Tiberius/TLS may reserve more than 1 GiB of virtual address space while
// resident memory stays small; keep RLIMIT_AS well above that normal mapping.
#[cfg(all(unix, not(target_os = "macos")))]
const UNIX_ADDRESS_SPACE_HEADROOM_MULTIPLIER: u64 = 8;

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WorkerRequest {
    ConnectPostgres {
        host: String,
        port: u16,
        database: String,
        user: String,
        password: String,
        transport_mode: PostgresTransportMode,
        insecure_exception: Option<PostgresInsecureException>,
        trust_server_cert_acknowledged: bool,
    },
    ConnectMssql {
        host: String,
        port: u16,
        database: String,
        user: String,
        password: String,
        trust_cert: bool,
    },
    Probe,
    ListTables,
    TableColumns {
        catalog: String,
        schema: String,
        name: String,
        object_kind: DatabaseObjectKind,
    },
    Query {
        sql: String,
    },
    StopStreaming,
    CancelQuery,
    Close,
}

impl WorkerRequest {
    fn zeroize_secrets(&mut self) {
        match self {
            Self::ConnectPostgres { password, .. } | Self::ConnectMssql { password, .. } => {
                password.zeroize();
            }
            Self::Probe
            | Self::ListTables
            | Self::TableColumns { .. }
            | Self::Query { .. }
            | Self::StopStreaming
            | Self::CancelQuery
            | Self::Close => {}
        }
    }
}

impl fmt::Debug for WorkerRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectPostgres {
                host,
                port,
                database,
                user,
                transport_mode,
                insecure_exception,
                trust_server_cert_acknowledged,
                ..
            } => formatter
                .debug_struct("ConnectPostgres")
                .field("host", host)
                .field("port", port)
                .field("database", database)
                .field("user", user)
                .field("password", &"<redacted>")
                .field("transport_mode", transport_mode)
                .field("insecure_exception", insecure_exception)
                .field(
                    "trust_server_cert_acknowledged",
                    trust_server_cert_acknowledged,
                )
                .finish(),
            Self::ConnectMssql {
                host,
                port,
                database,
                user,
                trust_cert,
                ..
            } => formatter
                .debug_struct("ConnectMssql")
                .field("host", host)
                .field("port", port)
                .field("database", database)
                .field("user", user)
                .field("password", &"<redacted>")
                .field("trust_cert", trust_cert)
                .finish(),
            Self::Probe => formatter.write_str("Probe"),
            Self::ListTables => formatter.write_str("ListTables"),
            Self::TableColumns {
                catalog,
                schema,
                name,
                object_kind,
            } => formatter
                .debug_struct("TableColumns")
                .field("catalog", catalog)
                .field("schema", schema)
                .field("name", name)
                .field("object_kind", object_kind)
                .finish(),
            Self::Query { sql } => formatter.debug_struct("Query").field("sql", sql).finish(),
            Self::StopStreaming => formatter.write_str("StopStreaming"),
            Self::CancelQuery => formatter.write_str("CancelQuery"),
            Self::Close => formatter.write_str("Close"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WorkerResponse {
    Ready { engine: String },
    Version { value: Option<String> },
    Tables { tables: Vec<TableInfo> },
    Columns { columns: Vec<ColumnInfo> },
    Execute { affected_rows: Option<String> },
    RowMeta { columns: Vec<String> },
    Row { values: Vec<DbValue> },
    End { affected_rows: Option<String> },
    ValueTooLarge,
    Cancelled,
    Error { error: DatabaseError },
    Closed,
}

pub fn helper_program() -> PathBuf {
    if let Ok(path) = std::env::var(WORKER_BIN_ENV) {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(name) = exe.file_name().and_then(|name| name.to_str()) {
            if name == "yuzora" || name == "yuzora.exe" {
                return exe;
            }
        }
        if let Some(parent) = exe.parent() {
            for name in ["yuzora", "yuzora.exe"] {
                let candidate = parent.join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
            if parent.file_name().and_then(|name| name.to_str()) == Some("deps") {
                if let Some(target_dir) = parent.parent() {
                    for name in ["yuzora", "yuzora.exe"] {
                        let candidate = target_dir.join(name);
                        if candidate.is_file() {
                            return candidate;
                        }
                    }
                }
            }
        }
        return exe;
    }
    PathBuf::from("yuzora")
}

pub fn apply_process_memory_limit(bytes: u64) -> Result<(), String> {
    #[cfg(unix)]
    {
        #[cfg(not(target_os = "macos"))]
        install_unix_address_space_limit(unix_address_space_limit(bytes))?;
        start_resident_memory_watchdog(bytes)
    }
    #[cfg(windows)]
    {
        windows_job_memory_limit(bytes)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = bytes;
        Err("no platform memory ceiling is available".to_string())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn unix_address_space_limit(bytes: u64) -> u64 {
    bytes.saturating_mul(UNIX_ADDRESS_SPACE_HEADROOM_MULTIPLIER)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn install_unix_address_space_limit(bytes: u64) -> Result<(), String> {
    let limit = libc::rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) };
    if rc != 0 {
        return Err(format!(
            "setrlimit(RLIMIT_AS) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn resident_memory_limit_exceeded(observed_bytes: u64, limit_bytes: u64) -> bool {
    observed_bytes > limit_bytes
}

#[cfg(unix)]
fn start_resident_memory_watchdog(bytes: u64) -> Result<(), String> {
    use sysinfo::{get_current_pid, ProcessRefreshKind, ProcessesToUpdate, System};

    let pid = get_current_pid().map_err(|error| format!("resolve worker pid failed: {error}"))?;
    std::thread::Builder::new()
        .name("yuzora-db-memory-watchdog".to_string())
        .spawn(move || {
            let mut system = System::new();
            loop {
                let pids = [pid];
                system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&pids),
                    true,
                    ProcessRefreshKind::nothing().with_memory(),
                );
                let Some(process) = system.process(pid) else {
                    return;
                };
                if resident_memory_limit_exceeded(process.memory(), bytes) {
                    std::process::abort();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
        .map(|_| ())
        .map_err(|error| format!("start resident-memory watchdog failed: {error}"))
}

#[cfg(windows)]
fn windows_job_memory_limit(bytes: u64) -> Result<(), String> {
    use std::mem::{size_of, zeroed};
    use std::ptr::null_mut;

    #[repr(C)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }
    #[repr(C)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x0000_0100;

    extern "system" {
        fn CreateJobObjectW(
            attributes: *mut core::ffi::c_void,
            name: *const u16,
        ) -> *mut core::ffi::c_void;
        fn SetInformationJobObject(
            job: *mut core::ffi::c_void,
            class: i32,
            info: *mut core::ffi::c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(
            job: *mut core::ffi::c_void,
            process: *mut core::ffi::c_void,
        ) -> i32;
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
    }

    unsafe {
        let job = CreateJobObjectW(null_mut(), null_mut());
        if job.is_null() {
            return Err("CreateJobObjectW failed".to_string());
        }
        let mut info: JobObjectExtendedLimitInformation = zeroed();
        info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.process_memory_limit = usize::try_from(bytes).unwrap_or(usize::MAX);
        if SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            (&mut info as *mut JobObjectExtendedLimitInformation).cast(),
            size_of::<JobObjectExtendedLimitInformation>() as u32,
        ) == 0
        {
            CloseHandle(job);
            return Err("SetInformationJobObject failed".to_string());
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            CloseHandle(job);
            return Err("AssignProcessToJobObject failed".to_string());
        }
        std::mem::forget(job);
        Ok(())
    }
}

pub fn worker_error(code: &str, message: &str) -> DatabaseError {
    DatabaseError {
        engine: DatabaseErrorEngine::Yuzora,
        message: message.to_string(),
        code: Some(code.to_string()),
        position: None,
        detail: None,
        hint: None,
        retryability: Retryability::NotRetryable,
    }
}

pub fn value_too_large_error() -> DatabaseError {
    worker_error(
        "valueTooLarge",
        "a database result value or helper frame exceeded the hard ceiling",
    )
}

pub fn cancelled_error() -> DatabaseError {
    worker_error("cancelled", "database query was cancelled")
}

pub fn helper_timeout_error() -> DatabaseError {
    worker_error(
        "helperTimeout",
        "database helper did not respond before the I/O deadline",
    )
}

async fn write_len_prefixed<W>(writer: &mut W, mut body: Vec<u8>) -> Result<(), DatabaseError>
where
    W: AsyncWrite + Unpin,
{
    if body.len() > MAX_HELPER_FRAME_BYTES {
        body.zeroize();
        return Err(value_too_large_error());
    }
    let len = u32::try_from(body.len()).map_err(|_| {
        body.zeroize();
        value_too_large_error()
    })?;
    let result = async {
        writer
            .write_all(&len.to_be_bytes())
            .await
            .map_err(|error| worker_error("helperIo", &error.to_string()))?;
        writer
            .write_all(&body)
            .await
            .map_err(|error| worker_error("helperIo", &error.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|error| worker_error("helperIo", &error.to_string()))
    }
    .await;
    body.zeroize();
    result
}

pub async fn write_frame<W>(writer: &mut W, response: &WorkerResponse) -> Result<(), DatabaseError>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(response).map_err(|error| {
        worker_error(
            "helperProtocol",
            &format!("failed to encode helper frame: {error}"),
        )
    })?;
    write_len_prefixed(writer, body).await
}

pub async fn write_request<W>(
    writer: &mut W,
    request: &mut WorkerRequest,
) -> Result<(), DatabaseError>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(request).map_err(|error| {
        request.zeroize_secrets();
        worker_error(
            "helperProtocol",
            &format!("failed to encode helper request: {error}"),
        )
    })?;
    request.zeroize_secrets();
    write_len_prefixed(writer, body).await
}

pub async fn read_frame<R>(reader: &mut R) -> Result<WorkerResponse, DatabaseError>
where
    R: AsyncRead + Unpin,
{
    let body = read_len_prefixed(reader).await?;
    let decoded = serde_json::from_slice(&body)
        .map_err(|error| worker_error("helperProtocol", &format!("invalid helper frame: {error}")));
    decoded
}

pub async fn read_request<R>(reader: &mut R) -> Result<WorkerRequest, DatabaseError>
where
    R: AsyncRead + Unpin,
{
    let mut body = read_len_prefixed(reader).await?;
    let decoded = serde_json::from_slice(&body).map_err(|error| {
        worker_error(
            "helperProtocol",
            &format!("invalid helper request: {error}"),
        )
    });
    body.zeroize();
    decoded
}

async fn read_len_prefixed<R>(reader: &mut R) -> Result<Vec<u8>, DatabaseError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; 4];
    reader.read_exact(&mut header).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            value_too_large_error()
        } else {
            worker_error("helperIo", &error.to_string())
        }
    })?;
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 || len > MAX_HELPER_FRAME_BYTES {
        return Err(value_too_large_error());
    }
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|error| worker_error("helperIo", &error.to_string()))?;
    Ok(body)
}

pub struct NetworkQueryWorker {
    child: Arc<AsyncMutex<Option<Child>>>,
    stdin: Arc<AsyncMutex<Option<ChildStdin>>>,
    stdout: AsyncMutex<Option<ChildStdout>>,
    closed: Arc<AtomicBool>,
    frame_timeout: Duration,
}

#[derive(Clone)]
pub struct NetworkCancelHandle {
    child: Arc<AsyncMutex<Option<Child>>>,
    stdin: Arc<AsyncMutex<Option<ChildStdin>>>,
    closed: Arc<AtomicBool>,
}

pub enum NetworkQueryStart {
    Execute { affected_rows: Option<String> },
    Rows { columns: Vec<String> },
}

pub enum NetworkRow {
    Value(Vec<DbValue>),
    End { affected_rows: Option<String> },
    ValueTooLarge,
    Cancelled,
}

impl NetworkQueryWorker {
    pub fn stub() -> Self {
        Self {
            child: Arc::new(AsyncMutex::new(None)),
            stdin: Arc::new(AsyncMutex::new(None)),
            stdout: AsyncMutex::new(None),
            closed: Arc::new(AtomicBool::new(true)),
            frame_timeout: HELPER_IO_TIMEOUT,
        }
    }

    pub fn from_child(mut child: Child) -> Result<Self, DatabaseError> {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| worker_error("helperSpawn", "helper stdin is missing"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| worker_error("helperSpawn", "helper stdout is missing"))?;
        Ok(Self {
            child: Arc::new(AsyncMutex::new(Some(child))),
            stdin: Arc::new(AsyncMutex::new(Some(stdin))),
            stdout: AsyncMutex::new(Some(stdout)),
            closed: Arc::new(AtomicBool::new(false)),
            frame_timeout: HELPER_IO_TIMEOUT,
        })
    }

    #[cfg(test)]
    fn with_frame_timeout(mut self, timeout: Duration) -> Self {
        self.frame_timeout = timeout;
        self
    }

    pub async fn spawn_postgres(
        host: String,
        port: u16,
        database: String,
        user: String,
        password: SecretString,
        transport_mode: PostgresTransportMode,
        insecure_exception: Option<PostgresInsecureException>,
        trust_server_cert_acknowledged: bool,
    ) -> Result<Self, DatabaseError> {
        let worker = spawn_helper_process()?;
        let mut password = password.expose_secret().to_string();
        let request = WorkerRequest::ConnectPostgres {
            host,
            port,
            database,
            user,
            password: password.clone(),
            transport_mode,
            insecure_exception,
            trust_server_cert_acknowledged,
        };
        password.zeroize();
        match worker.handshake(request, "postgres").await {
            Ok(()) => Ok(worker),
            Err(error) => {
                worker.reap().await;
                Err(error)
            }
        }
    }

    pub async fn spawn_mssql(
        host: String,
        port: u16,
        database: String,
        user: String,
        password: SecretString,
        trust_cert: bool,
    ) -> Result<Self, DatabaseError> {
        let worker = spawn_helper_process()?;
        let mut password = password.expose_secret().to_string();
        let request = WorkerRequest::ConnectMssql {
            host,
            port,
            database,
            user,
            password: password.clone(),
            trust_cert,
        };
        password.zeroize();
        match worker.handshake(request, "mssql").await {
            Ok(()) => Ok(worker),
            Err(error) => {
                worker.reap().await;
                Err(error)
            }
        }
    }

    pub fn cancel_handle(&self) -> NetworkCancelHandle {
        NetworkCancelHandle {
            child: self.child.clone(),
            stdin: self.stdin.clone(),
            closed: self.closed.clone(),
        }
    }

    pub fn abort(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut stdin) = self.stdin.try_lock() {
            stdin.take();
        }
        if let Ok(mut child) = self.child.try_lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
            }
        }
    }

    pub async fn reap(&self) {
        self.closed.store(true, Ordering::SeqCst);
        {
            let mut stdin = self.stdin.lock().await;
            stdin.take();
        }
        {
            let mut stdout = self.stdout.lock().await;
            stdout.take();
        }
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(HELPER_REAP_TIMEOUT, child.wait()).await;
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub async fn probe_version(&self) -> Result<Option<String>, DatabaseError> {
        match self.roundtrip(WorkerRequest::Probe).await? {
            WorkerResponse::Version { value } => Ok(value),
            WorkerResponse::ValueTooLarge => Err(value_too_large_error()),
            WorkerResponse::Cancelled => Err(cancelled_error()),
            WorkerResponse::Error { error } => Err(error),
            _ => Err(worker_error("helperProtocol", "unexpected probe response")),
        }
    }

    pub async fn list_tables(&self) -> Result<Vec<TableInfo>, DatabaseError> {
        match self.roundtrip(WorkerRequest::ListTables).await? {
            WorkerResponse::Tables { tables } => Ok(tables),
            WorkerResponse::ValueTooLarge => Err(value_too_large_error()),
            WorkerResponse::Cancelled => Err(cancelled_error()),
            WorkerResponse::Error { error } => Err(error),
            _ => Err(worker_error("helperProtocol", "unexpected tables response")),
        }
    }

    pub async fn table_columns(&self, table: &TableInfo) -> Result<Vec<ColumnInfo>, DatabaseError> {
        match self
            .roundtrip(WorkerRequest::TableColumns {
                catalog: table.catalog.clone(),
                schema: table.schema.clone(),
                name: table.name.clone(),
                object_kind: table.kind,
            })
            .await?
        {
            WorkerResponse::Columns { columns } => Ok(columns),
            WorkerResponse::ValueTooLarge => Err(value_too_large_error()),
            WorkerResponse::Cancelled => Err(cancelled_error()),
            WorkerResponse::Error { error } => Err(error),
            _ => Err(worker_error(
                "helperProtocol",
                "unexpected columns response",
            )),
        }
    }

    pub async fn start_query(&self, sql: &str) -> Result<NetworkQueryStart, DatabaseError> {
        match self
            .roundtrip(WorkerRequest::Query {
                sql: sql.to_string(),
            })
            .await?
        {
            WorkerResponse::Execute { affected_rows } => {
                Ok(NetworkQueryStart::Execute { affected_rows })
            }
            WorkerResponse::RowMeta { columns } => Ok(NetworkQueryStart::Rows { columns }),
            WorkerResponse::ValueTooLarge => Err(value_too_large_error()),
            WorkerResponse::Cancelled => Err(cancelled_error()),
            WorkerResponse::Error { error } => Err(error),
            other => {
                let message = format!("unexpected query response: {other:?}");
                Err(worker_error("helperProtocol", &message))
            }
        }
    }

    pub async fn next_row(&self) -> Result<NetworkRow, DatabaseError> {
        match self.read_response().await? {
            WorkerResponse::Row { values } => Ok(NetworkRow::Value(values)),
            WorkerResponse::End { affected_rows } => Ok(NetworkRow::End { affected_rows }),
            WorkerResponse::ValueTooLarge => Ok(NetworkRow::ValueTooLarge),
            WorkerResponse::Cancelled => Ok(NetworkRow::Cancelled),
            WorkerResponse::Error { error } => Err(error),
            _ => Err(worker_error("helperProtocol", "unexpected row response")),
        }
    }

    pub async fn stop_streaming(&self) -> Result<(), DatabaseError> {
        self.send(WorkerRequest::StopStreaming).await
    }

    pub async fn cancel_query(&self) -> Result<(), DatabaseError> {
        self.send(WorkerRequest::CancelQuery).await
    }

    pub async fn close(&self) -> Result<(), DatabaseError> {
        let _ = self.send(WorkerRequest::Close).await;
        self.reap().await;
        Ok(())
    }

    async fn handshake(
        &self,
        request: WorkerRequest,
        expected_engine: &str,
    ) -> Result<(), DatabaseError> {
        match self.roundtrip(request).await {
            Ok(WorkerResponse::Ready { engine }) if engine == expected_engine => Ok(()),
            Ok(WorkerResponse::Error { error }) => Err(error),
            Ok(WorkerResponse::ValueTooLarge) => Err(value_too_large_error()),
            Ok(WorkerResponse::Cancelled) => Err(cancelled_error()),
            Ok(_) => Err(worker_error(
                "helperProtocol",
                "helper did not acknowledge the connection",
            )),
            Err(error) => Err(error),
        }
    }

    async fn roundtrip(&self, request: WorkerRequest) -> Result<WorkerResponse, DatabaseError> {
        self.send(request).await?;
        self.read_response().await
    }

    async fn send(&self, mut request: WorkerRequest) -> Result<(), DatabaseError> {
        if self.closed.load(Ordering::SeqCst) {
            request.zeroize_secrets();
            return Err(value_too_large_error());
        }
        let result = {
            let mut stdin = self.stdin.lock().await;
            let Some(stdin) = stdin.as_mut() else {
                request.zeroize_secrets();
                return Err(value_too_large_error());
            };
            write_request(stdin, &mut request).await
        };
        if result.is_err() {
            self.reap().await;
        }
        result
    }

    async fn read_response(&self) -> Result<WorkerResponse, DatabaseError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(value_too_large_error());
        }
        let read = async {
            let mut stdout = self.stdout.lock().await;
            let Some(stdout) = stdout.as_mut() else {
                return Err(value_too_large_error());
            };
            read_frame(stdout).await
        };
        match tokio::time::timeout(self.frame_timeout, read).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => {
                self.reap().await;
                Err(error)
            }
            Err(_) => {
                self.reap().await;
                Err(helper_timeout_error())
            }
        }
    }
}

impl NetworkCancelHandle {
    pub async fn cancel_query(&self) -> Result<(), DatabaseError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(value_too_large_error());
        }
        let write = {
            let mut stdin = self.stdin.lock().await;
            let Some(stdin) = stdin.as_mut() else {
                return Err(value_too_large_error());
            };
            write_request(stdin, &mut WorkerRequest::CancelQuery).await
        };
        write
    }

    pub fn abort(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut stdin) = self.stdin.try_lock() {
            stdin.take();
        }
        if let Ok(mut child) = self.child.try_lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
            }
        }
    }
}

impl Drop for NetworkQueryWorker {
    fn drop(&mut self) {
        self.abort();
    }
}

fn helper_command() -> Result<Command, DatabaseError> {
    let program = helper_program();
    if !program.is_file() {
        return Err(worker_error(
            "helperSpawn",
            "database query helper binary is not available",
        ));
    }
    let mut command = Command::new(program);
    command
        .env(WORKER_ENV, "1")
        .env_remove("YUZORA_ASKPASS_ENDPOINT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

fn spawn_helper_process() -> Result<NetworkQueryWorker, DatabaseError> {
    let child = helper_command()?
        .spawn()
        .map_err(|error| worker_error("helperSpawn", &error.to_string()))?;
    NetworkQueryWorker::from_child(child)
}

const HELPER_MEMORY_LIMIT_EXIT: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperStartupDiagnostic {
    code: &'static str,
    message: String,
}

fn memory_limit_diagnostic(error: String) -> HelperStartupDiagnostic {
    HelperStartupDiagnostic {
        code: "helperMemoryLimit",
        message: error,
    }
}

pub fn run() -> i32 {
    run_with_memory_limit(apply_process_memory_limit)
}

fn run_with_memory_limit(install_limit: fn(u64) -> Result<(), String>) -> i32 {
    if let Err(error) = install_limit(HELPER_MEMORY_BYTES) {
        let diagnostic = memory_limit_diagnostic(error);
        eprintln!(
            "{}",
            serde_json::to_string(&diagnostic).unwrap_or_else(|_| {
                r#"{"code":"helperMemoryLimit","message":"memory limit setup failed"}"#.to_string()
            })
        );
        return HELPER_MEMORY_LIMIT_EXIT;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return 2,
    };
    match runtime.block_on(crate::db_service::query_worker_loop()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn connect_request(password: &str) -> WorkerRequest {
        WorkerRequest::ConnectPostgres {
            host: "127.0.0.1".into(),
            port: 5432,
            database: "app".into(),
            user: "alice".into(),
            password: password.into(),
            transport_mode: PostgresTransportMode::VerifyFull,
            insecure_exception: None,
            trust_server_cert_acknowledged: false,
        }
    }

    async fn hanging_child() -> (NetworkQueryWorker, u32) {
        let mut command = Command::new(if cfg!(windows) { "ping" } else { "sleep" });
        if cfg!(windows) {
            command.args(["-n", "30", "127.0.0.1"]);
        } else {
            command.arg("30");
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let child = command.spawn().unwrap();
        let pid = child.id().expect("helper child pid");
        (NetworkQueryWorker::from_child(child).unwrap(), pid)
    }

    fn process_is_alive(pid: u32) -> bool {
        #[cfg(unix)]
        {
            unsafe { libc::kill(pid as i32, 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }

    #[tokio::test]
    async fn declared_oversize_frame_is_rejected_without_reading_the_body() {
        let (mut client, mut server) = duplex(64);
        let too_big = (MAX_HELPER_FRAME_BYTES as u32)
            .saturating_add(1)
            .to_be_bytes();
        tokio::spawn(async move {
            let _ = server.write_all(&too_big).await;
            let _ = server.write_all(&vec![b'x'; 64]).await;
        });
        let error = read_frame(&mut client).await.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("valueTooLarge"));
    }

    #[tokio::test]
    async fn helper_frame_roundtrip_stays_within_the_declared_ceiling() {
        let (mut left, mut right) = duplex(4096);
        let response = WorkerResponse::Row {
            values: vec![DbValue::Integer { value: "1".into() }],
        };
        write_frame(&mut left, &response).await.unwrap();
        let decoded = read_frame(&mut right).await.unwrap();
        match decoded {
            WorkerResponse::Row { values } => {
                assert_eq!(values, vec![DbValue::Integer { value: "1".into() }]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_helper_frame_is_a_typed_protocol_error() {
        let (mut client, mut server) = duplex(64);
        tokio::spawn(async move {
            let _ = server.write_all(&5u32.to_be_bytes()).await;
            let _ = server.write_all(b"notjs").await;
        });
        let error = read_frame(&mut client).await.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("helperProtocol"));
    }

    #[tokio::test]
    async fn helper_process_exit_is_typed_value_too_large() {
        let mut command = Command::new(if cfg!(windows) { "cmd" } else { "true" });
        if cfg!(windows) {
            command.args(["/C", "exit", "0"]);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let child = command.spawn().unwrap();
        let worker = NetworkQueryWorker::from_child(child).unwrap();
        let error = worker.probe_version().await.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("valueTooLarge"));
        assert!(worker.is_closed());
    }

    #[tokio::test]
    async fn helper_timeout_reaps_the_child() {
        let (worker, pid) = hanging_child().await;
        let worker = worker.with_frame_timeout(Duration::from_millis(200));
        let error = worker.probe_version().await.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("helperTimeout"));
        assert!(worker.is_closed());
        if cfg!(unix) {
            assert!(
                !process_is_alive(pid),
                "timed-out helper child {pid} was not reaped"
            );
        }
    }

    #[tokio::test]
    async fn cancel_and_reap_terminates_the_child() {
        let (worker, pid) = hanging_child().await;
        worker.reap().await;
        assert!(worker.is_closed());
        if cfg!(unix) {
            assert!(
                !process_is_alive(pid),
                "cancelled helper child {pid} was not reaped"
            );
        }
    }

    #[tokio::test]
    async fn write_request_zeroizes_the_password_after_encoding() {
        let (mut left, mut right) = duplex(4096);
        let mut request = connect_request("s3cret-password");
        write_request(&mut left, &mut request).await.unwrap();
        match &request {
            WorkerRequest::ConnectPostgres { password, .. } => {
                assert!(
                    !password.contains("s3cret-password"),
                    "password survived write_request"
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        let decoded = read_request(&mut right).await.unwrap();
        match decoded {
            WorkerRequest::ConnectPostgres { password, .. } => {
                assert_eq!(password, "s3cret-password");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn connect_request_debug_redacts_the_password() {
        let request = connect_request("s3cret-password");
        let rendered = format!("{request:?}");
        assert!(
            !rendered.contains("s3cret-password"),
            "debug leaked the password: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("alice"));
    }

    #[test]
    fn helper_command_does_not_place_credentials_in_argv_or_env() {
        let Ok(command) = helper_command() else {
            return;
        };
        let std_command = command.as_std();
        assert!(
            std_command.get_args().next().is_none(),
            "helper argv must not carry extra tokens"
        );
        for (key, value) in std_command.get_envs() {
            let key = key.to_string_lossy().to_ascii_lowercase();
            assert!(
                !key.contains("password") && !key.contains("secret"),
                "helper env leaked a credential key: {key}"
            );
            if let Some(value) = value {
                let value = value.to_string_lossy().to_ascii_lowercase();
                assert!(
                    !value.contains("password") && !value.contains("s3cret"),
                    "helper env leaked a credential value"
                );
            }
        }
        let worker_env = std_command
            .get_envs()
            .find(|(key, _)| key.to_string_lossy() == WORKER_ENV)
            .and_then(|(_, value)| value.map(|value| value.to_os_string()));
        assert_eq!(worker_env.as_deref(), Some(std::ffi::OsStr::new("1")));
    }

    #[test]
    fn memory_limit_api_is_defined_without_changing_the_test_process_ceiling() {
        assert!(HELPER_MEMORY_BYTES >= DEFAULT_PROCESS_BYTES as u64);
        assert!(MAX_HELPER_FRAME_BYTES > DEFAULT_ROW_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn unix_memory_watchdog_installs_and_trips_only_above_the_limit() {
        start_resident_memory_watchdog(u64::MAX).unwrap();
        assert!(!resident_memory_limit_exceeded(
            HELPER_MEMORY_BYTES,
            HELPER_MEMORY_BYTES
        ));
        assert!(resident_memory_limit_exceeded(
            HELPER_MEMORY_BYTES + 1,
            HELPER_MEMORY_BYTES
        ));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn unix_address_space_fail_safe_keeps_driver_headroom() {
        assert_eq!(
            unix_address_space_limit(HELPER_MEMORY_BYTES),
            HELPER_MEMORY_BYTES * UNIX_ADDRESS_SPACE_HEADROOM_MULTIPLIER
        );
    }

    #[test]
    fn helper_fails_closed_when_memory_limit_cannot_be_installed() {
        fn reject_limit(bytes: u64) -> Result<(), String> {
            assert_eq!(bytes, HELPER_MEMORY_BYTES);
            Err("setrlimit(RLIMIT_AS) failed: injected".into())
        }
        assert_eq!(
            run_with_memory_limit(reject_limit),
            HELPER_MEMORY_LIMIT_EXIT
        );
        let diagnostic = memory_limit_diagnostic("setrlimit failed".into());
        assert_eq!(diagnostic.code, "helperMemoryLimit");
        assert_eq!(
            serde_json::to_value(diagnostic).unwrap(),
            serde_json::json!({
                "code": "helperMemoryLimit",
                "message": "setrlimit failed"
            })
        );
    }
}
