//! `yuzora:ssh`: what Yuzora's mods ask of this crate. Every async function
//! runs on the crate's runtime ([`crate::runtime`]); errors reach scripts as
//! `"[code] message"` (see [`crate::ErrorKind::code`]).

use crate::error::SshError;
use crate::hosts::{Machine, Profile};
use crate::prompt::Answer;
use crate::runtime::bridge;
use crate::session::{Interaction, Manager, manager};
use crate::sftp::Side;
use crate::transfer::TransferSpec;
use crate::tunnel::TunnelSpec;
use serde::Serialize;
use serde_json::{Value, json};
use std::future::Future;
use std::sync::Arc;
use yuzora_core::host::{arg, to_host};
use yuzora_core::{HostArguments, HostError, HostModule, HostValue};
use zeroize::Zeroizing;

/// The import specifier: `yuzora:ssh`.
pub const NAME: &str = "yuzora:ssh";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
export interface SshMachine { name: string; target?: string | null; state?: string | null; temporary?: boolean }
export interface SshVerified { algorithm: string; kind: string; fingerprint: string; source: string | null; newlyTrusted: boolean }
export interface SshProfile { id?: string; name?: string; host: string; port?: number | null; user?: string | null; identityFile?: string | null; proxyJump?: string | null }
export interface SshHost {
  id: string; name: string; source: "config" | "profile" | "machine" | "direct";
  target: string; hostname: string; user: string; port: number; proxyJump: string | null;
  machine: string | null; machineState: string | null; editable: boolean; profile: SshProfile | null;
  state: "idle" | "connecting" | "connected" | "error"; latencyMs: number | null;
  error: string | null; errorCode: string | null; verified: SshVerified | null; method: string | null;
}
export interface SshPrompt {
  id: string; host: string; label: string; kind: "hostKey" | "hostKeyChanged" | "password" | "passphrase" | "keyboard";
  endpoint: string | null; algorithm: string | null; fingerprint: string | null; previous: string[]; previousSource: string | null;
  keyPath: string | null; name: string | null; instructions: string | null; questions: { text: string; echo: boolean }[];
  canRemember: boolean; retry: boolean; claimed: boolean;
}
export type SshAnswer = { trust: boolean } | { secret: string; remember?: boolean } | { responses: string[]; remember?: boolean } | null;
export interface SftpEntry { name: string; path: string; kind: "dir" | "file" | "link" | "other"; dir: boolean; size: number; mtime: number | null; mode: number | null; hidden: boolean }
export interface SftpListing { path: string; parent: string | null; entries: SftpEntry[] }
export interface SshTransfer {
  id: number; host: string; direction: "upload" | "download"; name: string; from: string; to: string; preserve: boolean;
  state: "queued" | "running" | "done" | "failed" | "cancelled"; total: number; done: number; rate: number;
  files: number; filesDone: number; resumed: boolean; resumedFrom: number; error: string | null; queuedAt: number; finishedAt: number | null;
}
export interface SshEdit { id: number; host: string; remote: string; local: string; state: "watching" | "uploading" | "conflict" | "error" | "stopped"; uploads: number; lastUpload: number | null; error: string | null }
export interface SshKnownHost { file: string; line: number; raw: string; marker: "none" | "cert-authority" | "revoked"; patterns: string; hashed: boolean; algorithm: string; key: string; fingerprint: string; comment: string | null; editable: boolean }
export interface SshTunnel { id: number; host: string; remoteHost: string; remotePort: number; localPort: number; active: number; total: number; bytesUp: number; bytesDown: number; openedAt: number }
/** The yuzora-ssh crate's version (Yuzora's). */
export function version(): string;
/** Every host: ~/.ssh/config, Yuzora profiles, then herdr machines (pass `hc machine ls --json`'s machines; omit to reuse the last). */
export function hosts(machines?: SshMachine[] | null): SshHost[];
/** Connection states of the hosts that have one (cheap; poll this, then hosts() when it changes). */
export function hostStates(): { id: string; state: SshHost["state"]; latencyMs: number | null; error: string | null }[];
/** Connects (or reuses the open session); questions show up in prompts(). Resolves with the host. */
export function connect(host: string): Promise<SshHost>;
/** Ends a host's session, its tunnels and SFTP. */
export function disconnect(host: string): Promise<null>;
/** Measures a round trip on the open session, in ms. */
export function ping(host: string): Promise<number>;
/** Questions connections are waiting on, oldest first. */
export function prompts(): SshPrompt[];
/** Marks a question as shown; true only for the first caller (open one dialog per question). */
export function claimPrompt(id: string): boolean;
/** Answers a question; null cancels. False when it is gone. */
export function answerPrompt(id: string, answer: SshAnswer): boolean;
/** The status bar's line: a verified host, a connection, transfers. */
export function status(): { text: string; tone: "muted" | "working" | "blocked" | "done" | "accent" } | null;
/** Adds or replaces a Yuzora profile; a password goes to the credential store. Returns its id. */
export function saveProfile(profile: SshProfile, password?: string | null): string;
/** Removes a profile and its stored password. */
export function removeProfile(id: string): boolean;
/** Forgets a host's stored password. */
export function forgetPassword(host: string): null;
/** How to open the host in a terminal with OpenSSH. */
export function sshArgv(host: string): string[];
/** This computer's home folder. */
export function localHome(): string;
/** Windows: opens a local copy `openRemote` made in Notepad and returns at once (Notepad keeps running; `exec` would wait for it and stop it after 30 s). */
export function openInNotepad(path: string): null;
/** A folder's entries; side null is this computer, else a host. "~" or "" is the home folder. */
export function list(side: string | null, path: string): Promise<SftpListing>;
export function mkdir(side: string | null, dir: string, name: string): Promise<string>;
export function rename(side: string | null, path: string, newName: string): Promise<string>;
/** Deletes a file, or a folder with everything in it. */
export function remove(side: string | null, path: string): Promise<null>;
/** Queues a copy of a file or folder into toDir on the other side; returns its id. */
export function transfer(spec: { host: string; direction: "upload" | "download"; from: string; toDir: string; preserve?: boolean }): number;
export function transfers(): SshTransfer[];
export function cancelTransfer(id: number): boolean;
/** Runs a failed or cancelled transfer again, resuming its .part; the new id. */
export function retryTransfer(id: number): number;
/** Forgets finished transfers. */
export function clearTransfers(): null;
/** Downloads a host's file to a temporary folder and uploads every save back. */
export function openRemote(host: string, path: string): Promise<SshEdit>;
export function edits(): SshEdit[];
export function stopEdit(id: number): boolean;
/** After a conflict, uploads the local copy over the host's. */
export function overwriteEdit(id: number): boolean;
/** Every known_hosts line: Yuzora's own (editable), then ~/.ssh's and the system's (read only). */
export function knownHosts(): SshKnownHost[];
/** Removes a line from Yuzora's known_hosts, if it still reads `raw`. */
export function removeKnownHost(file: string, line: number, raw: string): boolean;
/** Forwards a local port to remoteHost:remotePort through the host (asks for a new key or password). */
export function openTunnel(host: string, remoteHost: string, remotePort: number, localPort?: number): Promise<SshTunnel>;
export function tunnels(): SshTunnel[];
export function closeTunnel(id: number): boolean;
"#;

fn fail(e: SshError) -> HostError {
    HostError::new(format!("[{}] {}", e.code(), e.message))
}

fn json<T: Serialize>(v: &T) -> HostValue {
    to_host(&serde_json::to_value(v).unwrap_or(Value::Null))
}

fn side(a: &HostArguments, i: usize) -> Side {
    Side::from_key(a.get(i).and_then(HostValue::as_str))
}

fn id(a: &HostArguments, i: usize) -> Result<u64, HostError> {
    let n = a.number(i)?;
    if n < 0.0 || n.fract() != 0.0 {
        return Err(HostError::new(format!("argument {i} is not an id")));
    }
    Ok(n as u64)
}

/// An async host function whose work runs on the SSH runtime.
fn on_runtime<F>(work: F) -> impl Future<Output = Result<HostValue, HostError>> + Send + 'static
where
    F: Future<Output = Result<HostValue, SshError>> + Send + 'static,
{
    let done = bridge(work);
    async move { done.await.unwrap_or_else(|| Err(SshError::closed())).map_err(fail) }
}

fn host_entry(m: &Manager, id: &str) -> HostValue {
    let all = m.hosts_json();
    to_host(all.as_array().and_then(|l| l.iter().find(|h| h["id"] == id)).unwrap_or(&Value::Null))
}

fn answer(v: Value) -> Answer {
    let remember = v.get("remember").and_then(Value::as_bool).unwrap_or(false);
    if let Some(t) = v.get("trust").and_then(Value::as_bool) {
        Answer::Trust(t)
    } else if let Some(s) = v.get("secret").and_then(Value::as_str) {
        Answer::Secret { value: Zeroizing::new(s.to_owned()), remember }
    } else if let Some(list) = v.get("responses").and_then(Value::as_array) {
        Answer::Responses { values: list.iter().map(|r| Zeroizing::new(r.as_str().unwrap_or_default().to_owned())).collect(), remember }
    } else {
        Answer::Cancel
    }
}

/// Every known_hosts file the person's connections consult.
fn known_hosts_lines(m: &Manager) -> Value {
    let own = &m.ctx.known_hosts;
    let mut files = vec![own.clone()];
    if m.ctx.system_known_hosts {
        let ssh = m.ctx.config.home.join(".ssh");
        files.extend([ssh.join("known_hosts"), ssh.join("known_hosts2")]);
        #[cfg(unix)]
        files.extend([std::path::PathBuf::from("/etc/ssh/ssh_known_hosts"), std::path::PathBuf::from("/etc/ssh/ssh_known_hosts2")]);
    }
    Value::Array(
        files
            .iter()
            .flat_map(|f| crate::known_hosts::read_file(f))
            .map(|l| {
                let editable = &l.file == own;
                let mut v = serde_json::to_value(&l).unwrap_or(Value::Null);
                v["editable"] = json!(editable);
                v
            })
            .collect(),
    )
}

/// The `yuzora:ssh` module.
pub fn host_module() -> HostModule {
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(HostValue::Str(yuzora_core::VERSION.into())))
        .function("hosts", |a| {
            let m = manager();
            if let Value::Array(list) = arg(a, 0) {
                m.set_machines(list.into_iter().filter_map(|v| serde_json::from_value::<Machine>(v).ok()).collect());
            }
            Ok(to_host(&m.hosts_json()))
        })
        .function("hostStates", |_| Ok(to_host(&manager().states_json())))
        .async_function("connect", |a| {
            let host = a.string(0)?.to_owned();
            Ok(on_runtime(async move {
                let m = manager();
                let s = m.session(&host, Interaction::Ask).await?;
                Ok(host_entry(&m, &s.host_id))
            }))
        })
        .async_function("disconnect", |a| {
            let host = a.string(0)?.to_owned();
            Ok(on_runtime(async move {
                manager().disconnect(&host).await;
                Ok(HostValue::Null)
            }))
        })
        .async_function("ping", |a| {
            let host = a.string(0)?.to_owned();
            Ok(on_runtime(async move {
                let s = manager().open_session(&host).ok_or_else(SshError::closed)?;
                Ok(HostValue::Number(s.ping().await? as f64))
            }))
        })
        .function("prompts", |_| Ok(json(&manager().prompts.list())))
        .function("claimPrompt", |a| Ok(HostValue::Bool(manager().prompts.claim(a.string(0)?))))
        .function("answerPrompt", |a| {
            let id = a.string(0)?.to_owned();
            Ok(HostValue::Bool(manager().prompts.answer(&id, answer(arg(a, 1)))))
        })
        .function("status", |_| Ok(match manager().status() {
            Some((text, tone)) => to_host(&json!({ "text": text, "tone": tone })),
            None => HostValue::Null,
        }))
        .function("saveProfile", |a| {
            let m = manager();
            let profile: Profile = serde_json::from_value(arg(a, 0)).map_err(|e| HostError::new(e.to_string()))?;
            let id = crate::hosts::save_profile(&m.ctx, profile).map_err(fail)?;
            if let Some(pw) = a.get(1).and_then(HostValue::as_str).filter(|s| !s.is_empty()) {
                m.ctx.secrets.set(&crate::auth::password_key(&format!("profile:{id}")), pw).map_err(|e| HostError::new(e.to_string()))?;
            }
            Ok(HostValue::Str(id))
        })
        .function("removeProfile", |a| {
            let m = manager();
            let id = a.string(0)?.trim_start_matches("profile:").to_owned();
            let _ = m.ctx.secrets.delete(&crate::auth::password_key(&format!("profile:{id}")));
            Ok(HostValue::Bool(crate::hosts::remove_profile(&m.ctx, &id).map_err(fail)?))
        })
        .function("forgetPassword", |a| {
            let m = manager();
            let def = m.lookup(a.string(0)?).map_err(fail)?;
            m.ctx.secrets.delete(&crate::auth::password_key(&def.id)).map_err(|e| HostError::new(e.to_string()))?;
            Ok(HostValue::Null)
        })
        .function("sshArgv", |a| Ok(json(&manager().lookup(a.string(0)?).map_err(fail)?.ssh_argv())))
        .function("localHome", |_| Ok(HostValue::Str(manager().ctx.config.home.to_string_lossy().into_owned())))
        .function("openInNotepad", |a| {
            let path = a.string(0)?;
            // Only the copies `openRemote` made: never a program or file a script names.
            if !manager().edits.list().iter().any(|e| e.local == path) {
                return Err(fail(SshError::text(crate::ErrorKind::Invalid, "not-an-edit-copy", &[])));
            }
            open_in_notepad(path).map_err(|e| fail(SshError::io(e)))?;
            Ok(HostValue::Null)
        })
        .async_function("list", |a| {
            let (side, path) = (side(a, 0), a.string(1)?.to_owned());
            Ok(on_runtime(async move { Ok(json(&manager().list(&side, &path).await?)) }))
        })
        .async_function("mkdir", |a| {
            let (side, dir, name) = (side(a, 0), a.string(1)?.to_owned(), a.string(2)?.to_owned());
            Ok(on_runtime(async move { Ok(HostValue::Str(manager().mkdir(&side, &dir, &name).await?)) }))
        })
        .async_function("rename", |a| {
            let (side, path, name) = (side(a, 0), a.string(1)?.to_owned(), a.string(2)?.to_owned());
            Ok(on_runtime(async move { Ok(HostValue::Str(manager().rename(&side, &path, &name).await?)) }))
        })
        .async_function("remove", |a| {
            let (side, path) = (side(a, 0), a.string(1)?.to_owned());
            Ok(on_runtime(async move {
                manager().remove(&side, &path).await?;
                Ok(HostValue::Null)
            }))
        })
        .function("transfer", |a| {
            let spec: TransferSpec = serde_json::from_value(arg(a, 0)).map_err(|e| HostError::new(e.to_string()))?;
            Ok(HostValue::Number(manager().enqueue(spec).map_err(fail)? as f64))
        })
        .function("transfers", |_| Ok(json(&manager().transfers.list())))
        .function("cancelTransfer", |a| Ok(HostValue::Bool(manager().transfers.cancel(id(a, 0)?))))
        .function("retryTransfer", |a| Ok(HostValue::Number(manager().retry(id(a, 0)?).map_err(fail)? as f64)))
        .function("clearTransfers", |_| {
            manager().transfers.clear_finished();
            Ok(HostValue::Null)
        })
        .async_function("openRemote", |a| {
            let (host, path) = (a.string(0)?.to_owned(), a.string(1)?.to_owned());
            Ok(on_runtime(async move { Ok(json(&manager().open_remote(&host, &path).await?)) }))
        })
        .function("edits", |_| Ok(json(&manager().edits.list())))
        .function("stopEdit", |a| Ok(HostValue::Bool(manager().edits.stop(id(a, 0)?))))
        .function("overwriteEdit", |a| Ok(HostValue::Bool(manager().edits.overwrite(id(a, 0)?))))
        .function("knownHosts", |_| Ok(to_host(&known_hosts_lines(&manager()))))
        .function("removeKnownHost", |a| {
            let m = manager();
            let file = std::path::PathBuf::from(a.string(0)?);
            // Only Yuzora's own file is ever written; ~/.ssh stays the person's.
            if file != m.ctx.known_hosts {
                return Err(fail(SshError::text(crate::ErrorKind::Invalid, "known-hosts-readonly", &[])));
            }
            let line = id(a, 1)? as usize;
            Ok(HostValue::Bool(crate::known_hosts::remove_line(&file, line, a.string(2)?).map_err(|e| fail(SshError::io(e)))?))
        })
        .async_function("openTunnel", |a| {
            let host = a.string(0)?.to_owned();
            let remote_host = a.string(1)?.to_owned();
            let remote_port = a.integer(2)?;
            let local_port = a.get(3).and_then(HostValue::as_number).unwrap_or(0.0);
            let ports = u16::try_from(remote_port).ok().zip((local_port >= 0.0 && local_port <= 65535.0).then_some(local_port as u16));
            let Some((remote_port, local_port)) = ports else { return Err(HostError::new("[invalid] port out of range")) };
            Ok(on_runtime(async move {
                let m: Arc<Manager> = manager();
                let t = m.open_tunnel(TunnelSpec::new(host, remote_host, remote_port).local_port(local_port).interactive(true)).await?;
                let info = t.info();
                m.tunnels.hold(t);
                Ok(json(&info))
            }))
        })
        .function("tunnels", |_| Ok(json(&manager().tunnels.list())))
        .function("closeTunnel", |a| Ok(HostValue::Bool(manager().tunnels.close(id(a, 0)?))))
}

/// Notepad on `path`, not waited for: no shell is involved (the name is a file name, not
/// command text), and nothing runs the file. A thread reaps it when the person closes it.
fn open_in_notepad(path: &str) -> std::io::Result<()> {
    if !cfg!(windows) {
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Notepad is for Windows"));
    }
    let mut child = std::process::Command::new("notepad.exe")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notepad_opens_only_the_copies_an_edit_made() {
        let m = host_module();
        let err = m.call("openInNotepad", &HostArguments::new([HostValue::Str("C:\\Windows\\System32\\cmd.exe".into())])).unwrap_err();
        assert!(err.to_string().contains("not-an-edit-copy"), "{err}");
    }

    #[cfg(not(windows))]
    #[test]
    fn notepad_is_windows_only() {
        assert!(open_in_notepad("/tmp/x").is_err());
    }

    #[test]
    fn module_validates_and_reports_its_version() {
        let m = host_module();
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:ssh");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(yuzora_core::VERSION.into()));
        for f in ["connect", "disconnect", "ping", "list", "mkdir", "rename", "remove", "openRemote", "openTunnel"] {
            assert!(m.is_async(f), "{f} should be async");
        }
    }

    #[test]
    fn answers_read_every_shape() {
        assert!(matches!(answer(json!({ "trust": true })), Answer::Trust(true)));
        assert!(matches!(answer(json!({ "secret": "pw", "remember": true })), Answer::Secret { remember: true, .. }));
        assert!(matches!(answer(json!({ "responses": ["a", "b"] })), Answer::Responses { ref values, remember: false } if values.len() == 2));
        assert!(matches!(answer(Value::Null), Answer::Cancel));
    }
}
