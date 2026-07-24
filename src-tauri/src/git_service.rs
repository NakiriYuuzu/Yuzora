// M2 Task 4: git_service core (detection + run_git + git_status command)

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

pub struct GitServiceState(pub std::sync::Mutex<Option<RepoHandle>>);

#[derive(Clone)]
pub struct RepoHandle {
    pub root: PathBuf,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum GitEnvironment {
    Missing { reason: String },
    NotARepo,
    Ready { root: String, version: String },
}

pub struct GitOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: i32,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusDto {
    #[serde(flatten)]
    pub parsed: crate::git_status::ParsedStatus,
    pub in_progress: Option<String>,
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct GitProcessRegistry {
    active: Mutex<HashSet<u32>>,
}

impl GitProcessRegistry {
    fn register(&self, pid: u32) {
        if let Ok(mut active) = self.active.lock() {
            active.insert(pid);
        }
    }

    fn unregister(&self, pid: u32) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&pid);
        }
    }

    fn drain(&self) -> Vec<u32> {
        match self.active.lock() {
            Ok(mut active) => active.drain().collect(),
            Err(_) => Vec::new(),
        }
    }
}

static ACTIVE_GIT_PROCESSES: LazyLock<GitProcessRegistry> =
    LazyLock::new(GitProcessRegistry::default);

struct ActiveGitProcessGuard {
    pid: u32,
}

impl Drop for ActiveGitProcessGuard {
    fn drop(&mut self) {
        ACTIVE_GIT_PROCESSES.unregister(self.pid);
    }
}

pub fn kill_all_processes() {
    for pid in ACTIVE_GIT_PROCESSES.drain() {
        let _ = crate::process_kill::kill_tree_pid(pid);
    }
}

/// 純函式核心：偵測 git 環境。commands 是薄包裝。
pub fn detect_environment(path: &Path) -> GitEnvironment {
    let version_out = match run_git(path, &["--version"], DEFAULT_TIMEOUT, &[]) {
        Ok(out) => out,
        Err(_) => {
            return GitEnvironment::Missing {
                reason: "git binary not found or failed to spawn".to_string(),
            }
        }
    };
    let version = String::from_utf8_lossy(&version_out.stdout)
        .trim()
        .to_string();
    match parse_git_version(&version) {
        Some((major, minor)) if (major, minor) >= (2, 11) => {}
        _ => {
            return GitEnvironment::Missing {
                reason: format!("git version below 2.11 (porcelain v2 required): {version}"),
            }
        }
    }

    match run_git(
        path,
        &["rev-parse", "--show-toplevel"],
        DEFAULT_TIMEOUT,
        &[],
    ) {
        Ok(out) if out.code == 0 => {
            let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
            GitEnvironment::Ready { root, version }
        }
        _ => GitEnvironment::NotARepo,
    }
}

fn parse_git_version(version: &str) -> Option<(u32, u32)> {
    // e.g. "git version 2.50.1 (Apple Git-155)"
    let nums = version
        .split_whitespace()
        .find(|tok| tok.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = nums.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

pub fn run_git(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(String, String)],
) -> Result<GitOutput, String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .envs(extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process_kill::configure_background_process(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("git spawn failed: {e}"))?;
    let active_process = ActiveGitProcessGuard { pid: child.id() };
    ACTIVE_GIT_PROCESSES.register(active_process.pid);
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_thread = std::thread::spawn(move || {
        use std::io::Read;
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let err_thread = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let deadline = std::time::Instant::now() + timeout;
    let code = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(st) => break st.code().unwrap_or(-1),
            None if std::time::Instant::now() > deadline => {
                let _ = crate::process_kill::kill_tree(&mut child);
                return Err(format!(
                    "git {} timed out after {:?}",
                    args.first().unwrap_or(&""),
                    timeout
                ));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let out = GitOutput {
        stdout: out_thread.join().unwrap_or_default(),
        stderr: err_thread.join().unwrap_or_default(),
        code,
    };
    log_git_call(args, out.code, &out.stderr);
    Ok(out)
}

/// debug log：args join（URL userinfo 遮蔽）、code、stderr 前 200 字；不記 extra_env。
/// 走共享全域 sink——cargo test 下自動重導 tempdir，不汙染 ~/.yuzora/logs。
fn log_git_call(args: &[&str], code: i32, stderr: &str) {
    let stderr_head: String = stderr.chars().take(200).collect();
    crate::logging::write_global(crate::logging::LogEvent {
        level: "debug".to_string(),
        kind: "debug".to_string(),
        source: "git_service".to_string(),
        workspace_path: None,
        event: "run_git".to_string(),
        message: crate::logging::mask_url_userinfo(&format!("git {}", args.join(" "))),
        metadata: serde_json::json!({ "code": code, "stderr": stderr_head }),
    });
}

/// in_progress 判定：優先序 rebase > merge > cherry-pick > revert。
fn detect_in_progress(git_dir: &Path) -> Option<String> {
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        Some("rebase".to_string())
    } else if git_dir.join("MERGE_HEAD").exists() {
        Some("merge".to_string())
    } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
        Some("cherry-pick".to_string())
    } else if git_dir.join("REVERT_HEAD").exists() {
        Some("revert".to_string())
    } else {
        None
    }
}

/// 純函式核心：跑 status --porcelain=v2 並解析。commands 是薄包裝。
pub fn status_of(root: &Path, pathspec: Option<Vec<String>>) -> Result<GitStatusDto, String> {
    // `all` is safety-critical for path-scoped rollback: the default `normal`
    // mode collapses an untracked tree to `scratch/`, which would hide dirty
    // editor descendants while `git clean -fd -- scratch/` deletes them all.
    let mut args: Vec<&str> = vec![
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=all",
        "-z",
    ];
    let spec = pathspec.unwrap_or_default();
    if !spec.is_empty() {
        args.push("--");
        for p in &spec {
            args.push(p.as_str());
        }
    }
    let out = run_git(root, &args, DEFAULT_TIMEOUT, &[])?;
    if out.code != 0 {
        return Err(format!("git status failed: {}", out.stderr.trim()));
    }
    let parsed = crate::git_status::parse_porcelain_v2(&out.stdout)?;
    let in_progress = detect_in_progress(&root.join(".git"));
    Ok(GitStatusDto {
        parsed,
        in_progress,
    })
}

#[tauri::command]
pub fn git_detect(
    app: tauri::AppHandle,
    state: tauri::State<'_, GitServiceState>,
    watch_state: tauri::State<'_, crate::git_watch::GitWatchState>,
    path: String,
) -> Result<GitEnvironment, String> {
    use tauri::Emitter;
    let env = detect_environment(Path::new(&path));
    match env {
        GitEnvironment::Ready { ref root, .. } => {
            *state.0.lock().map_err(|e| e.to_string())? = Some(RepoHandle {
                root: PathBuf::from(root),
            });
            // Ready → 啟動 .git watcher；舊 debouncer 被替換即 drop 停止。
            let git_dir = PathBuf::from(root).join(".git");
            let watcher = crate::git_watch::build_git_watcher(&git_dir, move || {
                let _ = app.emit("git:state-changed", ());
            })?;
            *watch_state.0.lock().map_err(|e| e.to_string())? = Some(watcher);
        }
        GitEnvironment::NotARepo | GitEnvironment::Missing { .. } => {
            // Clear the authority as well as the watcher. Leaving the previous
            // RepoHandle alive would let an in-flight request for that old root
            // pass root validation after the UI switched to a non-repository.
            *state.0.lock().map_err(|e| e.to_string())? = None;
            // 非 repo / 無 git → 清空 watch state（drop 即停）。
            *watch_state.0.lock().map_err(|e| e.to_string())? = None;
        }
    }
    Ok(env)
}

#[tauri::command]
pub fn git_status_cmd(
    state: tauri::State<'_, GitServiceState>,
    pathspec: Option<Vec<String>>,
) -> Result<GitStatusDto, String> {
    let root = {
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| "no repository detected".to_string())?
            .root
            .clone()
    };
    status_of(&root, pathspec)
}

// ── M2 Task 6: git 操作 commands（stage/commit/branch/remote/diff/conflict）────

const REMOTE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub is_current: bool,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BranchList {
    pub local: Vec<BranchInfo>,
    pub remote: Vec<String>,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GradedText {
    Full { content: String },
    Limited { content: String },
    TooLarge,
    Binary,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiffContent {
    pub original: GradedText,
    pub modified: GradedText,
}

/// Frontend 對單一路徑所見的完整 status 快照。Rollback 執行前會和最新
/// porcelain v2 status 做 exact match，避免 stale menu 對已變化的檔案動手。
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GitRollbackClassification {
    Tracked {
        staged_status: Option<String>,
        unstaged_status: Option<String>,
        orig_path: Option<String>,
    },
    Added {
        staged_status: Option<String>,
        unstaged_status: Option<String>,
    },
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRollbackTarget {
    pub path: String,
    pub classification: GitRollbackClassification,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRollbackResult {
    pub restored: Vec<String>,
    pub preserved_untracked: Vec<String>,
    pub deleted: Vec<String>,
}

/// 非零 exit → 統一錯誤格式 "git <sub>: <stderr 摘要 500 字>"。
pub(crate) fn git_err(sub: &str, stderr: &str) -> String {
    let summary: String = stderr.trim().chars().take(500).collect();
    format!("git {sub}: {summary}")
}

/// 跑一個必須成功（code==0）的 git 指令；非零回統一錯誤格式。
fn run_ok(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    env: &[(String, String)],
) -> Result<GitOutput, String> {
    let out = run_git(root, args, timeout, env)?;
    if out.code != 0 {
        return Err(git_err(args.first().unwrap_or(&""), &out.stderr));
    }
    Ok(out)
}

pub fn stage(root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    run_ok(root, &args, DEFAULT_TIMEOUT, &[])?;
    Ok(())
}

pub fn unstage(root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(String::as_str));
    run_ok(root, &args, DEFAULT_TIMEOUT, &[])?;
    Ok(())
}

/// tracked → restore --；untracked → clean -f --（前端已確認過 confirm）。
pub fn discard(root: &Path, paths: &[String], untracked: &[String]) -> Result<(), String> {
    if !paths.is_empty() {
        let mut args: Vec<&str> = vec!["restore", "--"];
        args.extend(paths.iter().map(String::as_str));
        run_ok(root, &args, DEFAULT_TIMEOUT, &[])?;
    }
    if !untracked.is_empty() {
        let mut args: Vec<&str> = vec!["clean", "-f", "--"];
        args.extend(untracked.iter().map(String::as_str));
        run_ok(root, &args, DEFAULT_TIMEOUT, &[])?;
    }
    Ok(())
}

#[derive(Debug)]
enum GitRollbackPlan {
    Tracked {
        path: String,
        restore_paths: Vec<String>,
    },
    Added {
        path: String,
    },
    Untracked {
        path: String,
    },
}

/// Git pathspec 必須是 repo-relative，且現存檔案（或最近的現存 parent）不可經
/// symlink 逃出 repo。此檢查也套用於由最新 status 取得的 rename origPath。
fn validate_repo_relative_path(
    root: &Path,
    canonical_root: &Path,
    value: &str,
) -> Result<(), String> {
    use std::path::Component;

    if value.is_empty() || value.contains('\0') {
        return Err("git rollback rejected an empty or NUL-containing path".to_string());
    }
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "git rollback rejected non-repo-relative path: {value}"
        ));
    }

    let joined = root.join(relative);
    let mut existing = joined.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            format!("git rollback could not resolve path inside repository: {value}")
        })?;
    }
    let canonical_existing = existing
        .canonicalize()
        .map_err(|e| format!("git rollback could not resolve {value}: {e}"))?;
    if !canonical_existing.starts_with(canonical_root) {
        return Err(format!(
            "git rollback rejected path outside repository: {value}"
        ));
    }
    Ok(())
}

fn actual_rollback_classification(
    parsed: &crate::git_status::ParsedStatus,
    path: &str,
) -> Result<Option<GitRollbackClassification>, String> {
    if parsed.conflicted.iter().any(|entry| entry.path == path) {
        return Ok(Some(GitRollbackClassification::Conflicted));
    }
    if parsed.untracked.iter().any(|entry| entry == path) {
        return Ok(Some(GitRollbackClassification::Untracked));
    }

    let staged = parsed.staged.iter().find(|entry| entry.path == path);
    let unstaged = parsed.unstaged.iter().find(|entry| entry.path == path);
    if staged.is_none() && unstaged.is_none() {
        return Ok(None);
    }

    let staged_orig = staged.and_then(|entry| entry.orig_path.clone());
    let unstaged_orig = unstaged.and_then(|entry| entry.orig_path.clone());
    if staged_orig.is_some() && unstaged_orig.is_some() && staged_orig != unstaged_orig {
        return Err(format!(
            "git rollback found inconsistent rename origins for {path}"
        ));
    }
    let orig_path = staged_orig.or(unstaged_orig);
    let staged_status = staged.map(|entry| entry.status.clone());
    let unstaged_status = unstaged.map(|entry| entry.status.clone());
    let is_added = staged_status.as_deref() == Some("A") || unstaged_status.as_deref() == Some("A");

    if is_added {
        if orig_path.is_some() {
            return Err(format!(
                "git rollback found an added path with a rename origin: {path}"
            ));
        }
        Ok(Some(GitRollbackClassification::Added {
            staged_status,
            unstaged_status,
        }))
    } else {
        Ok(Some(GitRollbackClassification::Tracked {
            staged_status,
            unstaged_status,
            orig_path,
        }))
    }
}

fn rollback_failure(path: &str, stage: &str, completed: &[String], error: String) -> String {
    let completed = if completed.is_empty() {
        "none".to_string()
    } else {
        completed.join(", ")
    };
    format!("git rollback failed for {path} during {stage}; completed stages: {completed}; {error}")
}

/// JetBrains-aligned path-scoped rollback。所有 target 先做 path + latest-status preflight；
/// preflight 全數通過後才開始 mutation，避免 stale selection 造成部分改動。
pub fn rollback_paths(
    root: &Path,
    targets: &[GitRollbackTarget],
    delete_untracked_or_added: bool,
) -> Result<GitRollbackResult, String> {
    if targets.is_empty() {
        return Err("git rollback paths requires at least one target".to_string());
    }

    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("git rollback could not resolve repository root: {e}"))?;
    let latest = status_of(root, None)?;
    let has_head = latest.parsed.head_oid != "(initial)";
    let mut seen = std::collections::HashSet::new();
    let mut plans = Vec::with_capacity(targets.len());

    for target in targets {
        validate_repo_relative_path(root, &canonical_root, &target.path)?;
        if !seen.insert(target.path.clone()) {
            return Err(format!(
                "git rollback rejected duplicate target path: {}",
                target.path
            ));
        }

        let actual = actual_rollback_classification(&latest.parsed, &target.path)?
            .ok_or_else(|| format!("git rollback target is no longer changed: {}", target.path))?;
        if matches!(actual, GitRollbackClassification::Conflicted)
            || matches!(target.classification, GitRollbackClassification::Conflicted)
        {
            return Err(format!(
                "git rollback rejected conflicted path: {}",
                target.path
            ));
        }
        if actual != target.classification {
            let expected = serde_json::to_string(&target.classification)
                .unwrap_or_else(|_| format!("{:?}", target.classification));
            let actual_text =
                serde_json::to_string(&actual).unwrap_or_else(|_| format!("{actual:?}"));
            return Err(format!(
                "git rollback classification drift for {}: expected {}, latest {}",
                target.path, expected, actual_text
            ));
        }

        match actual {
            GitRollbackClassification::Tracked { orig_path, .. } => {
                if !has_head {
                    return Err(format!(
                        "git rollback cannot restore tracked path without HEAD: {}",
                        target.path
                    ));
                }
                let mut restore_paths = Vec::with_capacity(2);
                if let Some(orig_path) = orig_path {
                    validate_repo_relative_path(root, &canonical_root, &orig_path)?;
                    restore_paths.push(orig_path);
                }
                if !restore_paths.iter().any(|path| path == &target.path) {
                    restore_paths.push(target.path.clone());
                }
                plans.push(GitRollbackPlan::Tracked {
                    path: target.path.clone(),
                    restore_paths,
                });
            }
            GitRollbackClassification::Added { .. } => {
                plans.push(GitRollbackPlan::Added {
                    path: target.path.clone(),
                });
            }
            GitRollbackClassification::Untracked => {
                plans.push(GitRollbackPlan::Untracked {
                    path: target.path.clone(),
                });
            }
            GitRollbackClassification::Conflicted => unreachable!("rejected above"),
        }
    }

    let mut result = GitRollbackResult {
        restored: Vec::new(),
        preserved_untracked: Vec::new(),
        deleted: Vec::new(),
    };
    let mut completed = Vec::new();

    for plan in plans {
        match plan {
            GitRollbackPlan::Tracked {
                path,
                restore_paths,
            } => {
                let mut args: Vec<&str> =
                    vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
                args.extend(restore_paths.iter().map(String::as_str));
                run_ok(root, &args, DEFAULT_TIMEOUT, &[]).map_err(|error| {
                    rollback_failure(&path, "restore tracked path", &completed, error)
                })?;
                completed.push(format!("restore:{path}"));
                result.restored.push(path);
            }
            GitRollbackPlan::Added { path } => {
                run_ok(
                    root,
                    &["rm", "--cached", "-f", "--", path.as_str()],
                    DEFAULT_TIMEOUT,
                    &[],
                )
                .map_err(|error| {
                    rollback_failure(&path, "unstage added path", &completed, error)
                })?;
                completed.push(format!("unstage-added:{path}"));

                if delete_untracked_or_added {
                    run_ok(
                        root,
                        &["clean", "-fd", "--", path.as_str()],
                        DEFAULT_TIMEOUT,
                        &[],
                    )
                    .map_err(|error| {
                        rollback_failure(&path, "delete added path", &completed, error)
                    })?;
                    completed.push(format!("clean-command:{path}"));
                    if std::fs::symlink_metadata(root.join(&path)).is_ok() {
                        return Err(rollback_failure(
                            &path,
                            "verify added path deletion",
                            &completed,
                            "git clean completed but left the path in place".to_string(),
                        ));
                    }
                    completed.push(format!("delete:{path}"));
                    result.deleted.push(path);
                } else {
                    completed.push(format!("preserve-untracked:{path}"));
                    result.preserved_untracked.push(path);
                }
            }
            GitRollbackPlan::Untracked { path } => {
                if delete_untracked_or_added {
                    run_ok(
                        root,
                        &["clean", "-fd", "--", path.as_str()],
                        DEFAULT_TIMEOUT,
                        &[],
                    )
                    .map_err(|error| {
                        rollback_failure(&path, "delete untracked path", &completed, error)
                    })?;
                    completed.push(format!("clean-command:{path}"));
                    if std::fs::symlink_metadata(root.join(&path)).is_ok() {
                        return Err(rollback_failure(
                            &path,
                            "verify untracked path deletion",
                            &completed,
                            "git clean completed but left the path in place".to_string(),
                        ));
                    }
                    completed.push(format!("delete:{path}"));
                    result.deleted.push(path);
                } else {
                    completed.push(format!("preserve-untracked:{path}"));
                    result.preserved_untracked.push(path);
                }
            }
        }
    }

    Ok(result)
}

pub fn commit(root: &Path, message: &str) -> Result<(), String> {
    run_ok(root, &["commit", "-m", message], DEFAULT_TIMEOUT, &[])?;
    Ok(())
}

pub fn branches(root: &Path) -> Result<BranchList, String> {
    let out = run_git(
        root,
        &[
            "for-each-ref",
            "refs/heads",
            "--format=%(HEAD)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)",
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        return Err(git_err("for-each-ref", &out.stderr));
    }
    let mut local = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 4 {
            continue;
        }
        let (mut ahead, mut behind) = (0u32, 0u32);
        // %(upstream:track) 形如 "[ahead 2, behind 1]"、"[gone]" 或空字串
        for part in f[3]
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(", ")
        {
            if let Some(n) = part.strip_prefix("ahead ") {
                ahead = n.parse().unwrap_or(0)
            }
            if let Some(n) = part.strip_prefix("behind ") {
                behind = n.parse().unwrap_or(0)
            }
        }
        local.push(BranchInfo {
            name: f[1].to_string(),
            upstream: if f[2].is_empty() {
                None
            } else {
                Some(f[2].to_string())
            },
            ahead,
            behind,
            is_current: f[0] == "*",
        })
    }
    let remotes = run_git(
        root,
        &["for-each-ref", "refs/remotes", "--format=%(refname:short)"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if remotes.code != 0 {
        return Err(git_err("for-each-ref", &remotes.stderr));
    }
    let remote = String::from_utf8_lossy(&remotes.stdout)
        .lines()
        .filter(|l| !l.ends_with("/HEAD"))
        .map(String::from)
        .collect();
    Ok(BranchList { local, remote })
}

pub fn create_branch(root: &Path, name: &str) -> Result<(), String> {
    run_ok(root, &["switch", "-c", name], DEFAULT_TIMEOUT, &[])?;
    Ok(())
}

pub fn checkout(root: &Path, name: &str) -> Result<(), String> {
    run_ok(root, &["switch", name], DEFAULT_TIMEOUT, &[])?;
    Ok(())
}

/// cherry-pick <hash>。GUI 無 TTY：GIT_EDITOR=true 防 sequencer editor 卡死（乾淨 pick
/// 會沿用原訊息、通常不開 editor，但保險）。衝突留 CHERRY_PICK_HEAD → 前端接 ConflictBanner。
pub fn cherry_pick(root: &Path, hash: &str) -> Result<(), String> {
    run_ok(
        root,
        &["cherry-pick", hash],
        DEFAULT_TIMEOUT,
        &editor_true(),
    )?;
    Ok(())
}

/// 契約：op ∈ merge|rebase|cherry-pick|revert（brief 明列）。校驗攔截任意 subcommand。
fn check_conflict_op(op: &str) -> Result<(), String> {
    if !matches!(op, "merge" | "rebase" | "cherry-pick" | "revert") {
        return Err(format!("invalid conflict op: {op}"));
    }
    Ok(())
}

/// merge|rebase|cherry-pick|revert → <op> --abort
pub fn conflict_abort(root: &Path, op: &str) -> Result<(), String> {
    check_conflict_op(op)?;
    run_ok(root, &[op, "--abort"], DEFAULT_TIMEOUT, &[])?;
    Ok(())
}

/// <op> --continue。GUI 無 TTY：GIT_EDITOR=true 讓 commit message editor 立即成功退出
/// （否則 git 報「Terminal is dumb, but EDITOR unset」exit 1）。GIT_TERMINAL_PROMPT=0 只擋
/// credential prompt、不擋 editor，故需另加。
pub fn conflict_continue(root: &Path, op: &str) -> Result<(), String> {
    check_conflict_op(op)?;
    run_ok(root, &[op, "--continue"], DEFAULT_TIMEOUT, &editor_true())?;
    Ok(())
}

/// GUI 環境無 TTY 時抑制 git 開 editor（continue/pull 沿用既有 commit message）。
fn editor_true() -> Vec<(String, String)> {
    vec![("GIT_EDITOR".to_string(), "true".to_string())]
}

/// bytes 過分級：與 fs_service::classify_and_read 同標準，但輸入是 bytes 而非 path，
/// 且回 GradedText（無 size 欄）以符 T9 types.ts DiffContent 契約。
fn grade_bytes(bytes: &[u8]) -> GradedText {
    if bytes.len() as u64 > crate::file_content::HARD_CAP_BYTES {
        return GradedText::TooLarge;
    }
    let sniff = &bytes[..bytes.len().min(crate::file_content::FILE_ANALYSIS_BYTES)];
    if crate::file_content::analyze_byte_content(sniff) == crate::file_content::ByteContent::Binary
    {
        return GradedText::Binary;
    }
    let content = String::from_utf8_lossy(bytes).into_owned();
    if bytes.len() as u64 > crate::file_content::FULL_FEATURE_MAX_BYTES {
        GradedText::Limited { content }
    } else {
        GradedText::Full { content }
    }
}

/// `git show <rev>:<path>` 取物件 bytes。物件不存在（exit!=0）→ None（呼叫端當空內容處理）。
fn show_object(root: &Path, spec: &str) -> Result<Option<Vec<u8>>, String> {
    let out = run_git(root, &["show", spec], DEFAULT_TIMEOUT, &[])?;
    if out.code != 0 {
        return Ok(None);
    }
    Ok(Some(out.stdout))
}

/// 讀工作樹檔案 bytes；不存在（刪除）→ None。
fn read_worktree(root: &Path, path: &str) -> Option<Vec<u8>> {
    std::fs::read(root.join(path)).ok()
}

pub fn diff_content(root: &Path, path: &str, staged: bool) -> Result<DiffContent, String> {
    let (original, modified) = if staged {
        // staged：original=show HEAD:<path>（新增檔 HEAD 無此檔→空）；modified=show :0:<path>（index）
        let orig = show_object(root, &format!("HEAD:{path}"))?;
        let modi = show_object(root, &format!(":0:{path}"))?;
        (
            orig.map(|b| grade_bytes(&b)).unwrap_or(GradedText::Full {
                content: String::new(),
            }),
            modi.map(|b| grade_bytes(&b)).unwrap_or(GradedText::Full {
                content: String::new(),
            }),
        )
    } else {
        // unstaged：original=show :0:<path>（index；untracked 無→空）；modified=工作樹（刪除→空）
        let orig = show_object(root, &format!(":0:{path}"))?;
        let modi = read_worktree(root, path);
        (
            orig.map(|b| grade_bytes(&b)).unwrap_or(GradedText::Full {
                content: String::new(),
            }),
            modi.map(|b| grade_bytes(&b)).unwrap_or(GradedText::Full {
                content: String::new(),
            }),
        )
    };
    Ok(DiffContent { original, modified })
}

/// remote_probe：無 upstream→"unknown"；本地=遠端→"no"；不等→"yes"；任何遠端存取失敗→"unknown"。
/// askpass env 一律 background=1（背景鐵律）；timeout 30s。
pub fn remote_probe(root: &Path, env: &[(String, String)]) -> Result<String, String> {
    let up = run_git(
        root,
        &["rev-parse", "--abbrev-ref", "@{upstream}"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if up.code != 0 {
        return Ok("unknown".to_string());
    }
    let upstream = String::from_utf8_lossy(&up.stdout).trim().to_string();
    let (remote, branch) = match upstream.split_once('/') {
        Some((r, b)) => (r.to_string(), b.to_string()),
        None => return Ok("unknown".to_string()),
    };
    let local = run_git(root, &["rev-parse", "@{upstream}"], DEFAULT_TIMEOUT, &[])?;
    if local.code != 0 {
        return Ok("unknown".to_string());
    }
    let local_sha = String::from_utf8_lossy(&local.stdout).trim().to_string();
    let ls = run_git(
        root,
        &["ls-remote", &remote, &format!("refs/heads/{branch}")],
        DEFAULT_TIMEOUT,
        env,
    )?;
    if ls.code != 0 {
        return Ok("unknown".to_string());
    }
    let remote_sha = String::from_utf8_lossy(&ls.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if remote_sha.is_empty() {
        return Ok("unknown".to_string());
    }
    Ok(if remote_sha == local_sha { "no" } else { "yes" }.to_string())
}

/// commands 共用：取當前 repo root。
fn repo_root(state: &tauri::State<'_, GitServiceState>) -> Result<PathBuf, String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    Ok(guard
        .as_ref()
        .ok_or_else(|| "no repository detected".to_string())?
        .root
        .clone())
}

/// Bind a mutating request to the repository snapshot the frontend acted on.
/// The state lock stays held through `operation`, making compare + mutation
/// atomic relative to `git_detect` switching the active repository.
fn with_requested_repo<T>(
    state: &GitServiceState,
    requested_root: &str,
    operation: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    let active = &guard
        .as_ref()
        .ok_or_else(|| "no repository detected".to_string())?
        .root;
    let canonical_active = active
        .canonicalize()
        .map_err(|e| format!("git could not resolve active repository root: {e}"))?;
    let canonical_requested = Path::new(requested_root)
        .canonicalize()
        .map_err(|e| format!("git could not resolve requested repository root: {e}"))?;
    if canonical_active != canonical_requested {
        return Err(format!(
            "git repository changed before operation: requested {}, active {}",
            canonical_requested.display(),
            canonical_active.display()
        ));
    }
    operation(&canonical_active)
}

#[tauri::command]
pub fn git_stage(
    state: tauri::State<'_, GitServiceState>,
    repository_root: String,
    paths: Vec<String>,
) -> Result<(), String> {
    with_requested_repo(state.inner(), &repository_root, |root| stage(root, &paths))
}

#[tauri::command]
pub fn git_unstage(
    state: tauri::State<'_, GitServiceState>,
    repository_root: String,
    paths: Vec<String>,
) -> Result<(), String> {
    with_requested_repo(state.inner(), &repository_root, |root| {
        unstage(root, &paths)
    })
}

#[tauri::command]
pub fn git_discard(
    state: tauri::State<'_, GitServiceState>,
    repository_root: String,
    paths: Vec<String>,
    untracked: Vec<String>,
) -> Result<(), String> {
    with_requested_repo(state.inner(), &repository_root, |root| {
        discard(root, &paths, &untracked)
    })
}

#[tauri::command]
pub fn git_rollback_paths(
    state: tauri::State<'_, GitServiceState>,
    repository_root: String,
    targets: Vec<GitRollbackTarget>,
    delete_untracked_or_added: bool,
) -> Result<GitRollbackResult, String> {
    with_requested_repo(state.inner(), &repository_root, |root| {
        rollback_paths(root, &targets, delete_untracked_or_added)
    })
}

#[tauri::command]
pub fn git_commit_cmd(
    state: tauri::State<'_, GitServiceState>,
    message: String,
) -> Result<(), String> {
    commit(&repo_root(&state)?, &message)
}

#[tauri::command]
pub fn git_branches(state: tauri::State<'_, GitServiceState>) -> Result<BranchList, String> {
    branches(&repo_root(&state)?)
}

#[tauri::command]
pub fn git_create_branch(
    state: tauri::State<'_, GitServiceState>,
    name: String,
) -> Result<(), String> {
    create_branch(&repo_root(&state)?, &name)
}

#[tauri::command]
pub fn git_checkout(state: tauri::State<'_, GitServiceState>, name: String) -> Result<(), String> {
    checkout(&repo_root(&state)?, &name)
}

#[tauri::command]
pub fn git_cherry_pick(
    state: tauri::State<'_, GitServiceState>,
    hash: String,
) -> Result<(), String> {
    cherry_pick(&repo_root(&state)?, &hash)
}

#[tauri::command]
pub fn git_fetch_cmd(
    state: tauri::State<'_, GitServiceState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
    background: bool,
    repository_root: Option<String>,
) -> Result<(), String> {
    let env = askpass.env_for(background);
    if let Some(requested_root) = repository_root {
        with_requested_repo(state.inner(), &requested_root, |root| {
            run_ok(root, &["fetch"], REMOTE_TIMEOUT, &env).map(|_| ())
        })
    } else {
        run_ok(&repo_root(&state)?, &["fetch"], REMOTE_TIMEOUT, &env).map(|_| ())
    }
}

#[tauri::command]
pub fn git_pull_cmd(
    state: tauri::State<'_, GitServiceState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
    repository_root: Option<String>,
) -> Result<(), String> {
    // pull 觸發的 merge commit 在無 TTY 下同樣需抑制 editor（見 conflict_continue 註）。
    let mut env = askpass.env_for(false);
    env.extend(editor_true());
    if let Some(requested_root) = repository_root {
        with_requested_repo(state.inner(), &requested_root, |root| {
            run_ok(root, &["pull"], REMOTE_TIMEOUT, &env).map(|_| ())
        })
    } else {
        run_ok(&repo_root(&state)?, &["pull"], REMOTE_TIMEOUT, &env).map(|_| ())
    }
}

#[tauri::command]
pub fn git_push_cmd(
    state: tauri::State<'_, GitServiceState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
    repository_root: Option<String>,
) -> Result<(), String> {
    let env = askpass.env_for(false);
    if let Some(requested_root) = repository_root {
        with_requested_repo(state.inner(), &requested_root, |root| {
            run_ok(root, &["push"], REMOTE_TIMEOUT, &env).map(|_| ())
        })
    } else {
        run_ok(&repo_root(&state)?, &["push"], REMOTE_TIMEOUT, &env).map(|_| ())
    }
}

#[tauri::command]
pub fn git_remote_probe(
    state: tauri::State<'_, GitServiceState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
) -> Result<String, String> {
    let root = repo_root(&state)?;
    let env = askpass.env_for(true);
    remote_probe(&root, &env)
}

#[tauri::command]
pub fn git_diff_content(
    state: tauri::State<'_, GitServiceState>,
    path: String,
    staged: bool,
) -> Result<DiffContent, String> {
    diff_content(&repo_root(&state)?, &path, staged)
}

#[tauri::command]
pub fn git_conflict_abort(
    state: tauri::State<'_, GitServiceState>,
    op: String,
) -> Result<(), String> {
    conflict_abort(&repo_root(&state)?, &op)
}

#[tauri::command]
pub fn git_conflict_continue(
    state: tauri::State<'_, GitServiceState>,
    op: String,
) -> Result<(), String> {
    conflict_continue(&repo_root(&state)?, &op)
}

#[cfg(test)]
pub mod test_repo {
    use super::run_git;
    use std::path::Path;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(30);

    /// 隔離使用者設定：GIT_CONFIG_GLOBAL / GIT_CONFIG_SYSTEM 指向 /dev/null。
    /// 所有 fixture git 呼叫共用。
    fn isolated_env() -> Vec<(String, String)> {
        vec![
            ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
            ("GIT_CONFIG_SYSTEM".to_string(), "/dev/null".to_string()),
        ]
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = run_git(dir, args, TIMEOUT, &isolated_env())
            .unwrap_or_else(|e| panic!("git {args:?} failed: {e}"));
        assert_eq!(
            out.code, 0,
            "git {:?} exited {}: {}",
            args, out.code, out.stderr
        );
    }

    pub fn init(dir: &Path) {
        git(dir, &["init", "-b", "main"]);
        git(dir, &["config", "user.email", "t@t"]);
        git(dir, &["config", "user.name", "t"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
    }

    pub fn write_and_commit(dir: &Path, name: &str, content: &str, msg: &str) {
        std::fs::write(dir.join(name), content).unwrap();
        git(dir, &["add", name]);
        git(dir, &["commit", "-m", msg]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rollback_target(path: &str, classification: GitRollbackClassification) -> GitRollbackTarget {
        GitRollbackTarget {
            path: path.to_string(),
            classification,
        }
    }

    fn tracked_classification(
        staged_status: Option<&str>,
        unstaged_status: Option<&str>,
        orig_path: Option<&str>,
    ) -> GitRollbackClassification {
        GitRollbackClassification::Tracked {
            staged_status: staged_status.map(String::from),
            unstaged_status: unstaged_status.map(String::from),
            orig_path: orig_path.map(String::from),
        }
    }

    fn added_classification(
        staged_status: Option<&str>,
        unstaged_status: Option<&str>,
    ) -> GitRollbackClassification {
        GitRollbackClassification::Added {
            staged_status: staged_status.map(String::from),
            unstaged_status: unstaged_status.map(String::from),
        }
    }

    #[test]
    fn detect_ready_on_fixture_repo() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        let env = detect_environment(tmp.path());
        match env {
            GitEnvironment::Ready { .. } => {}
            other => panic!("{:?} not Ready", serde_json::to_value(&other).unwrap()),
        }
    }

    #[test]
    fn detect_not_a_repo_on_plain_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            detect_environment(tmp.path()),
            GitEnvironment::NotARepo
        ));
    }

    #[test]
    fn run_git_times_out_and_kills() {
        // 用會掛住的 git 指令模擬 timeout：git alias 執行 `sleep 30`。
        // （brief 原稿的 `credential fill` 在 stdin=null 下會即刻退出，無法觸發 timeout 路徑；
        //  改用 alias-sleep 是確定性 hang，仍是真正的 git 子行程，驗證同樣的 kill 行為。）
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        let started = std::time::Instant::now();
        let r = run_git(
            tmp.path(),
            &["-c", "alias.hang=!sleep 30", "hang"],
            Duration::from_millis(300),
            &[],
        );
        assert!(r.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn run_git_timeout_kills_grandchild() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        let pid_file = tmp.path().join("grandchild.pid");
        let alias = format!(
            "alias.hang=!sh -c 'sleep 30 & echo $! > \"{}\"; wait'",
            pid_file.display()
        );
        let started = std::time::Instant::now();
        let r = run_git(
            tmp.path(),
            &["-c", alias.as_str(), "hang"],
            Duration::from_millis(300),
            &[],
        );
        assert!(r.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("pid file exists")
            .trim()
            .parse()
            .expect("pid is numeric");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            let alive = unsafe { libc::kill(pid as libc::pid_t, 0) == 0 };
            if !alive {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("grandchild {pid} still exists after timeout");
    }

    #[test]
    fn status_detects_merge_in_progress() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        test_repo::write_and_commit(tmp.path(), "a.txt", "base\n", "c1");
        std::fs::write(tmp.path().join(".git/MERGE_HEAD"), "deadbeef\n").unwrap();
        let dto = status_of(tmp.path(), None).unwrap();
        assert_eq!(dto.in_progress.as_deref(), Some("merge"));
    }

    #[test]
    fn status_lists_staged_and_untracked_via_real_git() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        test_repo::write_and_commit(tmp.path(), "a.txt", "one\n", "c1");
        std::fs::write(tmp.path().join("a.txt"), "two\n").unwrap();
        std::fs::write(tmp.path().join("b new.txt"), "x\n").unwrap();
        let dto = status_of(tmp.path(), None).unwrap();
        assert_eq!(dto.parsed.unstaged[0].path, "a.txt");
        assert_eq!(dto.parsed.untracked, vec!["b new.txt".to_string()]);
    }

    #[test]
    fn status_expands_untracked_directories_to_leaf_paths_for_safe_ui_gates() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "base.txt", "base\n", "base");
        std::fs::create_dir_all(r.join("scratch/sub")).unwrap();
        std::fs::write(r.join("scratch/b.txt"), "b\n").unwrap();
        std::fs::write(r.join("scratch/sub/a.txt"), "a\n").unwrap();

        let status = status_of(r, None).unwrap().parsed;
        assert_eq!(
            status.untracked,
            vec!["scratch/b.txt".to_string(), "scratch/sub/a.txt".to_string()]
        );

        let targets: Vec<_> = status
            .untracked
            .iter()
            .map(|path| rollback_target(path, GitRollbackClassification::Untracked))
            .collect();
        let result = rollback_paths(r, &targets, true).unwrap();
        assert_eq!(result.deleted, status.untracked);
        assert!(!r.join("scratch/b.txt").exists());
        assert!(!r.join("scratch/sub/a.txt").exists());
    }

    #[test]
    fn status_on_empty_repo_has_initial_head_and_main_branch() {
        // T3 review 遺留：空 repo（init 後未 commit）→ branch=Some("main")、head_oid="(initial)"、不 panic
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        let dto = status_of(tmp.path(), None).unwrap();
        assert_eq!(dto.parsed.branch.as_deref(), Some("main"));
        assert_eq!(dto.parsed.head_oid, "(initial)");
        assert!(dto.in_progress.is_none());
    }

    // ── M2 Task 6: git 操作 commands 核心 ─────────────────────────────

    #[test]
    fn mutating_request_rejects_a_switched_repository_before_running_closure() {
        let repo_a = tempfile::tempdir().unwrap();
        let repo_b = tempfile::tempdir().unwrap();
        test_repo::init(repo_a.path());
        test_repo::init(repo_b.path());
        std::fs::write(repo_a.path().join("same.txt"), "a\n").unwrap();
        std::fs::write(repo_b.path().join("same.txt"), "b\n").unwrap();
        let state = GitServiceState(std::sync::Mutex::new(Some(RepoHandle {
            root: repo_b.path().to_path_buf(),
        })));

        let error = with_requested_repo(&state, repo_a.path().to_str().unwrap(), |root| {
            stage(root, &["same.txt".into()])
        })
        .unwrap_err();
        assert!(error.contains("repository changed before operation"));
        assert!(status_of(repo_b.path(), None)
            .unwrap()
            .parsed
            .staged
            .is_empty());
        assert_eq!(
            status_of(repo_b.path(), None).unwrap().parsed.untracked,
            vec!["same.txt".to_string()]
        );

        *state.0.lock().unwrap() = Some(RepoHandle {
            root: repo_a.path().to_path_buf(),
        });
        with_requested_repo(&state, repo_a.path().to_str().unwrap(), |root| {
            assert!(
                state.0.try_lock().is_err(),
                "repository lock must cover mutation"
            );
            stage(root, &["same.txt".into()])
        })
        .unwrap();
        assert_eq!(
            status_of(repo_a.path(), None).unwrap().parsed.staged[0].path,
            "same.txt"
        );

        *state.0.lock().unwrap() = None;
        let error =
            with_requested_repo(&state, repo_a.path().to_str().unwrap(), |_| Ok(())).unwrap_err();
        assert!(error.contains("no repository detected"));
    }

    #[test]
    fn stage_unstage_discard_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "one\n", "c1");
        std::fs::write(r.join("a.txt"), "two\n").unwrap();
        stage(r, &["a.txt".into()]).unwrap();
        assert_eq!(status_of(r, None).unwrap().parsed.staged[0].path, "a.txt");
        unstage(r, &["a.txt".into()]).unwrap();
        assert!(status_of(r, None).unwrap().parsed.staged.is_empty());
        discard(r, &["a.txt".into()], &[]).unwrap();
        assert_eq!(std::fs::read_to_string(r.join("a.txt")).unwrap(), "one\n");
        std::fs::write(r.join("junk.txt"), "x").unwrap();
        discard(r, &[], &["junk.txt".into()]).unwrap();
        assert!(!r.join("junk.txt").exists());
    }

    #[test]
    fn rollback_tracked_resets_staged_unstaged_and_partially_staged_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "unstaged.txt", "base-u\n", "base unstaged");
        test_repo::write_and_commit(r, "partial.txt", "base-p\n", "base partial");

        std::fs::write(r.join("unstaged.txt"), "changed-u\n").unwrap();
        std::fs::write(r.join("partial.txt"), "staged-p\n").unwrap();
        stage(r, &["partial.txt".into()]).unwrap();
        std::fs::write(r.join("partial.txt"), "worktree-p\n").unwrap();

        let targets = vec![
            rollback_target(
                "unstaged.txt",
                tracked_classification(None, Some("M"), None),
            ),
            rollback_target(
                "partial.txt",
                tracked_classification(Some("M"), Some("M"), None),
            ),
        ];
        let result = rollback_paths(r, &targets, false).unwrap();

        assert_eq!(
            result,
            GitRollbackResult {
                restored: vec!["unstaged.txt".into(), "partial.txt".into()],
                preserved_untracked: vec![],
                deleted: vec![],
            }
        );
        assert_eq!(
            std::fs::read_to_string(r.join("unstaged.txt")).unwrap(),
            "base-u\n"
        );
        assert_eq!(
            std::fs::read_to_string(r.join("partial.txt")).unwrap(),
            "base-p\n"
        );
        let status = status_of(r, None).unwrap().parsed;
        assert!(status.staged.is_empty());
        assert!(status.unstaged.is_empty());
    }

    #[test]
    fn rollback_rejects_duplicate_path_before_mutating_partially_staged_file() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "partial.txt", "base\n", "base");
        std::fs::write(r.join("partial.txt"), "staged\n").unwrap();
        stage(r, &["partial.txt".into()]).unwrap();
        std::fs::write(r.join("partial.txt"), "worktree\n").unwrap();

        let target = rollback_target(
            "partial.txt",
            tracked_classification(Some("M"), Some("M"), None),
        );
        let error = rollback_paths(r, &[target.clone(), target], false).unwrap_err();

        assert!(error.contains("duplicate target path: partial.txt"));
        let status = status_of(r, None).unwrap().parsed;
        assert_eq!(status.staged[0].status, "M");
        assert_eq!(status.unstaged[0].status, "M");
        assert_eq!(
            std::fs::read_to_string(r.join("partial.txt")).unwrap(),
            "worktree\n"
        );
    }

    #[test]
    fn rollback_added_preserves_by_default_and_deletes_only_when_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "base.txt", "base\n", "base");
        std::fs::write(r.join("keep.txt"), "keep\n").unwrap();
        std::fs::write(r.join("delete.txt"), "delete\n").unwrap();
        stage(r, &["keep.txt".into(), "delete.txt".into()]).unwrap();

        let kept = rollback_paths(
            r,
            &[rollback_target(
                "keep.txt",
                added_classification(Some("A"), None),
            )],
            false,
        )
        .unwrap();
        assert_eq!(kept.preserved_untracked, vec!["keep.txt"]);
        assert!(r.join("keep.txt").exists());
        assert!(status_of(r, None)
            .unwrap()
            .parsed
            .untracked
            .contains(&"keep.txt".to_string()));

        let deleted = rollback_paths(
            r,
            &[rollback_target(
                "delete.txt",
                added_classification(Some("A"), None),
            )],
            true,
        )
        .unwrap();
        assert_eq!(deleted.deleted, vec!["delete.txt"]);
        assert!(!r.join("delete.txt").exists());
    }

    #[test]
    fn rollback_untracked_preserves_by_default_and_deletes_only_when_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "base.txt", "base\n", "base");
        std::fs::write(r.join("keep.txt"), "keep\n").unwrap();
        std::fs::write(r.join("delete.txt"), "delete\n").unwrap();

        let kept = rollback_paths(
            r,
            &[rollback_target(
                "keep.txt",
                GitRollbackClassification::Untracked,
            )],
            false,
        )
        .unwrap();
        assert_eq!(kept.preserved_untracked, vec!["keep.txt"]);
        assert!(r.join("keep.txt").exists());

        let deleted = rollback_paths(
            r,
            &[rollback_target(
                "delete.txt",
                GitRollbackClassification::Untracked,
            )],
            true,
        )
        .unwrap();
        assert_eq!(deleted.deleted, vec!["delete.txt"]);
        assert!(!r.join("delete.txt").exists());
    }

    #[test]
    fn rollback_does_not_report_success_when_git_clean_skips_nested_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "base.txt", "base\n", "base");
        let nested = r.join("nested");
        std::fs::create_dir(&nested).unwrap();
        test_repo::init(&nested);
        let path = status_of(r, None).unwrap().parsed.untracked[0].clone();

        let error = rollback_paths(
            r,
            &[rollback_target(&path, GitRollbackClassification::Untracked)],
            true,
        )
        .unwrap_err();

        assert!(error.contains("git clean completed but left the path in place"));
        assert!(nested.exists());
    }

    #[test]
    fn rollback_added_handles_unborn_head_for_preserve_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        std::fs::write(r.join("keep.txt"), "keep\n").unwrap();
        stage(r, &["keep.txt".into()]).unwrap();
        assert_eq!(status_of(r, None).unwrap().parsed.head_oid, "(initial)");

        rollback_paths(
            r,
            &[rollback_target(
                "keep.txt",
                added_classification(Some("A"), None),
            )],
            false,
        )
        .unwrap();
        assert!(r.join("keep.txt").exists());
        assert!(status_of(r, None)
            .unwrap()
            .parsed
            .untracked
            .contains(&"keep.txt".to_string()));

        std::fs::write(r.join("delete.txt"), "delete\n").unwrap();
        stage(r, &["delete.txt".into()]).unwrap();
        rollback_paths(
            r,
            &[rollback_target(
                "delete.txt",
                added_classification(Some("A"), None),
            )],
            true,
        )
        .unwrap();
        assert!(!r.join("delete.txt").exists());
        assert_eq!(status_of(r, None).unwrap().parsed.head_oid, "(initial)");
    }

    #[test]
    fn rollback_rejects_conflicted_path() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "conflict.txt", "base\n", "base");
        create_branch(r, "side").unwrap();
        test_repo::write_and_commit(r, "conflict.txt", "side\n", "side");
        checkout(r, "main").unwrap();
        test_repo::write_and_commit(r, "conflict.txt", "main\n", "main");
        let merge = run_git(r, &["merge", "side"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_ne!(merge.code, 0);

        let error = rollback_paths(
            r,
            &[rollback_target(
                "conflict.txt",
                GitRollbackClassification::Conflicted,
            )],
            false,
        )
        .unwrap_err();
        assert!(error.contains("rejected conflicted path: conflict.txt"));
        assert_eq!(status_of(r, None).unwrap().parsed.conflicted.len(), 1);
    }

    #[test]
    fn rollback_rejects_classification_drift_without_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "drift.txt", "base\n", "base");
        std::fs::write(r.join("drift.txt"), "changed\n").unwrap();
        let stale = rollback_target("drift.txt", tracked_classification(None, Some("M"), None));
        stage(r, &["drift.txt".into()]).unwrap();

        let error = rollback_paths(r, &[stale], false).unwrap_err();
        assert!(error.contains("classification drift for drift.txt"));
        assert_eq!(status_of(r, None).unwrap().parsed.staged[0].status, "M");
        assert_eq!(
            std::fs::read_to_string(r.join("drift.txt")).unwrap(),
            "changed\n"
        );
    }

    #[test]
    fn rollback_rename_uses_latest_exact_orig_path() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "old.txt", "base\n", "base");
        let moved = run_git(r, &["mv", "--", "old.txt", "new.txt"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(moved.code, 0, "{}", moved.stderr);

        let fake_orig = rollback_target(
            "new.txt",
            tracked_classification(Some("R"), None, Some("other.txt")),
        );
        let error = rollback_paths(r, &[fake_orig], false).unwrap_err();
        assert!(error.contains("classification drift for new.txt"));
        assert!(r.join("new.txt").exists());
        assert!(!r.join("old.txt").exists());

        let exact = rollback_target(
            "new.txt",
            tracked_classification(Some("R"), None, Some("old.txt")),
        );
        rollback_paths(r, &[exact], false).unwrap();
        assert_eq!(
            std::fs::read_to_string(r.join("old.txt")).unwrap(),
            "base\n"
        );
        assert!(!r.join("new.txt").exists());
        let status = status_of(r, None).unwrap().parsed;
        assert!(status.staged.is_empty());
        assert!(status.unstaged.is_empty());
    }

    #[test]
    fn rollback_rejects_traversal_and_absolute_paths() {
        let outer = tempfile::tempdir().unwrap();
        let r = outer.path().join("repo");
        std::fs::create_dir(&r).unwrap();
        test_repo::init(&r);
        test_repo::write_and_commit(&r, "base.txt", "base\n", "base");
        let outside = outer.path().join("outside.txt");
        std::fs::write(&outside, "outside\n").unwrap();

        let traversal = rollback_target("../outside.txt", GitRollbackClassification::Untracked);
        let traversal_error = rollback_paths(&r, &[traversal], true).unwrap_err();
        assert!(traversal_error.contains("non-repo-relative path"));

        let absolute = rollback_target(
            outside.to_str().unwrap(),
            GitRollbackClassification::Untracked,
        );
        let absolute_error = rollback_paths(&r, &[absolute], true).unwrap_err();
        assert!(absolute_error.contains("non-repo-relative path"));
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside\n");
    }

    #[cfg(unix)]
    #[test]
    fn rollback_rejects_symlink_that_resolves_outside_repository() {
        use std::os::unix::fs::symlink;

        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let r = repo.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "base.txt", "base\n", "base");
        let outside_file = outside.path().join("outside.txt");
        std::fs::write(&outside_file, "outside\n").unwrap();
        symlink(&outside_file, r.join("link.txt")).unwrap();

        let error = rollback_paths(
            r,
            &[rollback_target(
                "link.txt",
                GitRollbackClassification::Untracked,
            )],
            true,
        )
        .unwrap_err();
        assert!(error.contains("path outside repository: link.txt"));
        assert_eq!(std::fs::read_to_string(outside_file).unwrap(), "outside\n");
        assert!(r.join("link.txt").exists());
    }

    #[test]
    fn rollback_command_error_reports_completed_stages() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "tracked.txt", "base\n", "base");
        std::fs::write(r.join("tracked.txt"), "changed\n").unwrap();
        std::fs::write(r.join("loose.txt"), "loose\n").unwrap();
        std::fs::write(r.join(".git/index.lock"), "locked").unwrap();

        let targets = vec![
            rollback_target("loose.txt", GitRollbackClassification::Untracked),
            rollback_target("tracked.txt", tracked_classification(None, Some("M"), None)),
        ];
        let error = rollback_paths(r, &targets, false).unwrap_err();

        assert!(error.contains("during restore tracked path"));
        assert!(error.contains("completed stages: preserve-untracked:loose.txt"));
        assert!(r.join("loose.txt").exists());
        assert_eq!(
            std::fs::read_to_string(r.join("tracked.txt")).unwrap(),
            "changed\n"
        );
    }

    #[test]
    fn commit_creates_new_head() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "one\n", "c1");
        std::fs::write(r.join("a.txt"), "two\n").unwrap();
        stage(r, &["a.txt".into()]).unwrap();
        commit(r, "feat: two").unwrap();
        let out = run_git(
            r,
            &["log", "--format=%s", "-1"],
            Duration::from_secs(30),
            &[],
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "feat: two");
    }

    #[test]
    fn branches_lists_current_and_created() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        create_branch(r, "feature/x").unwrap();
        let b = branches(r).unwrap();
        let cur: Vec<_> = b.local.iter().filter(|x| x.is_current).collect();
        assert_eq!(cur[0].name, "feature/x");
        checkout(r, "main").unwrap();
        assert!(branches(r)
            .unwrap()
            .local
            .iter()
            .any(|x| x.name == "main" && x.is_current));
    }

    #[test]
    fn cherry_pick_clean_and_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "base\n", "c1");
        create_branch(r, "side").unwrap();
        test_repo::write_and_commit(r, "b.txt", "sidefile\n", "add b");
        let pick = run_git(r, &["rev-parse", "HEAD"], DEFAULT_TIMEOUT, &[]).unwrap();
        let sha = String::from_utf8_lossy(&pick.stdout).trim().to_string();
        checkout(r, "main").unwrap();
        cherry_pick(r, &sha).unwrap();
        let head = run_git(r, &["log", "--format=%s", "-1"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "add b");

        test_repo::write_and_commit(r, "a.txt", "main-x\n", "cm");
        checkout(r, "side").unwrap();
        test_repo::write_and_commit(r, "a.txt", "side-x\n", "cs");
        let cs = String::from_utf8_lossy(
            &run_git(r, &["rev-parse", "HEAD"], DEFAULT_TIMEOUT, &[])
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        checkout(r, "main").unwrap();
        let err = cherry_pick(r, &cs).unwrap_err();
        assert!(!err.trim().is_empty());
        assert_eq!(
            status_of(r, None).unwrap().in_progress.as_deref(),
            Some("cherry-pick")
        );
        conflict_abort(r, "cherry-pick").unwrap();
        assert!(status_of(r, None).unwrap().in_progress.is_none());
    }

    #[test]
    fn cherry_pick_redundant_empty_stays_abortable_without_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "base\n", "c1");

        create_branch(r, "side").unwrap();
        test_repo::write_and_commit(r, "a.txt", "same\n", "side same");
        let redundant = String::from_utf8_lossy(
            &run_git(r, &["rev-parse", "HEAD"], DEFAULT_TIMEOUT, &[])
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        checkout(r, "main").unwrap();
        test_repo::write_and_commit(r, "a.txt", "same\n", "main same");
        let err = cherry_pick(r, &redundant).unwrap_err();
        assert!(!err.trim().is_empty());

        let dto = status_of(r, None).unwrap();
        assert_eq!(dto.in_progress.as_deref(), Some("cherry-pick"));
        assert!(dto.parsed.conflicted.is_empty());

        conflict_abort(r, "cherry-pick").unwrap();
        assert!(status_of(r, None).unwrap().in_progress.is_none());
    }

    #[test]
    fn cherry_pick_merge_commit_without_mainline_returns_readable_error() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "base.txt", "base\n", "base");

        create_branch(r, "side").unwrap();
        test_repo::write_and_commit(r, "side.txt", "side\n", "side");
        checkout(r, "main").unwrap();
        test_repo::write_and_commit(r, "main.txt", "main\n", "main");
        run_ok(
            r,
            &["merge", "--no-ff", "side", "-m", "merge side"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        let merge_sha = String::from_utf8_lossy(
            &run_git(r, &["rev-parse", "HEAD"], DEFAULT_TIMEOUT, &[])
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        run_ok(
            r,
            &["switch", "-c", "target", "HEAD~1"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();

        let err = cherry_pick(r, &merge_sha).unwrap_err();
        assert!(!err.trim().is_empty());
    }

    #[test]
    fn merge_conflict_flow_abort_restores_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        create_branch(r, "side").unwrap();
        test_repo::write_and_commit(r, "f.txt", "side\n", "c2");
        checkout(r, "main").unwrap();
        test_repo::write_and_commit(r, "f.txt", "main\n", "c3");
        let merge = run_git(r, &["merge", "side"], Duration::from_secs(30), &[]).unwrap();
        assert_ne!(merge.code, 0);
        let dto = status_of(r, None).unwrap();
        assert_eq!(dto.in_progress.as_deref(), Some("merge"));
        assert_eq!(dto.parsed.conflicted[0].path, "f.txt");
        conflict_abort(r, "merge").unwrap();
        let dto2 = status_of(r, None).unwrap();
        assert_eq!(dto2.in_progress, None);
        assert!(dto2.parsed.conflicted.is_empty());
    }

    /// 製造一個處於 merge conflict 狀態、且衝突已解決並 staged 的 fixture repo。
    /// 回傳 tempdir（呼叫端持有以維持存活）。
    fn conflict_repo_resolved_staged() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        create_branch(r, "side").unwrap();
        test_repo::write_and_commit(r, "f.txt", "side\n", "c2");
        checkout(r, "main").unwrap();
        test_repo::write_and_commit(r, "f.txt", "main\n", "c3");
        let merge = run_git(r, &["merge", "side"], Duration::from_secs(30), &[]).unwrap();
        assert_ne!(merge.code, 0);
        assert_eq!(
            status_of(r, None).unwrap().in_progress.as_deref(),
            Some("merge")
        );
        std::fs::write(r.join("f.txt"), "resolved\n").unwrap();
        stage(r, &["f.txt".into()]).unwrap();
        tmp
    }

    #[test]
    fn merge_conflict_continue_completes() {
        // GUI 無 TTY：merge --continue 會為 merge commit 開 editor；EDITOR unset + dumb terminal
        // 下報「Terminal is dumb, but EDITOR unset」exit 1。conflict_continue 需以 GIT_EDITOR=true
        // 抑制。此測試把繼承環境的 GIT_EDITOR/EDITOR/VISUAL 汙染成必失敗的 editor（`false`），
        // 只有 conflict_continue 內部以 editor_true() 覆蓋才能成功——形成不依賴宿主環境的 RED baseline。
        //
        // 注意：測試會修改 process 全域 env，屬程序級副作用；本測試不與其他測試共享 fixture，
        // 且立即還原，風險有界。
        struct EnvGuard {
            saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for (k, v) in &self.saved {
                    match v {
                        Some(val) => std::env::set_var(k, val),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
        let keys = ["GIT_EDITOR", "EDITOR", "VISUAL"];
        let _guard = EnvGuard {
            saved: keys.iter().map(|k| (*k, std::env::var_os(k))).collect(),
        };
        // 汙染繼承環境：任何 fallback 到繼承 editor 的路徑都會用 `false`（exit 1）。
        for k in keys {
            std::env::set_var(k, "false");
        }

        // RED baseline：不帶 override 直接 merge --continue → 繼承的 `false` editor 使其失敗。
        let red = conflict_repo_resolved_staged();
        let raw = run_git(
            red.path(),
            &["merge", "--continue"],
            Duration::from_secs(30),
            &[],
        )
        .unwrap();
        assert_ne!(
            raw.code, 0,
            "raw merge --continue 應因繼承的失敗 editor 而失敗"
        );
        assert_eq!(
            status_of(red.path(), None).unwrap().in_progress.as_deref(),
            Some("merge"),
            "失敗後仍停在 merge in-progress"
        );

        // GREEN：conflict_continue 以 GIT_EDITOR=true 覆蓋繼承 editor → 成功。
        let green = conflict_repo_resolved_staged();
        conflict_continue(green.path(), "merge").unwrap();
        let dto = status_of(green.path(), None).unwrap();
        assert_eq!(dto.in_progress, None);
        assert!(dto.parsed.conflicted.is_empty());
    }

    #[test]
    fn conflict_op_rejects_non_whitelisted() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        // 白名單外的 op（如 "push"）不得組成 subcommand、不執行任何 git → Err。
        // 斷言錯誤來自白名單校驗（而非 git 執行後的 stderr），才能證明未觸發 git。
        assert_eq!(
            conflict_abort(r, "push").unwrap_err(),
            "invalid conflict op: push"
        );
        assert_eq!(
            conflict_continue(r, "push").unwrap_err(),
            "invalid conflict op: push"
        );
    }

    #[test]
    fn diff_content_unstaged_and_staged_and_untracked() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "one\n", "c1");
        std::fs::write(r.join("a.txt"), "two\n").unwrap();
        let d = diff_content(r, "a.txt", false).unwrap();
        assert!(matches!(&d.original, GradedText::Full { content } if content == "one\n"));
        assert!(matches!(&d.modified, GradedText::Full { content } if content == "two\n"));
        std::fs::write(r.join("new.txt"), "n\n").unwrap();
        let d2 = diff_content(r, "new.txt", false).unwrap();
        assert!(matches!(&d2.original, GradedText::Full { content } if content.is_empty()));
        stage(r, &["a.txt".into()]).unwrap();
        let d3 = diff_content(r, "a.txt", true).unwrap();
        assert!(matches!(&d3.original, GradedText::Full { content } if content == "one\n"));
        assert!(matches!(&d3.modified, GradedText::Full { content } if content == "two\n"));
    }

    #[test]
    fn remote_probe_unknown_without_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        assert_eq!(remote_probe(r, &[]).unwrap(), "unknown");
    }
}
