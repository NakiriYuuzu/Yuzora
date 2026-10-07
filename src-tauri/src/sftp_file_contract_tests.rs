//! File close ownership and accounting over an in-memory SFTP transport.
use russh_sftp::client::{error::Error, SftpSession};
use russh_sftp::protocol::{
    Attrs, ExtendedReply, FileAttributes, Handle, OpenFlags, Packet, Status, StatusCode, Version,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
struct State {
    live: Mutex<HashSet<String>>,
    events: Mutex<Vec<&'static str>>,
    opens: AtomicUsize,
    closes: AtomicUsize,
    deny_close: AtomicBool,
    gate_close: AtomicBool,
    empty_on_stat: AtomicBool,
    dropped: AtomicBool,
    entered: Notify,
    resume: Notify,
    changed: Notify,
}
struct Peer {
    state: Arc<State>,
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
        let mut v = Version::new();
        v.extensions
            .insert(russh_sftp::extensions::LIMITS.into(), "1".into());
        Ok(v)
    }
    async fn extended(
        &mut self,
        id: u32,
        request: String,
        _: Vec<u8>,
    ) -> Result<Packet, Self::Error> {
        assert_eq!(request, russh_sftp::extensions::LIMITS);
        let limits = russh_sftp::extensions::LimitsExtension {
            max_packet_len: 262144,
            max_read_len: 32768,
            max_write_len: 32768,
            max_open_handles: 2,
        };
        Ok(ExtendedReply {
            id,
            data: russh_sftp::ser::to_bytes(&limits).unwrap().to_vec(),
        }
        .into())
    }
    async fn open(
        &mut self,
        id: u32,
        _: String,
        _: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        self.state.events.lock().unwrap().push("open");
        let n = self.state.opens.fetch_add(1, Ordering::SeqCst);
        let handle = format!("owned-{n}");
        assert!(self.state.live.lock().unwrap().insert(handle.clone()));
        Ok(Handle { id, handle })
    }
    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.state.events.lock().unwrap().push("close");
        self.state.closes.fetch_add(1, Ordering::SeqCst);
        self.state.entered.notify_one();
        if self.state.gate_close.swap(false, Ordering::SeqCst) {
            self.state.resume.notified().await;
        }
        if self.state.deny_close.load(Ordering::SeqCst) {
            return Err(StatusCode::PermissionDenied);
        }
        assert!(
            self.state.live.lock().unwrap().remove(&handle),
            "duplicate close"
        );
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en".into(),
        })
    }
    async fn stat(&mut self, id: u32, _: String) -> Result<Attrs, Self::Error> {
        self.state.events.lock().unwrap().push("stat");
        if self.state.empty_on_stat.load(Ordering::SeqCst) {
            assert!(
                self.state.live.lock().unwrap().is_empty(),
                "CLOSE must precede later caller requests"
            );
        }
        Ok(Attrs {
            id,
            attrs: FileAttributes::dummy(),
        })
    }
}
struct Fixture {
    client: SftpSession,
    state: Arc<State>,
}
impl Fixture {
    async fn new() -> Self {
        let (client, server) = tokio::io::duplex(8192);
        let state = Arc::new(State::default());
        russh_sftp::server::run(
            server,
            Peer {
                state: state.clone(),
            },
        )
        .await;
        Self {
            client: SftpSession::new(client).await.unwrap(),
            state,
        }
    }
    async fn fence(&self) {
        self.client.metadata("/fence").await.unwrap();
        tokio::task::yield_now().await;
        self.client.metadata("/fence").await.unwrap();
    }
    async fn finish(self) {
        self.client.close().await.unwrap();
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
        .unwrap();
        assert!(self.state.live.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn dropped_files_return_capacity_and_queue_close_before_later_requests() {
    let f = Fixture::new().await;
    f.state.empty_on_stat.store(true, Ordering::SeqCst);
    for _ in 0..100 {
        let file = f.client.open("/owned").await.unwrap();
        f.state.events.lock().unwrap().clear();
        drop(file);
        f.fence().await;
        assert_eq!(&f.state.events.lock().unwrap()[..2], &["close", "stat"]);
    }
    assert_eq!(f.state.opens.load(Ordering::SeqCst), 100);
    assert_eq!(f.state.closes.load(Ordering::SeqCst), 100);
    f.finish().await;
}

#[tokio::test]
async fn cancelled_file_close_owns_pending_and_already_delivered_ack() {
    for queued_ack in [false, true] {
        let f = Fixture::new().await;
        for n in 1..=100 {
            let file = f.client.open("/owned").await.unwrap();
            f.state.gate_close.store(!queued_ack, Ordering::SeqCst);
            let mut closing = Box::pin(file.close());
            tokio::select! {biased;
                reached=tokio::time::timeout(Duration::from_secs(2),f.state.entered.notified())=>reached.expect("CLOSE reached peer"),
                value=&mut closing=>panic!("close escaped gate {value:?}"),
            }
            if queued_ack {
                f.fence().await;
            }
            drop(closing);
            if !queued_ack {
                f.state.resume.notify_one();
            }
            f.fence().await;
            assert_eq!(f.state.opens.load(Ordering::SeqCst), n);
            assert_eq!(f.state.closes.load(Ordering::SeqCst), n);
            assert!(f.state.live.lock().unwrap().is_empty());
        }
        f.finish().await;
    }
}

#[tokio::test]
async fn rejected_close_does_not_free_another_live_handle_budget() {
    let f = Fixture::new().await;
    let first = f.client.open("/first").await.unwrap();
    let second = f.client.open("/second").await.unwrap();
    f.state.deny_close.store(true, Ordering::SeqCst);
    drop(first);
    f.fence().await;
    assert!(matches!(
        f.client.open("/blocked").await,
        Err(Error::Limited(_))
    ));
    assert_eq!(f.state.live.lock().unwrap().len(), 2);
    f.state.deny_close.store(false, Ordering::SeqCst);
    second.close().await.unwrap();
    let third = f.client.open("/third").await.unwrap();
    third.close().await.unwrap();
    assert_eq!(f.state.live.lock().unwrap().len(), 1);
    f.finish().await;
}
