//! Logging in, in OpenSSH's order unless PreferredAuthentications says
//! otherwise: public keys (ssh-agent first, then key files, asking for a
//! passphrase), keyboard-interactive, then password. Passwords and
//! passphrases come from the credential store, else from the person (and go
//! back to the store only when they ask).

use crate::context::{Context, SECRETS};
use crate::error::{ErrorKind, Result, SshError};
use crate::hosts::HostDef;
use crate::prompt::{Answer, PROMPT_TIMEOUT, Prompt, Prompts, Question};
use crate::session::Client;
use russh::MethodKind;
use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse as Kbd};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use yuzora_core::secrets::SecretKey;
use zeroize::Zeroizing;

/// How many agent keys to offer before moving on (servers stop at MaxAuthTries, 6 by default).
const AGENT_KEYS: usize = 5;

pub(crate) struct Login<'a> {
    pub ctx: &'a Context,
    pub def: &'a HostDef,
    /// None: nobody can be asked (a background tunnel).
    pub prompts: Option<&'a Prompts>,
}

enum Step {
    Success,
    /// The server accepted this method but wants another one too.
    Partial(Vec<MethodKind>),
    Failed(Option<Vec<MethodKind>>),
}

fn methods(set: &russh::MethodSet) -> Vec<MethodKind> {
    set.iter().cloned().collect()
}

fn step(r: AuthResult) -> Step {
    match r {
        AuthResult::Success => Step::Success,
        AuthResult::Failure { remaining_methods, partial_success: true } => Step::Partial(methods(&remaining_methods)),
        AuthResult::Failure { remaining_methods, .. } => Step::Failed(Some(methods(&remaining_methods))),
    }
}

fn transport(e: russh::Error) -> SshError {
    SshError::text(ErrorKind::Connect, "auth-transport", &[("why", e.to_string())])
}

pub(crate) fn password_key(host_id: &str) -> SecretKey {
    SecretKey::new(SECRETS, format!("password:{host_id}"))
}

pub(crate) fn passphrase_key(path: &Path) -> SecretKey {
    SecretKey::new(SECRETS, format!("passphrase:{}", path.display()))
}

impl Login<'_> {
    fn user(&self) -> &str {
        &self.def.resolved.user
    }

    fn secret(&self, key: &SecretKey) -> Option<Zeroizing<String>> {
        self.ctx.secrets.get(key).ok().flatten().map(Zeroizing::new)
    }

    fn remember(&self, key: &SecretKey, value: &str) {
        let _ = self.ctx.secrets.set(key, value);
    }

    fn prompt(&self, kind: &'static str) -> Prompt {
        Prompt::new(&self.def.id, &self.def.name, kind)
    }

    /// Logs in; the method that worked.
    pub async fn run(&self, handle: &mut Handle<Client>) -> Result<&'static str> {
        let mut remaining = match handle.authenticate_none(self.user()).await.map_err(transport)? {
            AuthResult::Success => return Ok("none"),
            AuthResult::Failure { remaining_methods, .. } => methods(&remaining_methods),
        };
        let r = &self.def.resolved;
        let mut order: Vec<MethodKind> = r
            .preferred_authentications
            .iter()
            .filter_map(|m| match m.as_str() {
                "publickey" => Some(MethodKind::PublicKey),
                "keyboard-interactive" => Some(MethodKind::KeyboardInteractive),
                "password" => Some(MethodKind::Password),
                _ => None,
            })
            .collect();
        if order.is_empty() {
            order = vec![MethodKind::PublicKey, MethodKind::KeyboardInteractive, MethodKind::Password];
        }
        order.retain(|m| match m {
            MethodKind::PublicKey => r.pubkey_authentication,
            MethodKind::KeyboardInteractive => r.kbd_interactive_authentication,
            MethodKind::Password => r.password_authentication,
            _ => false,
        });
        let mut tried: Vec<MethodKind> = Vec::new();
        let mut needed_input = false;
        while let Some(method) = order.iter().find(|m| remaining.contains(m) && !tried.contains(m)).cloned() {
            tried.push(method.clone());
            let outcome = match method {
                MethodKind::PublicKey => self.publickey(handle, &mut needed_input).await?,
                MethodKind::KeyboardInteractive => self.keyboard(handle, &mut needed_input).await?,
                _ => self.password(handle, &mut needed_input).await?,
            };
            match outcome {
                Step::Success => {
                    return Ok(match method {
                        MethodKind::PublicKey => "publickey",
                        MethodKind::KeyboardInteractive => "keyboard-interactive",
                        _ => "password",
                    });
                }
                Step::Partial(next) => {
                    remaining = next;
                    // A second factor may reuse a method only after the other one.
                    tried.retain(|m| *m == method);
                }
                Step::Failed(Some(next)) if !next.is_empty() => remaining = next,
                Step::Failed(_) => {}
            }
        }
        Err(if needed_input {
            SshError::text(ErrorKind::NeedsInput, "needs-input", &[("host", self.def.name.clone())])
        } else {
            SshError::text(ErrorKind::AuthFailed, "auth-failed", &[("host", self.def.name.clone()), ("user", self.user().to_owned())])
        })
    }

    async fn rsa_hash(&self, handle: &Handle<Client>, key: &PublicKey) -> Option<HashAlg> {
        if !matches!(key.algorithm(), russh::keys::Algorithm::Rsa { .. }) {
            return None;
        }
        handle.best_supported_rsa_hash().await.ok().flatten().flatten()
    }

    async fn publickey(&self, handle: &mut Handle<Client>, needed_input: &mut bool) -> Result<Step> {
        let r = &self.def.resolved;
        let files: Vec<PathBuf> = if !r.identity_files.is_empty() {
            r.identity_files.clone()
        } else if self.ctx.default_identities {
            ["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|n| self.ctx.config.home.join(".ssh").join(n)).collect()
        } else {
            Vec::new()
        };
        let files: Vec<PathBuf> = files.into_iter().filter(|f| f.is_file()).collect();
        let wanted: Vec<PublicKey> = files.iter().filter_map(|f| public_of(f)).collect();
        let mut offered: Vec<PublicKey> = Vec::new();
        let mut last = Step::Failed(None);

        if self.ctx.agent && r.identity_agent.as_deref() != Some("none") {
            if let Some(mut agent) = agent::connect(r.identity_agent.as_deref()).await {
                let identities = agent.request_identities().await.unwrap_or_default();
                for id in identities {
                    let key = id.public_key().into_owned();
                    if r.identities_only && !wanted.contains(&key) {
                        continue;
                    }
                    if offered.len() >= AGENT_KEYS {
                        break;
                    }
                    let hash = self.rsa_hash(handle, &key).await;
                    offered.push(key.clone());
                    match handle.authenticate_publickey_with(self.user(), key, hash, &mut agent).await {
                        Ok(r) => match step(r) {
                            Step::Failed(rest) => last = Step::Failed(rest),
                            done => return Ok(done),
                        },
                        // The agent itself failed (locked, gone): keys on disk may still work.
                        Err(_) => break,
                    }
                }
            }
        }

        for file in files {
            if public_of(&file).is_some_and(|k| offered.contains(&k)) {
                continue;
            }
            let Some(key) = self.load_key(&file, needed_input).await? else { continue };
            let public = key.public_key().clone();
            let hash = self.rsa_hash(handle, &public).await;
            offered.push(public);
            let res = handle.authenticate_publickey(self.user(), PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await.map_err(transport)?;
            match step(res) {
                Step::Failed(rest) => last = Step::Failed(rest),
                done => return Ok(done),
            }
        }
        Ok(last)
    }

    /// A key file, decrypted with a stored or asked passphrase. None: skip it.
    async fn load_key(&self, file: &Path, needed_input: &mut bool) -> Result<Option<PrivateKey>> {
        let path = file.to_string_lossy().into_owned();
        match russh::keys::load_secret_key(file, None) {
            Ok(k) => return Ok(Some(k)),
            Err(russh::keys::Error::KeyIsEncrypted) => {}
            Err(_) if is_encrypted_pkcs8(file) => {}
            Err(_) => return Ok(None),
        }
        let store = passphrase_key(file);
        if let Some(stored) = self.secret(&store)
            && let Ok(k) = russh::keys::load_secret_key(file, Some(stored.as_str()))
        {
            return Ok(Some(k));
        }
        let Some(prompts) = self.prompts else {
            *needed_input = true;
            return Ok(None);
        };
        for attempt in 0..3 {
            let mut p = self.prompt("passphrase");
            p.key_path = Some(path.clone());
            p.can_remember = true;
            p.retry = attempt > 0;
            match prompts.ask(p, PROMPT_TIMEOUT).await {
                Some(Answer::Secret { value, remember }) => {
                    if let Ok(k) = russh::keys::load_secret_key(file, Some(value.as_str())) {
                        if remember {
                            self.remember(&store, &value);
                        }
                        return Ok(Some(k));
                    }
                }
                Some(Answer::Cancel) => return Err(SshError::text(ErrorKind::Cancelled, "cancelled", &[])),
                None => return Err(SshError::text(ErrorKind::Timeout, "prompt-timeout", &[])),
                Some(_) => return Ok(None),
            }
        }
        Ok(None)
    }

    async fn keyboard(&self, handle: &mut Handle<Client>, needed_input: &mut bool) -> Result<Step> {
        let store = password_key(&self.def.id);
        let mut stored = self.secret(&store);
        // A lone password to keep, kept only once the host has taken it.
        let mut keep: Option<Zeroizing<String>> = None;
        // A kept password the host refuses, or a mistyped one, asks again (like `password`).
        for attempt in 0..3 {
            let mut answered = false;
            let mut reply = handle.authenticate_keyboard_interactive_start(self.user(), None::<String>).await.map_err(transport)?;
            for _round in 0..6 {
                match reply {
                    Kbd::Success => {
                        self.keep(&store, keep.take());
                        return Ok(Step::Success);
                    }
                    Kbd::Failure { remaining_methods, partial_success: true } => {
                        self.keep(&store, keep.take());
                        return Ok(Step::Partial(methods(&remaining_methods)));
                    }
                    Kbd::Failure { remaining_methods, .. } => {
                        keep = None;
                        if answered && attempt < 2 && self.prompts.is_some() {
                            break;
                        }
                        return Ok(Step::Failed(Some(methods(&remaining_methods))));
                    }
                    Kbd::InfoRequest { name, instructions, prompts } => {
                        let lone_password = prompts.len() == 1 && is_password_prompt(&prompts[0].prompt, prompts[0].echo);
                        let responses: Vec<String> = if prompts.is_empty() {
                            Vec::new()
                        } else if lone_password && let Some(pw) = stored.take() {
                            answered = true;
                            vec![pw.to_string()]
                        } else if let Some(asker) = self.prompts {
                            answered = true;
                            let mut p = self.prompt("keyboard");
                            p.name = Some(name).filter(|s| !s.is_empty());
                            p.instructions = Some(instructions).filter(|s| !s.is_empty());
                            p.questions = prompts.iter().map(|q| Question { text: q.prompt.clone(), echo: q.echo }).collect();
                            p.can_remember = lone_password;
                            p.retry = attempt > 0;
                            match asker.ask(p, PROMPT_TIMEOUT).await {
                                Some(Answer::Responses { values, remember }) => {
                                    if remember && lone_password {
                                        keep = values.first().cloned();
                                    }
                                    values.iter().map(|v| v.to_string()).collect()
                                }
                                Some(Answer::Secret { value, remember }) => {
                                    if remember && lone_password {
                                        keep = Some(value.clone());
                                    }
                                    vec![value.to_string()]
                                }
                                None => return Err(SshError::text(ErrorKind::Timeout, "prompt-timeout", &[])),
                                Some(_) => return Err(SshError::text(ErrorKind::Cancelled, "cancelled", &[])),
                            }
                        } else {
                            // Nobody to ask: leave without answering (blank answers would
                            // count as a failed login against the account).
                            *needed_input = true;
                            return Ok(Step::Failed(None));
                        };
                        reply = handle.authenticate_keyboard_interactive_respond(responses).await.map_err(transport)?;
                    }
                }
            }
            if !answered {
                break;
            }
        }
        Ok(Step::Failed(None))
    }

    fn keep(&self, store: &SecretKey, value: Option<Zeroizing<String>>) {
        if let Some(v) = value {
            self.remember(store, &v);
        }
    }

    async fn password(&self, handle: &mut Handle<Client>, needed_input: &mut bool) -> Result<Step> {
        let store = password_key(&self.def.id);
        let stored = self.secret(&store);
        if let Some(pw) = &stored {
            match step(handle.authenticate_password(self.user(), pw.as_str()).await.map_err(transport)?) {
                Step::Failed(_) => {}
                done => return Ok(done),
            }
        }
        let Some(prompts) = self.prompts else {
            *needed_input = true;
            return Ok(Step::Failed(None));
        };
        let mut last = Step::Failed(None);
        for attempt in 0..3 {
            let mut p = self.prompt("password");
            p.can_remember = true;
            p.retry = attempt > 0 || stored.is_some();
            match prompts.ask(p, PROMPT_TIMEOUT).await {
                Some(Answer::Secret { value, remember }) => match step(handle.authenticate_password(self.user(), value.as_str()).await.map_err(transport)?) {
                    Step::Failed(rest) => last = Step::Failed(rest),
                    done => {
                        if remember {
                            self.remember(&store, &value);
                        }
                        return Ok(done);
                    }
                },
                None => return Err(SshError::text(ErrorKind::Timeout, "prompt-timeout", &[])),
                Some(_) => return Err(SshError::text(ErrorKind::Cancelled, "cancelled", &[])),
            }
        }
        Ok(last)
    }
}

/// The public half of a key file: `<file>.pub`, else the key itself when it isn't encrypted.
fn public_of(file: &Path) -> Option<PublicKey> {
    let mut pub_path = file.as_os_str().to_owned();
    pub_path.push(".pub");
    russh::keys::load_public_key(PathBuf::from(pub_path)).ok().or_else(|| russh::keys::load_secret_key(file, None).ok().map(|k| k.public_key().clone()))
}

/// PKCS#8 without a passphrase fails as a parse error rather than KeyIsEncrypted.
fn is_encrypted_pkcs8(file: &Path) -> bool {
    std::fs::read_to_string(file).is_ok_and(|pem| pem.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----"))
}

/// A server prompt that only wants the account password (not an OTP or a new password).
pub(crate) fn is_password_prompt(prompt: &str, echo: bool) -> bool {
    let text = prompt.trim().to_lowercase();
    let other = ["otp", "one-time", "one time", "new password", "verification", "token", "code"].iter().any(|w| text.contains(w));
    !echo
        && !other
        && (text.ends_with("password:")
            || text == "password"
            || (text.starts_with("password for ") && text.ends_with(':'))
            || ["密碼：", "密碼:", "密码：", "密码:"].contains(&text.as_str()))
}

mod agent {
    use russh::keys::agent::client::{AgentClient, AgentStream};

    pub type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>;

    /// The agent `IdentityAgent` names (SSH_AUTH_SOCK when unset), if one answers.
    #[cfg(unix)]
    pub async fn connect(identity_agent: Option<&str>) -> Option<Agent> {
        let path = match identity_agent {
            Some(p) if p != "SSH_AUTH_SOCK" => p.to_owned(),
            _ => std::env::var("SSH_AUTH_SOCK").ok()?,
        };
        AgentClient::connect_uds(path).await.ok().map(|a| a.dynamic())
    }

    /// Windows: OpenSSH's agent pipe, else Pageant.
    #[cfg(windows)]
    pub async fn connect(identity_agent: Option<&str>) -> Option<Agent> {
        let pipe = match identity_agent {
            Some(p) if p != "SSH_AUTH_SOCK" => p.to_owned(),
            _ => std::env::var("SSH_AUTH_SOCK").unwrap_or_else(|_| r"\\.\pipe\openssh-ssh-agent".to_owned()),
        };
        if let Ok(a) = AgentClient::connect_named_pipe(pipe).await {
            return Some(a.dynamic());
        }
        AgentClient::connect_pageant().await.ok().map(|a| a.dynamic())
    }

    #[cfg(not(any(unix, windows)))]
    pub async fn connect(_identity_agent: Option<&str>) -> Option<Agent> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_password_prompts_get_the_stored_password() {
        assert!(is_password_prompt("Password:", false));
        assert!(is_password_prompt("yuzu@lab's password:", false));
        assert!(is_password_prompt("Password for yuzu@lab:", false));
        assert!(is_password_prompt("密碼：", false));
        assert!(!is_password_prompt("Password:", true));
        assert!(!is_password_prompt("Verification code:", false));
        assert!(!is_password_prompt("New password:", false));
        assert!(!is_password_prompt("OTP password:", false));
    }
}
