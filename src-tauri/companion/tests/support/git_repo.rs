//! Real temporary Git repositories, and the desktop's side of workspace trust.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use yuzora_host::workspace_trust::WorkspaceTrustState;

/// Name of the directory that holds every fixture repository. It stands for the
/// private part of a path: no reply to a peer may contain it.
pub const SENTINEL: &str = "SECRET-PATH-SENTINEL";

/// Runs git with the user's own configuration out of the picture; stdout.
pub fn git_input(dir: &Path, args: &[&str], input: &str) -> String {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
pub fn git(dir: &Path, args: &[&str]) -> String {
    git_input(dir, args, "")
}
/// A repository with no commit yet.
pub fn init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.email", "tester@example.com"),
        ("user.name", "Tester"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git(dir, &["config", key, value]);
    }
}

pub struct Repo {
    _tmp: tempfile::TempDir,
    /// `<tmp>/SECRET-PATH-SENTINEL/repo`, canonical.
    pub dir: PathBuf,
    pub trust_store: PathBuf,
}
impl Repo {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp
            .path()
            .canonicalize()
            .unwrap()
            .join(SENTINEL)
            .join("repo");
        init(&dir);
        Self {
            trust_store: tmp.path().join("workspace-trust.json"),
            _tmp: tmp,
            dir,
        }
    }
    pub fn write(&self, name: &str, content: impl AsRef<[u8]>) {
        let path = self.dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    /// Commits `name`, returns the commit's full hash.
    pub fn commit(&self, name: &str, content: &str, message: &str) -> String {
        self.write(name, content);
        git(&self.dir, &["add", "--", name]);
        git(&self.dir, &["commit", "-q", "-m", message]);
        self.head()
    }
    pub fn head(&self) -> String {
        git(&self.dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }
    /// The desktop's own handle on the trust file; the gateway only ever reads it.
    pub fn desktop(&self) -> WorkspaceTrustState {
        WorkspaceTrustState::at(self.trust_store.clone())
    }
    pub fn trust(&self) {
        self.desktop().0.grant_for_tests(self.dir.to_str().unwrap());
    }
    pub fn revoke(&self) {
        self.desktop().0.revoke(self.dir.to_str().unwrap()).unwrap();
    }
    /// One `spaceRoots` entry, as the desktop sends it.
    pub fn root(&self, session: &str, workspace_id: &str) -> serde_json::Value {
        serde_json::json!({
            "session": session, "workspaceId": workspace_id, "root": self.dir,
        })
    }
}
