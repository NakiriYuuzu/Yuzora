use crate::files::WorkspaceFiles;
use crate::herdr_service::HerdrManager;
use crate::protocol::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

pub use crate::wire::read_frame;

/// Discovery asks the default runtime, not a HERDR pane that launched the helper.
fn herdr_command(binary: &str, args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    crate::herdr_service::pin_herdr_session(command.as_std_mut(), None);
    command
}

async fn command_json(binary: &str, args: &[&str]) -> Result<Value, String> {
    let mut child = herdr_command(binary, args)
        .spawn()
        .map_err(|e| format!("herdr-unavailable: {e}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("missing-stdout")?
        .take((MAX_FRAME_BYTES + 1) as u64);
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err("frame-too-large".into());
        }
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("herdr-command-failed: {status}"));
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("herdr-invalid-json: {e}"))
    })
    .await;
    match result {
        Ok(Ok(value)) => Ok(value),
        other => {
            let _ = child.kill().await;
            match other {
                Ok(Err(error)) => Err(error),
                _ => Err("herdr-timeout".into()),
            }
        }
    }
}

pub async fn socket_request(socket: &str, request: Value) -> Result<Value, String> {
    let socket = socket.to_owned();
    tokio::task::spawn_blocking(move || {
        use crate::herdr_transport::{
            connect_local_stream, read_local_ndjson_line, write_local_all_until,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut stream = connect_local_stream(&socket, deadline).map_err(|e| e.to_string())?;
        let mut bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if bytes.len() >= MAX_FRAME_BYTES {
            return Err("frame-too-large".into());
        }
        bytes.push(b'\n');
        write_local_all_until(&mut stream, &bytes, deadline).map_err(|e| e.to_string())?;
        let frame = read_local_ndjson_line(
            &mut stream,
            &mut Vec::new(),
            Some(deadline),
            crate::herdr_limits::MAX_NDJSON_LINE_BYTES,
        )
        .map_err(|e| e.to_string())?
        .ok_or("herdr-socket-closed")?;
        serde_json::from_str(&frame).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Default)]
pub struct HostServer {
    owner: Option<ConnectionOwner>,
    files: WorkspaceFiles,
    sockets: HashSet<String>,
    herdr: HashMap<String, Arc<HerdrManager>>,
    trust: Option<crate::workspace_trust::WorkspaceTrustState>,
    git: Arc<Mutex<crate::git_command::HostGit>>,
    cancelled: Arc<AtomicBool>,
}

struct CancelSearchOnDrop(Arc<AtomicU64>);

impl Drop for CancelSearchOnDrop {
    fn drop(&mut self) {
        self.0.store(1, Ordering::Relaxed);
    }
}

impl Drop for HostServer {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl HostServer {
    fn runtime(&mut self, binary: &str) -> Result<Arc<HerdrManager>, String> {
        if let Some(runtime) = self.herdr.get(binary) {
            return Ok(runtime.clone());
        }
        if self.herdr.len() >= 4 {
            return Err("too-many-runtime-binaries".into());
        }
        let manager = Arc::new(if binary == "herdr" {
            HerdrManager::new()
        } else {
            HerdrManager::with_binary(binary.into())
        });
        if manager.resolve_binary().is_none() {
            return Err("herdr-unavailable".into());
        }
        self.herdr.insert(binary.into(), manager.clone());
        Ok(manager)
    }
    pub async fn handle(&mut self, request: Request) -> Response {
        let result = if request.version != PROTOCOL_VERSION {
            Err("protocol-mismatch".to_owned())
        } else if self
            .owner
            .as_ref()
            .is_some_and(|owner| owner != &request.owner)
        {
            Err("connection-owner-mismatch".to_owned())
        } else if self.owner.is_none() && !matches!(&request.operation, Operation::Hello) {
            Err("handshake-required".to_owned())
        } else {
            if self.owner.is_none() {
                self.trust = crate::trust_command::host_trust(&request.owner.host_id).ok();
            }
            self.owner.get_or_insert_with(|| request.owner.clone());
            self.dispatch(request.operation).await
        };
        let outcome = match result {
            Ok(value) => Outcome::Ok { value },
            Err(message) => Outcome::Error {
                code: message.split(':').next().unwrap_or("host-error").to_owned(),
                message,
            },
        };
        Response {
            version: PROTOCOL_VERSION,
            id: request.id,
            owner: request.owner,
            outcome,
        }
    }

    async fn dispatch(&mut self, operation: Operation) -> Result<Value, String> {
        match operation {
            Operation::Trust { call } => call.execute(
                &self.files,
                self.trust.as_ref().ok_or("host-trust-unavailable")?,
            ),
            Operation::WorkspaceAuthorize { workspace } => {
                let path = self.files.canonical_root(&workspace)?;
                let identity = self
                    .trust
                    .as_ref()
                    .ok_or("host-trust-unavailable")?
                    .require_trusted(path)?;
                serde_json::to_value(identity).map_err(|e| e.to_string())
            }
            Operation::Git {
                workspace,
                repository_root,
                call,
            } => {
                let path = self.files.canonical_root(&workspace)?.to_owned();
                let trust = self.trust.as_ref().ok_or("host-trust-unavailable")?.clone();
                let git = self.git.clone();
                let cancelled = self.cancelled.clone();
                tokio::task::spawn_blocking(move || {
                    crate::git_process::with_cancellation(cancelled, || {
                        git.lock().map_err(|e| e.to_string())?.execute_root(
                            &path,
                            &trust,
                            &workspace,
                            repository_root.as_deref(),
                            call,
                        )
                    })
                })
                .await
                .map_err(|e| e.to_string())?
            }
            Operation::Hello => serde_json::to_value(Hello {
                protocol: PROTOCOL_VERSION,
                version: env!("CARGO_PKG_VERSION").into(),
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                home: dirs::home_dir()
                    .and_then(|path| path.to_str().map(str::to_owned))
                    .ok_or("home-unavailable")?,
                methods: methods(),
            })
            .map_err(|e| e.to_string()),
            Operation::ClipboardImage { png_base64 } => tokio::task::spawn_blocking(move || {
                crate::clipboard_image::stage(&png_base64).map(Value::String)
            })
            .await
            .map_err(|e| e.to_string())?,
            Operation::WorkspaceOpen { path } => self.files.open(&path),
            Operation::WorkspaceClose { workspace } => {
                self.git
                    .lock()
                    .map_err(|e| e.to_string())?
                    .close(&workspace);
                self.files.close(&workspace);
                Ok(Value::Null)
            }
            Operation::FileNameSearch { workspace, query } => {
                let root = self.files.file_name_search_root(&workspace)?;
                let generation = CancelSearchOnDrop(Arc::new(AtomicU64::new(0)));
                let source = generation.0.clone();
                let task = tokio::task::spawn_blocking(move || {
                    crate::file_name_search::run_pinned_file_name_search(
                        &root,
                        &query,
                        0,
                        &source,
                        Duration::from_secs(1),
                    )
                });
                // A slow filesystem syscall must not monopolize the serial transport.
                let result = match tokio::time::timeout(Duration::from_secs(1), task).await {
                    Ok(result) => result.map_err(|e| e.to_string())??,
                    Err(_) => {
                        generation.0.store(1, Ordering::Relaxed);
                        crate::file_name_search::FileNameSearchResult {
                            files: Vec::new(),
                            incomplete: true,
                        }
                    }
                };
                serde_json::to_value(result).map_err(|e| e.to_string())
            }
            Operation::FilesList { workspace, path } => self.files.list(&workspace, &path),
            Operation::FilesCreate {
                workspace,
                path,
                directory,
            } => self.files.create(&workspace, &path, directory),
            Operation::FilesRename {
                workspace,
                from,
                to,
            } => self.files.rename(&workspace, &from, &to),
            Operation::FilesDelete { workspace, path } => self.files.delete(&workspace, &path),
            Operation::FilesCopy {
                workspace,
                sources,
                target_dir,
            } => {
                // Up to 120 s of copying must not monopolize the serial transport.
                let root = self.files.pinned_root(&workspace)?;
                tokio::task::spawn_blocking(move || {
                    crate::file_transfer::copy_into(&root, &sources, &target_dir)
                        .map(|created| json!(created))
                })
                .await
                .map_err(|e| e.to_string())?
            }
            Operation::FilesImport {
                workspace,
                sources,
                target_dir,
            } => {
                let sources = crate::file_transfer::import_sources(&sources)?;
                let root = self.files.pinned_root(&workspace)?;
                let canonical = std::path::PathBuf::from(self.files.canonical_root(&workspace)?);
                tokio::task::spawn_blocking(move || {
                    crate::file_transfer::import_into(&root, &canonical, &sources, &target_dir)
                        .map(|created| json!(created))
                })
                .await
                .map_err(|e| e.to_string())?
            }
            Operation::FilesMove {
                workspace,
                sources,
                target_dir,
            } => self.files.move_paths(&workspace, &sources, &target_dir),
            Operation::FilesReadBase64 {
                workspace,
                path,
                max_bytes,
            } => self.files.read_base64(&workspace, &path, max_bytes),
            Operation::FilesRead { workspace, path } => self.files.read(&workspace, &path),
            Operation::FilesWrite {
                workspace,
                path,
                content,
                revision,
            } => self.files.write(&workspace, &path, &content, &revision),
            Operation::HerdrCall { binary, call } => {
                let manager = self.runtime(&binary)?;
                tokio::task::spawn_blocking(move || call.execute(&manager))
                    .await
                    .map_err(|e| e.to_string())?
            }
            Operation::HerdrMetadata {
                binary,
                query,
                session,
            } => {
                let manager = self.runtime(&binary)?;
                tokio::task::spawn_blocking(move || manager.metadata(query, session.as_deref()))
                    .await
                    .map_err(|e| e.to_string())?
            }
            Operation::HerdrStart { binary } => {
                let manager = self.runtime(&binary)?;
                tokio::task::spawn_blocking(move || {
                    manager
                        .ensure_server_running_on_startup()
                        .map(|started| json!({"started":started}))
                })
                .await
                .map_err(|e| e.to_string())?
            }
            Operation::HerdrDiscover { binary } => {
                let sessions = command_json(&binary, &["session", "list", "--json"]).await?;
                let schema = command_json(&binary, &["api", "schema", "--json"]).await?;
                self.sockets.clear();
                let rows = sessions
                    .as_array()
                    .or_else(|| sessions.get("sessions").and_then(Value::as_array));
                if let Some(rows) = rows {
                    for row in rows {
                        if let Some(socket) = row.get("socket_path").and_then(Value::as_str) {
                            self.sockets.insert(socket.into());
                        }
                    }
                }
                Ok(json!({"sessions":sessions,"schema":schema}))
            }
            Operation::HerdrRequest { socket, request } => {
                if !self.sockets.contains(&socket) {
                    return Err("undiscovered-herdr-socket".into());
                }
                socket_request(&socket, request).await
            }
        }
    }
}

pub async fn serve<R, W>(input: R, mut output: W) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut input = BufReader::new(input);
    let mut server = HostServer::default();
    let mut frame = read_frame(&mut input).await?;
    while let Some(bytes) = frame {
        let request: Request =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid-request: {e}"))?;
        // Poll EOF while a blocking workspace job runs. Keep this same frame
        // reader alive until completion; cancelling a partial read loses bytes.
        let next = read_frame(&mut input);
        tokio::pin!(next);
        let mut prefetched = None;
        let handling = server.handle(request);
        tokio::pin!(handling);
        let response = tokio::select! {
            response = &mut handling => response,
            incoming = &mut next => {
                let incoming = incoming?;
                if incoming.is_none() { return Ok(()); }
                prefetched = Some(incoming);
                handling.await
            }
        };
        let mut bytes = serde_json::to_vec(&response).map_err(|e| e.to_string())?;
        if bytes.len() >= MAX_FRAME_BYTES {
            return Err("response-too-large".into());
        }
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(10), async {
            output.write_all(&bytes).await?;
            output.flush().await
        })
        .await
        .map_err(|_| "host-write-timeout")?
        .map_err(|e| e.to_string())?;
        frame = match prefetched {
            Some(frame) => frame,
            None => next.await?,
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_discovery_ignores_the_launching_pane_environment() {
        let command = herdr_command("/herdr", &["session", "list", "--json"]);
        let removed: HashSet<_> = command
            .as_std()
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        for key in crate::herdr_service::PARENT_PANE_HERDR_ENV
            .into_iter()
            .chain(["HERDR_SESSION"])
        {
            assert!(removed.contains(key), "{key} must not be inherited");
        }
    }

    #[cfg(unix)]
    #[allow(
        clippy::assertions_on_constants,
        reason = "This manual probe requires comparable release binaries"
    )]
    #[test]
    #[ignore = "manual abandoned-search CPU and resource measurement"]
    fn performance_search_after_request_drop() {
        use std::future::Future;
        use std::task::Poll;
        use sysinfo::{get_current_pid, ProcessRefreshKind, ProcessesToUpdate, System};
        assert!(!cfg!(debug_assertions), "Use --release");
        fn cpu_ms() -> f64 {
            let usage = unsafe {
                let mut value = std::mem::MaybeUninit::<libc::rusage>::uninit();
                assert_eq!(libc::getrusage(libc::RUSAGE_SELF, value.as_mut_ptr()), 0);
                value.assume_init()
            };
            (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
                + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0
        }
        let directory = tempfile::tempdir().unwrap();
        for group in 0..16 {
            let path = directory.path().join(format!("group-{group}"));
            std::fs::create_dir(&path).unwrap();
            for index in 0..150 {
                std::fs::write(path.join(format!("entry-{index}.ts")), "").unwrap();
            }
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            // Exercise post-handshake request handling without loading user trust state.
            let mut server = HostServer::default();
            server.owner = Some(request(1, Operation::Hello).owner);
            let opened = server
                .handle(request(
                    1,
                    Operation::WorkspaceOpen {
                        path: directory.path().to_str().unwrap().into(),
                    },
                ))
                .await;
            let Outcome::Ok { value } = opened.outcome else {
                panic!("owned workspace open failed")
            };
            let workspace = value["capabilityId"].as_str().unwrap().to_owned();
            let mut memory = System::new();
            let pid = get_current_pid().unwrap();
            let soak = std::env::var_os("YUZORA_PERF_SEARCH_SOAK").is_some();
            for cancelled in [true, false] {
                for sample in 0..if soak { 20 } else { 6 } {
                    let mut cpu = 0.0;
                    let mut wall = 0.0;
                    let warmup = sample < if soak { 10 } else { 1 };
                    let cycles = 100;
                    for _ in 0..cycles {
                        let (release, held) = std::sync::mpsc::channel();
                        let (started, ready) = tokio::sync::oneshot::channel();
                        let blocker = tokio::task::spawn_blocking(move || {
                            started.send(()).unwrap();
                            held.recv().unwrap();
                        });
                        ready.await.unwrap();
                        let mut handling = Some(Box::pin(server.handle(request(
                            1,
                            Operation::FileNameSearch {
                                workspace: workspace.clone(),
                                query: "missing-term".into(),
                            },
                        ))));
                        std::future::poll_fn(|cx| {
                            assert!(handling.as_mut().unwrap().as_mut().poll(cx).is_pending());
                            Poll::Ready(())
                        })
                        .await;
                        if cancelled {
                            drop(handling.take());
                        }
                        // The locked Tokio pool uses FIFO; with one worker this
                        // marker completes only after the queued search settles.
                        let drained = tokio::task::spawn_blocking(|| ());
                        let before = cpu_ms();
                        let clock = std::time::Instant::now();
                        release.send(()).unwrap();
                        if let Some(handling) = handling {
                            let Outcome::Ok { value } = handling.await.outcome else {
                                panic!("healthy search failed")
                            };
                            assert!(value["files"].as_array().unwrap().is_empty());
                            assert_eq!(value["incomplete"], false);
                        }
                        drained.await.unwrap();
                        blocker.await.unwrap();
                        wall += clock.elapsed().as_secs_f64() * 1000.0;
                        cpu += cpu_ms() - before;
                    }
                    memory.refresh_processes_specifics(
                        ProcessesToUpdate::Some(&[pid]),
                        true,
                        ProcessRefreshKind::nothing().with_memory(),
                    );
                    let fd_path = if cfg!(target_os = "linux") {
                        "/proc/self/fd"
                    } else {
                        "/dev/fd"
                    };
                    let descriptors = std::fs::read_dir(fd_path).unwrap().count();
                    println!(
                        "SEARCH_DROP_MEASUREMENT {}",
                        serde_json::json!({
                        "cancelled": cancelled, "sample": sample, "warmup": warmup,
                        "cycles": cycles, "cpuMs": cpu, "wallMs": wall, "descriptors": descriptors,
                            "rssBytes": memory.process(pid).unwrap().memory(), "profile": "release"
                        })
                    );
                }
            }
        });
    }

    #[test]
    fn dropping_a_search_generation_cancels_only_its_scan() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("target.ts"), "").unwrap();
        let mut files = WorkspaceFiles::default();
        let opened = files.open(directory.path().to_str().unwrap()).unwrap();
        let root = files
            .file_name_search_root(opened["capabilityId"].as_str().unwrap())
            .unwrap();
        let scan = |source: &AtomicU64| {
            crate::file_name_search::run_pinned_file_name_search(
                &root,
                "target",
                0,
                source,
                Duration::from_secs(1),
            )
        };
        let independent = CancelSearchOnDrop(Arc::new(AtomicU64::new(0)));
        let abandoned = {
            let generation = CancelSearchOnDrop(Arc::new(AtomicU64::new(0)));
            let worker_source = generation.0.clone();
            assert_eq!(scan(&worker_source).unwrap().files.len(), 1);
            worker_source
        };
        assert_eq!(scan(&abandoned).unwrap_err(), "file-name-search-cancelled");
        assert_eq!(scan(&independent.0).unwrap().files.len(), 1);
    }

    fn request(generation: u64, operation: Operation) -> Request {
        Request {
            version: PROTOCOL_VERSION,
            id: "id".into(),
            owner: ConnectionOwner {
                host_id: "test".into(),
                generation,
            },
            operation,
        }
    }
    #[tokio::test]
    async fn handshake_and_owner_are_enforced() {
        let mut server = HostServer::default();
        assert!(matches!(
            server
                .handle(request(1, Operation::WorkspaceOpen { path: "/".into() }))
                .await
                .outcome,
            Outcome::Error { .. }
        ));
        assert!(matches!(
            server.handle(request(1, Operation::Hello)).await.outcome,
            Outcome::Ok { .. }
        ));
        assert!(matches!(
            server.handle(request(2, Operation::Hello)).await.outcome,
            Outcome::Error { .. }
        ));
    }
    #[tokio::test]
    async fn file_name_search_is_advertised_and_requires_workspace_capability() {
        let mut server = HostServer::default();
        let hello = server.handle(request(1, Operation::Hello)).await;
        let Outcome::Ok { value } = hello.outcome else {
            panic!()
        };
        assert!(value["methods"]
            .as_array()
            .unwrap()
            .contains(&json!("fileNameSearch")));
        let operation = |workspace| Operation::FileNameSearch {
            workspace,
            query: "TARGET".into(),
        };
        assert!(matches!(
            server
                .handle(request(1, operation("invalid".into())))
                .await
                .outcome,
            Outcome::Error { .. }
        ));
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("target.ts"), "").unwrap();
        let opened = server
            .handle(request(
                1,
                Operation::WorkspaceOpen {
                    path: tmp.path().to_str().unwrap().into(),
                },
            ))
            .await;
        let Outcome::Ok { value } = opened.outcome else {
            panic!()
        };
        let response = server
            .handle(request(
                1,
                operation(value["capabilityId"].as_str().unwrap().into()),
            ))
            .await;
        let Outcome::Ok { value } = response.outcome else {
            panic!()
        };
        assert_eq!(value["files"][0]["name"], "target.ts");
        assert_eq!(value["files"][0]["isDir"], false);
        assert_eq!(value["files"][0]["kind"], "file");
        assert_eq!(value["incomplete"], false);
    }

    #[tokio::test]
    async fn rejects_oversized_and_partial_frames() {
        let bytes = vec![b'a'; MAX_FRAME_BYTES + 1];
        assert_eq!(
            read_frame(&mut BufReader::new(bytes.as_slice()))
                .await
                .unwrap_err(),
            "frame-too-large"
        );
        assert_eq!(
            read_frame(&mut BufReader::new(&b"{"[..]))
                .await
                .unwrap_err(),
            "truncated-frame"
        );
    }
}
