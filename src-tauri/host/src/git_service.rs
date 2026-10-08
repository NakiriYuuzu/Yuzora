// Shared Git process, status and mutation core. Desktop commands retain UI trust and askpass.
use std::collections::HashSet;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

#[derive(serde::Serialize, Clone)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "status"
)]
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

/// `git merge --autostash` and `git pull --autostash` without `--rebase` need
/// git 2.27; older git only autostashes rebases.
const MERGE_AUTOSTASH_VERSION: (u32, u32) = (2, 27);

fn git_at_least(
    root: &Path,
    version: (u32, u32),
    env: &[(String, String)],
) -> Result<bool, String> {
    let out = run_ok(root, &["version"], DEFAULT_TIMEOUT, env)?;
    Ok(parse_git_version(&String::from_utf8_lossy(&out.stdout))
        .is_some_and(|found| found >= version))
}

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

/// Same process policy as `run_git`, but stdout may grow to `stdout_limit` bytes
/// (exceeding it still fails with `git-output-limit`).
pub fn run_git_with_stdout_limit(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(String, String)],
    stdout_limit: usize,
) -> Result<GitOutput, String> {
    let cmd = git_command(root, args, extra_env, false);
    crate::git_process::execute_with_stdout_limit(cmd, args, timeout, None, None, stdout_limit)
}

pub fn run_git_inner(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(String, String)],
    stdin_bytes: Option<&[u8]>,
    on_spawn: Option<&dyn Fn(u32)>,
) -> Result<GitOutput, String> {
    let cmd = git_command(root, args, extra_env, stdin_bytes.is_some());
    crate::git_process::execute(cmd, args, timeout, stdin_bytes, on_spawn)
}

/// Repository-location overrides inherited from the launching environment would
/// silently redirect `-C <root>` operations (e.g. `GIT_INDEX_FILE` makes `add`
/// write another index). Callers may still set them explicitly via `extra_env`.
const INHERITED_REPOSITORY_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
];

/// Reviewed escape hatch for `GIT_LITERAL_PATHSPECS`: `git stash push` builds
/// the `:/` pathspec internally (`-u`, `--keep-index`) and fails under literal
/// pathspecs. Only callers that pass no renderer-supplied pathspec may set it.
const ALLOW_PATHSPEC_MAGIC_ENV: &str = "YUZORA_ALLOW_GIT_PATHSPEC_MAGIC";

fn git_command(
    root: &Path,
    args: &[&str],
    extra_env: &[(String, String)],
    piped_stdin: bool,
) -> std::process::Command {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("git");
    for key in INHERITED_REPOSITORY_ENV {
        cmd.env_remove(key);
    }
    let allow_pathspec_magic = extra_env
        .iter()
        .any(|(key, _)| key == ALLOW_PATHSPEC_MAGIC_ENV);
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .envs(
            extra_env
                .iter()
                .filter(|(key, _)| {
                    key != "GIT_LITERAL_PATHSPECS" && key != ALLOW_PATHSPEC_MAGIC_ENV
                })
                .map(|(k, v)| (k.as_str(), v.as_str())),
        );
    // Forced after extra_env so callers cannot disable literal pathspecs.
    // Internal commands that genuinely need Git pathspec magic must add a
    // narrowly named, reviewed escape hatch rather than weakening this default.
    if allow_pathspec_magic {
        cmd.env_remove("GIT_LITERAL_PATHSPECS");
    } else {
        cmd.env("GIT_LITERAL_PATHSPECS", "1");
    }
    cmd.stdin(if piped_stdin {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    crate::process_kill::configure_background_process(&mut cmd);
    cmd
}

/// Stdout bound for whole-blob reads: one byte past the hard cap is enough for
/// grading to report `TooLarge`; callers map the `git-output-limit` error that
/// larger blobs raise to `TooLarge` as well.
pub const BLOB_READ_LIMIT: usize = crate::file_content::HARD_CAP_BYTES as usize + 1;

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
    let in_progress = detect_in_progress(&resolve_metadata_dir(root, "--git-dir")?);
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
    Ok(GitMetadataDirs {
        git_dir: resolve_metadata_dir(root, "--git-dir")?,
        common_dir: resolve_metadata_dir(root, "--git-common-dir")?,
    })
}

fn resolve_metadata_dir(root: &Path, flag: &str) -> Result<std::path::PathBuf, String> {
    let out = run_ok(root, &["rev-parse", flag], DEFAULT_TIMEOUT, &[])?;
    let path = std::str::from_utf8(&out.stdout)
        .map_err(|_| "git-metadata-path-not-utf8")?
        .trim_end_matches(['\r', '\n']);
    if path.is_empty() {
        return Err("git-metadata-path-empty".into());
    }
    root.join(path).canonicalize().map_err(|e| e.to_string())
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
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
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

/// 非零 exit → 統一錯誤格式 "git <sub>: <stderr 摘要>"。
/// 長 stderr（hook 輸出、remote 訊息、進度列）常把真正原因擠到後段，
/// 因此超過上限時保留 fatal/error/rejected/hint 等關鍵行與最後幾行。
pub fn git_err(sub: &str, stderr: &str) -> String {
    format!("git {sub}: {}", summarize_stderr(stderr))
}

const GIT_ERROR_SUMMARY_CHARS: usize = 1200;

fn summarize_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.chars().count() <= GIT_ERROR_SUMMARY_CHARS {
        return trimmed.to_string();
    }
    let lines: Vec<&str> = trimmed
        .lines()
        .map(|line| line.rsplit('\r').next().unwrap_or(line).trim_end())
        .filter(|line| !line.is_empty())
        .collect();
    let is_key = |line: &str| {
        let lower = line.trim_start().to_ascii_lowercase();
        [
            "fatal:",
            "error:",
            "hint:",
            "! [",
            "remote: error",
            "remote: fatal",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
            || lower.contains("rejected")
    };
    let tail_start = lines.len().saturating_sub(3);
    let picked: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(index, line)| is_key(line) || *index >= tail_start)
        .map(|(_, line)| *line)
        .collect();
    let mut summary = picked.join("\n");
    if summary.chars().count() > GIT_ERROR_SUMMARY_CHARS {
        summary = summary.chars().take(GIT_ERROR_SUMMARY_CHARS).collect();
    }
    summary
}

fn git_config_value(
    root: &Path,
    key: &str,
    env: &[(String, String)],
) -> Result<Option<String>, String> {
    let out = run_git(root, &["config", "--get", key], DEFAULT_TIMEOUT, env)?;
    // `git config --get` exits 1 when the key is unset.
    if out.code != 0 {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

/// Arguments for `git push`. A branch without an upstream is published with
/// `--set-upstream` (JetBrains/VS Code behaviour) instead of failing with
/// "has no upstream branch".
pub fn push_args(root: &Path) -> Result<Vec<String>, String> {
    push_args_with_options(root, false, false)
}

/// `push_args` plus `--force-with-lease` and/or `--follow-tags` (push the tags
/// reachable from the pushed commits, JetBrains "push tags on current branch").
pub fn push_args_with_options(
    root: &Path,
    force_with_lease: bool,
    tags: bool,
) -> Result<Vec<String>, String> {
    push_args_with_options_env(root, force_with_lease, tags, &[])
}

fn push_args_with_options_env(
    root: &Path,
    force_with_lease: bool,
    tags: bool,
    env: &[(String, String)],
) -> Result<Vec<String>, String> {
    let mut args = push_args_with_env(root, env)?;
    // Options go right after the subcommand, before any remote/branch operands.
    if tags {
        args.insert(1, "--follow-tags".into());
    }
    if force_with_lease {
        args.insert(1, "--force-with-lease".into());
    }
    Ok(args)
}

fn push_args_with_env(root: &Path, env: &[(String, String)]) -> Result<Vec<String>, String> {
    let upstream = run_git(
        root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        DEFAULT_TIMEOUT,
        env,
    )?;
    if upstream.code == 0 {
        return Ok(vec!["push".into()]);
    }
    let head = run_git(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        DEFAULT_TIMEOUT,
        env,
    )?;
    if head.code != 0 {
        // Detached HEAD: let git explain what it cannot push.
        return Ok(vec!["push".into()]);
    }
    let branch = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let remote = match push_remote(root, &branch, env)? {
        Some(remote) => remote,
        None => return Err("git push: no remote is configured for this repository".into()),
    };
    Ok(vec!["push".into(), "--set-upstream".into(), remote, branch])
}

fn push_remote(
    root: &Path,
    branch: &str,
    env: &[(String, String)],
) -> Result<Option<String>, String> {
    for key in [
        format!("branch.{branch}.pushRemote"),
        "remote.pushDefault".to_string(),
        format!("branch.{branch}.remote"),
    ] {
        if let Some(remote) = git_config_value(root, &key, env)? {
            return Ok(Some(remote));
        }
    }
    let out = run_ok(root, &["remote"], DEFAULT_TIMEOUT, env)?;
    let remotes: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if remotes.iter().any(|name| name == "origin") {
        return Ok(Some("origin".into()));
    }
    Ok((remotes.len() == 1).then(|| remotes[0].clone()))
}

/// Arguments for `git pull`. Without any reconcile preference git ≥ 2.27
/// aborts diverged pulls ("Need to specify how to reconcile divergent
/// branches"); default to a merge like JetBrains does, but never override a
/// configured `pull.rebase`, `branch.<name>.rebase` or `pull.ff`.
pub fn pull_args(root: &Path) -> Result<Vec<String>, String> {
    pull_args_with_env(root, &[])
}

/// `pull_args` with an explicit reconcile mode: "rebase", "merge" or `None`
/// (keep the configured / default behaviour).
pub fn pull_args_with_mode(root: &Path, mode: Option<&str>) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = match mode {
        None => return pull_args(root),
        Some("rebase") => vec!["pull".into(), "--rebase".into()],
        Some("merge") => vec!["pull".into(), "--no-rebase".into()],
        Some(other) => return Err(format!("git: unsupported pull mode '{other}'")),
    };
    if mode == Some("rebase") || git_at_least(root, MERGE_AUTOSTASH_VERSION, &[])? {
        args.push("--autostash".into());
    }
    Ok(args)
}

fn pull_args_with_env(root: &Path, env: &[(String, String)]) -> Result<Vec<String>, String> {
    let mut keys = vec!["pull.rebase".to_string(), "pull.ff".to_string()];
    let head = run_git(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        DEFAULT_TIMEOUT,
        env,
    )?;
    if head.code == 0 {
        let branch = String::from_utf8_lossy(&head.stdout).trim().to_string();
        keys.push(format!("branch.{branch}.rebase"));
    }
    let mut args: Vec<String> = vec!["pull".into(), "--no-rebase".into()];
    for key in keys {
        if git_config_value(root, &key, env)?.is_some() {
            args.truncate(1);
            break;
        }
    }
    // Local changes must not block an update; JetBrains stashes them around it.
    if git_at_least(root, MERGE_AUTOSTASH_VERSION, env)? {
        args.push("--autostash".into());
    }
    Ok(args)
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
    reject_unresolved_conflicts(root, paths)?;
    let mut args: Vec<&str> = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    mutate_then_assert_literal_scope(root, "add", paths, &args)
}

/// Staging an unmerged path marks its conflict resolved (JetBrains "Mark as
/// Resolved"). Refuse while the working file still carries conflict markers so
/// a half-merged file cannot be committed by a stray click.
fn reject_unresolved_conflicts(root: &Path, paths: &[String]) -> Result<(), String> {
    const MARKER_SCAN_LIMIT: u64 = 8 * 1024 * 1024;
    // The index alone lists unmerged paths; a full status would also walk the
    // working tree on every stage.
    let out = run_ok(
        root,
        &["ls-files", "--unmerged", "-z"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    let listing = String::from_utf8_lossy(&out.stdout);
    let unmerged: HashSet<&str> = listing
        .split('\0')
        .filter_map(|record| record.split_once('\t').map(|(_, path)| path))
        .collect();
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("git add could not resolve repository root: {e}"))?;
    for path in paths.iter().filter(|path| unmerged.contains(path.as_str())) {
        validate_repo_relative_path(root, &canonical_root, path)?;
        let file = root.join(path);
        // A deleted side or an oversized file has nothing to scan.
        let Ok(metadata) = std::fs::metadata(&file) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MARKER_SCAN_LIMIT {
            continue;
        }
        let bytes = std::fs::read(&file).map_err(|e| format!("git add: {path}: {e}"))?;
        if has_conflict_markers(&bytes) {
            return Err(format!(
                "git add: conflict markers remain in {path}; resolve them before marking the file as resolved"
            ));
        }
    }
    Ok(())
}

fn has_conflict_markers(bytes: &[u8]) -> bool {
    let mut opened = false;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.starts_with(b"<<<<<<< ") || line == b"<<<<<<<" {
            opened = true;
        } else if opened && (line.starts_with(b">>>>>>> ") || line == b">>>>>>>") {
            return true;
        }
    }
    false
}

pub fn unstage(root: &Path, paths: &[String]) -> Result<(), String> {
    let before = status_of(root, None)?;
    // An unborn branch has no HEAD for `restore --staged`. Remove only the
    // index entries; --cached preserves working files, including later edits.
    let mut args: Vec<&str> = if before.parsed.head_oid == "(initial)" {
        vec!["rm", "--cached", "-f", "--"]
    } else {
        vec!["restore", "--staged", "--"]
    };
    args.extend(paths.iter().map(String::as_str));
    run_ok(root, &args, DEFAULT_TIMEOUT, &[])?;
    let after = status_of(root, None)?;
    ensure_literal_path_scope("unstage", paths, &before.parsed, &after.parsed)
}

/// Status paths are decoded lossily, so U+FFFD may stand for undecodable bytes and
/// the string may name a different file on disk. Destructive mutations refuse it.
fn reject_lossy_path(operation: &str, value: &str) -> Result<(), String> {
    if value.contains('\u{FFFD}') {
        return Err(format!(
            "git {operation} rejected a path that is not valid UTF-8: {value}"
        ));
    }
    Ok(())
}

/// tracked → restore --；untracked → clean -f --（前端已確認過 confirm）。
pub fn discard(root: &Path, paths: &[String], untracked: &[String]) -> Result<(), String> {
    for path in paths.iter().chain(untracked) {
        reject_lossy_path("discard", path)?;
    }
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

/// A pure rename leaves its source absent from the working tree. The only allowed
/// exception is a case-only rename on a case-insensitive filesystem: the old spelling
/// then resolves to the renamed entry, which the directory lists under its new
/// spelling only. Any entry listed under the exact old name (a new file, a hard
/// link, or a case-sensitive directory holding both names) is preserved.
/// Fails closed: any I/O error while proving the source is free counts as occupied.
fn rename_source_is_occupied(root: &Path, orig_path: &str, path: &str) -> bool {
    let orig = root.join(orig_path);
    match std::fs::symlink_metadata(&orig) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    }
    if orig_path.to_lowercase() != path.to_lowercase() {
        return true;
    }
    let (Some(parent), Some(name)) = (orig.parent(), orig.file_name()) else {
        return true;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return true;
    };
    for entry in entries {
        match entry {
            Ok(entry) if entry.file_name() != name => {}
            // The exact old spelling exists, or the listing could not be completed.
            _ => return true,
        }
    }
    false
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
        reject_lossy_path("rollback", &target.path)?;
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
                    reject_lossy_path("rollback", &orig_path)?;
                    validate_repo_relative_path(root, &canonical_root, &orig_path)?;
                    // `restore --staged --worktree` rewrites the rename source; anything now
                    // occupying it (untracked, ignored or newly added) would be overwritten.
                    if rename_source_is_occupied(root, &orig_path, &target.path) {
                        return Err(format!(
                            "git rollback rejected {}: rename source {} has new working-tree content; move or delete it first",
                            target.path, orig_path
                        ));
                    }
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
    commit_with_options(root, message, None)
}

pub fn commit_with_options(
    root: &Path,
    message: &str,
    amend_head: Option<&str>,
) -> Result<(), String> {
    if message.trim().is_empty() {
        return Err("Commit message must not be empty".into());
    }
    if let Some(expected) = amend_head {
        let latest = status_of(root, None)?;
        if latest.parsed.head_oid != expected || expected == "(initial)" {
            return Err("HEAD changed. Reload the last commit before amending.".into());
        }
        if latest.in_progress.is_some() || !latest.parsed.conflicted.is_empty() {
            return Err("Finish the current Git operation before amending.".into());
        }
        // No --all: amend consumes only the existing index, including the
        // message-only case with no staged changes. Never push rewritten history.
        run_ok(
            root,
            &["commit", "--amend", "-m", message],
            DEFAULT_TIMEOUT,
            &[],
        )?;
    } else {
        run_ok(root, &["commit", "-m", message], DEFAULT_TIMEOUT, &[])?;
    }
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
            "refs/remotes",
            "refs/tags",
            "--format=%(refname)%00%(HEAD)%00%(upstream:lstrip=2)%00%(upstream:track)%00%(creatordate:iso-strict)",
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        return Err(git_err("for-each-ref", &out.stderr));
    }
    let mut local = Vec::new();
    let mut remote = Vec::new();
    let mut tags = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 5 {
            continue;
        }
        if let Some(name) = f[0].strip_prefix("refs/heads/") {
            let (ahead, behind, gone) = parse_upstream_track(f[3]);
            local.push(BranchInfo {
                name: name.to_string(),
                upstream: if f[2].is_empty() {
                    None
                } else {
                    Some(f[2].to_string())
                },
                ahead,
                behind,
                is_current: f[1] == "*",
                gone,
            });
        } else if let Some(name) = f[0].strip_prefix("refs/remotes/") {
            if !name.ends_with("/HEAD") {
                remote.push(name.to_string());
            }
        } else if let Some(name) = f[0].strip_prefix("refs/tags/") {
            tags.push(TagInfo {
                name: name.to_string(),
                date: f[4].to_string(),
            });
        }
    }

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

const SMART_CHECKOUT_STASH: &str = "Yuzora smart checkout";

/// Runs a branch switch. With `smart` (JetBrains "Smart Checkout") local
/// changes are stashed for the switch and restored on the new HEAD; a restore
/// that conflicts keeps the stash and leaves the conflicts to the merge tool.
pub fn switch_keeping_changes(
    root: &Path,
    smart: bool,
    switch: impl FnOnce() -> Result<(), String>,
) -> Result<GitOperationOutcome, String> {
    if !smart {
        return switch().map(|()| GitOperationOutcome { conflicts: false });
    }
    let magic = [(ALLOW_PATHSPEC_MAGIC_ENV.to_string(), "1".to_string())];
    let pushed = run_ok(
        root,
        &["stash", "push", "-m", SMART_CHECKOUT_STASH],
        DEFAULT_TIMEOUT,
        &magic,
    )?;
    // git reports, rather than fails, when there was nothing to save.
    if String::from_utf8_lossy(&pushed.stdout).contains("No local changes to save") {
        return switch().map(|()| GitOperationOutcome { conflicts: false });
    }
    let parked = stash_top(root).ok().flatten();
    let switched = switch();
    // A failed switch left HEAD where it was, so the changes go back as they were.
    let restored = restore_parked_changes(root, parked.as_deref());
    match (switched, restored) {
        (Ok(()), restored) => restored,
        (Err(error), Ok(_)) => Err(error),
        (Err(error), Err(restore_error)) => Err(format!("{error}\n{restore_error}")),
    }
}

fn stash_top(root: &Path) -> Result<Option<String>, String> {
    let out = run_git(
        root,
        &["rev-parse", "--quiet", "--verify", "refs/stash"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    Ok((out.code == 0).then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// Pops the stash just parked. `parked` is its oid when known: a different
/// top-most stash means someone else stashed meanwhile, so leave both alone.
fn restore_parked_changes(
    root: &Path,
    parked: Option<&str>,
) -> Result<GitOperationOutcome, String> {
    let kept = || {
        format!("git stash: your local changes are kept in the stash \"{SMART_CHECKOUT_STASH}\"")
    };
    if let Some(parked) = parked {
        if stash_top(root).map_err(|_| kept())?.as_deref() != Some(parked) {
            return Err(kept());
        }
    }
    let mut out = run_git(root, &["stash", "pop", "--index"], DEFAULT_TIMEOUT, &[])
        .map_err(|error| format!("{error}\n{}", kept()))?;
    // `--index` refuses when the staged part no longer applies on the new HEAD
    // (git then keeps the stash); restore the changes without their staging,
    // as JetBrains does.
    let conflicted = || {
        status_of(root, None)
            .map(|status| !status.parsed.conflicted.is_empty())
            .map_err(|error| format!("{error}\n{}", kept()))
    };
    if out.code != 0 && !conflicted()? {
        out = run_git(root, &["stash", "pop"], DEFAULT_TIMEOUT, &[])
            .map_err(|error| format!("{error}\n{}", kept()))?;
    }
    operation_outcome(root, "stash", out).map_err(|error| format!("{error}\n{}", kept()))
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

/// Result of an operation that can stop halfway on merge conflicts. The
/// repository is then left mid-operation for the conflict banner to resume.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitOperationOutcome {
    pub conflicts: bool,
}

/// Success → `conflicts: false`. A non-zero exit that left unmerged paths is a
/// conflict stop (`conflicts: true`); any other failure stays an error.
fn operation_outcome(
    root: &Path,
    sub: &str,
    out: GitOutput,
) -> Result<GitOperationOutcome, String> {
    if out.code == 0 {
        return Ok(GitOperationOutcome { conflicts: false });
    }
    if !status_of(root, None)?.parsed.conflicted.is_empty() {
        return Ok(GitOperationOutcome { conflicts: true });
    }
    Err(git_err(sub, &out.stderr))
}

fn reject_option_like(kind: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains('\0') || crate::git_oid::looks_like_git_option(value) {
        return Err(format!("git rejected an invalid {kind}"));
    }
    Ok(())
}

/// Merge `name` into the checked-out branch. The branch name (not its oid) is
/// passed so the generated merge message names the branch.
pub fn merge_branch(root: &Path, name: &str) -> Result<GitOperationOutcome, String> {
    reject_option_like("branch name", name)?;
    crate::git_oid::resolve_commit_oid(root, name)?;
    let out = run_git(
        root,
        &merge_args(root, name)?,
        DEFAULT_TIMEOUT,
        &editor_true(),
    )?;
    operation_outcome(root, "merge", out)
}

fn merge_args<'a>(root: &Path, name: &'a str) -> Result<Vec<&'a str>, String> {
    let mut args = vec!["merge", "--no-edit"];
    if git_at_least(root, MERGE_AUTOSTASH_VERSION, &[])? {
        args.push("--autostash");
    }
    args.extend(["--end-of-options", name]);
    Ok(args)
}

/// Rebase the checked-out branch onto `upstream`.
pub fn rebase_onto(root: &Path, upstream: &str) -> Result<GitOperationOutcome, String> {
    reject_option_like("rebase target", upstream)?;
    let oid = crate::git_oid::resolve_commit_oid(root, upstream)?;
    let out = run_git(
        root,
        &["rebase", "--autostash", "--end-of-options", oid.as_str()],
        DEFAULT_TIMEOUT,
        &editor_true(),
    )?;
    operation_outcome(root, "rebase", out)
}

pub fn rename_branch(root: &Path, old_name: &str, new_name: &str) -> Result<(), String> {
    reject_option_like("branch name", old_name)?;
    reject_option_like("branch name", new_name)?;
    let valid = run_git(
        root,
        &["check-ref-format", "--branch", new_name],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if valid.code != 0 {
        return Err("git: invalid branch name".to_string());
    }
    run_ok(
        root,
        &["branch", "-m", "--end-of-options", old_name, new_name],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    Ok(())
}

/// Delete a local branch. Without `force`, git refuses an unmerged branch with
/// "not fully merged", which the UI uses to offer a forced delete.
pub fn delete_branch(root: &Path, name: &str, force: bool) -> Result<(), String> {
    reject_option_like("branch name", name)?;
    let head = run_git(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if head.code == 0 && String::from_utf8_lossy(&head.stdout).trim() == name {
        return Err(format!("git: cannot delete the current branch '{name}'"));
    }
    let flag = if force { "-D" } else { "-d" };
    run_ok(
        root,
        &["branch", flag, "--end-of-options", name],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    Ok(())
}

pub fn revert_commit(root: &Path, hash: &str) -> Result<GitOperationOutcome, String> {
    let oid = crate::git_oid::resolve_commit_oid(root, hash)?;
    let out = run_git(
        root,
        &["revert", "--no-edit", "--end-of-options", oid.as_str()],
        DEFAULT_TIMEOUT,
        &editor_true(),
    )?;
    operation_outcome(root, "revert", out)
}

/// `reset --<mode> <hash>`; "Undo last commit" is a soft reset to `HEAD~1`.
pub fn reset_branch(root: &Path, hash: &str, mode: &str) -> Result<(), String> {
    let flag = match mode {
        "soft" => "--soft",
        "mixed" => "--mixed",
        "hard" => "--hard",
        "keep" => "--keep",
        _ => return Err(format!("git: unsupported reset mode '{mode}'")),
    };
    let oid = crate::git_oid::resolve_commit_oid(root, hash)?;
    run_ok(
        root,
        &["reset", flag, "--end-of-options", oid.as_str()],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    Ok(())
}

#[derive(serde::Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitStashEntry {
    pub index: u32,
    pub oid: String,
    pub message: String,
    pub timestamp: i64,
}

pub fn stash_list(root: &Path) -> Result<Vec<GitStashEntry>, String> {
    let out = run_ok(
        root,
        &["stash", "list", "-z", "--format=%gd%x1f%H%x1f%s%x1f%ct"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| {
            let mut fields = record.splitn(4, '\u{1f}');
            let (Some(name), Some(oid), Some(message), Some(time)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err("git stash list: unexpected output".to_string());
            };
            let index = name
                .strip_prefix("stash@{")
                .and_then(|rest| rest.strip_suffix('}'))
                .and_then(|n| n.parse::<u32>().ok())
                .ok_or_else(|| "git stash list: unexpected stash name".to_string())?;
            Ok(GitStashEntry {
                index,
                oid: oid.to_string(),
                message: message.to_string(),
                timestamp: time.trim().parse::<i64>().unwrap_or(0),
            })
        })
        .collect()
}

pub fn stash_push(
    root: &Path,
    message: Option<&str>,
    include_untracked: bool,
    keep_index: bool,
) -> Result<(), String> {
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("-u");
    }
    if keep_index {
        args.push("--keep-index");
    }
    if let Some(message) = message.filter(|m| !m.is_empty()) {
        args.extend(["-m", message]);
    }
    let magic = [(ALLOW_PATHSPEC_MAGIC_ENV.to_string(), "1".to_string())];
    let out = run_ok(root, &args, DEFAULT_TIMEOUT, &magic)?;
    // git reports "No local changes to save" yet exits 0.
    if String::from_utf8_lossy(&out.stdout).contains("No local changes to save") {
        return Err("git stash: nothing to stash".to_string());
    }
    Ok(())
}

/// Abort unless `stash@{index}` still names `oid`: another client may have
/// pushed or dropped a stash since the list was read, renumbering the entries.
fn verify_stash_identity(root: &Path, name: &str, oid: &str) -> Result<(), String> {
    let current = run_git(
        root,
        &["rev-parse", "--quiet", "--verify", name],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if current.code == 0 && String::from_utf8_lossy(&current.stdout).trim() == oid {
        return Ok(());
    }
    Err(format!(
        "git stash: {name} changed since the list was loaded; reload and try again"
    ))
}

/// Arguments that apply a stash by its commit oid. Unlike `stash@{N}`, an oid
/// cannot be renumbered by another client pushing or dropping a stash after
/// the identity check.
fn stash_apply_args(oid: &str) -> [&str; 3] {
    ["stash", "apply", oid]
}

/// Apply (or pop) `stash@{index}`, which must still be commit `oid`. The
/// stash is applied by oid, so the check cannot be outrun. A conflicting apply
/// leaves unmerged paths; a conflicting `pop` keeps the stash, as git does.
/// `git stash pop` only accepts `stash@{N}`, so pop is apply-by-oid followed by
/// a drop that re-verifies the entry and is skipped if it moved meanwhile.
pub fn stash_apply(
    root: &Path,
    index: u32,
    oid: &str,
    pop: bool,
) -> Result<GitOperationOutcome, String> {
    let name = format!("stash@{{{index}}}");
    verify_stash_identity(root, &name, oid)?;
    let out = run_git(root, &stash_apply_args(oid), DEFAULT_TIMEOUT, &[])?;
    let outcome = operation_outcome(root, "stash", out)?;
    // Changes are applied; the stash is only removed when it is still the
    // listed one. Otherwise keep it: the list reloads and still shows it.
    if pop && !outcome.conflicts && verify_stash_identity(root, &name, oid).is_ok() {
        // The changes are already applied: report a failed drop as such, not as a failed pop that a
        // retry would apply again.
        run_ok(root, &["stash", "drop", &name], DEFAULT_TIMEOUT, &[]).map_err(|error| {
            format!("git stash: applied {name} but could not drop it, so it is kept: {error}")
        })?;
    }
    Ok(outcome)
}

/// Drop `stash@{index}` after checking it is still commit `oid`. A residual
/// millisecond window remains between the check and the drop: `git stash drop`
/// only accepts `stash@{N}` and offers no atomic compare-and-delete.
pub fn stash_drop(root: &Path, index: u32, oid: &str) -> Result<(), String> {
    let name = format!("stash@{{{index}}}");
    verify_stash_identity(root, &name, oid)?;
    run_ok(root, &["stash", "drop", &name], DEFAULT_TIMEOUT, &[])?;
    Ok(())
}

/// One unmerged path for the merge tool: stage 1 (common base), 2 (ours, the
/// checked-out side), 3 (theirs, the incoming side) and the working-tree file
/// as Git left it, with conflict markers. A missing side is `None`.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitConflictSides {
    pub code: String,
    pub base: Option<GradedText>,
    pub ours: Option<GradedText>,
    pub theirs: Option<GradedText>,
    pub worktree: Option<GradedText>,
}

pub fn conflict_sides(root: &Path, path: &str) -> Result<GitConflictSides, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("git could not resolve repository root: {error}"))?;
    validate_diff_relative_path(root, &canonical_root, path)?;
    let status = status_of(root, None)?;
    let code = status
        .parsed
        .conflicted
        .iter()
        .find(|entry| entry.path == path)
        .map(|entry| entry.status.clone())
        .ok_or_else(|| format!("git: {path} is not in conflict"))?;
    Ok(GitConflictSides {
        code,
        base: show_object(root, &format!(":1:{path}"))?,
        ours: show_object(root, &format!(":2:{path}"))?,
        theirs: show_object(root, &format!(":3:{path}"))?,
        worktree: read_worktree(root, path)?.map(|bytes| grade_bytes(&bytes)),
    })
}

/// Index stages present for an unmerged path (`git ls-files --stage`).
fn conflict_stages(root: &Path, path: &str) -> Result<Vec<u8>, String> {
    let out = run_ok(
        root,
        &["ls-files", "--stage", "-z", "--", path],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    Ok(out
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|record| {
            // "<mode> <oid> <stage>\t<path>"
            let head = record.split(|byte| *byte == b'\t').next()?;
            let stage = head.rsplit(|byte| *byte == b' ').next()?;
            (stage.len() == 1).then(|| stage[0].wrapping_sub(b'0'))
        })
        .collect())
}

/// JetBrains "Accept Yours / Accept Theirs" for whole files: take one side of
/// each unmerged path and mark it resolved. A side that deleted the file
/// resolves to the deletion.
pub fn conflict_resolve(root: &Path, paths: &[String], side: &str) -> Result<(), String> {
    let (stage, flag) = match side {
        "ours" => (2u8, "--ours"),
        "theirs" => (3u8, "--theirs"),
        _ => return Err(format!("invalid conflict side: {side}")),
    };
    if paths.is_empty() {
        return Err("git conflict resolve requires at least one path".into());
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("git could not resolve repository root: {error}"))?;
    let conflicted: std::collections::HashSet<String> = status_of(root, None)?
        .parsed
        .conflicted
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    for path in paths {
        reject_lossy_path("conflict resolve", path)?;
        validate_diff_relative_path(root, &canonical_root, path)?;
        if !conflicted.contains(path) {
            return Err(format!("git: {path} is not in conflict"));
        }
    }
    for path in paths {
        if conflict_stages(root, path)?.contains(&stage) {
            run_ok(root, &["checkout", flag, "--", path], DEFAULT_TIMEOUT, &[])?;
            run_ok(root, &["add", "--", path], DEFAULT_TIMEOUT, &[])?;
        } else {
            run_ok(root, &["rm", "--quiet", "--", path], DEFAULT_TIMEOUT, &[])?;
        }
    }
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

/// rebase|cherry-pick|revert → <op> --skip (drops the commit being applied).
/// A merge has no commit to skip.
pub fn conflict_skip(root: &Path, op: &str) -> Result<(), String> {
    check_conflict_op(op)?;
    if op == "merge" {
        return Err("git merge has no --skip; abort or resolve the conflicts".into());
    }
    run_ok(root, &[op, "--skip"], DEFAULT_TIMEOUT, &editor_true())?;
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

/// Read and grade an object when it exists. Operational `git show` failures
/// must reach the UI; a normal missing side is `None`.
pub fn show_object(root: &Path, spec: &str) -> Result<Option<GradedText>, String> {
    // `show` already distinguishes a missing object from other failures. A
    // separate existence probe doubles process startup on every diff side.
    let out = match run_git_with_stdout_limit(
        root,
        &["show", "--end-of-options", spec],
        DEFAULT_TIMEOUT,
        &[],
        BLOB_READ_LIMIT,
    ) {
        Err(error) if error == "git-output-limit" => return Ok(Some(GradedText::TooLarge)),
        other => other?,
    };
    if out.code != 0 {
        let stderr = out.stderr.to_ascii_lowercase();
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
            Err(git_err("show", &out.stderr))
        };
    }
    Ok(Some(grade_bytes(&out.stdout)))
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
        if !metadata.is_file() {
            return Err(format!("git diff cannot read non-regular file {path}"));
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(&full)
            .and_then(|file| {
                file.take(crate::file_content::HARD_CAP_BYTES + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| format!("git diff could not read {path}: {error}"))?;
        Ok(Some(bytes))
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

        // FIFOs/devices would block the open or read forever while the repository
        // operation lock is held; only regular files carry diffable content.
        if (st.st_mode & libc::S_IFMT) != libc::S_IFREG {
            return Err(format!("git diff cannot read non-regular file {path}"));
        }
        // O_NONBLOCK keeps a FIFO swapped in after fstatat from blocking the open;
        // it has no effect on regular-file reads.
        let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        let fd = unsafe { libc::openat(current.as_raw_fd(), name_c.as_ptr(), flags) };
        if fd < 0 {
            return map_open_err(path, std::io::Error::last_os_error());
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            return Err(format!("git diff cannot read non-regular file {path}"));
        }
        // Bound the read: grade_bytes reports anything above the hard cap as TooLarge.
        let mut bytes = Vec::new();
        file.take(crate::file_content::HARD_CAP_BYTES + 1)
            .read_to_end(&mut bytes)
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
            orig.unwrap_or_else(empty_text),
            modi.unwrap_or_else(empty_text),
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
            orig.unwrap_or_else(empty_text),
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
    pub fn isolated_env() -> Vec<(String, String)> {
        vec![
            ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
            ("GIT_CONFIG_SYSTEM".to_string(), "/dev/null".to_string()),
        ]
    }

    pub fn git(dir: &Path, args: &[&str]) {
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
        // Windows Git defaults to autocrlf=true; fixtures compare exact bytes.
        git(dir, &["config", "core.autocrlf", "false"]);
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

    fn clone_with_bare_remote() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("remote.git");
        std::fs::create_dir_all(&bare).unwrap();
        test_repo::git(&bare, &["init", "--bare", "-b", "main"]);
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        test_repo::init(&work);
        test_repo::write_and_commit(&work, "a.txt", "1", "c1");
        test_repo::git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        (tmp, work)
    }

    #[test]
    fn push_publishes_a_branch_without_upstream() {
        let (_tmp, work) = clone_with_bare_remote();
        let env = test_repo::isolated_env();
        assert_eq!(
            push_args_with_env(&work, &env).unwrap(),
            ["push", "--set-upstream", "origin", "main"]
        );
        test_repo::git(&work, &["push", "--set-upstream", "origin", "main"]);
        assert_eq!(push_args_with_env(&work, &env).unwrap(), ["push"]);
    }

    #[test]
    fn push_prefers_configured_push_remote_and_reports_missing_remote() {
        let (_tmp, work) = clone_with_bare_remote();
        let env = test_repo::isolated_env();
        test_repo::git(&work, &["remote", "add", "fork", "../remote.git"]);
        test_repo::git(&work, &["config", "remote.pushDefault", "fork"]);
        assert_eq!(
            push_args_with_env(&work, &env).unwrap(),
            ["push", "--set-upstream", "fork", "main"]
        );

        let lonely = tempfile::tempdir().unwrap();
        test_repo::init(lonely.path());
        test_repo::write_and_commit(lonely.path(), "a.txt", "1", "c1");
        let error = push_args_with_env(lonely.path(), &env).unwrap_err();
        assert!(error.contains("no remote"), "{error}");
    }

    #[test]
    fn pull_defaults_to_merge_but_keeps_configured_strategy() {
        let (_tmp, work) = clone_with_bare_remote();
        let env = test_repo::isolated_env();
        assert_eq!(
            pull_args_with_env(&work, &env).unwrap(),
            ["pull", "--no-rebase", "--autostash"]
        );
        test_repo::git(&work, &["config", "branch.main.rebase", "true"]);
        assert_eq!(
            pull_args_with_env(&work, &env).unwrap(),
            ["pull", "--autostash"]
        );
        test_repo::git(&work, &["config", "--unset", "branch.main.rebase"]);
        test_repo::git(&work, &["config", "pull.ff", "only"]);
        assert_eq!(
            pull_args_with_env(&work, &env).unwrap(),
            ["pull", "--autostash"]
        );
    }

    #[test]
    fn diverged_pull_with_default_args_merges_instead_of_aborting() {
        let (tmp, work) = clone_with_bare_remote();
        test_repo::git(&work, &["push", "--set-upstream", "origin", "main"]);
        let other = tmp.path().join("other");
        test_repo::git(tmp.path(), &["clone", "remote.git", "other"]);
        test_repo::git(&other, &["config", "user.email", "t@t"]);
        test_repo::git(&other, &["config", "user.name", "t"]);
        test_repo::write_and_commit(&other, "b.txt", "2", "theirs");
        test_repo::git(&other, &["push"]);
        test_repo::write_and_commit(&work, "c.txt", "3", "ours");

        let mut env = test_repo::isolated_env();
        env.extend(editor_true());
        let bare_pull = run_git(&work, &["pull"], REMOTE_TIMEOUT, &env).unwrap();
        assert_ne!(
            bare_pull.code, 0,
            "a bare diverged pull must abort without a strategy"
        );

        let args = pull_args_with_env(&work, &test_repo::isolated_env()).unwrap();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run_git(&work, &args, REMOTE_TIMEOUT, &env).unwrap();
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(work.join("b.txt").exists());
    }

    #[test]
    fn staging_a_conflicted_file_marks_it_resolved_only_without_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "c.txt", "base\n", "base");
        test_repo::git(repo, &["switch", "-c", "feature"]);
        test_repo::write_and_commit(repo, "c.txt", "theirs\n", "theirs");
        test_repo::git(repo, &["switch", "main"]);
        test_repo::write_and_commit(repo, "c.txt", "ours\n", "ours");
        let picked = run_git(
            repo,
            &["cherry-pick", "feature"],
            DEFAULT_TIMEOUT,
            &test_repo::isolated_env(),
        )
        .unwrap();
        assert_ne!(picked.code, 0, "the cherry-pick must conflict");
        let paths = vec!["c.txt".to_string()];

        let error = stage(repo, &paths).unwrap_err();
        assert!(
            error.contains("conflict markers remain in c.txt"),
            "{error}"
        );
        assert_eq!(status_of(repo, None).unwrap().parsed.conflicted.len(), 1);

        std::fs::write(repo.join("c.txt"), "ours\ntheirs\n").unwrap();
        stage(repo, &paths).unwrap();
        assert!(status_of(repo, None).unwrap().parsed.conflicted.is_empty());
        let mut env = test_repo::isolated_env();
        env.extend(editor_true());
        let resumed = run_git(repo, &["cherry-pick", "--continue"], DEFAULT_TIMEOUT, &env).unwrap();
        assert_eq!(resumed.code, 0, "{}", resumed.stderr);
    }

    /// main and `feature` both edit c.txt; `feature` also deletes d.txt that
    /// main edits. Cherry-picking `feature` onto main conflicts on both.
    fn conflicted_cherry_pick() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        std::fs::write(repo.join("d.txt"), "base\n").unwrap();
        test_repo::git(repo, &["add", "d.txt"]);
        test_repo::write_and_commit(repo, "c.txt", "base\n", "base");
        test_repo::git(repo, &["switch", "-c", "feature"]);
        std::fs::write(repo.join("c.txt"), "theirs\n").unwrap();
        test_repo::git(repo, &["rm", "-q", "d.txt"]);
        test_repo::git(repo, &["commit", "-qam", "theirs"]);
        test_repo::git(repo, &["switch", "main"]);
        std::fs::write(repo.join("d.txt"), "ours edit\n").unwrap();
        test_repo::git(repo, &["add", "d.txt"]);
        test_repo::write_and_commit(repo, "c.txt", "ours\n", "ours");
        let picked = run_git(
            repo,
            &["cherry-pick", "feature"],
            DEFAULT_TIMEOUT,
            &test_repo::isolated_env(),
        )
        .unwrap();
        assert_ne!(picked.code, 0, "the cherry-pick must conflict");
        tmp
    }

    fn text(side: &Option<GradedText>) -> Option<String> {
        match side {
            Some(GradedText::Full { content }) => Some(content.clone()),
            _ => None,
        }
    }

    #[test]
    fn conflict_sides_report_the_xy_code_and_every_stage() {
        let tmp = conflicted_cherry_pick();
        let repo = tmp.path();
        let codes: Vec<(String, String)> = status_of(repo, None)
            .unwrap()
            .parsed
            .conflicted
            .into_iter()
            .map(|entry| (entry.path, entry.status))
            .collect();
        assert_eq!(
            codes,
            [
                ("c.txt".to_string(), "UU".to_string()),
                ("d.txt".to_string(), "UD".to_string())
            ]
        );

        let sides = conflict_sides(repo, "c.txt").unwrap();
        assert_eq!(sides.code, "UU");
        assert_eq!(text(&sides.base).as_deref(), Some("base\n"));
        assert_eq!(text(&sides.ours).as_deref(), Some("ours\n"));
        assert_eq!(text(&sides.theirs).as_deref(), Some("theirs\n"));
        assert!(text(&sides.worktree).unwrap().contains("<<<<<<<"));

        let deleted = conflict_sides(repo, "d.txt").unwrap();
        assert!(deleted.theirs.is_none());
        assert!(conflict_sides(repo, "missing.txt").is_err());
    }

    #[test]
    fn accepting_a_side_resolves_whole_files_including_deletions() {
        let tmp = conflicted_cherry_pick();
        let repo = tmp.path();
        conflict_resolve(repo, &["c.txt".to_string()], "theirs").unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("c.txt")).unwrap(),
            "theirs\n"
        );
        // Theirs deleted d.txt: accepting theirs keeps the deletion.
        conflict_resolve(repo, &["d.txt".to_string()], "theirs").unwrap();
        assert!(!repo.join("d.txt").exists());
        assert!(status_of(repo, None).unwrap().parsed.conflicted.is_empty());
        assert!(conflict_resolve(repo, &["c.txt".to_string()], "ours").is_err());
        assert!(conflict_resolve(repo, &[], "ours").is_err());

        let tmp = conflicted_cherry_pick();
        let repo = tmp.path();
        assert!(conflict_resolve(repo, &["c.txt".to_string()], "mine").is_err());
        conflict_resolve(repo, &["c.txt".to_string(), "d.txt".to_string()], "ours").unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("c.txt")).unwrap(),
            "ours\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("d.txt")).unwrap(),
            "ours edit\n"
        );
        assert!(status_of(repo, None).unwrap().parsed.conflicted.is_empty());
    }

    #[test]
    fn skip_drops_the_conflicting_commit_but_never_a_merge() {
        let tmp = conflicted_cherry_pick();
        let repo = tmp.path();
        assert!(conflict_skip(repo, "merge").is_err());
        assert!(conflict_skip(repo, "status").is_err());
        conflict_skip(repo, "cherry-pick").unwrap();
        let status = status_of(repo, None).unwrap();
        assert!(status.parsed.conflicted.is_empty());
        assert!(!repo.join(".git/CHERRY_PICK_HEAD").exists());
        assert_eq!(
            std::fs::read_to_string(repo.join("c.txt")).unwrap(),
            "ours\n"
        );
    }

    #[test]
    fn conflict_marker_scan_needs_an_opening_and_closing_marker() {
        assert!(has_conflict_markers(
            b"a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> feature\n"
        ));
        assert!(has_conflict_markers(b"<<<<<<< HEAD\r\nx\r\n>>>>>>> f\r\n"));
        assert!(!has_conflict_markers(b"Title\n=======\n>>>>>>> quoted\n"));
        assert!(!has_conflict_markers(b"plain text\n"));
    }

    #[test]
    fn long_stderr_keeps_the_fatal_and_rejected_lines() {
        let mut stderr = String::new();
        for index in 0..200 {
            stderr.push_str(&format!("remote: hook output line {index}\n"));
        }
        stderr.push_str("To github.com:owner/repo.git\n");
        stderr.push_str(" ! [rejected]        main -> main (fetch first)\n");
        stderr.push_str("error: failed to push some refs to 'github.com:owner/repo.git'\n");
        stderr.push_str("hint: Updates were rejected because the remote contains work\n");
        let message = git_err("push", &stderr);
        assert!(message.starts_with("git push: "));
        assert!(message.contains("! [rejected]"), "{message}");
        assert!(message.contains("error: failed to push"), "{message}");
        assert!(!message.contains("hook output line 0\n"), "{message}");
        assert_eq!(git_err("push", "  short  "), "git push: short");
    }

    #[test]
    fn git_command_forces_literal_pathspecs_unless_the_reviewed_escape_hatch_is_set() {
        let literal = |env: &[(String, String)]| {
            let cmd = git_command(Path::new("."), &["status"], env, false);
            let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
            (
                envs.get(std::ffi::OsStr::new("GIT_LITERAL_PATHSPECS"))
                    .cloned()
                    .flatten()
                    .map(|v| v.to_os_string()),
                envs.contains_key(std::ffi::OsStr::new(ALLOW_PATHSPEC_MAGIC_ENV)),
            )
        };
        let forced = [("GIT_LITERAL_PATHSPECS".to_string(), "0".to_string())];
        assert_eq!(literal(&forced), (Some("1".into()), false));
        let magic = [(ALLOW_PATHSPEC_MAGIC_ENV.to_string(), "1".to_string())];
        assert_eq!(literal(&magic), (None, false));
    }

    #[test]
    fn git_command_drops_inherited_repository_location_overrides() {
        let cmd = git_command(Path::new("."), &["status"], &[], false);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
        for key in INHERITED_REPOSITORY_ENV {
            assert_eq!(
                envs.get(std::ffi::OsStr::new(key)),
                Some(&None),
                "{key} must be removed"
            );
        }
        let explicit = [("GIT_INDEX_FILE".to_string(), "/tmp/index".to_string())];
        let cmd = git_command(Path::new("."), &["status"], &explicit, false);
        let envs: std::collections::HashMap<_, _> = cmd.get_envs().collect();
        assert_eq!(
            envs.get(std::ffi::OsStr::new("GIT_INDEX_FILE")),
            Some(&Some(std::ffi::OsStr::new("/tmp/index")))
        );
    }

    fn head_subject(repo: &Path) -> String {
        let out = run_git(repo, &["log", "-1", "--format=%s"], DEFAULT_TIMEOUT, &[]).unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn branch_names(repo: &Path) -> Vec<String> {
        let out = run_git(
            repo,
            &["branch", "--format=%(refname:short)"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// main: base -> m1; feature: base -> f1 (disjoint files unless `clash`).
    fn diverged_repo(clash: bool) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "c.txt", "base\n", "base");
        test_repo::git(repo, &["switch", "-c", "feature"]);
        test_repo::write_and_commit(
            repo,
            if clash { "c.txt" } else { "f.txt" },
            "feature\n",
            "f1",
        );
        test_repo::git(repo, &["switch", "main"]);
        test_repo::write_and_commit(repo, "c.txt", "main\n", "m1");
        tmp
    }

    #[test]
    fn merge_branch_merges_cleanly_and_names_the_branch() {
        let tmp = diverged_repo(false);
        let repo = tmp.path();
        // diverged_repo edits c.txt on main only, so feature's f.txt merges cleanly.
        let outcome = merge_branch(repo, "feature").unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert!(repo.join("f.txt").exists());
        assert!(
            head_subject(repo).contains("feature"),
            "{}",
            head_subject(repo)
        );
    }

    #[test]
    fn merge_branch_reports_conflicts_and_rejects_bad_names() {
        let tmp = diverged_repo(true);
        let repo = tmp.path();
        assert!(merge_branch(repo, "--abort").is_err());
        assert!(merge_branch(repo, "no-such-branch").is_err());
        let outcome = merge_branch(repo, "feature").unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: true });
        assert_eq!(
            status_of(repo, None).unwrap().in_progress.as_deref(),
            Some("merge")
        );
    }

    #[test]
    fn rebase_onto_replays_commits_and_reports_conflicts() {
        let tmp = diverged_repo(false);
        let repo = tmp.path();
        test_repo::git(repo, &["switch", "feature"]);
        let outcome = rebase_onto(repo, "main").unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert_eq!(head_subject(repo), "f1");
        let parent = run_git(repo, &["log", "-2", "--format=%s"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&parent.stdout)
                .lines()
                .collect::<Vec<_>>(),
            ["f1", "m1"]
        );

        let tmp = diverged_repo(true);
        let repo = tmp.path();
        test_repo::git(repo, &["switch", "feature"]);
        assert!(rebase_onto(repo, "--onto").is_err());
        let outcome = rebase_onto(repo, "main").unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: true });
        assert_eq!(
            status_of(repo, None).unwrap().in_progress.as_deref(),
            Some("rebase")
        );
    }

    #[test]
    fn rename_branch_renames_and_validates() {
        let tmp = diverged_repo(false);
        let repo = tmp.path();
        rename_branch(repo, "feature", "topic/x").unwrap();
        let names = branch_names(repo);
        assert!(names.contains(&"topic/x".to_string()) && !names.contains(&"feature".to_string()));
        assert!(rename_branch(repo, "topic/x", "bad name")
            .unwrap_err()
            .contains("invalid branch name"));
        assert!(rename_branch(repo, "topic/x", "-x").is_err());
        assert!(rename_branch(repo, "missing", "other").is_err());
        assert!(branch_names(repo).contains(&"topic/x".to_string()));
    }

    #[test]
    fn delete_branch_refuses_current_and_unmerged_until_forced() {
        let tmp = diverged_repo(false);
        let repo = tmp.path();
        let err = delete_branch(repo, "main", true).unwrap_err();
        assert!(err.contains("current branch"), "{err}");
        let err = delete_branch(repo, "feature", false).unwrap_err();
        assert!(err.contains("not fully merged"), "{err}");
        assert!(branch_names(repo).contains(&"feature".to_string()));
        delete_branch(repo, "feature", true).unwrap();
        assert!(!branch_names(repo).contains(&"feature".to_string()));
        assert!(delete_branch(repo, "-D", false).is_err());

        let tmp = diverged_repo(false);
        let repo = tmp.path();
        test_repo::git(repo, &["merge", "--no-edit", "feature"]);
        delete_branch(repo, "feature", false).unwrap();
        assert!(!branch_names(repo).contains(&"feature".to_string()));
    }

    #[test]
    fn revert_commit_adds_an_inverse_commit_or_reports_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        test_repo::write_and_commit(repo, "b.txt", "2\n", "c2");
        let outcome = revert_commit(repo, "HEAD").unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert!(!repo.join("b.txt").exists());
        assert!(
            head_subject(repo).starts_with("Revert"),
            "{}",
            head_subject(repo)
        );
        assert!(revert_commit(repo, "--abort").is_err());

        // Reverting c1 conflicts once a later commit edits the same file.
        test_repo::write_and_commit(repo, "a.txt", "changed\n", "c3");
        let outcome = revert_commit(repo, "HEAD~3").unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: true });
        assert_eq!(
            status_of(repo, None).unwrap().in_progress.as_deref(),
            Some("revert")
        );
    }

    #[test]
    fn reset_branch_modes_move_head_and_keep_the_right_state() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        test_repo::write_and_commit(repo, "a.txt", "2\n", "c2");

        // "Undo last commit": soft reset to HEAD~1 keeps the change staged.
        reset_branch(repo, "HEAD~1", "soft").unwrap();
        assert_eq!(head_subject(repo), "c1");
        let status = status_of(repo, None).unwrap();
        assert_eq!(status.parsed.staged.len(), 1);
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "2\n");

        reset_branch(repo, "HEAD", "mixed").unwrap();
        let status = status_of(repo, None).unwrap();
        assert!(status.parsed.staged.is_empty() && !status.parsed.unstaged.is_empty());

        reset_branch(repo, "HEAD", "hard").unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "1\n");

        let err = reset_branch(repo, "HEAD", "bogus").unwrap_err();
        assert!(err.contains("unsupported reset mode"), "{err}");
        assert!(reset_branch(repo, "--hard", "soft").is_err());
        reset_branch(repo, "HEAD", "keep").unwrap();
        // The root commit has no parent to undo onto.
        assert!(reset_branch(repo, "HEAD~1", "soft").is_err());
        assert!(crate::git_oid::resolve_commit_oid(repo, "HEAD~1").is_err());
    }

    #[test]
    fn stash_push_list_apply_pop_and_drop_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        assert!(stash_list(repo).unwrap().is_empty());
        let err = stash_push(repo, None, false, false).unwrap_err();
        assert!(err.contains("nothing to stash"), "{err}");

        std::fs::write(repo.join("a.txt"), "dirty\n").unwrap();
        stash_push(repo, Some("first work"), false, false).unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "1\n");
        std::fs::write(repo.join("new.txt"), "u\n").unwrap();
        stash_push(repo, Some("with untracked"), true, false).unwrap();
        assert!(!repo.join("new.txt").exists());

        let list = stash_list(repo).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].index, 0);
        assert!(list[0].message.contains("with untracked"), "{:?}", list[0]);
        assert!(list[1].message.contains("first work"), "{:?}", list[1]);
        assert!(list[0].timestamp > 1_000_000_000);

        // apply keeps the entry, pop removes it.
        let outcome = stash_apply(repo, 1, &list[1].oid, false).unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "dirty\n"
        );
        assert_eq!(stash_list(repo).unwrap().len(), 2);
        test_repo::git(repo, &["checkout", "--", "a.txt"]);
        stash_apply(repo, 1, &list[1].oid, true).unwrap();
        let list = stash_list(repo).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].message.contains("with untracked"));

        let last = stash_list(repo).unwrap()[0].oid.clone();
        stash_drop(repo, 0, &last).unwrap();
        assert!(stash_list(repo).unwrap().is_empty());
        assert!(stash_drop(repo, 0, &last).is_err());
    }

    #[test]
    fn stash_operations_abort_when_the_index_no_longer_names_the_listed_stash() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        std::fs::write(repo.join("a.txt"), "first\n").unwrap();
        stash_push(repo, Some("first"), false, false).unwrap();
        let listed = stash_list(repo).unwrap();
        assert_eq!(listed[0].oid.len(), 40);

        // Another client pushes: stash@{0} now names a different stash.
        std::fs::write(repo.join("a.txt"), "second\n").unwrap();
        stash_push(repo, Some("second"), false, false).unwrap();
        let before = stash_list(repo).unwrap();

        for pop in [false, true] {
            let err = stash_apply(repo, 0, &listed[0].oid, pop).unwrap_err();
            assert!(err.contains("changed since the list"), "{err}");
        }
        let err = stash_drop(repo, 0, &listed[0].oid).unwrap_err();
        assert!(err.contains("changed since the list"), "{err}");
        // A vanished index also aborts.
        assert!(stash_drop(repo, 5, &listed[0].oid).is_err());

        assert_eq!(stash_list(repo).unwrap(), before);
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "1\n");

        // The matching oid still works at its renumbered index.
        stash_drop(repo, 1, &listed[0].oid).unwrap();
        assert_eq!(stash_list(repo).unwrap().len(), 1);
    }

    #[test]
    fn stash_apply_targets_the_oid_not_a_renumberable_index() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(stash_apply_args(oid), ["stash", "apply", oid]);
        assert!(!stash_apply_args(oid).iter().any(|a| a.contains("stash@")));
    }

    #[cfg(unix)]
    #[test]
    fn stash_pop_reports_an_applied_stash_whose_drop_failed() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        std::fs::write(repo.join("a.txt"), "stashed\n").unwrap();
        stash_push(repo, None, false, false).unwrap();
        let oid = stash_list(repo).unwrap()[0].oid.clone();
        // Applying needs no ref writes; dropping must lock refs/stash, which a read-only refs dir refuses.
        let refs = repo.join(".git/refs");
        std::fs::set_permissions(&refs, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = stash_apply(repo, 0, &oid, true);
        std::fs::set_permissions(&refs, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = result.unwrap_err();
        assert!(
            error.contains("applied stash@{0} but could not drop it"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "stashed\n"
        );
        assert_eq!(stash_list(repo).unwrap().len(), 1);
    }

    #[test]
    fn stash_pop_removes_only_the_popped_stash() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        std::fs::write(repo.join("a.txt"), "first\n").unwrap();
        stash_push(repo, Some("first"), false, false).unwrap();
        std::fs::write(repo.join("a.txt"), "second\n").unwrap();
        stash_push(repo, Some("second"), false, false).unwrap();
        let listed = stash_list(repo).unwrap();

        let outcome = stash_apply(repo, 1, &listed[1].oid, true).unwrap();
        assert!(!outcome.conflicts);
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "first\n"
        );
        let left = stash_list(repo).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].oid, listed[0].oid);
    }

    #[test]
    fn stash_keep_index_leaves_staged_changes_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        std::fs::write(repo.join("a.txt"), "staged\n").unwrap();
        test_repo::git(repo, &["add", "a.txt"]);
        stash_push(repo, None, false, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "staged\n"
        );
        assert_eq!(stash_list(repo).unwrap().len(), 1);
    }

    #[test]
    fn conflicting_stash_pop_reports_conflicts_and_keeps_the_stash() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        std::fs::write(repo.join("a.txt"), "stashed\n").unwrap();
        stash_push(repo, None, false, false).unwrap();
        test_repo::write_and_commit(repo, "a.txt", "committed\n", "c2");
        let outcome = stash_apply(repo, 0, &stash_list(repo).unwrap()[0].oid, true).unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: true });
        assert_eq!(
            stash_list(repo).unwrap().len(),
            1,
            "a conflicting pop keeps the stash"
        );
        assert!(!status_of(repo, None).unwrap().parsed.conflicted.is_empty());
    }

    const EIGHT_LINES: &str = "1\n2\n3\n4\n5\n6\n7\n8\n";

    /// main and feature share c.txt; feature rewrites its first line.
    fn eight_line_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "c.txt", EIGHT_LINES, "base");
        test_repo::git(repo, &["switch", "-c", "feature"]);
        let first = EIGHT_LINES.replacen("1\n", "one\n", 1);
        test_repo::write_and_commit(repo, "c.txt", &first, "f1");
        test_repo::git(repo, &["switch", "main"]);
        tmp
    }

    fn git_line(repo: &Path, args: &[&str]) -> String {
        let out = run_git(repo, args, DEFAULT_TIMEOUT, &[]).unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn read_text(repo: &Path, name: &str) -> String {
        std::fs::read_to_string(repo.join(name)).unwrap()
    }

    #[test]
    fn smart_checkout_carries_staged_changes_to_the_other_branch() {
        let tmp = eight_line_repo();
        let repo = tmp.path();
        std::fs::write(repo.join("c.txt"), EIGHT_LINES.replace("8\n", "eight\n")).unwrap();
        test_repo::git(repo, &["add", "c.txt"]);
        let blocked =
            switch_keeping_changes(repo, false, || checkout(repo, "feature")).unwrap_err();
        assert!(
            blocked.contains("would be overwritten by checkout"),
            "{blocked}"
        );

        let outcome = switch_keeping_changes(repo, true, || checkout(repo, "feature")).unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert_eq!(git_line(repo, &["branch", "--show-current"]), "feature");
        assert_eq!(
            read_text(repo, "c.txt"),
            EIGHT_LINES
                .replacen("1\n", "one\n", 1)
                .replace("8\n", "eight\n")
        );
        assert_eq!(
            git_line(repo, &["diff", "--cached", "--name-only"]),
            "c.txt"
        );
        assert!(stash_list(repo).unwrap().is_empty());
    }

    #[test]
    fn smart_checkout_keeps_the_stash_when_restoring_conflicts() {
        let tmp = eight_line_repo();
        let repo = tmp.path();
        std::fs::write(repo.join("c.txt"), EIGHT_LINES.replacen("1\n", "uno\n", 1)).unwrap();
        let outcome = switch_keeping_changes(repo, true, || checkout(repo, "feature")).unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: true });
        assert_eq!(git_line(repo, &["branch", "--show-current"]), "feature");
        let stashes = stash_list(repo).unwrap();
        assert_eq!(stashes.len(), 1);
        assert!(stashes[0].message.contains(SMART_CHECKOUT_STASH));
    }

    #[test]
    fn smart_checkout_puts_changes_back_when_the_switch_fails() {
        let tmp = eight_line_repo();
        let repo = tmp.path();
        let edited = EIGHT_LINES.replace("8\n", "eight\n");
        std::fs::write(repo.join("c.txt"), &edited).unwrap();
        assert!(switch_keeping_changes(repo, true, || checkout(repo, "missing")).is_err());
        assert_eq!(git_line(repo, &["branch", "--show-current"]), "main");
        assert_eq!(read_text(repo, "c.txt"), edited);
        assert!(stash_list(repo).unwrap().is_empty());
        // Nothing to park: a clean tree switches directly.
        test_repo::git(repo, &["checkout", "--", "c.txt"]);
        let outcome = switch_keeping_changes(repo, true, || checkout(repo, "feature")).unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
    }

    #[test]
    fn merge_and_rebase_stash_local_changes_around_the_operation() {
        let tmp = eight_line_repo();
        let repo = tmp.path();
        test_repo::write_and_commit(repo, "m.txt", "m\n", "m1");
        let edited = EIGHT_LINES.replace("8\n", "eight\n");
        std::fs::write(repo.join("c.txt"), &edited).unwrap();
        let outcome = merge_branch(repo, "feature").unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert_eq!(read_text(repo, "c.txt"), edited.replacen("1\n", "one\n", 1));

        let tmp = eight_line_repo();
        let repo = tmp.path();
        test_repo::write_and_commit(repo, "m.txt", "m\n", "m1");
        test_repo::git(repo, &["switch", "feature"]);
        let edited = EIGHT_LINES
            .replacen("1\n", "one\n", 1)
            .replace("8\n", "eight\n");
        std::fs::write(repo.join("c.txt"), &edited).unwrap();
        let outcome = rebase_onto(repo, "main").unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert!(repo.join("m.txt").exists());
        assert_eq!(read_text(repo, "c.txt"), edited);
    }

    #[test]
    fn stash_apply_that_fails_without_conflicts_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        std::fs::write(repo.join("a.txt"), "stashed\n").unwrap();
        stash_push(repo, None, false, false).unwrap();
        // Local edits to the same file make git refuse the apply outright.
        std::fs::write(repo.join("a.txt"), "local\n").unwrap();
        let oid = stash_list(repo).unwrap()[0].oid.clone();
        assert!(stash_apply(repo, 0, &oid, false).is_err());
    }

    #[test]
    fn push_args_with_options_inserts_flags_before_operands() {
        let (_tmp, work) = clone_with_bare_remote();
        let env = test_repo::isolated_env();
        assert_eq!(
            push_args_with_options_env(&work, true, true, &env).unwrap(),
            [
                "push",
                "--force-with-lease",
                "--follow-tags",
                "--set-upstream",
                "origin",
                "main"
            ]
        );
        assert_eq!(
            push_args_with_options_env(&work, false, false, &env).unwrap(),
            push_args_with_env(&work, &env).unwrap()
        );
        test_repo::git(&work, &["push", "--set-upstream", "origin", "main"]);
        assert_eq!(
            push_args_with_options_env(&work, false, true, &env).unwrap(),
            ["push", "--follow-tags"]
        );
        // The produced arguments really are accepted by git, tags included.
        test_repo::git(&work, &["tag", "-a", "v1", "-m", "v1"]);
        test_repo::git(&work, &["push", "--force-with-lease", "--follow-tags"]);
        let remote_tags = run_git(
            &work,
            &["ls-remote", "--tags", "origin"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        assert!(
            String::from_utf8_lossy(&remote_tags.stdout).contains("refs/tags/v1"),
            "{}",
            String::from_utf8_lossy(&remote_tags.stdout)
        );
    }

    #[test]
    fn pull_args_with_mode_overrides_or_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        let root = tmp.path();
        assert_eq!(
            pull_args_with_mode(root, Some("rebase")).unwrap(),
            ["pull", "--rebase", "--autostash"]
        );
        assert_eq!(
            pull_args_with_mode(root, Some("merge")).unwrap(),
            ["pull", "--no-rebase", "--autostash"]
        );
        assert_eq!(
            pull_args_with_mode(root, None).unwrap(),
            pull_args(root).unwrap()
        );
        assert!(pull_args_with_mode(root, Some("--upload-pack=x")).is_err());
    }
}
