//! Raw HANDLE reply ownership across timeouts and dropped request receivers.
use russh_sftp::client::{error::Error, rawsession::Limits, RawSftpSession};
use russh_sftp::protocol::{
    Attrs, Data, FileAttributes, Handle, OpenFlags, Status, StatusCode, Version,
};
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
    gate: AtomicBool,
    entered: Notify,
    resume: Notify,
    dropped: AtomicBool,
    changed: Notify,
}
struct Peer(Arc<State>);
impl Peer {
    async fn gate(&self) {
        if self.0.gate.swap(false, Ordering::SeqCst) {
            self.0.entered.notify_one();
            self.0.resume.notified().await;
        }
    }
    async fn acquire(&self, id: u32, path: String) -> Result<Handle, StatusCode> {
        self.gate().await;
        if path == "/missing" {
            return Err(StatusCode::NoSuchFile);
        }
        let n = self.0.opens.fetch_add(1, Ordering::SeqCst);
        let handle = format!("owned-{n}");
        assert!(self.0.live.lock().unwrap().insert(handle.clone()));
        Ok(Handle { id, handle })
    }
}
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
        path: String,
        _: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        self.acquire(id, path).await
    }
    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        self.acquire(id, path).await
    }
    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.0.closes.fetch_add(1, Ordering::SeqCst);
        assert!(
            self.0.live.lock().unwrap().remove(&handle),
            "duplicate or unrelated CLOSE"
        );
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en".into(),
        })
    }
    async fn stat(&mut self, id: u32, _: String) -> Result<Attrs, Self::Error> {
        self.gate().await;
        Ok(Attrs {
            id,
            attrs: FileAttributes::dummy(),
        })
    }
    async fn read(&mut self, id: u32, handle: String, _: u64, _: u32) -> Result<Data, Self::Error> {
        assert!(self.0.live.lock().unwrap().contains(&handle));
        self.gate().await;
        Ok(Data {
            id,
            data: vec![1, 2, 3],
        })
    }
}
struct Fixture {
    raw: RawSftpSession,
    state: Arc<State>,
}
impl Fixture {
    async fn new() -> Self {
        let (client, server) = tokio::io::duplex(8192);
        let state = Arc::new(State::default());
        russh_sftp::server::run(server, Peer(state.clone())).await;
        let mut raw = RawSftpSession::new(client);
        raw.init().await.unwrap();
        raw.set_limits(Limits {
            open_handles: Some(2),
            ..Default::default()
        });
        Self { raw, state }
    }
    async fn acquire(&self, directory: bool) -> Result<Handle, Error> {
        if directory {
            self.raw.opendir("/owned").await
        } else {
            self.raw
                .open("/owned", OpenFlags::READ, FileAttributes::empty())
                .await
        }
    }
    async fn fence(&self) {
        self.raw.stat("/fence").await.unwrap();
        tokio::task::yield_now().await;
        self.raw.stat("/fence").await.unwrap();
    }
    async fn assert_active_and_budget(&self, active: &Handle) {
        {
            let live = self.state.live.lock().unwrap();
            assert_eq!(live.len(), 1);
            assert!(live.contains(&active.handle));
        }
        let other = self.acquire(false).await.unwrap();
        assert!(
            matches!(self.acquire(false).await, Err(Error::Limited(_))),
            "orphan CLOSE must not decrement another handle's local count"
        );
        self.raw.close(other.handle).await.unwrap();
    }
    async fn finish(self) {
        assert!(self.state.live.lock().unwrap().is_empty());
        self.raw.close_session().unwrap();
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
    }
}

#[tokio::test]
async fn timed_out_file_and_directory_opens_close_late_handles_without_spending_live_budget() {
    for directory in [false, true] {
        let f = Fixture::new().await;
        let active = f.acquire(false).await.unwrap();
        for _ in 0..100 {
            f.state.gate.store(true, Ordering::SeqCst);
            f.raw.set_timeout(0);
            assert!(matches!(f.acquire(directory).await, Err(Error::Timeout)));
            tokio::time::timeout(Duration::from_secs(2), f.state.entered.notified())
                .await
                .unwrap();
            f.raw.set_timeout(5);
            f.state.resume.notify_one();
            f.fence().await;
            f.assert_active_and_budget(&active).await;
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
async fn cancelled_file_and_directory_opens_close_late_handles_once() {
    for directory in [false, true] {
        let f = Fixture::new().await;
        let active = f.acquire(false).await.unwrap();
        for _ in 0..100 {
            f.state.gate.store(true, Ordering::SeqCst);
            let mut opening = Box::pin(f.acquire(directory));
            tokio::select! {
                done=tokio::time::timeout(Duration::from_secs(2),f.state.entered.notified())=>done.unwrap(),
                value=&mut opening=>panic!("OPEN escaped gate {value:?}"),
            }
            drop(opening);
            f.state.resume.notify_one();
            f.fence().await;
            f.assert_active_and_budget(&active).await;
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
async fn unpolled_delivered_handle_is_closed_when_its_request_is_dropped() {
    for directory in [false, true] {
        let f = Fixture::new().await;
        let active = f.acquire(false).await.unwrap();
        for _ in 0..100 {
            let mut opening = Box::pin(f.acquire(directory));
            std::future::poll_fn(|cx| {
                assert!(opening.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            f.fence().await;
            assert_eq!(f.state.live.lock().unwrap().len(), 2);
            drop(opening);
            f.fence().await;
            f.assert_active_and_budget(&active).await;
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
async fn late_errors_and_cancelled_data_leave_live_handles_and_session_usable() {
    let f = Fixture::new().await;
    let active = f.acquire(false).await.unwrap();
    for _ in 0..100 {
        f.state.gate.store(true, Ordering::SeqCst);
        f.raw.set_timeout(0);
        assert!(matches!(
            f.raw
                .open("/missing", OpenFlags::READ, FileAttributes::empty())
                .await,
            Err(Error::Timeout)
        ));
        tokio::time::timeout(Duration::from_secs(2), f.state.entered.notified())
            .await
            .unwrap();
        f.raw.set_timeout(5);
        f.state.resume.notify_one();
        f.fence().await;
        let mut reading = Box::pin(f.raw.read(active.handle.clone(), 0, 3));
        std::future::poll_fn(|cx| {
            assert!(reading.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        f.fence().await;
        drop(reading);
        f.fence().await;
        assert_eq!(f.state.closes.load(Ordering::SeqCst), 0);
        assert!(f.state.live.lock().unwrap().contains(&active.handle));
        assert_eq!(
            f.raw.read(active.handle.clone(), 0, 3).await.unwrap().data,
            vec![1, 2, 3]
        );
    }
    f.raw.close(active.handle).await.unwrap();
    f.finish().await;
}
