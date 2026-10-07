//! Owned loopback SFTP preflight stalls. No user hosts or files are involved.
use super::*;
use std::collections::HashSet;
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;

// SFTP v3 packet types, verified against the pinned russh-sftp codec.
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const LSTAT: u8 = 7;
const FSTAT: u8 = 8; // FSTAT: download preflight is bound to the opened handle.

struct Gate {
    opcode: u8,
    skip: usize,
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

#[derive(Default)]
pub(super) struct PacketProbe {
    gate: std::sync::Mutex<Option<Gate>>,
    counts: std::sync::Mutex<HashMap<u8, u64>>,
    children: std::sync::Mutex<HashSet<u32>>,
    pub(super) channels_opened: std::sync::atomic::AtomicU64,
    pub(super) channel_eofs: std::sync::atomic::AtomicU64,
    pub(super) channel_closes: std::sync::atomic::AtomicU64,
}

pub(super) struct ChildRecord {
    probe: Arc<PacketProbe>,
    pid: u32,
}
impl Drop for ChildRecord {
    fn drop(&mut self) {
        self.probe.children.lock().unwrap().remove(&self.pid);
    }
}

impl PacketProbe {
    pub(super) fn track_child(self: &Arc<Self>, pid: u32) -> ChildRecord {
        self.children.lock().unwrap().insert(pid);
        ChildRecord {
            probe: self.clone(),
            pid,
        }
    }
    fn count(&self, opcode: u8) -> u64 {
        *self.counts.lock().unwrap().get(&opcode).unwrap_or(&0)
    }
    fn arm(&self, opcode: u8, skip: usize) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, received) = oneshot::channel();
        let (release, released) = oneshot::channel();
        assert!(self
            .gate
            .lock()
            .unwrap()
            .replace(Gate {
                opcode,
                skip,
                entered,
                release: released
            })
            .is_none());
        (received, release)
    }
    pub(super) async fn forward<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &self,
        read: &mut R,
        write: &mut W,
    ) -> io::Result<()> {
        loop {
            let mut header = [0u8; 4];
            match read.read_exact(&mut header).await {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(error) => return Err(error),
            }
            let length = u32::from_be_bytes(header) as usize;
            if length == 0 || length > 2 * 1024 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "fixture packet size",
                ));
            }
            let mut packet = vec![0u8; length];
            read.read_exact(&mut packet).await?;
            let opcode = packet[0];
            *self.counts.lock().unwrap().entry(opcode).or_default() += 1;
            let gate = {
                let mut slot = self.gate.lock().unwrap();
                let take = match slot.as_mut() {
                    Some(gate) if gate.opcode == opcode => {
                        if gate.skip == 0 {
                            true
                        } else {
                            gate.skip -= 1;
                            false
                        }
                    }
                    _ => false,
                };
                if take {
                    slot.take()
                } else {
                    None
                }
            };
            if let Some(gate) = gate {
                let _ = gate.entered.send(());
                let _ = gate.release.await;
            }
            write.write_all(&header).await?;
            write.write_all(&packet).await?;
            write.flush().await?;
        }
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    local: PathBuf,
    remote: PathBuf,
    manager: SshManager,
    session: String,
    probe: Arc<PacketProbe>,
    server: tokio::task::JoinHandle<()>,
    selected: path_capability::SelectedPathRegistry,
    workspaces: path_capability::WorkspacePathRegistry,
    destinations: path_capability::DownloadDestinationRegistry,
    trust: crate::workspace_trust::WorkspaceTrustState,
    expected: String,
    download_key: String,
}

#[derive(Clone, Copy)]
enum Stage {
    DownloadMetadata,
    UploadPermissions,
    DownloadOpen,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::DownloadMetadata => "download-metadata",
            Self::UploadPermissions => "upload-permissions",
            Self::DownloadOpen => "download-open-guard",
        }
    }
}

impl Fixture {
    async fn new(deny_stat: bool) -> Self {
        let executable = std::env::var("YUZORA_SFTP_SERVER").expect("owned SFTP server executable");
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let local = path.join("local 中文");
        let remote = path.join("remote 中文");
        std::fs::create_dir(&local).unwrap();
        std::fs::create_dir(&remote).unwrap();
        std::fs::write(local.join("upload.bin"), b"upload-fixture").unwrap();
        std::fs::write(local.join("download.bin"), b"keep-local").unwrap();
        std::fs::write(remote.join("upload.bin"), b"keep-remote").unwrap();
        std::fs::write(remote.join("download.bin"), b"download-fixture").unwrap();
        std::fs::set_permissions(
            remote.join("upload.bin"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = russh::keys::PrivateKey::from_openssh(tests::TEST_HOST_KEY).unwrap();
        let fingerprint = fingerprint_sha256(key.public_key());
        let config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let password = uuid::Uuid::new_v4().to_string();
        let probe = Arc::new(PacketProbe::default());
        let mut fixture = FixtureServer {
            password: password.clone(),
            executable,
            helper: "/unused-fixture-helper".into(),
            helper_home: path.clone(),
            channels: HashMap::new(),
            relays: HashMap::new(),
            probe: Some(probe.clone()),
            deny_stat,
        };
        let server = tokio::spawn(async move {
            let _ = fixture.run_on_socket(config, &listener).await;
        });
        let manager = SshManager::with_parts(
            Box::new(|_| {}),
            path.join("known-hosts.json"),
            Arc::new(|_| {}),
            Duration::from_secs(2),
            Arc::new(StdHostKeyIo),
        );
        manager
            .host_keys
            .persist_pin(&canonical_endpoint("127.0.0.1", port), &fingerprint)
            .unwrap();
        let connected = manager
            .connect(
                "127.0.0.1".into(),
                port,
                "fixture".into(),
                SshAuth::Password { password },
            )
            .await
            .unwrap();
        let session = connected.session_id;
        let sftp = manager.ensure_sftp(&session).await.unwrap();
        let expected =
            crate::sftp_edit::remote_revision(&sftp, remote.join("upload.bin").to_str().unwrap())
                .await
                .unwrap()
                .unwrap();
        let destinations = path_capability::DownloadDestinationRegistry::default();
        let grant = destinations.grant(&local.join("download.bin")).unwrap();
        let scratch = destinations
            .take_scratch(&grant.id, "xfer-key-probe")
            .unwrap();
        let download_key = scratch.dest_key();
        drop(scratch);
        Self {
            _root: root,
            local,
            remote,
            manager,
            session,
            probe,
            server,
            selected: Default::default(),
            workspaces: Default::default(),
            destinations,
            trust: crate::workspace_trust::WorkspaceTrustState::at(path.join("trust.json")),
            expected,
            download_key,
        }
    }
    async fn barrier(&self) {
        // The existing SFTP session must survive late replies to cancelled STAT.
        let sftp = self.manager.ensure_sftp(&self.session).await.unwrap();
        sftp.symlink_metadata(self.remote.join("download.bin").to_str().unwrap())
            .await
            .unwrap();
    }
    async fn close(self) {
        self.manager.disconnect(&self.session).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.probe.children.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        self.server.abort();
        let _ = self.server.await;
        assert!(self.probe.children.lock().unwrap().is_empty());
    }
    fn scratch_count(&self) -> usize {
        [&self.local, &self.remote]
            .into_iter()
            .flat_map(|path| std::fs::read_dir(path).unwrap())
            .filter(|entry| {
                let name = entry.as_ref().unwrap().file_name();
                let name = name.to_string_lossy();
                name.starts_with('.') || name.contains(".yz-tmp-")
            })
            .count()
    }
    async fn cancel_case(&self, stage: Stage) -> serde_json::Value {
        let id = self.manager.transfers.reserve(&self.session).unwrap();
        let (opcode, skip) = match stage {
            Stage::DownloadMetadata => (FSTAT, 0),
            Stage::UploadPermissions => (LSTAT, 1),
            Stage::DownloadOpen => (OPEN, 0),
        };
        let (entered, release) = self.probe.arm(opcode, skip);
        let target = self.remote.join("upload.bin");
        let key = match stage {
            Stage::UploadPermissions => {
                path_capability::remote_dest_key(&self.session, target.to_str().unwrap())
            }
            _ => self.download_key.clone(),
        };
        let cancelled = AtomicBool::new(false);
        let late_progress = std::sync::atomic::AtomicUsize::new(0);
        let progress = |_: u64, _: u64, done: bool| {
            assert!(!done, "cancelled fixture must never promote");
            if cancelled.load(Ordering::Acquire) {
                late_progress.fetch_add(1, Ordering::Relaxed);
            }
        };
        let operation = async {
            let result = match stage {
                Stage::UploadPermissions => {
                    self.manager
                        .sftp_upload(
                            &progress,
                            &self.selected,
                            &self.workspaces,
                            &self.trust,
                            &self.session,
                            SftpUploadRequest {
                                transfer_id: id.clone(),
                                source: SftpUploadSource::Selected {
                                    capability_id: self
                                        .selected
                                        .grant(self.local.join("upload.bin").to_str().unwrap())
                                        .unwrap(),
                                },
                                remote_dir: self.remote.to_string_lossy().into_owned(),
                                expected_revision: Some(self.expected.clone()),
                            },
                        )
                        .await
                }
                _ => {
                    let grant = self
                        .destinations
                        .grant(&self.local.join("download.bin"))
                        .unwrap();
                    self.manager
                        .sftp_download(
                            &progress,
                            &self.session,
                            &id,
                            self.remote.join("download.bin").to_str().unwrap(),
                            &grant.id,
                            &self.destinations,
                        )
                        .await
                }
            };
            (std::time::Instant::now(), result)
        };
        tokio::pin!(operation);
        tokio::select! {
            signal = entered => signal.unwrap(),
            _ = &mut operation => panic!("fixture completed before gated preflight"),
            _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("preflight gate not reached"),
        }
        assert!(self.manager.transfer_dests.acquire(key.clone()).is_err());
        let opens_before = self.probe.count(OPEN);
        let started = std::time::Instant::now();
        cancelled.store(true, Ordering::Release);
        self.manager.transfers.cancel(&self.session, &id).unwrap();
        let release_gate = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let slot_free = self.manager.transfer_dests.acquire(key.clone()).is_ok();
            let scratch_at_20ms = self.scratch_count();
            tokio::time::sleep(Duration::from_millis(80)).await;
            let _ = release.send(());
            (slot_free, scratch_at_20ms)
        };
        let ((finished, result), (slot_free, scratch_at_20ms)) =
            tokio::join!(&mut operation, release_gate);
        assert_eq!(result.unwrap_err(), "sftp-transfer-cancelled");
        assert!(self.manager.transfers.cancel(&self.session, &id).is_err());
        assert!(self.manager.transfer_dests.acquire(key).is_ok());
        self.barrier().await;
        assert_eq!(
            self.probe.count(OPEN),
            self.probe.count(CLOSE),
            "every known read handle closes"
        );
        assert_eq!(self.scratch_count(), 0);
        assert_eq!(
            std::fs::read(self.local.join("download.bin")).unwrap(),
            b"keep-local"
        );
        assert_eq!(std::fs::read(target).unwrap(), b"keep-remote");
        serde_json::json!({ "stage": stage.name(), "cancelMs": finished.duration_since(started).as_secs_f64() * 1000.0,
            "slotFreeAt20Ms": slot_free, "scratchAt20Ms": scratch_at_20ms,
            "opensAfterCancel": self.probe.count(OPEN) - opens_before,
            "lateProgress": late_progress.load(Ordering::Relaxed), "scratchAfter": self.scratch_count() })
    }

    async fn healthy_transfer(&self, upload: bool, bytes: &[u8]) -> f64 {
        let id = self.manager.transfers.reserve(&self.session).unwrap();
        let progress = std::sync::Mutex::new(Vec::new());
        let report =
            |transferred, total, done| progress.lock().unwrap().push((transferred, total, done));
        let elapsed;
        if upload {
            std::fs::write(self.local.join("upload.bin"), bytes).unwrap();
            std::fs::write(self.remote.join("upload.bin"), b"keep-remote").unwrap();
            let request = SftpUploadRequest {
                transfer_id: id,
                source: SftpUploadSource::Selected {
                    capability_id: self
                        .selected
                        .grant(self.local.join("upload.bin").to_str().unwrap())
                        .unwrap(),
                },
                remote_dir: self.remote.to_string_lossy().into_owned(),
                expected_revision: Some(self.expected.clone()),
            };
            let started = std::time::Instant::now();
            self.manager
                .sftp_upload(
                    &report,
                    &self.selected,
                    &self.workspaces,
                    &self.trust,
                    &self.session,
                    request,
                )
                .await
                .unwrap();
            elapsed = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(
                std::fs::read(self.remote.join("upload.bin")).unwrap(),
                bytes
            );
            assert_eq!(
                std::fs::metadata(self.remote.join("upload.bin"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o640
            );
        } else {
            let remote = self.remote.join("healthy.bin");
            let local = self.local.join("healthy.bin");
            std::fs::write(&remote, bytes).unwrap();
            let grant = self.destinations.grant(&local).unwrap();
            let started = std::time::Instant::now();
            self.manager
                .sftp_download(
                    &report,
                    &self.session,
                    &id,
                    remote.to_str().unwrap(),
                    &grant.id,
                    &self.destinations,
                )
                .await
                .unwrap();
            elapsed = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(std::fs::read(local).unwrap(), bytes);
        }
        self.barrier().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.probe.children.lock().unwrap().len() != 1 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        let progress = progress.lock().unwrap();
        let total = bytes.len() as u64;
        assert_eq!(progress.last(), Some(&(bytes.len() as u64, total, true)));
        assert_eq!(progress.iter().filter(|event| event.2).count(), 1);
        assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(self.scratch_count(), 0);
        assert_eq!(self.probe.count(OPEN), self.probe.count(CLOSE));
        elapsed
    }

    fn resources(&self, memory: &mut sysinfo::System) -> serde_json::Value {
        use sysinfo::{get_current_pid, Pid, ProcessRefreshKind, ProcessesToUpdate};
        let parent = get_current_pid().unwrap();
        let children: Vec<_> = self
            .probe
            .children
            .lock()
            .unwrap()
            .iter()
            .copied()
            .map(Pid::from_u32)
            .collect();
        let mut pids = children.clone();
        pids.push(parent);
        memory.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        let parent_rss = memory.process(parent).unwrap().memory();
        let child_rss: Vec<_> = children
            .iter()
            .map(|pid| {
                memory
                    .process(*pid)
                    .expect("owned SFTP child alive")
                    .memory()
            })
            .collect();
        let descriptor_path = if cfg!(target_os = "linux") {
            "/proc/self/fd"
        } else {
            "/dev/fd"
        };
        serde_json::json!({ "parentRssBytes": parent_rss, "childRssBytes": child_rss,
            "processTreeRssBytes": parent_rss + child_rss.iter().sum::<u64>(),
            "parentFds": std::fs::read_dir(descriptor_path).unwrap().count(),
            "ownedSftpChildren": children.len(), "readHandlesOutstanding": self.probe.count(OPEN) - self.probe.count(CLOSE),
            "aliveTasks": tokio::runtime::Handle::current().metrics().num_alive_tasks(),
            "channelsOpened": self.probe.channels_opened.load(Ordering::Relaxed),
            "channelEofs": self.probe.channel_eofs.load(Ordering::Relaxed),
            "channelCloses": self.probe.channel_closes.load(Ordering::Relaxed),
        })
    }
}

fn parent_cpu_ms() -> f64 {
    let usage = unsafe {
        let mut value = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, value.as_mut_ptr()), 0);
        value.assume_init()
    };
    (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned OpenSSH preflight probe; requires YUZORA_SFTP_SERVER"]
async fn performance_cancelled_sftp_preflight() {
    let smoke = std::env::var_os("YUZORA_SFTP_PREFLIGHT_SMOKE").is_some();
    let extended = std::env::var_os("YUZORA_SFTP_PREFLIGHT_EXTENDED_HEALTHY").is_some();
    assert!(
        smoke || !cfg!(debug_assertions),
        "Use release for measurements"
    );
    let fixture = Fixture::new(false).await;
    let original_children = fixture.probe.children.lock().unwrap().clone();
    assert_eq!(original_children.len(), 1);
    let mut memory = sysinfo::System::new();
    for stage in [
        Stage::DownloadMetadata,
        Stage::UploadPermissions,
        Stage::DownloadOpen,
    ] {
        for cycle in 0..if smoke { 1 } else { 110 } {
            let mut value = fixture.cancel_case(stage).await;
            value["cycle"] = cycle.into();
            value["warmup"] = (cycle < 10).into();
            println!("SFTP_PREFLIGHT_CANCEL {value}");
            if smoke {
                println!(
                    "SFTP_PREFLIGHT_SMOKE_RESOURCES {}",
                    fixture.resources(&mut memory)
                );
            }
            assert_eq!(
                *fixture.probe.children.lock().unwrap(),
                original_children,
                "persistent connection must not restart during lifecycle measurements"
            );
            if cycle >= 10 && (cycle + 1) % 10 == 0 {
                println!(
                    "SFTP_PREFLIGHT_RESOURCES {}",
                    serde_json::json!({
                        "stage": stage.name(), "completedCycles": cycle - 9, "warmup": 10,
                        "resources": fixture.resources(&mut memory),
                    })
                );
            }
        }
    }
    for upload in [false, true] {
        for size in [0, 4096, 262144] {
            let bytes = vec![b'x'; size];
            let warmup_batches = if extended { 5 } else { 1 };
            for batch in 0..if smoke { 1 } else { warmup_batches + 7 } {
                let count = if smoke {
                    1
                } else if extended {
                    32
                } else {
                    4
                };
                let cpu = parent_cpu_ms();
                let started = std::time::Instant::now();
                let mut latency = Vec::new();
                for _ in 0..count {
                    latency.push(fixture.healthy_transfer(upload, &bytes).await);
                }
                println!(
                    "SFTP_PREFLIGHT_HEALTHY {}",
                    serde_json::json!({
                    "upload": upload, "bytes": size, "batch": batch, "warmup": batch < warmup_batches,
                        "iterations": count, "parentCpuMs": parent_cpu_ms() - cpu,
                        "pipelineWallMs": started.elapsed().as_secs_f64() * 1000.0, "methodLatencyMs": latency,
                        "scope": "CPU includes fixture writes,byte verification and child cleanup;method latency is the transfer await",
                    })
                );
            }
            println!(
                "SFTP_PREFLIGHT_HEALTHY_RESOURCES {}",
                serde_json::json!({
                    "upload": upload, "bytes": size, "resources": fixture.resources(&mut memory),
                })
            );
        }
    }
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned OpenSSH fallback guard; requires YUZORA_SFTP_SERVER"]
async fn metadata_failure_rejects_unknown_size_without_replacing_destination() {
    let fixture = Fixture::new(true).await;
    let target = fixture.local.join("download.bin");
    let grant = fixture.destinations.grant(&target).unwrap();
    let id = fixture.manager.transfers.reserve(&fixture.session).unwrap();
    let result = fixture
        .manager
        .sftp_download(
            &|_, _, done| assert!(!done),
            &fixture.session,
            &id,
            fixture.remote.join("download.bin").to_str().unwrap(),
            &grant.id,
            &fixture.destinations,
        )
        .await;
    assert_eq!(result.unwrap_err(), "sftp-size-unknown");
    assert_eq!(std::fs::read(&target).unwrap(), b"keep-local");
    assert!(std::fs::read_dir(&fixture.local)
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".yz-tmp-")));
    fixture.close().await;
}

#[path = "sftp_revision_cancel_tests.rs"]
mod revision_cancel;

#[path = "sftp_session_lifecycle_tests.rs"]
mod session_lifecycle;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned OpenSSH size guard; requires YUZORA_SFTP_SERVER"]
async fn changed_download_size_never_promotes_a_partial_or_overlong_stream() {
    for replacement in [
        b"".as_slice(),
        b"longer-than-the-original-download-fixture".as_slice(),
    ] {
        let fixture = Fixture::new(false).await;
        let target = fixture.local.join("download.bin");
        let remote = fixture.remote.join("download.bin");
        let grant = fixture.destinations.grant(&target).unwrap();
        let id = fixture.manager.transfers.reserve(&fixture.session).unwrap();
        let (entered, release) = fixture.probe.arm(5, 0); // READ, after FSTAT completed.
        let report = |_: u64, _: u64, done: bool| assert!(!done);
        {
            let operation = fixture.manager.sftp_download(
                &report,
                &fixture.session,
                &id,
                remote.to_str().unwrap(),
                &grant.id,
                &fixture.destinations,
            );
            tokio::pin!(operation);
            tokio::select! {
                signal = entered => signal.unwrap(),
                result = &mut operation => panic!("download completed before gated read: {result:?}"),
                _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("read gate not reached"),
            }
            std::fs::write(&remote, replacement).unwrap();
            release.send(()).unwrap();
            assert_eq!(operation.await.unwrap_err(), "sftp-size-mismatch");
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"keep-local");
        assert_eq!(fixture.scratch_count(), 0);
        fixture.close().await;
    }
}
