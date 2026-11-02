//! Where the crate reads and writes: the person's real files in the app, a
//! temporary world in tests. Nothing here is global, so a test never touches
//! `~/.ssh`, the system credential store or the app home.

use crate::config::ConfigContext;
use std::path::PathBuf;
use std::sync::Arc;
use yuzora_core::secrets::{KeyringStore, SecretStore};

/// The settings section the crate owns (`yuzora/settings.json` → `"ssh"`).
pub const SECTION: &str = "ssh";
/// The credential store namespace for passwords and passphrases.
pub const SECRETS: &str = "ssh";

#[derive(Clone)]
pub struct Context {
    /// Home and local user for ssh config expansion.
    pub config: ConfigContext,
    /// `~/.ssh/config`; read only.
    pub ssh_config: PathBuf,
    /// Yuzora's own known_hosts (OpenSSH format), the only one written to.
    pub known_hosts: PathBuf,
    /// Also trust what the ssh config's UserKnownHostsFile / GlobalKnownHostsFile
    /// hold (`~/.ssh/known_hosts`, …), read only.
    pub system_known_hosts: bool,
    /// Yuzora's settings.json (profiles live in its `ssh` section).
    pub settings: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    /// Ask ssh-agent (SSH_AUTH_SOCK, IdentityAgent, Windows' agent pipe) for keys.
    pub agent: bool,
    /// Try ~/.ssh/id_ed25519, id_ecdsa, id_rsa when the config names no IdentityFile.
    pub default_identities: bool,
    /// Where remote files opened for editing are downloaded.
    pub temp: PathBuf,
}

impl Context {
    /// The person's real setup.
    pub fn system() -> Self {
        let config = ConfigContext::system();
        Self {
            ssh_config: config.user_config(),
            config,
            known_hosts: yuzora_core::paths::yuzora_dir().join("known_hosts"),
            system_known_hosts: true,
            settings: yuzora_core::settings::path(),
            secrets: Arc::new(KeyringStore),
            agent: true,
            default_identities: true,
            temp: std::env::temp_dir().join("yuzora-ssh"),
        }
    }
}
