//! Protocol-level directory ownership guards, with no network or filesystem I/O.
use russh_sftp::client::{error::Error, SftpSession};
use russh_sftp::protocol::{
    Attrs, File, FileAttributes, Handle, Name, Status, StatusCode, Version,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
struct State {
    live: Mutex<HashSet<String>>,
    pages: Mutex<Option<Vec<Vec<File>>>>,
    opens: AtomicUsize,
    closes: AtomicUsize,
    deny_read: AtomicBool,
    deny_close: AtomicBool,
    dropped: AtomicBool,
    gate: AtomicU8,
    entered: Notify,
    resume: Notify,
    changed: Notify,
}

impl State {
    async fn gate(&self, stage: u8) {
        if self
            .gate
            .compare_exchange(stage, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.entered.notify_one();
            self.resume.notified().await;
        }
    }

    async fn drained(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let changed = self.changed.notified();
                if self.live.lock().unwrap().is_empty() {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("directory close must reach peer");
    }
}

struct Peer {
    state: Arc<State>,
    page: usize,
}

#[tokio::test]
async fn listing_limits_release_the_handle_without_returning_a_partial_directory() {
    for (entries, bytes) in [(2, usize::MAX), (usize::MAX, 1)] {
        let fixture = Fixture::new().await;
        let result = fixture
            .client
            .read_dir_bounded("/owned", entries, bytes)
            .await;
        assert!(matches!(result, Err(Error::Limited(_))));
        fixture.state.drained().await;
        fixture.fence().await;
        assert_eq!(fixture.state.closes.load(Ordering::SeqCst), 1);
        fixture.finish().await;
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.state.live.lock().unwrap().clear();
        self.state.dropped.store(true, Ordering::SeqCst);
        self.state.changed.notify_one();
    }
}
impl russh_sftp::server::Handler for Peer {
    type Error = StatusCode;
    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }
    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }
    async fn opendir(&mut self, id: u32, _: String) -> Result<Handle, Self::Error> {
        self.page = 0;
        let index = self.state.opens.fetch_add(1, Ordering::SeqCst);
        let handle = format!("owned-{index}");
        assert!(self.state.live.lock().unwrap().insert(handle.clone()));
        self.state.gate(1).await;
        Ok(Handle { id, handle })
    }
    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        assert!(self.state.live.lock().unwrap().contains(&handle));
        self.state.gate(2).await;
        if self.state.deny_read.load(Ordering::SeqCst) {
            return Err(StatusCode::PermissionDenied);
        }
        let configured_page = self
            .state
            .pages
            .lock()
            .unwrap()
            .as_ref()
            .map(|pages| pages.get(self.page).cloned());
        if let Some(files) = configured_page {
            self.page += 1;
            return files.map(|files| Name { id, files }).ok_or(StatusCode::Eof);
        }
        let names = match self.page {
            0 => vec![".", "..", "first", "second"],
            1 => vec!["third"],
            _ => return Err(StatusCode::Eof),
        };
        self.page += 1;
        Ok(Name {
            id,
            files: names
                .into_iter()
                .map(|n| File::new(n, FileAttributes::dummy()))
                .collect(),
        })
    }
    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.state.closes.fetch_add(1, Ordering::SeqCst);
        self.state.gate(3).await;
        if self.state.deny_close.load(Ordering::SeqCst) {
            return Err(StatusCode::PermissionDenied);
        }
        assert!(
            self.state.live.lock().unwrap().remove(&handle),
            "duplicate or unknown close"
        );
        self.state.changed.notify_one();
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en".into(),
        })
    }
    async fn stat(&mut self, id: u32, _: String) -> Result<Attrs, Self::Error> {
        Ok(Attrs {
            id,
            attrs: FileAttributes::dummy(),
        })
    }
}

struct Fixture {
    client: SftpSession,
    state: Arc<State>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let (client, server) = tokio::io::duplex(8192);
        let state = Arc::new(State::default());
        let peer = Peer {
            state: state.clone(),
            page: 0,
        };
        let server = tokio::spawn(russh_sftp::server::run(server, peer));
        Self {
            client: SftpSession::new(client).await.unwrap(),
            state,
            server,
        }
    }
    async fn fence(&self) {
        self.client.metadata("/fence").await.unwrap();
        tokio::task::yield_now().await;
        self.client.metadata("/fence").await.unwrap();
    }
    async fn finish(self) {
        self.client.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), self.server)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let changed = self.state.changed.notified();
                if self.state.dropped.load(Ordering::SeqCst) {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("inner SFTP server must stop");
        assert!(self.state.live.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn normal_listing_closes_once_and_preserves_packet_order() {
    let fixture = Fixture::new().await;
    let names: Vec<_> = fixture
        .client
        .read_dir("/owned")
        .await
        .unwrap()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names, ["third", "first", "second"]);
    assert_eq!(fixture.state.closes.load(Ordering::SeqCst), 1);
    assert!(fixture.state.live.lock().unwrap().is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn read_errors_release_every_directory_and_keep_client_usable() {
    let fixture = Fixture::new().await;
    fixture.state.deny_read.store(true, Ordering::SeqCst);
    for _ in 0..100 {
        let result = fixture.client.read_dir("/owned").await;
        assert!(
            matches!(result, Err(Error::Status(status)) if status.status_code == StatusCode::PermissionDenied)
        );
        fixture.state.drained().await;
    }
    fixture.fence().await;
    assert_eq!(fixture.state.opens.load(Ordering::SeqCst), 100);
    assert_eq!(fixture.state.closes.load(Ordering::SeqCst), 100);
    fixture.state.deny_read.store(false, Ordering::SeqCst);
    assert_eq!(fixture.client.read_dir("/owned").await.unwrap().count(), 3);
    fixture.finish().await;
}

#[tokio::test]
async fn cancelled_open_read_and_close_each_release_once() {
    for stage in 1..=3 {
        let fixture = Fixture::new().await;
        fixture.state.gate.store(stage, Ordering::SeqCst);
        let mut operation = Box::pin(fixture.client.read_dir("/owned"));
        tokio::select! {
            entered = tokio::time::timeout(Duration::from_secs(2), fixture.state.entered.notified()) => entered.expect("stage reached"),
            _ = &mut operation => panic!("operation escaped gate"),
        }
        drop(operation);
        fixture.state.resume.notify_one();
        fixture.state.drained().await;
        fixture.fence().await;
        assert_eq!(fixture.state.opens.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.closes.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.client.read_dir("/next").await.unwrap().count(), 3);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn close_error_is_preserved_without_a_duplicate_close() {
    let fixture = Fixture::new().await;
    fixture.state.deny_close.store(true, Ordering::SeqCst);
    let result = fixture.client.read_dir("/owned").await;
    assert!(
        matches!(result, Err(Error::Status(status)) if status.status_code == StatusCode::PermissionDenied)
    );
    fixture.fence().await;
    assert_eq!(fixture.state.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.state.live.lock().unwrap().len(),
        1,
        "peer refused close"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn listing_accumulation_preserves_all_packet_layouts_and_wire_metadata() {
    let names = [
        "Alpha",
        "alpha",
        "İ",
        "i\u{307}",
        "中文😀",
        ".",
        "..",
        "",
        "a\nb",
        "same",
        "same",
    ];
    for count in [0, 1, 7, 516, 4096] {
        for page_size in [1, 2, 31, 100, 257] {
            let fixture = Fixture::new().await;
            let files: Vec<_> = (0..count)
                .map(|i| {
                    File::new(
                        names[i % names.len()],
                        FileAttributes {
                            size: Some(u64::MAX - i as u64),
                            uid: Some(i as u32),
                            gid: Some(5000 - i as u32),
                            permissions: Some(match i % 3 {
                                0 => 0o040755,
                                1 => 0o100644,
                                _ => 0o120777,
                            }),
                            atime: Some(1000 + i as u32),
                            mtime: Some(5000 + i as u32),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            let mut pages = vec![Vec::new(), vec![File::dummy("."), File::dummy("..")]];
            pages.extend(files.chunks(page_size).map(|page| page.to_vec()));
            pages.insert(pages.len() / 2, Vec::new());
            pages.push(Vec::new());
            let expected: Vec<_> = pages.iter().rev().flat_map(|page|page.iter())
                .filter(|file|file.filename!="." && file.filename!="..")
                .map(|file|serde_json::json!({"name":file.filename,"path":format!("/owned/{}",file.filename),"metadata":file.attrs}))
                .collect();
            *fixture.state.pages.lock().unwrap() = Some(pages);
            let actual: Vec<_> = fixture.client.read_dir("/owned").await.unwrap()
                .map(|entry|serde_json::json!({"name":entry.file_name(),"path":entry.path(),"metadata":entry.metadata()}))
                .collect();
            assert_eq!(actual, expected, "{count} entries / {page_size} per packet");
            assert_eq!(fixture.state.closes.load(Ordering::SeqCst), 1);
            assert!(fixture.state.live.lock().unwrap().is_empty());
            fixture.finish().await;
        }
    }
}
