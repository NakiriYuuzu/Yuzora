//! Integration tests against an in-process SSH server (see fixture.rs), each
//! in its own temporary world: its own home and `.ssh/config`, its own
//! known_hosts and settings, an in-memory credential store, no ssh-agent.

#![cfg(unix)]

mod fixture;

use crate::error::ErrorKind;
use crate::prompt::{Answer, Prompt};
use crate::session::{Interaction, Manager};
use crate::sftp::Side;
use crate::testing::TestWorld;
use crate::transfer::{Direction, State, TransferSpec};
use crate::tunnel::TunnelSpec;
use fixture::{CODE, Fixture, Options, PASSWORD};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yuzora_core::secrets::{SecretKey, SecretStore};
use zeroize::Zeroizing;

type Asked = Arc<Mutex<Vec<&'static str>>>;

/// Answers every question the way `reply` says, and remembers which kinds came.
fn answer_with(m: &Arc<Manager>, reply: impl Fn(&Prompt) -> Answer + Send + 'static) -> Asked {
    let asked: Asked = Arc::default();
    let (m, seen) = (m.clone(), asked.clone());
    tokio::spawn(async move {
        loop {
            for p in m.prompts.list() {
                if m.prompts.claim(&p.id) {
                    seen.lock().unwrap().push(p.kind);
                    m.prompts.answer(&p.id, reply(&p));
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    asked
}

fn secret(s: &str, remember: bool) -> Answer {
    Answer::Secret { value: Zeroizing::new(s.to_owned()), remember }
}

/// Trusts host keys and gives the fixture's password.
fn trusting(p: &Prompt) -> Answer {
    match p.kind {
        "hostKey" | "hostKeyChanged" => Answer::Trust(true),
        _ => secret(PASSWORD, true),
    }
}

async fn wait_transfer(m: &Manager, id: u64) -> crate::transfer::TransferInfo {
    for _ in 0..2000 {
        let info = m.transfers.get(id).unwrap();
        if !matches!(info.state, State::Queued | State::Running) {
            return info;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("transfer {id} never finished");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_host_key_is_asked_once_then_known_and_the_password_kept() {
    let w = TestWorld::new("it-first-contact");
    let fx = Fixture::start(&w.root, &w.root.join("srv"), Options::default()).await;
    w.write_ssh_config(&fx.config("lab", ""));
    let m = Manager::new(w.context());

    // Nobody to ask: an unknown key is refused.
    let e = m.session("lab", Interaction::Never).await.err().unwrap();
    assert_eq!(e.kind, ErrorKind::HostKeyUnknown, "{e}");

    let asked = answer_with(&m, trusting);
    let s = m.session("lab", Interaction::Ask).await.unwrap();
    assert_eq!(*asked.lock().unwrap(), ["hostKey", "password"]);
    assert_eq!((s.method, s.verified.kind.as_str(), s.verified.newly_trusted), ("password", "ED25519", true));
    assert_eq!(s.verified.fingerprint, fx.host_key.fingerprint(russh::keys::HashAlg::Sha256).to_string());
    assert!(s.latency_ms().is_some());
    let recorded = std::fs::read_to_string(&w.context().known_hosts).unwrap();
    assert!(recorded.starts_with(&format!("[127.0.0.1]:{} ssh-ed25519 ", fx.port)), "{recorded}");
    assert_eq!(w.secrets.get(&SecretKey::new("ssh", "password:ssh:lab")).unwrap().as_deref(), Some(PASSWORD));
    let status = m.status().unwrap();
    assert_eq!(status.1, "done");
    assert!(status.0.contains("lab") && status.0.contains("ED25519"), "{}", status.0);

    // The same session is shared; after a disconnect nothing needs asking.
    assert!(Arc::ptr_eq(&s, &m.session("ssh:lab", Interaction::Never).await.unwrap()));
    m.disconnect("lab").await;
    let again = m.session("lab", Interaction::Never).await.unwrap();
    assert!(!again.verified.newly_trusted);
    assert_eq!(asked.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_host_key_is_refused_unless_the_person_accepts_it() {
    let w = TestWorld::new("it-changed-key");
    let fx = Fixture::start(&w.root, &w.root.join("srv"), Options::default()).await;
    w.write_ssh_config(&fx.config("lab", ""));
    let ctx = w.context();
    // A different key recorded for the same endpoint.
    let other = w.root.join("other");
    fixture::keygen(&other, "");
    let old = russh::keys::load_public_key(other.with_extension("pub")).unwrap();
    crate::known_hosts::trust(&ctx.known_hosts, "127.0.0.1", fx.port, "ssh-ed25519", &fixture::public_b64(&old)).unwrap();
    w.secrets.set(&SecretKey::new("ssh", "password:ssh:lab"), PASSWORD).unwrap();
    let m = Manager::new(ctx.clone());

    assert_eq!(m.session("lab", Interaction::Never).await.err().unwrap().kind, ErrorKind::HostKeyChanged);
    let asked = answer_with(&m, |p| if p.kind == "hostKeyChanged" { Answer::Trust(false) } else { secret(PASSWORD, false) });
    assert_eq!(m.session("lab", Interaction::Ask).await.err().unwrap().kind, ErrorKind::HostKeyRejected);
    assert_eq!(*asked.lock().unwrap(), ["hostKeyChanged"]);

    let m = Manager::new(ctx.clone());
    let asked = answer_with(&m, |p| {
        assert_eq!(p.previous.len(), 1);
        trusting(p)
    });
    let s = m.session("lab", Interaction::Ask).await.unwrap();
    assert!(s.verified.newly_trusted);
    assert_eq!(*asked.lock().unwrap(), ["hostKeyChanged"]);
    let lines = crate::known_hosts::read_file(&ctx.known_hosts);
    assert_eq!(lines.len(), 1, "the old key is replaced");
    assert_eq!(lines[0].key, fixture::public_b64(&fx.host_key));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_encrypted_key_asks_for_its_passphrase() {
    let w = TestWorld::new("it-passphrase");
    let key = w.root.join("id_test");
    fixture::keygen(&key, "s3cret phrase");
    let public = russh::keys::load_public_key(key.with_extension("pub")).unwrap();
    let fx = Fixture::start(&w.root, &w.root.join("srv"), Options { authorized: vec![public], ..Default::default() }).await;
    w.write_ssh_config(&fx.config("lab", &format!("  IdentityFile {}\n  PreferredAuthentications publickey\n", key.display())));
    fx.trust_in(&w.context().known_hosts);
    let m = Manager::new(w.context());

    // Without anyone to ask, the key is skipped and nothing else is allowed.
    assert_eq!(m.session("lab", Interaction::Never).await.err().unwrap().kind, ErrorKind::NeedsInput);

    let asked = answer_with(&m, |p| {
        assert_eq!(p.kind, "passphrase");
        // Wrong first, right second.
        if p.retry { secret("s3cret phrase", true) } else { secret("nope", false) }
    });
    let s = m.session("lab", Interaction::Ask).await.unwrap();
    assert_eq!(s.method, "publickey");
    assert_eq!(*asked.lock().unwrap(), ["passphrase", "passphrase"]);
    let stored = w.secrets.get(&crate::auth::passphrase_key(&key)).unwrap();
    assert_eq!(stored.as_deref(), Some("s3cret phrase"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyboard_interactive_asks_every_question() {
    let w = TestWorld::new("it-keyboard");
    let fx = Fixture::start(&w.root, &w.root.join("srv"), Options { keyboard: true, ..Default::default() }).await;
    w.write_ssh_config(&fx.config("lab", ""));
    fx.trust_in(&w.context().known_hosts);
    let m = Manager::new(w.context());
    let asked = answer_with(&m, |p| match p.kind {
        "keyboard" => {
            let texts: Vec<&str> = p.questions.iter().map(|q| q.text.as_str()).collect();
            assert_eq!(texts, ["Password:", "Verification code:"]);
            assert!(!p.questions[0].echo && p.questions[1].echo && !p.can_remember);
            Answer::Responses { values: vec![Zeroizing::new(PASSWORD.into()), Zeroizing::new(CODE.into())], remember: false }
        }
        _ => Answer::Cancel,
    });
    let s = m.session("lab", Interaction::Ask).await.unwrap();
    assert_eq!(s.method, "keyboard-interactive");
    assert_eq!(*asked.lock().unwrap(), ["keyboard"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keyboard_password_is_kept_only_once_the_host_takes_it() {
    let w = TestWorld::new("it-kbd-kept");
    let fx = Fixture::start(&w.root, &w.root.join("srv"), Options { keyboard_password: true, ..Default::default() }).await;
    w.write_ssh_config(&fx.config("lab", ""));
    fx.trust_in(&w.context().known_hosts);
    let key = SecretKey::new("ssh", "password:ssh:lab");
    let m = Manager::new(w.context());

    // Nobody to ask, nothing kept: no blank answers (a failed login on the account), just "needs input".
    let e = m.session("lab", Interaction::Never).await.err().unwrap();
    assert_eq!(e.kind, ErrorKind::NeedsInput, "{e}");
    assert_eq!(fx.answers.load(std::sync::atomic::Ordering::Relaxed), 0);

    // A mistyped password with "remember" is asked again and never kept.
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = n.clone();
    let asked = answer_with(&m, move |_| {
        let first = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0;
        let value = if first { "typo" } else { PASSWORD };
        Answer::Responses { values: vec![Zeroizing::new(value.into())], remember: first }
    });
    let s = m.session("lab", Interaction::Ask).await.unwrap();
    assert_eq!(s.method, "keyboard-interactive");
    assert_eq!(*asked.lock().unwrap(), ["keyboard", "keyboard"]);
    assert_eq!(w.secrets.get(&key).unwrap(), None);
    m.disconnect("lab").await;
    drop(asked);

    // A kept password the host no longer takes: asked again; the new one is kept once it works.
    w.secrets.set(&key, "an old password").unwrap();
    let m = Manager::new(w.context());
    let asked = answer_with(&m, |_| Answer::Responses { values: vec![Zeroizing::new(PASSWORD.into())], remember: true });
    m.session("lab", Interaction::Ask).await.unwrap();
    assert_eq!(*asked.lock().unwrap(), ["keyboard"]);
    assert_eq!(w.secrets.get(&key).unwrap().as_deref(), Some(PASSWORD));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_key_alias_is_looked_up_without_the_port() {
    let w = TestWorld::new("it-alias");
    let fx = Fixture::start(&w.root, &w.root.join("srv"), Options::default()).await;
    w.write_ssh_config(&fx.config("db", "  HostKeyAlias db.internal\n"));
    w.secrets.set(&SecretKey::new("ssh", "password:ssh:db"), PASSWORD).unwrap();
    let known = w.context().known_hosts;
    // OpenSSH records an alias as written, never as [alias]:port.
    let algorithm = fx.host_key.algorithm();
    crate::known_hosts::trust(&known, "db.internal", 22, algorithm.as_str(), &fixture::public_b64(&fx.host_key)).unwrap();
    let m = Manager::new(w.context());
    assert!(!m.session("db", Interaction::Never).await.unwrap().verified.newly_trusted);
    m.disconnect("db").await;

    // Another key under that alias is a changed key, not a first contact.
    let other = w.root.join("other_ed25519");
    fixture::keygen(&other, "");
    let other = russh::keys::load_secret_key(&other, None).unwrap();
    std::fs::remove_file(&known).unwrap();
    crate::known_hosts::trust(&known, "db.internal", 22, algorithm.as_str(), &fixture::public_b64(other.public_key())).unwrap();
    let e = m.session("db", Interaction::Never).await.err().unwrap();
    assert_eq!(e.kind, ErrorKind::HostKeyChanged, "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tunnel_forwards_local_connections_through_the_host() {
    let w = TestWorld::new("it-tunnel");
    let fx = Fixture::start(&w.root, &w.root.join("srv"), Options::default()).await;
    w.write_ssh_config(&fx.config("lab", ""));
    fx.trust_in(&w.context().known_hosts);
    w.secrets.set(&SecretKey::new("ssh", "password:ssh:lab"), PASSWORD).unwrap();
    // A service only the "host" can reach: an echo server.
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut wr) = s.split();
                let _ = tokio::io::copy(&mut r, &mut wr).await;
            });
        }
    });
    let m = Manager::new(w.context());
    let t = m.open_tunnel(TunnelSpec::new("lab", "127.0.0.1", echo_port)).await.unwrap();
    assert_ne!(t.local_port(), 0);
    for round in 0..2 {
        let mut c = tokio::net::TcpStream::connect(t.local_addr()).await.unwrap();
        let msg = format!("ping {round}");
        c.write_all(msg.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; msg.len()];
        c.read_exact(&mut buf).await.unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), msg);
    }
    let info = t.info().unwrap();
    assert_eq!((info.total, info.host.as_str(), info.remote_port), (2, "ssh:lab", echo_port));
    assert_eq!(m.tunnels.list().len(), 1);
    let addr = t.local_addr();
    t.close();
    assert!(m.tunnels.list().is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(tokio::net::TcpStream::connect(addr).await.is_err(), "the port is closed");

    // A refused far end fails that connection only.
    let t = m.open_tunnel(TunnelSpec::new("lab", "127.0.0.1", 1)).await.unwrap();
    let mut c = tokio::net::TcpStream::connect(t.local_addr()).await.unwrap();
    let mut buf = [0u8; 1];
    assert_eq!(c.read(&mut buf).await.unwrap_or(0), 0);
    assert!(t.is_open());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_jump_reaches_a_host_through_another() {
    let w = TestWorld::new("it-jump");
    let fx = Fixture::start(&w.root, &w.root.join("srv"), Options::default()).await;
    // "inner" is the same server, reached from "bastion" over direct-tcpip.
    w.write_ssh_config(&format!("{}{}", fx.config("bastion", ""), fx.config("inner", "  ProxyJump bastion\n")));
    fx.trust_in(&w.context().known_hosts);
    for host in ["ssh:bastion", "ssh:inner"] {
        w.secrets.set(&SecretKey::new("ssh", format!("password:{host}")), PASSWORD).unwrap();
    }
    let m = Manager::new(w.context());
    let s = m.session("inner", Interaction::Never).await.unwrap();
    assert_eq!(s.host_id, "ssh:inner");
    assert!(s.ping().await.is_ok());
}

/// A profile edited to reach another server is another host: it comes back under a new id.
/// The session to the first server ends, and nothing reaches the second through the old id:
/// an SFTP pane listing the first server, a transfer with a `.part` from it, a file opened
/// from it would otherwise go on working on the second.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_profile_that_now_reaches_another_server_is_another_host() {
    let w = TestWorld::new("it-repoint");
    let (first, second) = (w.root.join("first"), w.root.join("second"));
    let a = Fixture::start(&first, &first.join("srv"), Options::default()).await;
    let b = Fixture::start(&second, &second.join("srv"), Options::default()).await;
    let ctx = w.context();
    for fx in [&a, &b] {
        fx.trust_in(&ctx.known_hosts);
    }
    let profile = |id: &str, name: &str, port: u16| crate::hosts::Profile { id: id.into(), name: name.into(), host: "127.0.0.1".into(), port: Some(port), user: Some(fixture::USER.into()), ..Default::default() };
    let password = |id: &str| SecretKey::new("ssh", format!("password:profile:{id}"));
    let m = Manager::new(ctx.clone());
    let state = |key: &str| m.hosts_json().as_array().unwrap().iter().find(|h| h["id"] == key).map(|h| h["state"].clone());

    let id = m.save_profile(profile("", "box", a.port), Some(PASSWORD)).unwrap();
    let key = format!("profile:{id}");
    let to_a = m.session(&key, Interaction::Never).await.unwrap();
    assert!(to_a.target.ends_with(&format!(":{}", a.port)), "{}", to_a.target);
    let tunnel = m.open_tunnel(TunnelSpec::new(key.as_str(), "127.0.0.1", 1)).await.unwrap();

    // Another name is the same server: the id, the session and the tunnel stay.
    assert_eq!(m.save_profile(profile(&id, "renamed", a.port), None).unwrap(), id);
    assert!(Arc::ptr_eq(&m.session(&key, Interaction::Never).await.unwrap(), &to_a));
    assert!(tunnel.is_open());

    // Another port is another server.
    let moved = m.save_profile(profile(&id, "renamed", b.port), None).unwrap();
    assert_ne!(moved, id);
    assert!(!tunnel.is_open(), "a tunnel through the first server is still listening");
    assert_eq!(crate::hosts::profiles(&ctx).iter().map(|p| p.id.clone()).collect::<Vec<_>>(), [moved.clone()]);
    let failed = m.session(&key, Interaction::Never).await.err().map(|e| e.kind);
    assert_eq!(failed, Some(ErrorKind::UnknownHost), "the old id still reaches a server");
    assert!(m.open_session(&key).is_none());
    for _ in 0..100 {
        if to_a.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(to_a.is_closed(), "the connection to the first server was left open");

    let new_key = format!("profile:{moved}");
    assert_eq!(state(&new_key), Some("idle".into()), "the list says connected to a server nothing is connected to");
    assert_eq!(w.secrets.get(&password(&moved)).unwrap().as_deref(), Some(PASSWORD), "the saved password did not go with the profile");
    assert_eq!(w.secrets.get(&password(&id)).unwrap(), None);
    let to_b = m.session(&new_key, Interaction::Never).await.unwrap();
    assert!(to_b.target.ends_with(&format!(":{}", b.port)), "{}", to_b.target);
    assert!(to_b.ping().await.is_ok());
}

/// A credential store that can be told to refuse writes (a locked keychain, no Secret Service).
struct Refusing(Arc<yuzora_core::secrets::MemoryStore>, std::sync::atomic::AtomicBool);

impl SecretStore for Refusing {
    fn set(&self, key: &SecretKey, secret: &str) -> std::result::Result<(), yuzora_core::secrets::SecretError> {
        match self.1.load(std::sync::atomic::Ordering::Relaxed) {
            true => Err(yuzora_core::secrets::SecretError::Denied("refused".into())),
            false => self.0.set(key, secret),
        }
    }
    fn get(&self, key: &SecretKey) -> std::result::Result<Option<String>, yuzora_core::secrets::SecretError> {
        self.0.get(key)
    }
    fn delete(&self, key: &SecretKey) -> std::result::Result<(), yuzora_core::secrets::SecretError> {
        self.0.delete(key)
    }
}

/// The dialog stays open on a failed save, holding the id it opened with: saving again from it
/// has to be the same save, not one for a profile that has meanwhile moved to a new id.
#[test]
fn a_save_whose_password_cannot_be_stored_changes_nothing() {
    let w = TestWorld::new("it-save-refused");
    let store = Arc::new(Refusing(w.secrets.clone(), false.into()));
    let ctx = crate::context::Context { secrets: store.clone(), ..w.context() };
    let refuse = |yes: bool| store.1.store(yes, std::sync::atomic::Ordering::Relaxed);
    let profile = |id: &str, port: u16| crate::hosts::Profile { id: id.into(), name: "box".into(), host: "h.example".into(), port: Some(port), ..Default::default() };
    let password = |id: &str| w.secrets.get(&SecretKey::new("ssh", format!("password:profile:{id}"))).unwrap();
    let saved = || crate::hosts::profiles(&ctx).into_iter().map(|p| (p.id, p.port)).collect::<Vec<_>>();
    let m = Manager::new(ctx.clone());
    let id = m.save_profile(profile("", 22), Some("first")).unwrap();

    refuse(true);
    // A new address with a new password, then a new address with the one it has.
    for given in [Some("second"), None] {
        assert_eq!(m.save_profile(profile(&id, 2222), given).unwrap_err().kind, ErrorKind::Io, "{given:?}");
        assert_eq!(saved(), [(id.clone(), Some(22))], "{given:?}");
        assert_eq!(password(&id).as_deref(), Some("first"), "{given:?}");
    }
    // The same address with a new password, and a new profile.
    assert!(m.save_profile(crate::hosts::Profile { name: "renamed".into(), ..profile(&id, 22) }, Some("second")).is_err());
    assert!(m.save_profile(profile("", 22), Some("third")).is_err());
    assert_eq!(crate::hosts::profiles(&ctx).into_iter().map(|p| (p.id, p.name)).collect::<Vec<_>>(), [(id.clone(), "box".to_owned())]);

    refuse(false);
    let moved = m.save_profile(profile(&id, 2222), Some("second")).unwrap();
    assert_ne!(moved, id);
    assert_eq!(saved(), [(moved.clone(), Some(2222))], "the save that failed left a profile behind");
    assert_eq!((password(&moved).as_deref(), password(&id)), (Some("second"), None));
    // Saved again under the id it had: there is no such profile now, and none is added.
    assert_eq!(m.save_profile(profile(&id, 2222), None).unwrap_err().kind, ErrorKind::UnknownHost);
    assert_eq!(saved(), [(moved, Some(2222))]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sftp_browses_and_changes_both_sides() {
    let w = TestWorld::new("it-sftp-browse");
    let srv = w.root.join("srv");
    let fx = Fixture::start(&w.root, &srv, Options::default()).await;
    if !fx.has_sftp {
        eprintln!("skipped: no OpenSSH sftp-server on this machine");
        return;
    }
    std::fs::create_dir_all(srv.join("releases/0.0.16")).unwrap();
    std::fs::write(srv.join("deploy.sh"), "#!/bin/sh\n").unwrap();
    w.write_ssh_config(&fx.config("lab", ""));
    fx.trust_in(&w.context().known_hosts);
    w.secrets.set(&SecretKey::new("ssh", "password:ssh:lab"), PASSWORD).unwrap();
    let m = Manager::new(w.context());
    let remote = Side::Remote("lab".into());

    let home = m.list(&remote, "~").await.unwrap();
    let srv_real = std::fs::canonicalize(&srv).unwrap().to_string_lossy().into_owned();
    assert_eq!(home.path, srv_real);
    let names: Vec<&str> = home.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["releases", "deploy.sh"]);
    assert!(home.entries[0].dir && !home.entries[1].dir);

    let made = m.mkdir(&remote, &home.path, "new dir").await.unwrap();
    std::fs::write(srv.join("new dir/inner.txt"), "x").unwrap();
    let renamed = m.rename(&remote, &made, "renamed").await.unwrap();
    assert!(srv.join("renamed/inner.txt").is_file());
    assert!(m.rename(&remote, &renamed, "releases").await.is_err(), "won't replace");
    m.remove(&remote, &renamed).await.unwrap();
    assert!(!srv.join("renamed").exists());
    assert!(m.remove(&remote, "/").await.is_err());
    assert!(m.mkdir(&remote, &home.path, "../escape").await.is_err());

    let local = m.list(&Side::Local, w.home().to_str().unwrap()).await.unwrap();
    assert!(local.entries.iter().any(|e| e.name == ".ssh" && e.hidden && e.dir));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_resume_a_part_and_keep_mode_and_time() {
    use std::os::unix::fs::PermissionsExt;
    let w = TestWorld::new("it-upload");
    let srv = w.root.join("srv");
    let fx = Fixture::start(&w.root, &srv, Options::default()).await;
    if !fx.has_sftp {
        eprintln!("skipped: no OpenSSH sftp-server on this machine");
        return;
    }
    w.write_ssh_config(&fx.config("lab", ""));
    fx.trust_in(&w.context().known_hosts);
    w.secrets.set(&SecretKey::new("ssh", "password:ssh:lab"), PASSWORD).unwrap();
    let m = Manager::new(w.context());

    let data: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let src = w.root.join("build-artifacts.tar.zst");
    std::fs::write(&src, &data).unwrap();
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o640)).unwrap();
    let when = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    std::fs::File::options().write(true).open(&src).unwrap().set_times(std::fs::FileTimes::new().set_modified(when)).unwrap();
    // A broken earlier upload left the first MiB.
    std::fs::write(srv.join("build-artifacts.tar.zst.part"), &data[..1024 * 1024]).unwrap();

    let spec = TransferSpec { host: "lab".into(), direction: Direction::Upload, from: src.to_string_lossy().into(), to_dir: srv.to_string_lossy().into(), preserve: true };
    let id = m.enqueue(spec.clone()).unwrap();
    // A second click on the same file is the same transfer, not a second writer on its .part.
    assert_eq!(m.enqueue(spec).unwrap(), id);
    assert_eq!(m.transfers.list().len(), 1);
    let info = wait_transfer(&m, id).await;
    assert_eq!(info.state, State::Done, "{:?}", info.error);
    assert!(info.resumed);
    assert_eq!((info.resumed_from, info.done, info.total), (1024 * 1024, data.len() as u64, data.len() as u64));
    let out = srv.join("build-artifacts.tar.zst");
    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert!(!srv.join("build-artifacts.tar.zst.part").exists());
    let meta = std::fs::metadata(&out).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o640);
    assert_eq!(meta.modified().unwrap(), when);

    // A whole folder, then cancel and retry.
    std::fs::create_dir_all(w.root.join("site/assets")).unwrap();
    std::fs::write(w.root.join("site/index.html"), "<p>hi</p>").unwrap();
    std::fs::write(w.root.join("site/assets/app.js"), "1;").unwrap();
    let id = m.enqueue(TransferSpec { host: "lab".into(), direction: Direction::Upload, from: w.root.join("site").to_string_lossy().into(), to_dir: srv.to_string_lossy().into(), preserve: false }).unwrap();
    let info = wait_transfer(&m, id).await;
    assert_eq!((info.state, info.files, info.files_done), (State::Done, 2, 2));
    assert_eq!(std::fs::read_to_string(srv.join("site/assets/app.js")).unwrap(), "1;");

    let id = m.enqueue(TransferSpec { host: "lab".into(), direction: Direction::Upload, from: src.to_string_lossy().into(), to_dir: srv.join("site").to_string_lossy().into(), preserve: false }).unwrap();
    assert!(m.transfers.cancel(id));
    assert_eq!(wait_transfer(&m, id).await.state, State::Cancelled);
    let again = m.retry(id).unwrap();
    assert_eq!(wait_transfer(&m, again).await.state, State::Done);
    assert_eq!(std::fs::read(srv.join("site/build-artifacts.tar.zst")).unwrap(), data);
    m.transfers.clear_finished();
    assert!(m.transfers.list().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_resume_and_bring_whole_folders() {
    let w = TestWorld::new("it-download");
    let srv = w.root.join("srv");
    let fx = Fixture::start(&w.root, &srv, Options::default()).await;
    if !fx.has_sftp {
        eprintln!("skipped: no OpenSSH sftp-server on this machine");
        return;
    }
    w.write_ssh_config(&fx.config("lab", ""));
    fx.trust_in(&w.context().known_hosts);
    w.secrets.set(&SecretKey::new("ssh", "password:ssh:lab"), PASSWORD).unwrap();
    let m = Manager::new(w.context());
    let data: Vec<u8> = (0..2 * 1024 * 1024u32).map(|i| (i % 241) as u8).collect();
    std::fs::create_dir_all(srv.join("logs/old")).unwrap();
    std::fs::write(srv.join("logs/app.log"), &data).unwrap();
    std::fs::write(srv.join("logs/old/a.log"), "old").unwrap();
    let down = w.root.join("down");
    std::fs::create_dir_all(&down).unwrap();
    std::fs::write(down.join("app.log.part"), &data[..512 * 1024]).unwrap();

    let file = m.enqueue(TransferSpec { host: "lab".into(), direction: Direction::Download, from: srv.join("logs/app.log").to_string_lossy().into(), to_dir: down.to_string_lossy().into(), preserve: true }).unwrap();
    let info = wait_transfer(&m, file).await;
    assert_eq!(info.state, State::Done, "{:?}", info.error);
    assert_eq!((info.resumed, info.resumed_from), (true, 512 * 1024));
    assert_eq!(std::fs::read(down.join("app.log")).unwrap(), data);

    let folder = m.enqueue(TransferSpec { host: "lab".into(), direction: Direction::Download, from: srv.join("logs").to_string_lossy().into(), to_dir: down.to_string_lossy().into(), preserve: false }).unwrap();
    let info = wait_transfer(&m, folder).await;
    assert_eq!((info.state, info.files), (State::Done, 2));
    assert_eq!(std::fs::read_to_string(down.join("logs/old/a.log")).unwrap(), "old");
}

/// A save made the moment the file opens (before the watcher's task first runs,
/// which one thread makes certain) and keeping the size (so only the content, not
/// the clock, can tell) still goes up.
#[tokio::test(flavor = "current_thread")]
async fn a_save_right_after_opening_is_uploaded() {
    let w = TestWorld::new("it-edit-quick");
    let srv = w.root.join("srv");
    let fx = Fixture::start(&w.root, &srv, Options::default()).await;
    if !fx.has_sftp {
        eprintln!("skipped: no OpenSSH sftp-server on this machine");
        return;
    }
    w.write_ssh_config(&fx.config("lab", ""));
    fx.trust_in(&w.context().known_hosts);
    w.secrets.set(&SecretKey::new("ssh", "password:ssh:lab"), PASSWORD).unwrap();
    let m = Manager::new(w.context());
    let remote = srv.join("app.env");
    std::fs::write(&remote, "MODE=dev\n").unwrap();

    let ed = m.open_remote("lab", remote.to_str().unwrap()).await.unwrap();
    let opened = std::fs::metadata(&ed.local).unwrap().modified().unwrap();
    std::fs::write(&ed.local, "MODE=prd\n").unwrap();
    // As a coarse clock leaves it: the save keeps the time the download had.
    std::fs::File::options().write(true).open(&ed.local).unwrap().set_times(std::fs::FileTimes::new().set_modified(opened)).unwrap();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if m.edits.list()[0].uploads >= 1 {
            break;
        }
    }
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "MODE=prd\n", "{:?}", m.edits.list());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_opened_file_uploads_its_saves_and_stops_on_a_conflict() {
    let w = TestWorld::new("it-edit");
    let srv = w.root.join("srv");
    let fx = Fixture::start(&w.root, &srv, Options::default()).await;
    if !fx.has_sftp {
        eprintln!("skipped: no OpenSSH sftp-server on this machine");
        return;
    }
    w.write_ssh_config(&fx.config("lab", ""));
    fx.trust_in(&w.context().known_hosts);
    w.secrets.set(&SecretKey::new("ssh", "password:ssh:lab"), PASSWORD).unwrap();
    let m = Manager::new(w.context());
    let remote = srv.join("nginx.conf");
    std::fs::write(&remote, "worker_processes 1;\n").unwrap();

    let ed = m.open_remote("lab", remote.to_str().unwrap()).await.unwrap();
    assert_eq!(std::fs::read_to_string(&ed.local).unwrap(), "worker_processes 1;\n");
    assert!(ed.local.starts_with(w.root.to_str().unwrap()));
    std::fs::write(&ed.local, "worker_processes 4;\n").unwrap();
    let mut uploaded = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if m.edits.list()[0].uploads >= 1 {
            uploaded = true;
            break;
        }
    }
    assert!(uploaded, "{:?}", m.edits.list());
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "worker_processes 4;\n");

    // Someone else saves on the host, then a local save: held as a conflict.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(&remote, "someone else\n").unwrap();
    std::fs::write(&ed.local, "mine\n").unwrap();
    let mut conflict = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if m.edits.list()[0].state == "conflict" {
            conflict = true;
            break;
        }
    }
    assert!(conflict, "{:?}", m.edits.list());
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "someone else\n");
    assert!(m.edits.overwrite(ed.id));
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if m.edits.list()[0].state == "watching" {
            break;
        }
    }
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "mine\n");
    assert!(m.edits.stop(ed.id));

    // Through a link: saves land in the file it points at, and the link stays a link.
    std::fs::create_dir_all(srv.join("sites-available")).unwrap();
    std::fs::write(srv.join("sites-available/default"), "listen 80;\n").unwrap();
    std::os::unix::fs::symlink("sites-available/default", srv.join("default")).unwrap();
    let ed = m.open_remote("lab", srv.join("default").to_str().unwrap()).await.unwrap();
    assert!(ed.local.ends_with("default"));
    std::fs::write(&ed.local, "listen 8080;\n").unwrap();
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if m.edits.list().iter().any(|e| e.id == ed.id && e.uploads >= 1) {
            break;
        }
    }
    assert_eq!(std::fs::read_to_string(srv.join("sites-available/default")).unwrap(), "listen 8080;\n");
    assert!(std::fs::symlink_metadata(srv.join("default")).unwrap().file_type().is_symlink());
}
