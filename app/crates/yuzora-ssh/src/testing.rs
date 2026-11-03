//! A disposable world for tests: a home with its own `.ssh`, an app home, an
//! in-memory credential store. Nothing touches the person's files.

use crate::config::ConfigContext;
use crate::context::Context;
use std::path::PathBuf;
use std::sync::Arc;
use yuzora_core::secrets::MemoryStore;

pub struct TestWorld {
    pub root: PathBuf,
    pub secrets: Arc<MemoryStore>,
}

impl TestWorld {
    pub fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("yuzora-ssh-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.ssh")).unwrap();
        std::fs::create_dir_all(root.join("app/yuzora")).unwrap();
        Self { root, secrets: Arc::new(MemoryStore::default()) }
    }

    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    pub fn write_ssh_config(&self, text: &str) {
        std::fs::write(self.home().join(".ssh/config"), text).unwrap();
    }

    pub fn context(&self) -> Context {
        Context {
            config: ConfigContext { home: self.home(), local_user: "tester".into() },
            ssh_config: self.home().join(".ssh/config"),
            known_hosts: self.root.join("app/yuzora/known_hosts"),
            system_known_hosts: false,
            settings: self.root.join("app/yuzora/settings.json"),
            secrets: self.secrets.clone(),
            agent: false,
            default_identities: false,
            temp: self.root.join("tmp"),
        }
    }
}

impl Drop for TestWorld {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
