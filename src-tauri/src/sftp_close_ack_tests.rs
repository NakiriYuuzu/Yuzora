//! Owned SFTP CLOSE reply accounting fixtures; never starts an external process.
use russh_sftp::client::{error::Error, rawsession::Limits, RawSftpSession};
use russh_sftp::protocol::{Attrs, FileAttributes, Handle, OpenFlags, Status, StatusCode, Version};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
struct State {
    live: Mutex<HashSet<String>>,
    opens: AtomicUsize,
    closes: AtomicUsize,
    gate_close: AtomicBool,
    deny_close: AtomicBool,
    entered: Notify,
    resume: Notify,
    dropped: AtomicBool,
    changed: Notify,
}
struct Peer(Arc<State>);
impl Drop for Peer {
    fn drop(&mut self) {
        self.0.live.lock().unwrap().clear();
        self.0.dropped.store(true, Ordering::SeqCst);
        self.0.changed.notify_one();
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
    async fn open(
        &mut self,
        id: u32,
        _: String,
        _: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let n = self.0.opens.fetch_add(1, Ordering::SeqCst);
        let handle = format!("owned-{n}");
        assert!(self.0.live.lock().unwrap().insert(handle.clone()));
        Ok(Handle { id, handle })
    }
    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        self.open(id, path, OpenFlags::READ, FileAttributes::empty())
            .await
    }
    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.0.closes.fetch_add(1, Ordering::SeqCst);
        if self.0.gate_close.swap(false, Ordering::SeqCst) {
            self.0.entered.notify_one();
            self.0.resume.notified().await;
        }
        if self.0.deny_close.swap(false, Ordering::SeqCst) {
            return Err(StatusCode::PermissionDenied);
        }
        if !self.0.live.lock().unwrap().remove(&handle) {
            return Err(StatusCode::NoSuchFile);
        }
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
    raw: RawSftpSession,
    state: Arc<State>,
}
impl Fixture {
    async fn new(limit: u64) -> Self {
        let (client, server) = tokio::io::duplex(8192);
        let state = Arc::new(State::default());
        russh_sftp::server::run(server, Peer(state.clone())).await;
        let mut raw = RawSftpSession::new(client);
        raw.init().await.unwrap();
        raw.set_limits(Limits {
            open_handles: Some(limit),
            ..Default::default()
        });
        Self { raw, state }
    }
    async fn open(&self) -> Result<Handle, Error> {
        self.raw
            .open("/owned", OpenFlags::READ, FileAttributes::empty())
            .await
    }
    async fn fence(&self) {
        self.raw.stat("/fence").await.unwrap();
        tokio::task::yield_now().await;
        self.raw.stat("/fence").await.unwrap();
    }
    async fn abandoned_close(&self, handle: String, mode: &str) {
        if mode == "delivered" {
            let mut close = Box::pin(self.raw.close(handle));
            std::future::poll_fn(|cx| {
                assert!(close.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            self.fence().await;
            drop(close);
            self.fence().await;
            return;
        }
        self.state.gate_close.store(true, Ordering::SeqCst);
        if mode == "timeout" {
            self.raw.set_timeout(0);
            assert!(matches!(self.raw.close(handle).await, Err(Error::Timeout)));
            tokio::time::timeout(Duration::from_secs(2), self.state.entered.notified())
                .await
                .unwrap();
            self.raw.set_timeout(5);
        } else {
            assert_eq!(mode, "cancelled");
            let mut close = Box::pin(self.raw.close(handle));
            tokio::select! {v=tokio::time::timeout(Duration::from_secs(2),self.state.entered.notified())=>v.unwrap(),v=&mut close=>panic!("CLOSE escaped gate {v:?}")};
            drop(close);
        }
        self.state.resume.notify_one();
        self.fence().await;
    }
    async fn finish(self) {
        assert!(self.state.live.lock().unwrap().is_empty());
        self.raw.close_session().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let done = self.state.changed.notified();
                if self.state.dropped.load(Ordering::SeqCst) {
                    break;
                }
                done.await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn late_close_success_restores_only_its_own_handle_capacity() {
    for mode in ["timeout", "cancelled", "delivered"] {
        let f = Fixture::new(2).await;
        let active = f.open().await.unwrap();
        for _ in 0..100 {
            let other = f.open().await.unwrap();
            f.abandoned_close(other.handle, mode).await;
            let next = f.open().await.unwrap();
            assert!(
                matches!(f.open().await, Err(Error::Limited(_))),
                "sentinel budget must remain charged"
            );
            f.raw.close(next.handle).await.unwrap();
            assert_eq!(f.state.live.lock().unwrap().len(), 1);
            assert!(f.state.live.lock().unwrap().contains(&active.handle));
        }
        f.raw.close(active.handle).await.unwrap();
        assert_eq!(
            f.state.opens.load(Ordering::SeqCst),
            f.state.closes.load(Ordering::SeqCst)
        );
        f.finish().await;
    }
}
#[tokio::test]
async fn late_rejected_close_preserves_capacity_until_a_successful_retry() {
    for mode in ["timeout", "cancelled", "delivered"] {
        let f = Fixture::new(2).await;
        let active = f.open().await.unwrap();
        let other = f.open().await.unwrap();
        f.state.deny_close.store(true, Ordering::SeqCst);
        f.abandoned_close(other.handle.clone(), mode).await;
        assert!(matches!(f.open().await, Err(Error::Limited(_))));
        assert_eq!(f.state.live.lock().unwrap().len(), 2);
        f.raw.close(other.handle).await.unwrap();
        let next = f.open().await.unwrap();
        assert!(matches!(f.open().await, Err(Error::Limited(_))));
        f.raw.close(next.handle).await.unwrap();
        f.raw.close(active.handle).await.unwrap();
        f.finish().await;
    }
}

#[tokio::test]
async fn nonzero_timeout_keeps_the_deadline_and_accepts_late_success() {
    for _ in 0..5 {
        let f = Fixture::new(2).await;
        let active = f.open().await.unwrap();
        let other = f.open().await.unwrap();
        f.state.gate_close.store(true, Ordering::SeqCst);
        f.raw.set_timeout(1);
        let started = std::time::Instant::now();
        assert!(matches!(
            f.raw.close(other.handle).await,
            Err(Error::Timeout)
        ));
        assert!(started.elapsed() >= Duration::from_secs(1));
        tokio::time::timeout(Duration::from_secs(2), f.state.entered.notified())
            .await
            .unwrap();
        assert!(
            matches!(f.open().await, Err(Error::Limited(_))),
            "no acknowledgement yet"
        );
        f.raw.set_timeout(5);
        f.state.resume.notify_one();
        f.fence().await;
        let next = f.open().await.unwrap();
        assert!(matches!(f.open().await, Err(Error::Limited(_))));
        f.raw.close(next.handle).await.unwrap();
        f.raw.close(active.handle).await.unwrap();
        f.finish().await;
    }
}

#[tokio::test]
async fn missing_close_ack_keeps_capacity_and_transport_shutdown_releases_tasks() {
    for _ in 0..100 {
        let original_tasks = tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks();
        let (client, mut proxy_client) = tokio::io::duplex(8192);
        let (mut proxy_server, server) = tokio::io::duplex(8192);
        let proxy = tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut proxy_client, &mut proxy_server).await;
        });
        let state = Arc::new(State::default());
        russh_sftp::server::run(server, Peer(state.clone())).await;
        let mut raw = RawSftpSession::new(client);
        raw.init().await.unwrap();
        raw.set_limits(Limits {
            open_handles: Some(1),
            ..Default::default()
        });
        let f = Fixture { raw, state };
        let h = f.open().await.unwrap();
        f.state.gate_close.store(true, Ordering::SeqCst);
        let mut close = Box::pin(f.raw.close(h.handle));
        tokio::select! {
            reached = tokio::time::timeout(Duration::from_secs(2), f.state.entered.notified()) => reached.unwrap(),
            value = &mut close => panic!("CLOSE escaped gate {value:?}"),
        }
        drop(close);
        assert!(matches!(f.open().await, Err(Error::Limited(_))));
        proxy.abort();
        assert!(proxy.await.unwrap_err().is_cancelled());
        f.state.resume.notify_one();
        f.raw.close_session().unwrap();
        let state = f.state.clone();
        drop(f);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.dropped.load(Ordering::SeqCst)
                    && tokio::runtime::Handle::current()
                        .metrics()
                        .num_alive_tasks()
                        == original_tasks
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(state.live.lock().unwrap().is_empty());
    }
}
