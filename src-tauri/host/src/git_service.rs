// Shared Git process, status and mutation core. Desktop commands retain UI trust and askpass.
use std::collections::HashSet;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum GitEnvironment {
    /// `kind` is a stable machine code for UI localization.
    /// - `notFound`: git binary missing/unusable
    /// - `unsupportedVersion`: installed git below MIN_GIT_VERSION
    Missing {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minimum_version: Option<String>,
    },
    NotARepo,
    Ready {
        root: String,
        version: String,
    },
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

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct GitProcessRegistry {
    active: Mutex<HashSet<u32>>,
}

impl GitProcessRegistry {
    pub fn register(&self, pid: u32) {
        if let Ok(mut active) = self.active.lock() {
            active.insert(pid);
        }
    }

    pub fn unregister(&self, pid: u32) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&pid);
        }
    }

    pub fn drain(&self) -> Vec<u32> {
        match self.active.lock() {
            Ok(mut active) => active.drain().collect(),
            Err(_) => Vec::new(),
        }
    }
}

static ACTIVE_GIT_PROCESSES: LazyLock<GitProcessRegistry> =
    LazyLock::new(GitProcessRegistry::default);

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
                kind: Some("notFound".to_string()),
                minimum_version: None,
            }
        }
    };
    let version = String::from_utf8_lossy(&version_out.stdout)
        .trim()
        .to_string();
    match parse_git_version(&version) {
        Some((major, minor)) if (major, minor) >= MIN_GIT_VERSION => {}
        _ => {
            return GitEnvironment::Missing {
                // Internal diagnostic only — UI must not render this raw string.
                reason: format!(
                    "git version below {MIN_GIT_VERSION_LABEL} (requires git switch and --end-of-options): {version}"
                ),
                kind: Some("unsupportedVersion".to_string()),
                minimum_version: Some(MIN_GIT_VERSION_LABEL.to_string()),
            };
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

/// Smallest Git version the app honestly supports.
///
/// Porcelain v2 needs ≥2.11, but checkout/create-branch use `git switch` (≥2.23)
/// and log/file-at-rev use `--end-of-options` (≥2.24). The floor is therefore 2.24.
pub const MIN_GIT_VERSION: (u32, u32) = (2, 24);
pub const MIN_GIT_VERSION_LABEL: &str = "2.24";

pub fn parse_git_version(version: &str) -> Option<(u32, u32)> {
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
    run_git_inner(root, args, timeout, extra_env, None, None)
}

/// Same process policy as `run_git`, with caller-supplied stdin (for length-framed
/// commands such as `git cat-file --batch`). Literal pathspecs stay forced.
pub fn run_git_with_stdin(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(String, String)],
    stdin: &[u8],
) -> Result<GitOutput, String> {
    run_git_inner(root, args, timeout, extra_env, Some(stdin), None)
}

pub fn run_git_inner(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(String, String)],
    stdin_bytes: Option<&[u8]>,
    on_spawn: Option<&dyn Fn(u32)>,
) -> Result<GitOutput, String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .envs(
            extra_env
                .iter()
                .filter(|(key, _)| key != "GIT_LITERAL_PATHSPECS")
                .map(|(k, v)| (k.as_str(), v.as_str())),
        )
        // Forced after extra_env so callers cannot disable literal pathspecs.
        // Internal commands that genuinely need Git pathspec magic must add a
        // narrowly named, reviewed escape hatch rather than weakening this default.
        .env("GIT_LITERAL_PATHSPECS", "1")
        .stdin(if stdin_bytes.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process_kill::configure_background_process(&mut cmd);
    crate::git_process::execute(cmd, args, timeout, stdin_bytes, on_spawn)
}

pub(crate) fn register_process(pid: u32) {
    ACTIVE_GIT_PROCESSES.register(pid);
}
pub(crate) fn unregister_process(pid: u32) {
    ACTIVE_GIT_PROCESSES.unregister(pid);
}

/// debug log：args join（URL userinfo 遮蔽）、code、stderr 前 200 字；不記 extra_env。
/// 走共享全域 sink——cargo test 下自動重導 tempdir，不汙染 ~/.yuzora/logs。
type GitLogger = fn(&[&str], i32, &str);
static GIT_LOGGER: std::sync::OnceLock<GitLogger> = std::sync::OnceLock::new();
pub fn set_logger(logger: GitLogger) {
    let _ = GIT_LOGGER.set(logger);
}
pub(crate) fn log_git_call(args: &[&str], code: i32, stderr: &str) {
    if let Some(logger) = GIT_LOGGER.get() {
        logger(args, code, stderr);
    }
}

/// in_progress 判定：優先序 rebase > merge > cherry-pick > revert。
pub fn detect_in_progress(git_dir: &Path) -> Option<String> {
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
    let in_progress = detect_in_progress(&metadata_dirs(root)?.git_dir);
    Ok(GitStatusDto {
        parsed,
        in_progress,
    })
}

#[derive(Debug)]
pub struct GitMetadataDirs {
    pub git_dir: std::path::PathBuf,
    pub common_dir: std::path::PathBuf,
}

/// Linked worktrees keep HEAD/index in a private git-dir and refs in a shared
/// common-dir. Resolve both through Git, including relative .git indirections.
pub fn metadata_dirs(root: &Path) -> Result<GitMetadataDirs, String> {
    fn resolve(root: &Path, flag: &str) -> Result<std::path::PathBuf, String> {
        let out = run_ok(root, &["rev-parse", flag], DEFAULT_TIMEOUT, &[])?;
        let path = std::str::from_utf8(&out.stdout)
            .map_err(|_| "git-metadata-path-not-utf8")?
            .trim_end_matches(['\r', '\n']);
        if path.is_empty() {
            return Err("git-metadata-path-empty".into());
        }
        root.join(path).canonicalize().map_err(|e| e.to_string())
    }
    Ok(GitMetadataDirs {
        git_dir: resolve(root, "--git-dir")?,
        common_dir: resolve(root, "--git-common-dir")?,
    })
}

pub const REMOTE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub is_current: bool,
    pub gone: bool,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub name: String,
    pub date: String,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BranchList {
    pub local: Vec<BranchInfo>,
    pub remote: Vec<String>,
    pub tags: Vec<TagInfo>,
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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
pub fn git_err(sub: &str, stderr: &str) -> String {
    let summary: String = stderr.trim().chars().take(500).collect();
    format!("git {sub}: {summary}")
}

/// 跑一個必須成功（code==0）的 git 指令；非零回統一錯誤格式。
pub fn run_ok(
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

pub fn status_path_fingerprint(
    parsed: &crate::git_status::ParsedStatus,
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let push = |map: &mut std::collections::BTreeMap<String, String>, path: &str, token: String| {
        map.entry(path.to_string())
            .and_modify(|existing| {
                existing.push('\u{1f}');
                existing.push_str(&token);
            })
            .or_insert(token);
    };
    for entry in &parsed.staged {
        push(
            &mut out,
            &entry.path,
            format!(
                "S:{}:{}",
                entry.status,
                entry.orig_path.as_deref().unwrap_or("")
            ),
        );
    }
    for entry in &parsed.unstaged {
        push(
            &mut out,
            &entry.path,
            format!(
                "U:{}:{}",
                entry.status,
                entry.orig_path.as_deref().unwrap_or("")
            ),
        );
    }
    for path in &parsed.untracked {
        push(&mut out, path, "?:".to_string());
    }
    for entry in &parsed.conflicted {
        push(&mut out, &entry.path, format!("C:{}", entry.status));
    }
    out
}

pub fn mutated_status_paths(
    before: &crate::git_status::ParsedStatus,
    after: &crate::git_status::ParsedStatus,
) -> HashSet<String> {
    let before_fp = status_path_fingerprint(before);
    let after_fp = status_path_fingerprint(after);
    let mut mutated = HashSet::new();
    for (path, signature) in &before_fp {
        if after_fp.get(path) != Some(signature) {
            mutated.insert(path.clone());
        }
    }
    for (path, signature) in &after_fp {
        if before_fp.get(path) != Some(signature) {
            mutated.insert(path.clone());
        }
    }
    mutated
}

pub fn ensure_literal_path_scope(
    operation: &str,
    requested: &[String],
    before: &crate::git_status::ParsedStatus,
    after: &crate::git_status::ParsedStatus,
) -> Result<(), String> {
    if before.head_oid != after.head_oid {
        return Err(format!(
            "git {operation} changed HEAD outside the requested path set"
        ));
    }
    let allowed: HashSet<&str> = requested.iter().map(String::as_str).collect();
    let extra: Vec<String> = mutated_status_paths(before, after)
        .into_iter()
        .filter(|path| !allowed.contains(path.as_str()))
        .collect();
    if extra.is_empty() {
        return Ok(());
    }
    Err(format!(
        "git {operation} changed paths outside the requested set: {}",
        extra.join(", ")
    ))
}

pub fn mutate_then_assert_literal_scope(
    root: &Path,
    operation: &str,
    requested: &[String],
    args: &[&str],
) -> Result<(), String> {
    let before = status_of(root, None)?;
    run_ok(root, args, DEFAULT_TIMEOUT, &[])?;
    let after = status_of(root, None)?;
    ensure_literal_path_scope(operation, requested, &before.parsed, &after.parsed)
}

pub fn stage(root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    mutate_then_assert_literal_scope(root, "add", paths, &args)
}

pub fn unstage(root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(String::as_str));
    mutate_then_assert_literal_scope(root, "restore", paths, &args)
}

/// tracked → restore --；untracked → clean -f --（前端已確認過 confirm）。
pub fn discard(root: &Path, paths: &[String], untracked: &[String]) -> Result<(), String> {
    if !paths.is_empty() {
        let mut args: Vec<&str> = vec!["restore", "--"];
        args.extend(paths.iter().map(String::as_str));
        mutate_then_assert_literal_scope(root, "restore", paths, &args)?;
    }
    if !untracked.is_empty() {
        let mut args: Vec<&str> = vec!["clean", "-f", "--"];
        args.extend(untracked.iter().map(String::as_str));
        mutate_then_assert_literal_scope(root, "clean", untracked, &args)?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum GitRollbackPlan {
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

/// Lexical repo-relative path check only (no filesystem resolve). Used by
/// historical object reads that must not depend on the current worktree.
pub fn validate_relative_components(value: &str, operation: &str) -> Result<(), String> {
    use std::path::Component;

    if value.is_empty() || value.contains('\0') {
        return Err(format!(
            "git {operation} rejected an empty or NUL-containing path"
        ));
    }
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "git {operation} rejected non-repo-relative path: {value}"
        ));
    }
    Ok(())
}

/// Mutation paths must resolve inside the repository, including a final symlink.
/// This is intentionally stricter than diff reads, which read final symlinks with
/// `read_link` and therefore never follow their target.
pub fn validate_repo_relative_path(
    root: &Path,
    canonical_root: &Path,
    value: &str,
) -> Result<(), String> {
    validate_relative_components(value, "rollback")?;
    let joined = root.join(value);
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

pub fn validate_diff_relative_path(
    root: &Path,
    canonical_root: &Path,
    value: &str,
) -> Result<(), String> {
    validate_relative_components(value, "diff")?;
    let joined = root.join(value);
    let boundary = if std::fs::symlink_metadata(&joined)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        joined.parent().unwrap_or(root)
    } else {
        let mut existing = joined.as_path();
        while !existing.exists() {
            existing = existing.parent().ok_or_else(|| {
                format!("git diff could not resolve path inside repository: {value}")
            })?;
        }
        existing
    };
    let canonical_boundary = boundary
        .canonicalize()
        .map_err(|error| format!("git diff could not resolve {value}: {error}"))?;
    if !canonical_boundary.starts_with(canonical_root) {
        return Err(format!(
            "git diff rejected path outside repository: {value}"
        ));
    }
    Ok(())
}

pub fn actual_rollback_classification(
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

pub fn rollback_failure(path: &str, stage: &str, completed: &[String], error: String) -> String {
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
    let mut previous = latest;

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
                let after = status_of(root, None).map_err(|error| {
                    rollback_failure(&path, "verify restore scope", &completed, error)
                })?;
                ensure_literal_path_scope(
                    "rollback",
                    &restore_paths,
                    &previous.parsed,
                    &after.parsed,
                )
                .map_err(|error| {
                    rollback_failure(&path, "verify restore scope", &completed, error)
                })?;
                previous = after;
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
                let after_unstage = status_of(root, None).map_err(|error| {
                    rollback_failure(&path, "verify unstage scope", &completed, error)
                })?;
                ensure_literal_path_scope(
                    "rollback",
                    std::slice::from_ref(&path),
                    &previous.parsed,
                    &after_unstage.parsed,
                )
                .map_err(|error| {
                    rollback_failure(&path, "verify unstage scope", &completed, error)
                })?;
                previous = after_unstage;
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
                    let after_clean = status_of(root, None).map_err(|error| {
                        rollback_failure(&path, "verify delete scope", &completed, error)
                    })?;
                    ensure_literal_path_scope(
                        "rollback",
                        std::slice::from_ref(&path),
                        &previous.parsed,
                        &after_clean.parsed,
                    )
                    .map_err(|error| {
                        rollback_failure(&path, "verify delete scope", &completed, error)
                    })?;
                    previous = after_clean;
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
                    let after_clean = status_of(root, None).map_err(|error| {
                        rollback_failure(&path, "verify delete scope", &completed, error)
                    })?;
                    ensure_literal_path_scope(
                        "rollback",
                        std::slice::from_ref(&path),
                        &previous.parsed,
                        &after_clean.parsed,
                    )
                    .map_err(|error| {
                        rollback_failure(&path, "verify delete scope", &completed, error)
                    })?;
                    previous = after_clean;
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

pub fn parse_upstream_track(track: &str) -> (u32, u32, bool) {
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut gone = false;
    for part in track
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(", ")
    {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        } else if part == "gone" {
            gone = true;
        }
    }
    (ahead, behind, gone)
}

pub fn branches(root: &Path) -> Result<BranchList, String> {
    let out = run_git(
        root,
        &[
            "for-each-ref",
            "refs/heads",
            "--format=%(HEAD)%00%(refname:lstrip=2)%00%(upstream:lstrip=2)%00%(upstream:track)",
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
        let (ahead, behind, gone) = parse_upstream_track(f[3]);
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
            gone,
        });
    }
    let remotes = run_git(
        root,
        &[
            "for-each-ref",
            "refs/remotes",
            "--format=%(refname:lstrip=2)",
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if remotes.code != 0 {
        return Err(git_err("for-each-ref", &remotes.stderr));
    }
    let remote = String::from_utf8_lossy(&remotes.stdout)
        .lines()
        .filter(|line| !line.ends_with("/HEAD"))
        .map(String::from)
        .collect();

    let tag_refs = run_git(
        root,
        &[
            "for-each-ref",
            "refs/tags",
            "--format=%(refname:lstrip=2)%00%(creatordate:iso-strict)",
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if tag_refs.code != 0 {
        return Err(git_err("for-each-ref", &tag_refs.stderr));
    }
    let tags = String::from_utf8_lossy(&tag_refs.stdout)
        .lines()
        .filter_map(|line| {
            let (name, date) = line.split_once('\0')?;
            Some(TagInfo {
                name: name.to_string(),
                date: date.to_string(),
            })
        })
        .collect();

    Ok(BranchList {
        local,
        remote,
        tags,
    })
}

pub fn create_branch(root: &Path, name: &str, start_point: Option<&str>) -> Result<(), String> {
    let mut args = vec!["switch", "-c", name];
    let resolved = start_point
        .map(|spec| {
            let oid = crate::git_oid::resolve_commit_oid(root, spec)?;
            let upstream = remote_tracking_start_ref(root, spec)?;
            Ok::<_, String>((oid, upstream))
        })
        .transpose()?;
    if let Some((oid, _)) = resolved.as_ref() {
        args.extend(["--end-of-options", oid.as_str()]);
    }
    run_ok(root, &args, DEFAULT_TIMEOUT, &[])?;
    if let Some((_, Some(upstream))) = resolved {
        // Implicit upstream is product behavior for remote-tracking starts, but
        // must not fail branch creation when the logical name is ambiguous or
        // the remote is not configured.
        let set_upstream = format!("--set-upstream-to={upstream}");
        let _ = run_ok(
            root,
            &["branch", &set_upstream, "--end-of-options", name],
            DEFAULT_TIMEOUT,
            &[],
        );
    }
    Ok(())
}

pub fn remote_tracking_start_ref(root: &Path, spec: &str) -> Result<Option<String>, String> {
    if crate::git_oid::looks_like_git_option(spec) {
        return Ok(None);
    }
    let out = run_git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--symbolic-full-name",
            "--end-of-options",
            spec,
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        return Ok(None);
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let Some(logical) = name.strip_prefix("refs/remotes/") else {
        return Ok(None);
    };
    if logical.is_empty()
        || logical.contains('\0')
        || crate::git_oid::looks_like_git_option(logical)
    {
        return Ok(None);
    }
    Ok(Some(logical.to_string()))
}

pub fn checkout(root: &Path, name: &str) -> Result<(), String> {
    run_ok(
        root,
        &["switch", "--no-guess", "--end-of-options", name],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    Ok(())
}

pub fn checkout_detached(root: &Path, rev: &str) -> Result<(), String> {
    let oid = crate::git_oid::resolve_commit_oid(root, rev)?;
    run_ok(
        root,
        &["switch", "--detach", "--end-of-options", oid.as_str()],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    Ok(())
}

/// cherry-pick <hash>。GUI 無 TTY：GIT_EDITOR=true 防 sequencer editor 卡死（乾淨 pick
/// 會沿用原訊息、通常不開 editor，但保險）。衝突留 CHERRY_PICK_HEAD → 前端接 ConflictBanner。
pub fn cherry_pick(root: &Path, hash: &str) -> Result<(), String> {
    let oid = crate::git_oid::resolve_commit_oid(root, hash)?;
    run_ok(
        root,
        &["cherry-pick", "--end-of-options", oid.as_str()],
        DEFAULT_TIMEOUT,
        &editor_true(),
    )?;
    Ok(())
}

/// 契約：op ∈ merge|rebase|cherry-pick|revert（brief 明列）。校驗攔截任意 subcommand。
pub fn check_conflict_op(op: &str) -> Result<(), String> {
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
pub fn editor_true() -> Vec<(String, String)> {
    vec![("GIT_EDITOR".to_string(), "true".to_string())]
}

/// bytes 過分級：與 fs_service::classify_and_read / git_log::grade_object_bytes
/// 同標準，但輸入是 bytes 而非 path，且回 GradedText（無 size 欄）以符 T9
/// types.ts DiffContent 契約。UTF-16 BOM 走 encoding_rs 解碼，避免
/// from_utf8_lossy 把可讀 worktree 檔案變成亂碼。
pub fn grade_bytes(bytes: &[u8]) -> GradedText {
    if bytes.len() as u64 > crate::file_content::HARD_CAP_BYTES {
        return GradedText::TooLarge;
    }
    let sniff = &bytes[..bytes.len().min(crate::file_content::FILE_ANALYSIS_BYTES)];
    let content = match crate::file_content::analyze_byte_content(sniff) {
        crate::file_content::ByteContent::Binary => return GradedText::Binary,
        crate::file_content::ByteContent::Utf16Le | crate::file_content::ByteContent::Utf16Be => {
            let codec = if crate::file_content::analyze_byte_content(&bytes[..bytes.len().min(2)])
                == crate::file_content::ByteContent::Utf16Be
            {
                encoding_rs::UTF_16BE
            } else {
                encoding_rs::UTF_16LE
            };
            let (cow, _, _) = codec.decode(bytes);
            cow.into_owned()
        }
        crate::file_content::ByteContent::Text => String::from_utf8_lossy(bytes).into_owned(),
    };
    if bytes.len() as u64 > crate::file_content::FULL_FEATURE_MAX_BYTES {
        GradedText::Limited { content }
    } else {
        GradedText::Full { content }
    }
}

/// Read an object when it exists. `git cat-file -e` distinguishes a normal
/// missing side from operational `git show` failures, which must reach the UI.
pub fn show_object(root: &Path, spec: &str) -> Result<Option<Vec<u8>>, String> {
    let exists = run_git(root, &["cat-file", "-e", spec], DEFAULT_TIMEOUT, &[])?;
    if exists.code != 0 {
        let stderr = exists.stderr.to_ascii_lowercase();
        // Staged-added files commonly yield:
        // "fatal: path 'x' exists on disk, but not in 'HEAD'"
        let missing = stderr.contains("does not exist")
            || stderr.contains("exists on disk, but not in")
            || stderr.contains("not a valid object name")
            || stderr.contains("invalid object name")
            || stderr.contains("unknown revision")
            || stderr.contains("bad revision")
            || stderr.contains("bad object")
            || stderr.contains("not in the index")
            || stderr.contains("not at stage 0")
            || stderr.contains("not at stage 1")
            || stderr.contains("not at stage 2")
            || stderr.contains("not at stage 3");
        return if missing {
            Ok(None)
        } else {
            Err(git_err("cat-file", &exists.stderr))
        };
    }
    let out = run_git(
        root,
        &["show", "--end-of-options", spec],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        return Err(git_err("show", &out.stderr));
    }
    Ok(Some(out.stdout))
}

/// Read worktree bytes without following symlinks. Git stores a symlink's link
/// target as its blob content; reading the target would disclose outside files.
///
/// On Unix this walks components with `openat`/`O_NOFOLLOW` so a concurrent
/// actor cannot TOCTOU-swap a validated path for an external symlink before the
/// read. Non-Unix falls back to `symlink_metadata` + `read`/`read_link` and
/// retains residual TOCTOU risk documented below.
pub fn read_worktree(root: &Path, path: &str) -> Result<Option<Vec<u8>>, String> {
    #[cfg(unix)]
    {
        read_worktree_nofollow_unix(root, path)
    }
    #[cfg(not(unix))]
    {
        // Residual: validation and read remain separate on non-Unix platforms.
        let full = root.join(path);
        let metadata = match std::fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("git diff could not inspect {path}: {error}")),
        };
        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&full)
                .map_err(|error| format!("git diff could not read symlink {path}: {error}"))?;
            return Ok(Some(target.as_os_str().as_encoded_bytes().to_vec()));
        }
        std::fs::read(&full)
            .map(Some)
            .map_err(|error| format!("git diff could not read {path}: {error}"))
    }
}

#[cfg(unix)]
pub fn c_component_name(
    component: &std::ffi::OsStr,
    label: &str,
) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(component.as_bytes())
        .map_err(|_| format!("git diff rejected path with interior NUL: {label}"))
}

/// Open an absolute directory by walking every component from `/` with
/// `openat(O_DIRECTORY|O_NOFOLLOW)`. Never opens the full path in one shot, so a
/// concurrent actor cannot substitute a symlink for a root path component after
/// canonicalize and before the read.
#[cfg(unix)]
pub fn open_absolute_dir_nofollow(absolute: &Path) -> Result<std::os::fd::OwnedFd, String> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    if !absolute.is_absolute() {
        return Err(format!(
            "git diff expected absolute repository root: {}",
            absolute.display()
        ));
    }

    let slash = CString::new("/").map_err(|_| "git diff could not open /".to_string())?;
    let root_fd = unsafe {
        libc::open(
            slash.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(format!(
            "git diff could not open /: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut current = unsafe { OwnedFd::from_raw_fd(root_fd) };

    for component in absolute.components() {
        match component {
            std::path::Component::RootDir => continue,
            std::path::Component::Normal(name) => {
                let name_c = c_component_name(name, &absolute.display().to_string())?;
                let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
                let fd = unsafe { libc::openat(current.as_raw_fd(), name_c.as_ptr(), flags) };
                if fd < 0 {
                    let err = std::io::Error::last_os_error();
                    return Err(format!(
                        "git diff rejected symlink path while opening repository root {}: {err}",
                        absolute.display()
                    ));
                }
                current = unsafe { OwnedFd::from_raw_fd(fd) };
            }
            _ => {
                return Err(format!(
                    "git diff rejected non-normal repository root component: {}",
                    absolute.display()
                ));
            }
        }
    }
    Ok(current)
}

#[cfg(unix)]
pub fn read_worktree_nofollow_unix(root: &Path, path: &str) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    fn map_open_err(path: &str, err: std::io::Error) -> Result<Option<Vec<u8>>, String> {
        match err.raw_os_error() {
            Some(libc::ENOENT) => Ok(None),
            // macOS often returns ENOTDIR for O_DIRECTORY|O_NOFOLLOW on a symlink.
            Some(libc::ELOOP) | Some(libc::EPERM) | Some(libc::ENOTDIR) => Err(format!(
                "git diff rejected symlink path while reading {path}: {err}"
            )),
            _ => Err(format!("git diff could not read {path}: {err}")),
        }
    }

    // Canonicalize first so the absolute walk starts from a resolved path, then
    // re-open every component with O_NOFOLLOW so post-canonicalize substitution
    // of a root component cannot re-anchor the read outside the repository.
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("git could not resolve repository root: {error}"))?;
    let mut current = open_absolute_dir_nofollow(&canonical_root)?;
    let components: Vec<_> = Path::new(path).components().collect();
    if components.is_empty() {
        return Err("git diff rejected empty path".to_string());
    }

    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(format!("git diff rejected non-repo-relative path: {path}"));
        };
        let name_c = c_component_name(name, path)?;
        let is_final = index + 1 == components.len();

        if !is_final {
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
            let fd = unsafe { libc::openat(current.as_raw_fd(), name_c.as_ptr(), flags) };
            if fd < 0 {
                return map_open_err(path, std::io::Error::last_os_error());
            }
            current = unsafe { OwnedFd::from_raw_fd(fd) };
            continue;
        }

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::fstatat(
                current.as_raw_fd(),
                name_c.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc != 0 {
            return map_open_err(path, std::io::Error::last_os_error());
        }

        if (st.st_mode & libc::S_IFMT) == libc::S_IFLNK {
            let mut buf = vec![0u8; 4096];
            let n = unsafe {
                libc::readlinkat(
                    current.as_raw_fd(),
                    name_c.as_ptr(),
                    buf.as_mut_ptr() as *mut libc::c_char,
                    buf.len(),
                )
            };
            if n < 0 {
                return Err(format!(
                    "git diff could not read symlink {path}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            buf.truncate(n as usize);
            return Ok(Some(buf));
        }

        let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        let fd = unsafe { libc::openat(current.as_raw_fd(), name_c.as_ptr(), flags) };
        if fd < 0 {
            return map_open_err(path, std::io::Error::last_os_error());
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("git diff could not read {path}: {error}"))?;
        return Ok(Some(bytes));
    }

    Ok(None)
}

pub fn empty_text() -> GradedText {
    GradedText::Full {
        content: String::new(),
    }
}

pub fn diff_content(
    root: &Path,
    path: &str,
    staged: bool,
    orig_path: Option<&str>,
) -> Result<DiffContent, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("git could not resolve repository root: {error}"))?;
    validate_diff_relative_path(root, &canonical_root, path)?;
    if let Some(value) = orig_path {
        validate_diff_relative_path(root, &canonical_root, value)?;
    }

    let (original, modified) = if staged {
        let head_path = orig_path.unwrap_or(path);
        let orig = show_object(root, &format!("HEAD:{head_path}"))?;
        let modi = show_object(root, &format!(":0:{path}"))?;
        (
            orig.map(|bytes| grade_bytes(&bytes))
                .unwrap_or_else(empty_text),
            modi.map(|bytes| grade_bytes(&bytes))
                .unwrap_or_else(empty_text),
        )
    } else {
        // Unmerged entries have no stage 0. Prefer stage 1 (merge base), then
        // stage 2 (ours), then HEAD, then empty — never compare markers only
        // against ours when a merge base exists.
        let orig = match show_object(root, &format!(":0:{path}"))? {
            some @ Some(_) => some,
            None => match show_object(root, &format!(":1:{path}"))? {
                some @ Some(_) => some,
                None => match show_object(root, &format!(":2:{path}"))? {
                    some @ Some(_) => some,
                    None => show_object(root, &format!("HEAD:{path}"))?,
                },
            },
        };
        let modi = read_worktree(root, path)?;
        (
            orig.map(|bytes| grade_bytes(&bytes))
                .unwrap_or_else(empty_text),
            modi.map(|bytes| grade_bytes(&bytes))
                .unwrap_or_else(empty_text),
        )
    };
    Ok(DiffContent { original, modified })
}

/// remote_probe：無 upstream→"unknown"；本地=遠端→"no"；不等→"yes"；任何遠端存取失敗→"unknown"。
/// askpass env 一律 background=1（背景鐵律）；timeout 30s。
pub fn remote_probe(root: &Path, env: &[(String, String)]) -> Result<String, String> {
    remote_probe_inner(root, env, None)
}

pub fn remote_probe_inner(
    root: &Path,
    env: &[(String, String)],
    on_spawn: Option<&dyn Fn(u32)>,
) -> Result<String, String> {
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
    let ls = run_git_inner(
        root,
        &["ls-remote", &remote, &format!("refs/heads/{branch}")],
        DEFAULT_TIMEOUT,
        env,
        None,
        on_spawn,
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
