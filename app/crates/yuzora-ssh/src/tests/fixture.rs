//! An SSH server in the test process (russh's server side): password,
//! public key and keyboard-interactive logins, OpenSSH's own `sftp-server`
//! for the SFTP subsystem (rooted in a temporary folder), and direct-tcpip
//! for tunnels and ProxyJump. Keys come from `ssh-keygen` into the test's
//! temporary world; nothing touches `~/.ssh`.

use base64::Engine;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub const USER: &str = "fixture";
pub const PASSWORD: &str = "correct horse";
pub const CODE: &str = "424242";

/// OpenSSH's SFTP server on this machine, if there is one.
pub fn sftp_server() -> Option<PathBuf> {
    ["/usr/libexec/sftp-server", "/usr/lib/openssh/sftp-server", "/usr/libexec/openssh/sftp-server", "/usr/lib/ssh/sftp-server"].iter().map(PathBuf::from).find(|p| p.is_file())
}

/// `ssh-keygen -t ed25519` into `path` (with a passphrase when given).
pub fn keygen(path: &Path, passphrase: &str) {
    let _ = std::fs::remove_file(path);
    let status = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", passphrase, "-C", "yuzora-test", "-f"]).arg(path).stdout(Stdio::null()).status().expect("ssh-keygen runs");
    assert!(status.success());
}

pub fn public_b64(key: &PublicKey) -> String {
    base64::engine::general_purpose::STANDARD.encode(key.to_bytes().unwrap())
}

#[derive(Clone, Default)]
pub struct Options {
    /// Ask keyboard-interactive (password + code) instead of accepting passwords.
    pub keyboard: bool,
    pub authorized: Vec<PublicKey>,
}

pub struct Fixture {
    pub port: u16,
    pub host_key: PublicKey,
    pub has_sftp: bool,
    _server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self._server.abort();
    }
}

struct Shared {
    options: Options,
    sftp: Option<PathBuf>,
    root: PathBuf,
}

#[derive(Clone)]
struct FixtureServer(Arc<Shared>);

struct Conn {
    shared: Arc<Shared>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl server::Server for FixtureServer {
    type Handler = Conn;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Conn {
        Conn { shared: self.0.clone(), channels: HashMap::new() }
    }
}

impl server::Handler for Conn {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if !self.shared.options.keyboard && user == USER && password == PASSWORD { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey_offered(&mut self, _user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(if self.shared.options.authorized.contains(key) { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(if user == USER && self.shared.options.authorized.contains(key) { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_keyboard_interactive<'a>(&'a mut self, user: &str, _sub: &str, response: Option<server::Response<'a>>) -> Result<Auth, Self::Error> {
        if !self.shared.options.keyboard || user != USER {
            return Ok(Auth::reject());
        }
        Ok(match response {
            None => Auth::Partial { name: "fixture".into(), instructions: "".into(), prompts: vec![("Password:".into(), false), ("Verification code:".into(), true)].into() },
            Some(r) => {
                let answers: Vec<String> = r.map(|b| String::from_utf8_lossy(&b).into_owned()).collect();
                if answers == [PASSWORD, CODE] { Auth::Accept } else { Auth::reject() }
            }
        })
    }

    async fn channel_open_session(&mut self, channel: Channel<Msg>, reply: server::ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(&mut self, id: ChannelId, name: &str, session: &mut Session) -> Result<(), Self::Error> {
        let (Some(exe), Some(channel), "sftp") = (self.shared.sftp.clone(), self.channels.remove(&id), name) else {
            session.channel_failure(id)?;
            return Ok(());
        };
        let mut child = tokio::process::Command::new(exe).arg("-d").arg(&self.shared.root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn()?;
        let (mut input, mut output) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        session.channel_success(id)?;
        tokio::spawn(async move {
            let (mut read, mut write) = tokio::io::split(channel.into_stream());
            let inbound = async {
                let _ = tokio::io::copy(&mut read, &mut input).await;
                let _ = input.shutdown().await;
            };
            let outbound = async {
                let _ = tokio::io::copy(&mut output, &mut write).await;
                let _ = write.shutdown().await;
            };
            tokio::join!(inbound, outbound);
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        });
        Ok(())
    }

    async fn channel_open_direct_tcpip(&mut self, channel: Channel<Msg>, host: &str, port: u32, _oa: &str, _op: u32, reply: server::ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        match tokio::net::TcpStream::connect((host.to_owned(), port as u16)).await {
            Ok(mut tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
                });
            }
            Err(_) => reply.reject(russh::ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }
}

impl Fixture {
    /// Starts a server; its SFTP root is `root`.
    pub async fn start(dir: &Path, root: &Path, options: Options) -> Fixture {
        std::fs::create_dir_all(root).unwrap();
        let key_path = dir.join("host_ed25519");
        keygen(&key_path, "");
        let key: PrivateKey = russh::keys::load_secret_key(&key_path, None).unwrap();
        let host_key = key.public_key().clone();
        let config = Arc::new(server::Config {
            keys: vec![key],
            methods: MethodSet::from(&[MethodKind::Password, MethodKind::PublicKey, MethodKind::KeyboardInteractive][..]),
            auth_rejection_time: Duration::from_millis(5),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let sftp = sftp_server();
        let has_sftp = sftp.is_some();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut server = FixtureServer(Arc::new(Shared { options, sftp, root: root.to_path_buf() }));
        let task = tokio::spawn(async move {
            let _ = server.run_on_socket(config, &listener).await;
        });
        Fixture { port, host_key, has_sftp, _server: task }
    }

    /// `Host <alias>` lines reaching this server as the fixture user.
    pub fn config(&self, alias: &str, extra: &str) -> String {
        format!("Host {alias}\n  HostName 127.0.0.1\n  Port {}\n  User {USER}\n{extra}", self.port)
    }

    /// Writes the server's key into a known_hosts file.
    pub fn trust_in(&self, known_hosts: &Path) {
        crate::known_hosts::trust(known_hosts, "127.0.0.1", self.port, self.host_key.algorithm().as_str(), &public_b64(&self.host_key)).unwrap();
    }
}
