// Yuzora rejected-open reply regressions, 2026-10-03. Licensed under Apache-2.0.
//! Isolated rejection probe: ephemeral key and owned loopback listener only.
use super::*;
use crate::server::{self, Auth, Server};
use std::sync::atomic::{AtomicUsize, Ordering};

struct RejectingServer;
impl server::Server for RejectingServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Self
    }
}

impl server::Handler for RejectingServer {
    type Error = Error;
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Error> {
        Ok(
            if user == "fixture" && password == "owned-rejected-channel-probe" {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }
    // The default channel-open handler drops its reply handle and rejects.
}

struct ProbeClient {
    expected_key: ssh_key::PublicKey,
    failures: Arc<AtomicUsize>,
}

impl Handler for ProbeClient {
    type Error = Error;
    async fn check_server_key(&mut self, key: &ssh_key::PublicKey) -> Result<bool, Error> {
        Ok(key == &self.expected_key)
    }
    async fn channel_open_failure(
        &mut self,
        _: ChannelId,
        reason: ChannelOpenFailure,
        _: &str,
        _: &str,
        _: &mut Session,
    ) -> Result<(), Error> {
        assert_eq!(reason, ChannelOpenFailure::AdministrativelyProhibited);
        self.failures.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn owned_client() -> (
    Handle<ProbeClient>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let key = ssh_key::PrivateKey::random(&mut rand::rng(), ssh_key::Algorithm::Ed25519).unwrap();
    let expected_key = key.public_key().clone();
    let server_config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });
    let mut server = RejectingServer;
    let task = tokio::spawn(async move {
        let _ = server.run_on_socket(server_config, &listener).await;
    });
    let failures = Arc::new(AtomicUsize::new(0));
    let mut handle = connect(
        Arc::new(Config::default()),
        endpoint,
        ProbeClient {
            expected_key,
            failures: failures.clone(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        handle
            .authenticate_password("fixture", "owned-rejected-channel-probe")
            .await
            .unwrap(),
        AuthResult::Success
    ));
    assert_eq!(handle.receiver.len(), 0);
    (handle, failures, task)
}

async fn reject_once(handle: &Handle<ProbeClient>) {
    assert!(matches!(
        handle.channel_open_session().await,
        Err(Error::ChannelOpenFailure(
            ChannelOpenFailure::AdministrativelyProhibited
        ))
    ));
}

async fn stop_owned(handle: Handle<ProbeClient>, task: tokio::task::JoinHandle<()>) {
    handle
        .disconnect(Disconnect::ByApplication, "owned fixture finished", "en")
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("owned client disconnect deadline");
    task.abort();
    let _ = task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_channels_do_not_accumulate_auth_replies() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (handle, failures, task) = owned_client().await;
        for _ in 0..32 {
            reject_once(&handle).await;
        }
        handle.send_ping().await.unwrap();
        assert!(!handle.is_closed());
        assert_eq!(failures.load(Ordering::SeqCst), 32);
        let queued = handle.receiver.len();
        stop_owned(handle, task).await;
        assert_eq!(
            queued, 0,
            "rejected opens must not retain authentication replies"
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual owned SSH rejected-open queue preflight"]
async fn performance_rejected_open_reply_retention() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (handle, failures, task) = owned_client().await;
        let mut samples = Vec::with_capacity(11);
        samples.push((0, handle.receiver.len(), 0));
        for cycle in 1..=1000 {
            reject_once(&handle).await;
            if cycle % 100 == 0 {
                handle.send_ping().await.unwrap();
                assert!(!handle.is_closed());
                assert_eq!(failures.load(Ordering::SeqCst), cycle);
                samples.push((cycle, handle.receiver.len(), failures.load(Ordering::SeqCst)));
            }
        }
        println!(
            "SSH_REJECT_QUEUE {{\"cycles\":1000,\"replySizeBytes\":{},\"samples\":{:?},\"queuedReplies\":{},\"handlerFailures\":{}}}",
            std::mem::size_of::<Reply>(),
            samples.iter().map(|&(a,b,c)| [a,b,c]).collect::<Vec<_>>(),
            handle.receiver.len(),
            failures.load(Ordering::SeqCst),
        );
        stop_owned(handle, task).await;
        println!("SSH_REJECT_QUEUE_END {{\"ownedServerStopped\":true}}");
    })
    .await
    .unwrap();
}
