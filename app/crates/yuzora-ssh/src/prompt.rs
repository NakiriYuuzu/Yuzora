//! Questions a connection asks the person while it runs: a new or changed host
//! key, a password, a key's passphrase, keyboard-interactive answers.
//!
//! The connecting task registers a [`Prompt`] and waits; any mod (the SSH
//! sidebar, the SFTP pane, another lane's) lists [`Prompts::list`], claims one
//! with [`Prompts::claim`] so only one dialog opens, and answers it with
//! [`Prompts::answer`]. A prompt nobody answers expires after [`PROMPT_TIMEOUT`].

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

/// How long a question waits for the person.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Question {
    pub text: String,
    pub echo: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    pub id: String,
    /// The host id the connection is for, and its display name.
    pub host: String,
    pub label: String,
    /// "hostKey", "hostKeyChanged", "password", "passphrase" or "keyboard".
    pub kind: &'static str,
    /// Host key prompts: where (`host:port`), the key type and its SHA256 fingerprint.
    pub endpoint: Option<String>,
    pub algorithm: Option<String>,
    pub fingerprint: Option<String>,
    /// hostKeyChanged: the recorded fingerprints and the file holding them.
    pub previous: Vec<String>,
    pub previous_source: Option<String>,
    /// passphrase: the key file.
    pub key_path: Option<String>,
    /// keyboard: the server's name, instructions and questions.
    pub name: Option<String>,
    pub instructions: Option<String>,
    pub questions: Vec<Question>,
    /// A secret answer may be kept in the credential store.
    pub can_remember: bool,
    /// The previous answer was wrong (a password or passphrase asked again).
    pub retry: bool,
    pub claimed: bool,
}

impl Prompt {
    pub fn new(host: &str, label: &str, kind: &'static str) -> Self {
        Self {
            id: String::new(),
            host: host.to_owned(),
            label: label.to_owned(),
            kind,
            endpoint: None,
            algorithm: None,
            fingerprint: None,
            previous: Vec::new(),
            previous_source: None,
            key_path: None,
            name: None,
            instructions: None,
            questions: Vec::new(),
            can_remember: false,
            retry: false,
            claimed: false,
        }
    }
}

#[derive(Debug)]
pub enum Answer {
    /// Host key prompts.
    Trust(bool),
    /// password / passphrase.
    Secret { value: Zeroizing<String>, remember: bool },
    /// keyboard: one answer per question, and whether to keep a lone password.
    Responses { values: Vec<Zeroizing<String>>, remember: bool },
    Cancel,
}

struct Pending {
    prompt: Prompt,
    reply: oneshot::Sender<Answer>,
}

#[derive(Default)]
pub struct Prompts {
    pending: Mutex<BTreeMap<u64, Pending>>,
    next: AtomicU64,
}

/// Removes a prompt when its asker stops waiting (answered, timed out or dropped).
struct Ticket<'a> {
    prompts: &'a Prompts,
    id: u64,
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        self.prompts.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

impl Prompts {
    /// Asks and waits; `None` when it timed out or the prompt went away.
    pub async fn ask(&self, mut prompt: Prompt, timeout: Duration) -> Option<Answer> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        prompt.id = format!("p{id}");
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Pending { prompt, reply: tx });
        let _ticket = Ticket { prompts: self, id };
        tokio::time::timeout(timeout, rx).await.ok().and_then(Result::ok)
    }

    /// Every open question, oldest first.
    pub fn list(&self) -> Vec<Prompt> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).values().map(|p| p.prompt.clone()).collect()
    }

    /// Marks a prompt as shown; true only for the first caller.
    pub fn claim(&self, id: &str) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        match parse(id).and_then(|n| pending.get_mut(&n)) {
            Some(p) if !p.prompt.claimed => {
                p.prompt.claimed = true;
                true
            }
            _ => false,
        }
    }

    /// Answers a prompt; false when it is gone.
    pub fn answer(&self, id: &str, answer: Answer) -> bool {
        let taken = parse(id).and_then(|n| self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&n));
        match taken {
            Some(p) => p.reply.send(answer).is_ok(),
            None => false,
        }
    }

    /// Cancels every prompt for a host (its connection was abandoned).
    pub fn cancel_host(&self, host: &str) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<u64> = pending.iter().filter(|(_, p)| p.prompt.host == host).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(p) = pending.remove(&id) {
                let _ = p.reply.send(Answer::Cancel);
            }
        }
    }
}

fn parse(id: &str) -> Option<u64> {
    id.strip_prefix('p')?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_is_claimed_once_answered_once_and_gone_after() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let prompts = std::sync::Arc::new(Prompts::default());
            let asker = {
                let prompts = prompts.clone();
                tokio::spawn(async move { prompts.ask(Prompt::new("ssh:lab", "lab", "hostKey"), Duration::from_secs(5)).await })
            };
            while prompts.list().is_empty() {
                tokio::task::yield_now().await;
            }
            let id = prompts.list()[0].id.clone();
            assert!(prompts.claim(&id));
            assert!(!prompts.claim(&id));
            assert!(prompts.list()[0].claimed);
            assert!(prompts.answer(&id, Answer::Trust(true)));
            assert!(!prompts.answer(&id, Answer::Trust(false)));
            assert!(matches!(asker.await.unwrap(), Some(Answer::Trust(true))));
            assert!(prompts.list().is_empty());
        });
    }

    #[test]
    fn an_unanswered_prompt_times_out_and_leaves() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let prompts = Prompts::default();
            assert!(prompts.ask(Prompt::new("h", "h", "password"), Duration::from_millis(10)).await.is_none());
            assert!(prompts.list().is_empty());
        });
    }
}
