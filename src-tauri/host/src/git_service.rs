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

/// Same process policy as `run_git`, but output beyond `stdout_cap` bytes is
/// dropped and the process killed rather than failing with `git-output-limit`.
/// The flag reports that output was cut; the exit code of a cut process is -1.
fn run_git_capped_stdout(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(String, String)],
    stdout_cap: usize,
) -> Result<(GitOutput, bool), String> {
    let cmd = git_command(root, args, extra_env, false);
    crate::git_process::execute_capped_stdout(cmd, args, timeout, stdout_cap)
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
pub(crate) const ALLOW_PATHSPEC_MAGIC_ENV: &str = "YUZORA_ALLOW_GIT_PATHSPEC_MAGIC";

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
    let spec = pathspec.unwrap_or_default();
    // A path as the index spells it (decomposed) matches nothing once git
    // precomposes it; a path the filesystem spelled does, for the precomposed
    // entry and the untracked file status lists. One call cannot serve both,
    // so a mix of the two keeps git's own spelling. An ASCII path reads the same
    // either way. A folder never goes as stored, however it is spelled: asked
    // as stored, git lists what is in it by the spelling of the directory, not
    // the precomposed one an index entry has, and each such file shows up
    // untracked. (A folder `stored_names` finds is a tree of HEAD.)
    let stored = stored_names(root, &spec)?;
    let as_stored = !stored.is_empty()
        && spec.iter().all(|path| {
            (stored.contains(path.as_str()) || path.is_ascii()) && !root.join(path).is_dir()
        });
    let mut args: Vec<&str> = if as_stored {
        TRACKED_NAMES_AS_STORED.to_vec()
    } else {
        Vec::new()
    };
    // `all` is safety-critical for path-scoped rollback: the default `normal`
    // mode collapses an untracked tree to `scratch/`, which would hide dirty
    // editor descendants while `git clean -fd -- scratch/` deletes them all.
    args.extend([
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=all",
        "-z",
    ]);
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
    let mut parsed = crate::git_status::parse_porcelain_v2(&out.stdout)?;
    drop_untracked_twins(root, &mut parsed);
    let git_dir = match git_dir_from_dot_git(root) {
        Some(dir) => dir,
        None => resolve_metadata_dir(root, "--git-dir")?,
    };
    let in_progress = detect_in_progress(&git_dir);
    Ok(GitStatusDto {
        parsed,
        in_progress,
    })
}

/// Resolves the git-dir of a repository root straight from `root/.git` (a
/// directory, or a worktree/submodule `gitdir:` file), saving a `rev-parse`
/// process per status. Anything unusual (subdirectory roots, bare/invalid
/// `.git`, odd files) returns None so the caller asks Git itself.
fn git_dir_from_dot_git(root: &Path) -> Option<std::path::PathBuf> {
    let dot_git = root.join(".git");
    let metadata = std::fs::metadata(&dot_git).ok()?;
    let candidate = if metadata.is_dir() {
        dot_git
    } else if metadata.is_file() && metadata.len() <= 4096 {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let target = text.lines().next()?.strip_prefix("gitdir: ")?.trim_end();
        if target.is_empty() {
            return None;
        }
        root.join(target)
    } else {
        return None;
    };
    candidate.join("HEAD").is_file().then_some(())?;
    candidate.canonicalize().ok()
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

/// Whether the rows that changed are the requested paths' own. `as_stored` is
/// the part of `requested` that went to git as the index spells it
/// (`run_ok_paths`), the rest went in git's own respelling.
pub fn ensure_literal_path_scope(
    root: &Path,
    operation: &str,
    requested: &[String],
    as_stored: &HashSet<String>,
    before: &crate::git_status::ParsedStatus,
    after: &crate::git_status::ParsedStatus,
) -> Result<(), String> {
    if before.head_oid != after.head_oid {
        return Err(format!(
            "git {operation} changed HEAD outside the requested path set"
        ));
    }
    let allowed: HashSet<&str> = requested.iter().map(String::as_str).collect();
    let mut extra: Vec<String> = mutated_status_paths(before, after)
        .into_iter()
        .filter(|path| !allowed.contains(path.as_str()))
        .collect();
    if !extra.is_empty() {
        let respelled = respelled_requests(root, requested, as_stored, before, after);
        extra.retain(|path| !respelled.contains(path));
    }
    if extra.is_empty() {
        return Ok(());
    }
    Err(format!(
        "git {operation} changed paths outside the requested set: {}",
        extra.join(", ")
    ))
}

/// The names besides the requested ones whose rows are a request's own. Where
/// git precomposes (macOS) it takes a decomposed request to another name, and
/// the rows of that name belong to the request: unstaging a decomposed entry
/// leaves the file untracked under the name status lists, `add` of a decomposed
/// file stages the name git composes it to. That name is what git says it is
/// (`git_spellings`), not what NFC would give, so the tracked file a request for
/// its twin changed is outside the request however either is spelled.
///
/// A request that went to git as stored (`as_stored`) was not respelled: only
/// the untracked file its unstaged entry leaves is its own, a row that was
/// untracked or absent before and is untracked or absent after. A row that was
/// or is a tracked entry's is not, whichever way it changed (the index of a
/// repository made where names are not normalized holds two entries for one
/// file, and restoring one of them can flip the other from modified to clean as
/// well as from clean to modified).
fn respelled_requests(
    root: &Path,
    requested: &[String],
    as_stored: &HashSet<String>,
    before: &crate::git_status::ParsedStatus,
    after: &crate::git_status::ParsedStatus,
) -> HashSet<String> {
    let asked: Vec<&str> = requested
        .iter()
        .map(String::as_str)
        .filter(|path| git_respells(path))
        .collect();
    let spelled = git_spellings(root, &asked);
    let before = status_path_fingerprint(before);
    let after = status_path_fingerprint(after);
    let untracked_or_absent = |rows: &std::collections::BTreeMap<String, String>, name: &String| {
        rows.get(name).is_none_or(|sig| sig.as_str() == "?:")
    };
    spelled
        .into_iter()
        .filter(|(request, name)| {
            !as_stored.contains(*request)
                || (untracked_or_absent(&before, name) && untracked_or_absent(&after, name))
        })
        .map(|(_, name)| name)
        .collect()
}

pub fn mutate_then_assert_literal_scope(
    root: &Path,
    operation: &str,
    prefix: &[&str],
    requested: &[String],
) -> Result<(), String> {
    let before = status_of(root, None)?;
    let as_stored = run_ok_paths(root, prefix, requested)?;
    let after = status_of(root, None)?;
    ensure_literal_path_scope(
        root,
        operation,
        requested,
        &as_stored,
        &before.parsed,
        &after.parsed,
    )
}

pub fn stage(root: &Path, paths: &[String]) -> Result<(), String> {
    let requested: Vec<&str> = paths.iter().map(String::as_str).collect();
    refuse_tracked_twins(root, "add", &requested)?;
    reject_unresolved_conflicts(root, paths)?;
    mutate_then_assert_literal_scope(root, "add", &["add", "--"], paths)
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
    let prefix: &[&str] = if before.parsed.head_oid == "(initial)" {
        &["rm", "--cached", "-f", "--"]
    } else {
        &["restore", "--staged", "--"]
    };
    let as_stored = run_ok_paths(root, prefix, paths)?;
    let after = status_of(root, None)?;
    ensure_literal_path_scope(
        root,
        "unstage",
        paths,
        &as_stored,
        &before.parsed,
        &after.parsed,
    )
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
    let untracked_names: Vec<&str> = untracked.iter().map(String::as_str).collect();
    refuse_tracked_twins(root, "clean", &untracked_names)?;
    if !paths.is_empty() {
        mutate_then_assert_literal_scope(root, "restore", &["restore", "--"], paths)?;
    }
    if !untracked.is_empty() {
        mutate_then_assert_literal_scope(root, "clean", &["clean", "-f", "--"], untracked)?;
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
    let mut cleaned: Vec<&str> = Vec::new();

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
                cleaned.push(&target.path);
            }
            GitRollbackClassification::Untracked => {
                plans.push(GitRollbackPlan::Untracked {
                    path: target.path.clone(),
                });
                cleaned.push(&target.path);
            }
            GitRollbackClassification::Conflicted => unreachable!("rejected above"),
        }
    }
    if delete_untracked_or_added {
        refuse_tracked_twins(root, "clean", &cleaned)?;
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
                let prefix = ["restore", "--source=HEAD", "--staged", "--worktree", "--"];
                let as_stored = run_ok_paths(root, &prefix, &restore_paths).map_err(|error| {
                    rollback_failure(&path, "restore tracked path", &completed, error)
                })?;
                let after = status_of(root, None).map_err(|error| {
                    rollback_failure(&path, "verify restore scope", &completed, error)
                })?;
                ensure_literal_path_scope(
                    root,
                    "rollback",
                    &restore_paths,
                    &as_stored,
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
                let as_stored = run_ok_paths(
                    root,
                    &["rm", "--cached", "-f", "--"],
                    std::slice::from_ref(&path),
                )
                .map_err(|error| {
                    rollback_failure(&path, "unstage added path", &completed, error)
                })?;
                let after_unstage = status_of(root, None).map_err(|error| {
                    rollback_failure(&path, "verify unstage scope", &completed, error)
                })?;
                ensure_literal_path_scope(
                    root,
                    "rollback",
                    std::slice::from_ref(&path),
                    &as_stored,
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
                        root,
                        "rollback",
                        std::slice::from_ref(&path),
                        &HashSet::new(),
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
                        root,
                        "rollback",
                        std::slice::from_ref(&path),
                        &HashSet::new(),
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
const SMART_CHECKOUT_UNTRACKED_STASH: &str = "Yuzora smart checkout (untracked files)";

/// The stash message for one smart checkout: `base` plus a nonce, so two
/// operations on one repository (two windows) never claim each other's entry.
fn stash_label(base: &str) -> String {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    format!("{base} [{}]", &nonce[..12])
}
const BLOCKED_BY_UNTRACKED: &str = "untracked working tree files would be overwritten by checkout";

/// Runs a branch switch. With `smart` (JetBrains "Smart Checkout") local
/// changes are stashed for the switch and restored on the new HEAD; a restore
/// that conflicts keeps the stash and leaves the conflicts to the merge tool.
/// `target` is the commit the switch lands on, so untracked files it also
/// tracks can be carried over.
pub fn switch_keeping_changes(
    root: &Path,
    smart: bool,
    target: Option<&str>,
    switch: impl Fn() -> Result<(), String>,
) -> Result<GitOperationOutcome, String> {
    if !smart {
        return switch().map(|()| GitOperationOutcome { conflicts: false });
    }
    let magic = [(ALLOW_PATHSPEC_MAGIC_ENV.to_string(), "1".to_string())];
    let before = stash_top(root)?;
    let label = stash_label(SMART_CHECKOUT_STASH);
    let pushed = run_ok(
        root,
        &["stash", "push", "-m", label.as_str()],
        DEFAULT_TIMEOUT,
        &magic,
    )?;
    // git reports, rather than fails, when there was nothing to save.
    if String::from_utf8_lossy(&pushed.stdout).contains("No local changes to save") {
        return switch_over_untracked(root, target, &switch)
            .map(|()| GitOperationOutcome { conflicts: false });
    }
    // Another client may stash right after us: find our entry, never assume
    // the top. Unidentified, the changes stay safely stashed and HEAD stays.
    let parked = pushed_stash(root, before.as_deref(), &label)
        .ok()
        .flatten()
        .ok_or_else(|| {
            format!("git stash: your local changes are kept in the stash \"{label}\"")
        })?;
    let switched = switch_over_untracked(root, target, &switch);
    // A failed switch left HEAD where it was, so the changes go back as they were.
    let restored = restore_parked_changes(root, Some(&parked), &label);
    match (switched, restored) {
        (Ok(()), restored) => restored,
        (Err(error), Ok(_)) => Err(error),
        (Err(error), Err(restore_error)) => Err(format!("{error}\n{restore_error}")),
    }
}

/// Runs `switch`. When untracked files block it because `target` tracks the
/// same paths, parks just those files, switches, and writes the local copies
/// back over the target's versions: they read as edits to its files, whose
/// committed version stays recoverable. A switch that still fails puts them
/// back as they were.
fn switch_over_untracked(
    root: &Path,
    target: Option<&str>,
    switch: &impl Fn() -> Result<(), String>,
) -> Result<(), String> {
    let blocked = match switch() {
        Err(error) if error.contains(BLOCKED_BY_UNTRACKED) => error,
        other => return other,
    };
    let Some(target) = target else {
        return Err(blocked);
    };
    let paths =
        untracked_paths_tracked_by(root, target).map_err(|error| format!("{blocked}\n{error}"))?;
    if paths.is_empty() {
        return Err(blocked);
    }
    let pathspec = paths.join("\0");
    let from_stdin = ["--pathspec-from-file=-", "--pathspec-file-nul"];
    let label = stash_label(SMART_CHECKOUT_UNTRACKED_STASH);
    let kept = || format!("git stash: your untracked files are kept in the stash \"{label}\"");
    let parked = match park_untracked(root, &pathspec, &label) {
        Err(error) => return Err(format!("{blocked}\n{error}")),
        // The blocking files vanished before git could stash them, so nothing
        // is parked and nothing needs restoring: just try the switch again.
        Ok(None) => return switch(),
        Ok(Some(parked)) => parked,
    };
    let switched = switch();
    // Either the target now holds its own versions or HEAD never moved; both
    // ways `--worktree` writes the parked copies and leaves the index alone.
    let source = format!("{parked}^3");
    let mut restore = vec!["restore", "--source", source.as_str(), "--worktree"];
    restore.extend(from_stdin);
    let restored = run_git_with_stdin(root, &restore, DEFAULT_TIMEOUT, &[], pathspec.as_bytes())
        .and_then(|out| match out.code {
            0 => Ok(()),
            _ => Err(git_err("restore", &out.stderr)),
        })
        .map_err(|error| format!("{error}\n{}", kept()));
    // Someone else stashing meanwhile leaves both stashes alone.
    let restored = restored.and_then(|()| {
        let top = stash_top(root).ok().flatten();
        let dropped = top.as_deref() == Some(parked.as_str())
            && run_ok(root, &["stash", "drop", "--quiet"], DEFAULT_TIMEOUT, &[]).is_ok();
        dropped.then_some(()).ok_or_else(|| {
            format!(
                "git stash: your untracked files are restored and a copy stays in the stash \"{label}\""
            )
        })
    });
    match (switched, restored) {
        (Ok(()), restored) => restored,
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restore_error)) => Err(format!("{error}\n{restore_error}")),
    }
}

/// The entry a `stash push -m message` just created: the newest one above
/// `before` recording that message. Another client may stash right after the
/// push, so the top alone proves nothing. `None` when the push saved nothing.
fn pushed_stash(
    root: &Path,
    before: Option<&str>,
    message: &str,
) -> Result<Option<String>, String> {
    let listed = run_ok(
        root,
        &["stash", "list", "--format=%H%x1f%gs"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    let mut ours = None;
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        let Some((oid, subject)) = line.split_once('\x1f') else {
            continue;
        };
        if Some(oid) == before {
            break;
        }
        // `stash push -m` records "On <branch>: <message>"; branch names never
        // contain ':'.
        if subject.split_once(": ").map(|(_, rest)| rest) == Some(message) {
            if ours.is_some() {
                return Err(format!(
                    "git stash: more than one new \"{message}\" stash; all are kept"
                ));
            }
            ours = Some(oid.to_string());
        }
    }
    Ok(ours)
}

/// Stashes just `pathspec`, untracked files included, and returns the new
/// stash. `None` when git saved nothing: it still exits 0 ("No local changes
/// to save") and the top stash is then someone else's, never ours to restore
/// or drop.
fn park_untracked(root: &Path, pathspec: &str, label: &str) -> Result<Option<String>, String> {
    let before = stash_top(root)?;
    let push = [
        "stash",
        "push",
        "--include-untracked",
        "-m",
        label,
        "--pathspec-from-file=-",
        "--pathspec-file-nul",
    ];
    let out = run_git_with_stdin(root, &push, DEFAULT_TIMEOUT, &[], pathspec.as_bytes())?;
    if out.code != 0 {
        return Err(git_err("stash", &out.stderr));
    }
    pushed_stash(root, before.as_deref(), label).map_err(|error| {
        format!("{error}\ngit stash: your untracked files may be kept in the stash \"{label}\"")
    })
}

/// Untracked, non-ignored files whose path `target` tracks as a file: the ones
/// a switch to it refuses to overwrite.
fn untracked_paths_tracked_by(root: &Path, target: &str) -> Result<Vec<String>, String> {
    let oid = crate::git_oid::resolve_commit_oid(root, target)?;
    let listed = run_ok(
        root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
            "--full-name",
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    let untracked: Vec<&str> = listed
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter(|path| !path.is_empty() && !path.contains('\n'))
        .collect();
    if untracked.is_empty() {
        return Ok(Vec::new());
    }
    let queries: String = untracked
        .iter()
        .map(|path| format!("{}:{path}\n", oid.as_str()))
        .collect();
    let kinds = run_git_with_stdin(
        root,
        &["cat-file", "--batch-check=%(objecttype)"],
        DEFAULT_TIMEOUT,
        &[],
        queries.as_bytes(),
    )?;
    if kinds.code != 0 {
        return Err(git_err("cat-file", &kinds.stderr));
    }
    // One answer per query: the object type, or "<query> missing".
    Ok(untracked
        .into_iter()
        .zip(String::from_utf8_lossy(&kinds.stdout).lines())
        .filter(|(_, kind)| *kind == "blob")
        .map(|(path, _)| path.to_string())
        .collect())
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
    label: &str,
) -> Result<GitOperationOutcome, String> {
    let kept = || format!("git stash: your local changes are kept in the stash \"{label}\"");
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
    if pop && !outcome.conflicts {
        drop_popped_stash(root, &name, oid)?;
    }
    Ok(outcome)
}

/// Remove a stash whose changes were just applied by OID. It is only dropped while `name` still names `oid`;
/// otherwise, or when the drop fails, report it as applied but kept, never as a failed pop that a retry would
/// apply again.
fn drop_popped_stash(root: &Path, name: &str, oid: &str) -> Result<(), String> {
    if verify_stash_identity(root, name, oid).is_err() {
        return Err(format!(
            "git stash: applied {name} but it moved before it could be dropped, so it is kept"
        ));
    }
    run_ok(root, &["stash", "drop", name], DEFAULT_TIMEOUT, &[])
        .map(|_| ())
        .map_err(|error| {
            format!("git stash: applied {name} but could not drop it, so it is kept: {error}")
        })
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
    let mut stages = Vec::new();
    for spelling in name_spellings(path) {
        let mut args = spelling.to_vec();
        args.extend(["ls-files", "--stage", "-z", "--", path]);
        let out = run_git(root, &args, DEFAULT_TIMEOUT, &[])?;
        if out.code != 0 {
            return Err(git_err("ls-files", &out.stderr));
        }
        stages = out
            .stdout
            .split(|byte| *byte == 0)
            .filter_map(|record| {
                // "<mode> <oid> <stage>\t<path>"
                let head = record.split(|byte| *byte == b'\t').next()?;
                let stage = head.rsplit(|byte| *byte == b' ').next()?;
                (stage.len() == 1).then(|| stage[0].wrapping_sub(b'0'))
            })
            .collect();
        if !stages.is_empty() {
            break;
        }
    }
    Ok(stages)
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
        let only = std::slice::from_ref(path);
        if conflict_stages(root, path)?.contains(&stage) {
            run_ok_paths(root, &["checkout", flag, "--"], only)?;
            run_ok_paths(root, &["add", "--"], only)?;
        } else {
            run_ok_paths(root, &["rm", "--quiet", "--"], only)?;
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
    let mut shown = None;
    for spelling in name_spellings(spec) {
        shown = show_object_spelled(root, spec, spelling)?;
        if shown.is_some() {
            break;
        }
    }
    Ok(shown)
}

fn show_object_spelled(
    root: &Path,
    spec: &str,
    spelling: &[&str],
) -> Result<Option<GradedText>, String> {
    // `show` already distinguishes a missing object from other failures. A
    // separate existence probe doubles process startup on every diff side.
    let mut args = spelling.to_vec();
    args.extend(["show", "--end-of-options", spec]);
    let out = match run_git_with_stdout_limit(root, &args, DEFAULT_TIMEOUT, &[], BLOB_READ_LIMIT) {
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

pub enum UnifiedDiffTarget<'a> {
    Worktree {
        path: &'a str,
        untracked: bool,
    },
    Staged {
        path: &'a str,
        orig_path: Option<&'a str>,
    },
    /// Diff against the first parent (the empty tree for a root commit).
    /// `orig_path` is the old name of a renamed file, so `-M` can pair them.
    Commit {
        hash: &'a str,
        path: &'a str,
        orig_path: Option<&'a str>,
    },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedDiff {
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
}

const UNIFIED_DIFF_FLAGS: [&str; 6] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "-U3",
];

/// A tracked diff is asked about a name as `status_of` and the commit listing
/// report it: the spelling of the index or the tree. Where git precomposes
/// (`core.precomposeunicode`, macOS) it turns the literal pathspec into NFC,
/// which no longer matches an NFD index entry and leaves the diff empty, so
/// these diffs ask git not to. The untracked (`--no-index`) path keeps its own
/// spelling rules (`ensure_untracked`) and does not take this. Object reads
/// (`name_spellings`) and index writes (`run_ok_paths`) take it for the names
/// they know as stored.
pub(crate) const TRACKED_NAMES_AS_STORED: &[&str] = &["-c", "core.precomposeunicode=false"];

/// Whether git may respell `value` when it arrives as an argument: where it
/// precomposes (macOS) it composes a decomposed name (`git_spellings`). A name
/// that is NFC already reads the same either way.
pub(crate) fn git_respells(value: &str) -> bool {
    cfg!(target_os = "macos") && !unicode_normalization::is_nfc(value)
}

/// What git turns each of `paths` into as an argument, asked of git itself:
/// where it precomposes names (macOS) it composes a decomposed name pair by
/// pair, which is not NFC (it does not reorder marks, does not compose past a
/// mark that does not combine, and leaves a singleton such as U+212A KELVIN
/// SIGN or a CJK compatibility ideograph alone while it composes the rest of
/// the name). Any rule written down here would be a model of that; `check-attr`
/// prints the path it was given as git has it. A path git does not answer for,
/// or answers with another path, has no entry.
fn git_spellings<'a>(root: &Path, paths: &[&'a str]) -> std::collections::HashMap<&'a str, String> {
    if paths.is_empty() {
        return std::collections::HashMap::new();
    }
    let mut args = vec!["check-attr", "-z", "yuzora-spelling", "--"];
    args.extend(paths);
    match run_git(root, &args, DEFAULT_TIMEOUT, &[]) {
        Ok(out) if out.code == 0 => parse_git_spellings(paths, &out.stdout),
        _ => std::collections::HashMap::new(),
    }
}

/// The `check-attr -z` records (path, attribute, value, each NUL-ended), one
/// per path in the order asked. The path git prints must be the one asked for
/// in another spelling (canonically equivalent): a request git normalized to
/// another path (`a/../b`) is not taken as a spelling of it.
fn parse_git_spellings<'a>(
    paths: &[&'a str],
    stdout: &[u8],
) -> std::collections::HashMap<&'a str, String> {
    use unicode_normalization::UnicodeNormalization;

    stdout
        .split(|byte| *byte == 0)
        .step_by(3)
        .zip(paths)
        .filter_map(|(spelled, path)| {
            let spelled = std::str::from_utf8(spelled).ok()?;
            spelled
                .nfd()
                .eq(path.nfd())
                .then(|| (*path, spelled.to_string()))
        })
        .collect()
}

/// The spellings in which a read asks git about the path or `<rev>:<path>` in
/// `value`, in order. A name git listed is the bytes of the index or of a tree,
/// so it is asked as stored first; a name the filesystem spelled (a decomposed
/// Finder name whose index entry is precomposed) is not found that way and is
/// then asked the way git respells it, as it always was. Only a name git would
/// respell has the second try; any other name reads the same either way.
pub(crate) fn name_spellings(value: &str) -> Vec<&'static [&'static str]> {
    if git_respells(value) {
        vec![TRACKED_NAMES_AS_STORED, &[]]
    } else {
        vec![&[]]
    }
}

/// Of `paths`, the ones git would respell that are an entry of the index or of
/// HEAD under exactly that spelling, which is how `status` and a commit listing
/// report them. A path no entry has is a name the filesystem gave (an untracked
/// file Finder stored decomposed, which `status` lists precomposed) and is best
/// left to git's own respelling, or the pathspec stops matching what it reads
/// from the directory. A failing query counts as no entry: the paths then go
/// to git as they always did (an unborn HEAD fails `ls-tree` this way).
fn stored_names<'a>(root: &Path, paths: &'a [String]) -> Result<HashSet<&'a str>, String> {
    let candidates: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .filter(|path| git_respells(path))
        .collect();
    let mut stored = HashSet::new();
    if candidates.is_empty() {
        return Ok(stored);
    }
    for query in [
        &["ls-files", "-z", "--"][..],
        &["ls-tree", "-z", "--name-only", "--full-name", "HEAD", "--"][..],
    ] {
        let mut args = TRACKED_NAMES_AS_STORED.to_vec();
        args.extend(query);
        args.extend(&candidates);
        let out = run_git(root, &args, DEFAULT_TIMEOUT, &[])?;
        if out.code != 0 {
            continue;
        }
        for listed in out.stdout.split(|byte| *byte == 0) {
            if let Some(path) = candidates.iter().find(|path| path.as_bytes() == listed) {
                stored.insert(*path);
            }
        }
    }
    Ok(stored)
}

/// `git <prefix> <paths>` with every path reaching git in the spelling it was
/// listed: the entries `stored_names` finds in one call that does not
/// precompose, all other paths in one that does. Git precomposing the first
/// kind would stage a second (NFC) entry beside the decomposed one, or match
/// nothing; not precomposing the second kind would stop an untracked Finder
/// name, listed precomposed, from matching the decomposed directory entry.
/// Returns the paths that went as stored.
fn run_ok_paths(root: &Path, prefix: &[&str], paths: &[String]) -> Result<HashSet<String>, String> {
    let stored = stored_names(root, paths)?;
    let (as_stored, rest): (Vec<&str>, Vec<&str>) = paths
        .iter()
        .map(String::as_str)
        .partition(|path| stored.contains(path));
    let mut calls: Vec<(&[&str], Vec<&str>)> = Vec::new();
    if !as_stored.is_empty() {
        calls.push((TRACKED_NAMES_AS_STORED, as_stored));
    }
    // With no path at all git still gets its say, as it did before.
    if !rest.is_empty() || calls.is_empty() {
        calls.push((&[], rest));
    }
    for (spelling, group) in calls {
        let mut args = spelling.to_vec();
        args.extend(prefix);
        args.extend(group);
        let out = run_git(root, &args, DEFAULT_TIMEOUT, &[])?;
        if out.code != 0 {
            return Err(git_err(prefix.first().unwrap_or(&""), &out.stderr));
        }
    }
    Ok(stored.into_iter().map(String::from).collect())
}

/// Most bytes of an index listing `tracked_twins` reads; the default output
/// limit would fail it for a repository of a few hundred thousand files.
#[cfg(target_os = "macos")]
const INDEX_LISTING_LIMIT: usize = 256 * 1024 * 1024;

/// Whether both paths are one file (device and inode), links not followed.
#[cfg(target_os = "macos")]
fn is_same_file(first: &Path, second: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (
        std::fs::symlink_metadata(first),
        std::fs::symlink_metadata(second),
    ) {
        (Ok(first), Ok(second)) => first.dev() == second.dev() && first.ino() == second.ino(),
        _ => false,
    }
}

/// Where git precomposes names (macOS), a tracked file whose directory entry
/// is spelled another way than its index entry (an entry stored decomposed, or
/// one stored precomposed whose file a save rewrote in a canonical form git
/// leaves as it is) is listed twice: as its index entry, and as an "untracked"
/// file, which is the same directory entry read under the other spelling. The
/// volume knows the two names as one file, so the second is no file of its own:
/// cleaning, adding or discarding it acts on the tracked one.
///
/// Of `paths`, the ones that are such a twin, each with the entry it is the
/// file of: a name of the index (as stored) that differs from the path, is
/// canonically equivalent to it as a whole path (so a decomposed folder name
/// counts) and is the same file on disk. Only a name that is not both NFC and
/// NFD can be spelled another way (an NFC name has a decomposed spelling, an NFD
/// one a composed one, git composes some names to neither: see `git_spellings`),
/// so any other (ASCII, plain CJK) answers without a process; otherwise one
/// as-stored listing of the index serves every path, whichever form the entry
/// has. The entry may be ASCII (a path spelled with U+212A KELVIN SIGN is the
/// file of `K.txt`).
///
/// A path that is an entry of the index itself (`own_entry_is_no_twin`) can be
/// left out: the index of a repository made where names are not normalized
/// holds two entries for one file, and `git add` of either updates that entry
/// and never makes a second one. `git clean` of it still deletes the file the
/// other entry names, so the callers that clean keep it.
///
/// Not found: a path no character of which decomposes (`K.txt`, a unified
/// ideograph) whose entry is spelled with the singleton (U+212A, U+F900). Only
/// a listing of the index for every such name could tell, and that would cost
/// a process on each status of a repository with a new ASCII file.
#[cfg(target_os = "macos")]
fn tracked_twins<'a>(
    root: &Path,
    paths: &[&'a str],
    own_entry_is_no_twin: bool,
) -> Result<Vec<(&'a str, String)>, String> {
    use std::collections::HashMap;
    use unicode_normalization::UnicodeNormalization;

    let mut candidates: HashMap<String, Vec<&'a str>> = HashMap::new();
    for path in paths.iter().copied().filter(|path| {
        !(unicode_normalization::is_nfc(path) && unicode_normalization::is_nfd(path))
    }) {
        candidates
            .entry(path.nfd().collect())
            .or_default()
            .push(path);
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = TRACKED_NAMES_AS_STORED.to_vec();
    args.extend(["ls-files", "-z"]);
    let listing =
        run_git_with_stdout_limit(root, &args, DEFAULT_TIMEOUT, &[], INDEX_LISTING_LIMIT)?;
    if listing.code != 0 {
        return Err(git_err("ls-files", &listing.stderr));
    }
    let mut twins = Vec::new();
    let mut own_entries: HashSet<&str> = HashSet::new();
    for entry in listing.stdout.split(|byte| *byte == 0) {
        let Ok(entry) = std::str::from_utf8(entry) else {
            continue;
        };
        // An ASCII entry is its own decomposed form, and a name spelled with
        // a singleton character (U+212A for K) decomposes to it.
        let spelled = if entry.is_ascii() {
            candidates.get(entry)
        } else {
            candidates.get(&entry.nfd().collect::<String>())
        };
        let Some(spelled) = spelled else {
            continue;
        };
        for path in spelled {
            if *path == entry {
                own_entries.insert(*path);
            } else if is_same_file(&root.join(path), &root.join(entry)) {
                twins.push((*path, entry.to_string()));
            }
        }
    }
    if own_entry_is_no_twin {
        twins.retain(|(path, _)| !own_entries.contains(path));
    }
    Ok(twins)
}

#[cfg(not(target_os = "macos"))]
fn tracked_twins<'a>(
    _root: &Path,
    _paths: &[&'a str],
    _own_entry_is_no_twin: bool,
) -> Result<Vec<(&'a str, String)>, String> {
    Ok(Vec::new())
}

/// Takes the twin rows out of the untracked list: the file is listed once, as
/// its index entry. A new file Finder stored decomposed has no entry and
/// stays. When the index cannot be listed the rows stay, and every write
/// refuses them anyway (`refuse_tracked_twins`).
fn drop_untracked_twins(root: &Path, parsed: &mut crate::git_status::ParsedStatus) {
    let untracked: Vec<&str> = parsed.untracked.iter().map(String::as_str).collect();
    let Ok(twins) = tracked_twins(root, &untracked, false) else {
        return;
    };
    let twins: HashSet<String> = twins
        .into_iter()
        .map(|(path, _)| path.to_string())
        .collect();
    parsed.untracked.retain(|path| !twins.contains(path));
}

/// Refuses, before anything changes, a request for a path that is a twin (see
/// `tracked_twins`): `git clean` would delete the tracked file and `git add`
/// would give it a second entry. A request git itself takes to the entry (a
/// decomposed spelling of a precomposed entry, a Finder file) is no twin, which
/// only git can say (`git_spellings`); when it cannot, the request is refused.
/// `add` of a path that is an entry itself is never refused (it updates that
/// entry, see `tracked_twins`); `clean` of it is.
fn refuse_tracked_twins(root: &Path, operation: &str, paths: &[&str]) -> Result<(), String> {
    let twins = tracked_twins(root, paths, operation == "add")?;
    if twins.is_empty() {
        return Ok(());
    }
    let asked: Vec<&str> = twins.iter().map(|(path, _)| *path).collect();
    let spelled = git_spellings(root, &asked);
    for (path, tracked) in twins {
        if spelled.get(path) == Some(&tracked) {
            continue;
        }
        return Err(format!(
            "git {operation} refused: {path} is the tracked file {tracked}"
        ));
    }
    Ok(())
}

/// Fixed answers, free of paths, that the caller maps to its own codes.
pub const DIFF_NOT_UNTRACKED: &str = "git-diff-not-untracked";
pub const DIFF_GIT_PATH: &str = "git-diff-git-path";

/// Code points an HFS+ volume ignores when it compares names (git's `next_hfs_char`).
fn is_hfs_ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}'
    )
}

/// `.git` under any spelling git itself refuses as a path component (mirrors
/// `is_hfs_dotgit` and `is_ntfs_dotgit`): on a volume that folds names it is
/// the same directory. Case-insensitive, with HFS+-ignorable code points
/// dropped, or the NTFS forms: `.git` or the short name `git~1` followed only
/// by dots and spaces, or by `:` (an alternate data stream).
fn is_dotgit_lookalike(part: &str) -> bool {
    let hfs: String = part.chars().filter(|c| !is_hfs_ignorable(*c)).collect();
    if hfs.eq_ignore_ascii_case(".git") {
        return true;
    }
    let bytes = part.as_bytes();
    let rest = if bytes.len() >= 4 && bytes[..4].eq_ignore_ascii_case(b".git") {
        &bytes[4..]
    } else if bytes.len() >= 5 && bytes[..5].eq_ignore_ascii_case(b"git~1") {
        &bytes[5..]
    } else {
        return false;
    };
    for byte in rest {
        match byte {
            b' ' | b'.' => {}
            b':' => return true,
            _ => return false,
        }
    }
    true
}

/// A path inside `.git`, however spelled (see `is_dotgit_lookalike`; either
/// separator), is never a file of the work tree.
fn reject_git_dir(value: &str) -> Result<(), String> {
    if value.split(['/', '\\']).any(is_dotgit_lookalike) {
        return Err(DIFF_GIT_PATH.into());
    }
    Ok(())
}

/// Whether git reports this repository's names precomposed (NFC) although the
/// volume stores them decomposed: `core.precomposeunicode`, which `git init`
/// turns on for macOS volumes (APFS included: Foundation creates NFD names).
/// Only macOS does that. Unset or unreadable counts as off, the strict side.
fn git_precomposes_names(root: &Path, extra_env: &[(String, String)]) -> bool {
    cfg!(target_os = "macos")
        && run_git(
            root,
            &["config", "--type=bool", "--get", "core.precomposeunicode"],
            DEFAULT_TIMEOUT,
            extra_env,
        )
        .is_ok_and(|out| out.code == 0 && out.stdout.trim_ascii() == b"true")
}

/// Whether the directory entry `name` may be the component `part`: byte for
/// byte, or, when git precomposes, canonically equivalent to it (the same NFD).
/// The first is the name as git reports it when it leaves the name alone
/// (U+2126, CJK compatibility ideographs: NFC would change those); the second
/// covers a name it precomposes, including one that mixes a compatibility
/// character it keeps (U+F900) with a decomposed sequence it composes, which
/// is neither the entry itself nor its NFC. Which spelling git really prints
/// is its own listing's call (`ensure_untracked`), so a request is not turned
/// away because NFC changes it.
fn entry_is_spelled_as(name: &std::ffi::OsStr, part: &str, precomposed: bool) -> bool {
    use unicode_normalization::UnicodeNormalization;
    name == std::ffi::OsStr::new(part)
        || (precomposed && name.to_str().is_some_and(|name| name.nfd().eq(part.nfd())))
}

/// The entry for the component `part`: the one named exactly like it, and only
/// when there is none the first `entry_is_spelled_as` accepts. A case-sensitive
/// HFS+ volume holds two names that differ only in normalisation side by side,
/// and each must reach its own file.
fn pick_spelled_entry<T>(
    entries: Vec<T>,
    name_of: impl Fn(&T) -> std::ffi::OsString,
    part: &str,
    precomposed: bool,
) -> Option<T> {
    let exact = entries
        .iter()
        .position(|entry| name_of(entry) == std::ffi::OsStr::new(part));
    let index = exact.or_else(|| {
        entries
            .iter()
            .position(|entry| entry_is_spelled_as(&name_of(entry), part, precomposed))
    })?;
    entries.into_iter().nth(index)
}

/// Git matches ignore rules and its own `.git` skip against the spelling of the
/// request, while a case- or normalisation-insensitive filesystem opens the same
/// file under another spelling. So every component must be a name `read_dir`
/// lists in its parent (see `entry_is_spelled_as`), and each directory on the
/// way a real directory (a link there may lead anywhere); only the last may be
/// a link, which `--no-index` diffs as its link text.
///
/// A volume may store another normalisation than git reports (macOS stores the
/// NFD that Foundation writes, git prints NFC). Where git precomposes
/// (`git_precomposes_names`) a name canonically equivalent to a listed one is
/// accepted too, an exact match first (`pick_spelled_entry`); elsewhere such a
/// name never matches, which fails closed.
///
/// Returns the path with every component as the volume stores it. That is the
/// spelling to ask git about: git names the folders of what it lists the way it
/// was asked, so only a path it was given as stored makes its answer the
/// spelling it reports.
fn ensure_spelled_as_on_disk(root: &Path, path: &str) -> Result<String, String> {
    let not_untracked = || String::from(DIFF_NOT_UNTRACKED);
    let precomposed = git_precomposes_names(root, &[]);
    let mut dir = root.to_path_buf();
    let mut stored = Vec::new();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        if matches!(part, "" | "." | "..") {
            return Err(not_untracked());
        }
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|_| not_untracked())?
            .filter_map(Result::ok)
            .collect();
        let entry = pick_spelled_entry(entries, |entry| entry.file_name(), part, precomposed)
            .ok_or_else(not_untracked)?;
        stored.push(
            entry
                .file_name()
                .into_string()
                .map_err(|_| not_untracked())?,
        );
        if parts.peek().is_some() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                return Err(not_untracked());
            }
            dir = entry.path();
        }
    }
    Ok(stored.join("/"))
}

/// `--no-index` reads any file under the root, ignored ones and `.git` included,
/// so only a path Git itself lists as untracked and not ignored may use it, and
/// only under the spelling the filesystem stores and git reports.
fn ensure_untracked(root: &Path, path: &str) -> Result<(), String> {
    let stored = ensure_spelled_as_on_disk(root, path)?;
    let out = run_git(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            &stored,
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        return Err(git_err("ls-files", &out.stderr));
    }
    let listed: Vec<&[u8]> = out
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect();
    if listed != [path.as_bytes()] {
        return Err(DIFF_NOT_UNTRACKED.into());
    }
    Ok(())
}

/// Unified diff (-U3) of one path, never longer than `max_bytes`: a larger diff
/// is cut after its last complete line (`truncated`) instead of failing.
pub fn unified_diff(
    root: &Path,
    target: UnifiedDiffTarget<'_>,
    max_bytes: usize,
) -> Result<UnifiedDiff, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("git could not resolve repository root: {error}"))?;
    let check = |value: &str| {
        reject_git_dir(value)?;
        validate_diff_relative_path(root, &canonical_root, value)
    };
    let mut args = vec!["-c", "core.quotePath=false"];
    let oid;
    let mut env = Vec::new();
    let mut no_index = false;
    let sub = match target {
        UnifiedDiffTarget::Worktree { path, untracked } => {
            check(path)?;
            no_index = untracked;
            if no_index {
                ensure_untracked(root, path)?;
            } else {
                args.extend(TRACKED_NAMES_AS_STORED);
            }
            args.push("diff");
            if no_index {
                args.push("--no-index");
            }
            args.extend(UNIFIED_DIFF_FLAGS);
            args.push("--");
            if no_index {
                args.push("/dev/null");
            }
            args.push(path);
            "diff"
        }
        UnifiedDiffTarget::Staged { path, orig_path } => {
            check(path)?;
            args.extend(TRACKED_NAMES_AS_STORED);
            args.extend(["diff", "--cached", "-M"]);
            args.extend(UNIFIED_DIFF_FLAGS);
            args.extend(["--", path]);
            if let Some(value) = orig_path {
                check(value)?;
                args.push(value);
            }
            "diff"
        }
        UnifiedDiffTarget::Commit {
            hash,
            path,
            orig_path,
        } => {
            check(path)?;
            if let Some(value) = orig_path {
                check(value)?;
            }
            oid = crate::git_oid::resolve_commit_oid(root, hash)?;
            // The commit was resolved ignoring `refs/replace/*`; showing it must too.
            env.push(("GIT_NO_REPLACE_OBJECTS".to_string(), "1".to_string()));
            args.extend(TRACKED_NAMES_AS_STORED);
            args.extend(["show", "--format=", "--first-parent", "-M"]);
            args.extend(UNIFIED_DIFF_FLAGS);
            args.extend([oid.as_str(), "--", path]);
            args.extend(orig_path);
            "show"
        }
    };
    let (out, cut) = run_git_capped_stdout(root, &args, DEFAULT_TIMEOUT, &env, max_bytes)?;
    // `git diff --no-index` exits 1 when the files differ.
    if !cut && out.code != 0 && !(no_index && out.code == 1) {
        return Err(git_err(sub, &out.stderr));
    }
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    // Lossy decoding can turn invalid bytes into longer replacement characters.
    let truncated = cut || text.len() > max_bytes;
    if truncated {
        let end = text.len().min(max_bytes);
        let keep = text.as_bytes()[..end]
            .iter()
            .rposition(|byte| *byte == b'\n');
        text.truncate(keep.map_or(0, |index| index + 1));
    }
    let binary = text
        .lines()
        .any(|line| line.starts_with("Binary files ") || line == "GIT binary patch");
    Ok(UnifiedDiff {
        text,
        truncated,
        binary,
    })
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
    fn stash_pop_keeps_an_applied_stash_that_moved_before_the_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        test_repo::init(repo);
        test_repo::write_and_commit(repo, "a.txt", "1\n", "c1");
        std::fs::write(repo.join("a.txt"), "first\n").unwrap();
        stash_push(repo, Some("first"), false, false).unwrap();
        let popped = stash_list(repo).unwrap()[0].oid.clone();
        // Another client pushes after the apply: stash@{0} now names a different stash.
        std::fs::write(repo.join("a.txt"), "second\n").unwrap();
        stash_push(repo, Some("second"), false, false).unwrap();
        let before = stash_list(repo).unwrap();
        let error = drop_popped_stash(repo, "stash@{0}", &popped).unwrap_err();
        assert!(error.contains("applied stash@{0} but it moved"), "{error}");
        assert_eq!(stash_list(repo).unwrap(), before);
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
        let blocked = switch_keeping_changes(repo, false, Some("refs/heads/feature"), || {
            checkout(repo, "feature")
        })
        .unwrap_err();
        assert!(
            blocked.contains("would be overwritten by checkout"),
            "{blocked}"
        );

        let outcome = switch_keeping_changes(repo, true, Some("refs/heads/feature"), || {
            checkout(repo, "feature")
        })
        .unwrap();
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
        let outcome = switch_keeping_changes(repo, true, Some("refs/heads/feature"), || {
            checkout(repo, "feature")
        })
        .unwrap();
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
        assert!(
            switch_keeping_changes(repo, true, Some("refs/heads/missing"), || checkout(
                repo, "missing"
            ))
            .is_err()
        );
        assert_eq!(git_line(repo, &["branch", "--show-current"]), "main");
        assert_eq!(read_text(repo, "c.txt"), edited);
        assert!(stash_list(repo).unwrap().is_empty());
        // Nothing to park: a clean tree switches directly.
        test_repo::git(repo, &["checkout", "--", "c.txt"]);
        let outcome = switch_keeping_changes(repo, true, Some("refs/heads/feature"), || {
            checkout(repo, "feature")
        })
        .unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
    }

    /// eight_line_repo plus feature-only `shared.txt` and `extra` files, with
    /// local untracked `shared.txt` and `keep.txt` on main.
    fn shared_file_repo(extra: &[&str]) -> tempfile::TempDir {
        let tmp = eight_line_repo();
        let repo = tmp.path();
        test_repo::git(repo, &["switch", "feature"]);
        test_repo::write_and_commit(repo, "shared.txt", "feature\n", "shared");
        for path in extra {
            std::fs::create_dir_all(repo.join(path).parent().unwrap()).unwrap();
            test_repo::write_and_commit(repo, path, "x\n", "extra");
        }
        test_repo::git(repo, &["switch", "main"]);
        std::fs::write(repo.join("shared.txt"), "local\n").unwrap();
        std::fs::write(repo.join("keep.txt"), "untouched\n").unwrap();
        tmp
    }

    fn assert_local_copy_over_feature(repo: &Path) {
        assert_eq!(git_line(repo, &["branch", "--show-current"]), "feature");
        assert_eq!(read_text(repo, "shared.txt"), "local\n");
        assert_eq!(git_line(repo, &["show", "HEAD:shared.txt"]), "feature");
        assert!(git_line(repo, &["diff", "--name-only"])
            .lines()
            .any(|path| path == "shared.txt"));
        assert!(!git_line(repo, &["diff", "--cached", "--name-only"]).contains("shared.txt"));
        assert_eq!(read_text(repo, "keep.txt"), "untouched\n");
        assert_eq!(
            git_line(repo, &["ls-files", "--others", "--exclude-standard"]),
            "keep.txt"
        );
        assert!(stash_list(repo).unwrap().is_empty());
    }

    #[test]
    fn smart_checkout_writes_blocking_untracked_files_over_the_target() {
        let tmp = shared_file_repo(&[]);
        let repo = tmp.path();
        let blocked = switch_keeping_changes(repo, false, Some("refs/heads/feature"), || {
            checkout(repo, "feature")
        })
        .unwrap_err();
        assert!(blocked.contains(BLOCKED_BY_UNTRACKED), "{blocked}");

        let outcome = switch_keeping_changes(repo, true, Some("refs/heads/feature"), || {
            checkout(repo, "feature")
        })
        .unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert_local_copy_over_feature(repo);
    }

    #[test]
    fn smart_checkout_carries_blocking_untracked_files_with_tracked_changes() {
        let tmp = shared_file_repo(&[]);
        let repo = tmp.path();
        std::fs::write(repo.join("c.txt"), EIGHT_LINES.replace("8\n", "eight\n")).unwrap();
        let outcome = switch_keeping_changes(repo, true, Some("refs/heads/feature"), || {
            checkout(repo, "feature")
        })
        .unwrap();
        assert_eq!(outcome, GitOperationOutcome { conflicts: false });
        assert_local_copy_over_feature(repo);
        assert_eq!(
            read_text(repo, "c.txt"),
            EIGHT_LINES
                .replacen("1\n", "one\n", 1)
                .replace("8\n", "eight\n")
        );
    }

    #[test]
    fn smart_checkout_puts_untracked_files_back_when_the_switch_still_fails() {
        let tmp = shared_file_repo(&["dir/inner.txt"]);
        let repo = tmp.path();
        // A file where feature needs a directory still blocks after parking.
        std::fs::write(repo.join("dir"), "blocker\n").unwrap();
        std::fs::write(repo.join("c.txt"), EIGHT_LINES.replace("8\n", "eight\n")).unwrap();

        let error = switch_keeping_changes(repo, true, Some("refs/heads/feature"), || {
            checkout(repo, "feature")
        })
        .unwrap_err();
        assert!(error.contains(BLOCKED_BY_UNTRACKED), "{error}");
        assert_eq!(git_line(repo, &["branch", "--show-current"]), "main");
        assert_eq!(read_text(repo, "shared.txt"), "local\n");
        assert_eq!(read_text(repo, "dir"), "blocker\n");
        assert_eq!(
            read_text(repo, "c.txt"),
            EIGHT_LINES.replace("8\n", "eight\n")
        );
        assert_eq!(
            git_line(repo, &["ls-files", "--others", "--exclude-standard"]),
            "dir\nkeep.txt\nshared.txt"
        );
        assert!(stash_list(repo).unwrap().is_empty());
    }

    #[test]
    fn parking_vanished_untracked_files_never_claims_an_existing_stash() {
        let tmp = shared_file_repo(&[]);
        let repo = tmp.path();
        test_repo::git(
            repo,
            &["stash", "push", "--include-untracked", "-m", "mine"],
        );
        let mine = stash_top(repo).unwrap().unwrap();
        // git exits 0 with "No local changes to save" for a path that is gone.
        assert_eq!(park_untracked(repo, "gone.txt", "mine [1]").unwrap(), None);
        assert_eq!(stash_top(repo).unwrap().as_deref(), Some(mine.as_str()));
        std::fs::write(repo.join("shared.txt"), "local\n").unwrap();
        let parked = park_untracked(repo, "shared.txt", "mine [2]")
            .unwrap()
            .unwrap();
        assert_ne!(parked, mine);
        assert!(!repo.join("shared.txt").exists());
        assert_eq!(stash_list(repo).unwrap().len(), 2);
    }

    #[test]
    fn pushed_stash_is_found_under_a_foreign_stash_pushed_right_after() {
        let tmp = shared_file_repo(&[]);
        let repo = tmp.path();
        let before = stash_top(repo).unwrap();
        test_repo::git(
            repo,
            &[
                "stash",
                "push",
                "--include-untracked",
                "-m",
                SMART_CHECKOUT_UNTRACKED_STASH,
                "--",
                "shared.txt",
            ],
        );
        let ours = git_line(repo, &["rev-parse", "refs/stash"]);
        std::fs::write(repo.join("c.txt"), "foreign\n").unwrap();
        test_repo::git(repo, &["stash", "push", "-m", "someone else"]);
        assert_ne!(stash_top(repo).unwrap().as_deref(), Some(ours.as_str()));
        assert_eq!(
            pushed_stash(repo, before.as_deref(), SMART_CHECKOUT_UNTRACKED_STASH).unwrap(),
            Some(ours.clone())
        );
        // Nothing of ours above `before`: none, whatever sits on top.
        let top = stash_top(repo).unwrap();
        assert_eq!(
            pushed_stash(repo, top.as_deref(), SMART_CHECKOUT_UNTRACKED_STASH).unwrap(),
            None
        );
        // Two of ours since `before`: refuse to guess.
        std::fs::write(repo.join("other.txt"), "x\n").unwrap();
        test_repo::git(
            repo,
            &[
                "stash",
                "push",
                "--include-untracked",
                "-m",
                SMART_CHECKOUT_UNTRACKED_STASH,
                "--",
                "other.txt",
            ],
        );
        assert!(pushed_stash(repo, before.as_deref(), SMART_CHECKOUT_UNTRACKED_STASH).is_err());
    }

    #[test]
    fn a_concurrent_smart_checkout_never_claims_another_operations_stash() {
        let tmp = shared_file_repo(&[]);
        let repo = tmp.path();
        let ours = stash_label(SMART_CHECKOUT_UNTRACKED_STASH);
        let theirs = stash_label(SMART_CHECKOUT_UNTRACKED_STASH);
        assert_ne!(ours, theirs);
        assert!(ours.starts_with(SMART_CHECKOUT_UNTRACKED_STASH));
        let before = stash_top(repo).unwrap();
        // The other window parks the same kind of file right after our `before`.
        test_repo::git(
            repo,
            &[
                "stash",
                "push",
                "--include-untracked",
                "-m",
                theirs.as_str(),
                "--",
                "shared.txt",
            ],
        );
        // Our own push saved nothing (its blockers vanished): no entry is ours.
        assert_eq!(park_untracked(repo, "gone.txt", &ours).unwrap(), None);
        assert_eq!(pushed_stash(repo, before.as_deref(), &ours).unwrap(), None);
        assert_eq!(stash_list(repo).unwrap().len(), 1);
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

    const DIFF_CAP: usize = 1 << 20;

    fn worktree(path: &str) -> UnifiedDiffTarget<'_> {
        UnifiedDiffTarget::Worktree {
            path,
            untracked: false,
        }
    }

    fn head(root: &Path) -> String {
        let out = run_git(root, &["rev-parse", "HEAD"], DEFAULT_TIMEOUT, &[]).unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn unified_diff_reads_worktree_changes_and_untracked_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::write_and_commit(root, "a.txt", "one\ntwo\nthree\n", "c1");
        test_repo::write_and_commit(root, "文件.txt", "x\n", "c2");

        let clean = unified_diff(root, worktree("a.txt"), DIFF_CAP).unwrap();
        assert_eq!(clean.text, "");
        assert!(!clean.truncated && !clean.binary);

        std::fs::write(root.join("a.txt"), "one\n2\nthree\n").unwrap();
        let changed = unified_diff(root, worktree("a.txt"), DIFF_CAP).unwrap();
        assert!(changed.text.starts_with("diff --git a/a.txt b/a.txt\n"));
        assert!(changed.text.contains("@@ -1,3 +1,3 @@"));
        assert!(changed.text.contains("\n-two\n+2\n three\n"));
        assert!(!changed.truncated && !changed.binary);

        // `core.quotePath=false` keeps non-ASCII names readable.
        std::fs::write(root.join("文件.txt"), "y\n").unwrap();
        let unicode = unified_diff(root, worktree("文件.txt"), DIFF_CAP).unwrap();
        assert!(unicode.text.contains("diff --git a/文件.txt b/文件.txt"));

        std::fs::write(root.join("new.txt"), "hello\n").unwrap();
        let untracked = unified_diff(
            root,
            UnifiedDiffTarget::Worktree {
                path: "new.txt",
                untracked: true,
            },
            DIFF_CAP,
        )
        .unwrap();
        assert!(untracked.text.contains("new file mode"));
        assert!(untracked.text.contains("--- /dev/null\n+++ b/new.txt\n"));
        assert!(untracked.text.contains("\n+hello\n"));
        assert!(!untracked.truncated && !untracked.binary);
    }

    #[test]
    fn unified_diff_staged_pairs_a_rename_through_orig_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        let body: String = (0..20).map(|n| format!("line {n}\n")).collect();
        test_repo::write_and_commit(root, "old.txt", &body, "c1");
        test_repo::git(root, &["mv", "old.txt", "new.txt"]);
        std::fs::write(
            root.join("new.txt"),
            body.replace("line 3\n", "line three\n"),
        )
        .unwrap();
        test_repo::git(root, &["add", "new.txt"]);

        let renamed = unified_diff(
            root,
            UnifiedDiffTarget::Staged {
                path: "new.txt",
                orig_path: Some("old.txt"),
            },
            DIFF_CAP,
        )
        .unwrap();
        assert!(renamed
            .text
            .contains("rename from old.txt\nrename to new.txt\n"));
        assert!(renamed.text.contains("-line 3\n+line three\n"));
        assert!(!renamed.text.contains("new file mode"));

        let added = unified_diff(
            root,
            UnifiedDiffTarget::Staged {
                path: "new.txt",
                orig_path: None,
            },
            DIFF_CAP,
        )
        .unwrap();
        assert!(added.text.contains("new file mode"));
        assert!(!added.text.contains("rename from"));
    }

    #[test]
    fn unified_diff_commit_covers_normal_root_and_merge_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::write_and_commit(root, "a.txt", "one\ntwo\n", "root");
        let root_commit = head(root);
        test_repo::write_and_commit(root, "a.txt", "one\n2\n", "normal");
        let normal_commit = head(root);
        test_repo::git(root, &["checkout", "-b", "side"]);
        test_repo::write_and_commit(root, "s.txt", "side\n", "side");
        test_repo::git(root, &["checkout", "main"]);
        test_repo::write_and_commit(root, "m.txt", "main\n", "main");
        test_repo::git(root, &["merge", "--no-ff", "side", "-m", "merge"]);
        let merge_commit = head(root);
        let commit = |hash: &str, path: &str| {
            let target = UnifiedDiffTarget::Commit {
                hash,
                path,
                orig_path: None,
            };
            unified_diff(root, target, DIFF_CAP).unwrap()
        };

        let first = commit(&root_commit, "a.txt");
        assert!(first.text.contains("new file mode"));
        assert!(first.text.contains("\n+one\n+two\n"));
        let normal = commit(&normal_commit, "a.txt");
        assert!(normal.text.contains("\n one\n-two\n+2\n"));
        assert!(!normal.text.contains("new file mode"));
        // Another path of the same commit is not part of this diff.
        assert_eq!(commit(&normal_commit, "s.txt").text, "");
        // A merge commit is compared with its first parent: the side branch's
        // file is added, the first parent's own file is not.
        let merged = commit(&merge_commit, "s.txt");
        assert!(merged.text.contains("new file mode"));
        assert!(merged.text.contains("\n+side\n"));
        assert_eq!(commit(&merge_commit, "m.txt").text, "");
        // A revision name resolves to the same commit.
        assert_eq!(commit("HEAD", "s.txt").text, merged.text);

        for hash in ["--output=/tmp/unified-diff-pwned", "", "no-such-rev"] {
            assert!(unified_diff(
                root,
                UnifiedDiffTarget::Commit {
                    hash,
                    path: "a.txt",
                    orig_path: None
                },
                DIFF_CAP
            )
            .is_err());
        }
    }

    #[test]
    fn unified_diff_truncates_at_a_line_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        let before: String = (0..2000).map(|n| format!("line {n}\n")).collect();
        test_repo::write_and_commit(root, "big.txt", &before, "c1");
        std::fs::write(root.join("big.txt"), before.replace("line", "LINE")).unwrap();
        let full = unified_diff(root, worktree("big.txt"), DIFF_CAP).unwrap();
        assert!(!full.truncated);
        assert!(full.text.len() > 1000);

        let capped = unified_diff(root, worktree("big.txt"), 1000).unwrap();
        assert!(capped.truncated);
        assert!(full.text.starts_with(&capped.text));
        assert!(capped.text.ends_with('\n'));
        let last_newline = full.text.as_bytes()[..1000]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .unwrap();
        assert_eq!(capped.text.len(), last_newline + 1);

        // The cap itself is allowed; one byte less cuts the last line.
        let exact = unified_diff(root, worktree("big.txt"), full.text.len()).unwrap();
        assert!(!exact.truncated);
        assert_eq!(exact.text, full.text);
        let short = unified_diff(root, worktree("big.txt"), full.text.len() - 1).unwrap();
        assert!(short.truncated);
        assert!(full.text.starts_with(&short.text));
        assert!(short.text.len() < full.text.len());

        // Without a newline inside the cap nothing complete is left.
        let none = unified_diff(root, worktree("big.txt"), 5).unwrap();
        assert!(none.truncated);
        assert_eq!(none.text, "");
    }

    #[test]
    fn unified_diff_cuts_output_beyond_the_process_limit_instead_of_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::write_and_commit(root, "a.txt", "1\n", "c1");
        // Well past the 8 MiB default stdout limit of the shared runner.
        let line = format!("{}\n", "x".repeat(79));
        std::fs::write(root.join("huge.txt"), line.repeat(160_000)).unwrap();
        let huge = unified_diff(
            root,
            UnifiedDiffTarget::Worktree {
                path: "huge.txt",
                untracked: true,
            },
            262_144,
        )
        .unwrap();
        assert!(huge.truncated);
        assert!(huge.text.len() <= 262_144);
        assert!(huge.text.len() > 262_144 - 100);
        assert!(huge.text.ends_with('\n'));
        assert!(huge.text.starts_with("diff --git a/huge.txt b/huge.txt\n"));
    }

    #[test]
    fn unified_diff_reports_binary_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        std::fs::write(root.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        test_repo::git(root, &["add", "bin.dat"]);
        test_repo::git(root, &["commit", "-m", "c1"]);
        std::fs::write(root.join("bin.dat"), [0u8, 9, 9, 9, 9]).unwrap();
        let changed = unified_diff(root, worktree("bin.dat"), DIFF_CAP).unwrap();
        assert!(changed.binary);
        assert!(changed
            .text
            .contains("Binary files a/bin.dat and b/bin.dat differ"));
        assert!(!changed.truncated);

        std::fs::write(root.join("other.dat"), [0u8, 1]).unwrap();
        let untracked = unified_diff(
            root,
            UnifiedDiffTarget::Worktree {
                path: "other.dat",
                untracked: true,
            },
            DIFF_CAP,
        )
        .unwrap();
        assert!(untracked.binary);

        std::fs::write(root.join("text.txt"), "Binary files are fun\n").unwrap();
        let text = unified_diff(
            root,
            UnifiedDiffTarget::Worktree {
                path: "text.txt",
                untracked: true,
            },
            DIFF_CAP,
        )
        .unwrap();
        assert!(!text.binary);
    }

    #[test]
    fn unified_diff_rejects_paths_outside_the_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        test_repo::init(&root);
        test_repo::write_and_commit(&root, "a.txt", "1\n", "c1");
        let hash = head(&root);
        let secret = tmp.path().join("secret.txt");
        std::fs::write(&secret, "TOPSECRET\n").unwrap();
        let absolute = secret.to_str().unwrap();

        for path in [absolute, "../secret.txt", "a/../../secret.txt", "", "a\0b"] {
            for untracked in [false, true] {
                let target = UnifiedDiffTarget::Worktree { path, untracked };
                assert!(unified_diff(&root, target, DIFF_CAP).is_err(), "{path:?}");
            }
            let staged = UnifiedDiffTarget::Staged {
                path,
                orig_path: None,
            };
            assert!(unified_diff(&root, staged, DIFF_CAP).is_err(), "{path:?}");
            let renamed = UnifiedDiffTarget::Staged {
                path: "a.txt",
                orig_path: Some(path),
            };
            assert!(unified_diff(&root, renamed, DIFF_CAP).is_err(), "{path:?}");
            let commit = UnifiedDiffTarget::Commit {
                hash: &hash,
                path,
                orig_path: None,
            };
            assert!(unified_diff(&root, commit, DIFF_CAP).is_err(), "{path:?}");
            let renamed = UnifiedDiffTarget::Commit {
                hash: &hash,
                path: "a.txt",
                orig_path: Some(path),
            };
            assert!(unified_diff(&root, renamed, DIFF_CAP).is_err(), "{path:?}");
        }

        // A link may not lead out of the repository, and a final symlink is
        // diffed as a link (its target path), never as the file it points to.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.path(), root.join("dir-link")).unwrap();
            let through = UnifiedDiffTarget::Worktree {
                path: "dir-link/secret.txt",
                untracked: true,
            };
            assert!(unified_diff(&root, through, DIFF_CAP).is_err());
            std::os::unix::fs::symlink(&secret, root.join("file-link")).unwrap();
            let link = UnifiedDiffTarget::Worktree {
                path: "file-link",
                untracked: true,
            };
            let diff = unified_diff(&root, link, DIFF_CAP).unwrap();
            assert!(diff.text.contains("new file mode 120000"));
            assert!(!diff.text.contains("TOPSECRET"));
        }
    }

    #[test]
    fn unified_diff_commit_pairs_a_rename_through_orig_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        let body: String = (0..20).map(|n| format!("line {n}\n")).collect();
        test_repo::write_and_commit(root, "old.txt", &body, "c1");
        test_repo::git(root, &["mv", "old.txt", "new.txt"]);
        std::fs::write(
            root.join("new.txt"),
            body.replace("line 3\n", "line three\n"),
        )
        .unwrap();
        test_repo::git(root, &["add", "new.txt"]);
        test_repo::git(root, &["commit", "-m", "rename"]);
        let hash = head(root);
        let commit = |orig_path| UnifiedDiffTarget::Commit {
            hash: &hash,
            path: "new.txt",
            orig_path,
        };

        // Without the old name the new path looks like a whole new file.
        let added = unified_diff(root, commit(None), DIFF_CAP).unwrap();
        assert!(added.text.contains("new file mode"));
        let renamed = unified_diff(root, commit(Some("old.txt")), DIFF_CAP).unwrap();
        assert!(renamed
            .text
            .contains("rename from old.txt\nrename to new.txt\n"));
        assert!(renamed.text.contains("-line 3\n+line three\n"));
        assert!(!renamed.text.contains("new file mode"));
    }

    #[test]
    fn unified_diff_untracked_only_reads_what_git_lists_as_untracked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        std::fs::write(root.join(".gitignore"), ".env\nsecrets/\n").unwrap();
        test_repo::git(root, &["add", ".gitignore"]);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED-CONTENT\n", "c1");
        let secrets = ["API_KEY=sk-live-123", "ID-RSA-PRIVATE", "EXCLUDED-CONTENT"];
        std::fs::write(root.join(".env"), format!("{}\n", secrets[0])).unwrap();
        std::fs::create_dir(root.join("secrets")).unwrap();
        std::fs::write(root.join("secrets/id_rsa"), format!("{}\n", secrets[1])).unwrap();
        std::fs::write(root.join(".git/info/exclude"), "local.txt\n").unwrap();
        std::fs::write(root.join("local.txt"), format!("{}\n", secrets[2])).unwrap();
        // Credentials a remote URL keeps in the repository's own config.
        test_repo::git(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://user:tok3n@example.com/x.git",
            ],
        );
        std::fs::write(root.join("staged.txt"), "STAGED-CONTENT\n").unwrap();
        test_repo::git(root, &["add", "staged.txt"]);
        std::fs::create_dir(root.join("dir")).unwrap();
        std::fs::write(root.join("dir/inner.txt"), "inner\n").unwrap();
        std::fs::write(root.join("new.txt"), "hello\n").unwrap();

        let untracked = |path| {
            unified_diff(
                root,
                UnifiedDiffTarget::Worktree {
                    path,
                    untracked: true,
                },
                DIFF_CAP,
            )
        };
        for path in [
            ".env",
            "secrets/id_rsa",
            "local.txt",
            "tracked.txt",
            "staged.txt",
            "missing.txt",
            // A directory is not one untracked file.
            "dir",
            "dir/",
            "secrets",
        ] {
            assert_eq!(
                untracked(path).err().as_deref(),
                Some(DIFF_NOT_UNTRACKED),
                "{path}"
            );
        }
        // Without the flag Git itself diffs only tracked paths: nothing of an ignored file.
        let ignored = unified_diff(root, worktree(".env"), DIFF_CAP).unwrap();
        assert_eq!(ignored.text, "");

        // `.git` is refused for every kind, in any case and with either separator.
        let hash = head(root);
        for path in [
            ".git/config",
            ".GIT/config",
            ".Git/config",
            "sub/.git/config",
            ".git",
            ".git\\config",
        ] {
            let targets = [
                UnifiedDiffTarget::Worktree {
                    path,
                    untracked: true,
                },
                UnifiedDiffTarget::Worktree {
                    path,
                    untracked: false,
                },
                UnifiedDiffTarget::Staged {
                    path,
                    orig_path: None,
                },
                UnifiedDiffTarget::Staged {
                    path: "tracked.txt",
                    orig_path: Some(path),
                },
                UnifiedDiffTarget::Commit {
                    hash: &hash,
                    path,
                    orig_path: None,
                },
                UnifiedDiffTarget::Commit {
                    hash: &hash,
                    path: "tracked.txt",
                    orig_path: Some(path),
                },
            ];
            for target in targets {
                let error = unified_diff(root, target, DIFF_CAP).err();
                assert_eq!(error.as_deref(), Some(DIFF_GIT_PATH), "{path}");
            }
        }

        // What is really untracked still diffs, in a subfolder too.
        for (path, text) in [("new.txt", "+hello\n"), ("dir/inner.txt", "+inner\n")] {
            let diff = untracked(path).unwrap();
            assert!(diff.text.contains("new file mode"), "{path}");
            assert!(diff.text.contains(text), "{path}");
        }
        // Names that merely contain ".git" are ordinary files.
        std::fs::write(root.join(".gitkeep"), "keep\n").unwrap();
        std::fs::write(root.join("a.git"), "dot\n").unwrap();
        assert!(untracked(".gitkeep").unwrap().text.contains("+keep\n"));
        assert!(untracked("a.git").unwrap().text.contains("+dot\n"));
    }

    /// A repository whose `secrets/`, `données/` (NFC) and `.env` are ignored,
    /// each holding a secret the diff must never show.
    fn ignored_secrets_repo(ignorecase: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        std::fs::write(root.join(".gitignore"), ".env\nsecrets/\ndonn\u{e9}es/\n").unwrap();
        test_repo::git(root, &["add", ".gitignore"]);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        std::fs::write(root.join(".env"), "API_KEY=sk-live-123\n").unwrap();
        std::fs::create_dir(root.join("secrets")).unwrap();
        std::fs::write(root.join("secrets/id_rsa"), "ID-RSA-PRIVATE\n").unwrap();
        std::fs::create_dir(root.join("donn\u{e9}es")).unwrap();
        std::fs::write(root.join("donn\u{e9}es/k"), "NFC-SECRET\n").unwrap();
        // A repository copied from another filesystem keeps its own idea of case
        // and normalisation, whatever this one does.
        test_repo::git(root, &["config", "core.ignorecase", ignorecase]);
        test_repo::git(root, &["config", "core.precomposeunicode", "false"]);
        tmp
    }

    fn untracked_diff(root: &Path, path: &str) -> Result<UnifiedDiff, String> {
        unified_diff(
            root,
            UnifiedDiffTarget::Worktree {
                path,
                untracked: true,
            },
            DIFF_CAP,
        )
    }

    #[test]
    fn unified_diff_untracked_refuses_another_spelling_of_an_ignored_file() {
        for ignorecase in ["false", "true"] {
            let tmp = ignored_secrets_repo(ignorecase);
            let root = tmp.path();
            // HFS+ stores names decomposed: there that spelling is the stored one.
            let stores_nfd = std::fs::read_dir(root)
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name() == "donne\u{301}es");
            // Another case, a zero-width character and the decomposed form of an
            // ignored name reach the same file on a folding filesystem; git then
            // matches its ignore rules against the spelling it was given.
            for path in [
                "secrets/id_rsa",
                "SECRETS/id_rsa",
                "Secrets/id_rsa",
                "secrets\u{200c}/id_rsa",
                "secrets/ID_RSA",
                ".env",
                ".ENV",
                ".env\u{200c}",
                "donn\u{e9}es/k",
                "donne\u{301}es/k",
                "DONN\u{c9}ES/k",
            ] {
                if stores_nfd && path == "donne\u{301}es/k" {
                    continue;
                }
                let error = untracked_diff(root, path).err();
                assert_eq!(
                    error.as_deref(),
                    Some(DIFF_NOT_UNTRACKED),
                    "{path:?} ignorecase={ignorecase}"
                );
            }
        }
    }

    #[test]
    fn unified_diff_untracked_needs_every_component_spelled_as_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        std::fs::create_dir_all(root.join("newdir/deeper")).unwrap();
        std::fs::write(root.join("newdir/deeper/f.txt"), "deep\n").unwrap();
        std::fs::write(root.join("plain.txt"), "plain\n").unwrap();

        // The same file under another case, an empty or `.` component, or below a
        // file is not the name the filesystem lists.
        for path in [
            "PLAIN.txt",
            "Plain.TXT",
            "NEWDIR/deeper/f.txt",
            "newdir/DEEPER/f.txt",
            "newdir/deeper/F.txt",
            "newdir//deeper/f.txt",
            "newdir/./deeper/f.txt",
            "plain.txt/x",
        ] {
            let error = untracked_diff(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path}");
        }

        // What is spelled as stored and untracked still diffs, in a new folder too.
        for (path, text) in [
            ("plain.txt", "+plain\n"),
            ("newdir/deeper/f.txt", "+deep\n"),
        ] {
            let diff = untracked_diff(root, path).unwrap();
            assert!(diff.text.contains("new file mode"), "{path}");
            assert!(diff.text.contains(text), "{path}");
        }
    }

    #[test]
    fn entry_spelling_is_exact_or_canonically_equivalent_where_git_precomposes() {
        let nfc = "caf\u{e9}.md";
        let nfd = "cafe\u{301}.md";
        // Exact bytes, whichever normalisation either side has.
        assert!(entry_is_spelled_as(nfc.as_ref(), nfc, false));
        assert!(entry_is_spelled_as(nfd.as_ref(), nfd, false));
        assert!(!entry_is_spelled_as(nfd.as_ref(), nfc, false));
        assert!(!entry_is_spelled_as(nfc.as_ref(), nfd, false));
        // Precomposing: the exact name still counts, and so does any spelling
        // canonically equivalent to the entry. Which of them git lists is
        // decided by its own listing.
        assert!(entry_is_spelled_as(nfd.as_ref(), nfc, true));
        assert!(entry_is_spelled_as(nfc.as_ref(), nfc, true));
        assert!(entry_is_spelled_as(nfd.as_ref(), nfd, true));
        assert!(entry_is_spelled_as(nfc.as_ref(), nfd, true));
        assert!(!entry_is_spelled_as(nfd.as_ref(), "CAF\u{c9}.md", true));
        // A name NFC would change (U+2126 OHM SIGN, which git lists as stored).
        let ohm = "10k\u{2126}.txt";
        let ohm_nfc = "10k\u{3a9}.txt";
        for precomposed in [false, true] {
            assert!(entry_is_spelled_as(ohm.as_ref(), ohm, precomposed));
        }
        for (entry, part) in [(ohm_nfc, ohm), (ohm, ohm_nfc)] {
            assert!(entry_is_spelled_as(entry.as_ref(), part, true));
            assert!(!entry_is_spelled_as(entry.as_ref(), part, false));
        }
        // A component that mixes a compatibility character (U+F900, which NFC
        // maps to U+8C48) with a decomposed sequence: it is neither the request
        // nor the NFC of it, yet canonically equivalent.
        let mixed = "\u{f900}\u{1112}\u{1161}\u{11ab}.txt";
        let request = "\u{f900}\u{d55c}.txt";
        for (entry, part, precomposed, expected) in [
            (mixed, request, true, true),
            (request, mixed, true, true),
            (mixed, request, false, false),
            // Equivalence is not a loose match: another syllable stays another name.
            (mixed, "\u{f900}\u{d55d}.txt", true, false),
        ] {
            assert_eq!(
                entry_is_spelled_as(entry.as_ref(), part, precomposed),
                expected,
                "{entry:?} {part:?} precomposed={precomposed}"
            );
        }
        assert!(!entry_is_spelled_as(ohm.as_ref(), "10kA.txt", true));
    }

    #[test]
    fn pick_spelled_entry_prefers_the_exact_name_over_an_equivalent_one() {
        // U+2000 EN QUAD is canonically U+2002 EN SPACE, and HFS+ keeps both
        // untouched: two names for one NFC, side by side on a case-sensitive volume.
        let quad = "a\u{2000}b.txt";
        let space = "a\u{2002}b.txt";
        let names = |list: &[&str]| -> Vec<std::ffi::OsString> {
            list.iter().map(std::ffi::OsString::from).collect()
        };
        let pick = |list: &[&str], part: &str, precomposed: bool| {
            pick_spelled_entry(names(list), |name| name.clone(), part, precomposed)
        };
        for order in [[quad, space], [space, quad]] {
            for precomposed in [false, true] {
                for part in [quad, space] {
                    assert_eq!(
                        pick(&order, part, precomposed),
                        Some(std::ffi::OsString::from(part)),
                        "{order:?} {part:?} precomposed={precomposed}"
                    );
                }
            }
        }
        // Without an exact name the first equivalent one is taken, and only
        // where git precomposes.
        assert_eq!(
            pick(&["other.txt", quad], space, true),
            Some(std::ffi::OsString::from(quad))
        );
        assert_eq!(pick(&["other.txt", quad], space, false), None);
        assert_eq!(pick(&["other.txt"], space, true), None);
        assert_eq!(pick(&[], space, true), None);
    }

    // The same names as Foundation stores them (NFD) and as git reports them (NFC).
    #[cfg(target_os = "macos")]
    const NFD_FILE: &str = "cafe\u{301}.md";
    #[cfg(target_os = "macos")]
    const NFC_FILE: &str = "caf\u{e9}.md";
    #[cfg(target_os = "macos")]
    const NFD_DIR: &str = "\u{30d5}\u{30a9}\u{30eb}\u{30bf}\u{3099}";
    #[cfg(target_os = "macos")]
    const NFC_DIR: &str = "\u{30d5}\u{30a9}\u{30eb}\u{30c0}";

    /// A repository with an untracked file and an untracked folder holding an
    /// ASCII-named file, both written under their NFD names: a macOS volume
    /// keeps the bytes it is given, as it does for Foundation's own writes.
    #[cfg(target_os = "macos")]
    fn nfd_stored_repo(precompose: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::git(root, &["config", "core.precomposeunicode", precompose]);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        std::fs::write(root.join(NFD_FILE), "file\n").unwrap();
        std::fs::create_dir(root.join(NFD_DIR)).unwrap();
        std::fs::write(root.join(NFD_DIR).join("readme.txt"), "inner\n").unwrap();
        let stored: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(stored.iter().any(|name| name == NFD_FILE), "{stored:?}");
        assert!(stored.iter().any(|name| name == NFD_DIR), "{stored:?}");
        tmp
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn git_precomposes_names_follows_the_effective_config_and_is_strict_when_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        let env = test_repo::isolated_env();
        let key = "core.precomposeunicode";
        // `git init` may have written the key; start from none.
        let _ = run_git(root, &["config", "--unset", key], DEFAULT_TIMEOUT, &env);
        assert!(!git_precomposes_names(root, &env));
        for (value, expected) in [
            ("true", true),
            ("yes", true),
            ("false", false),
            ("0", false),
        ] {
            test_repo::git(root, &["config", key, value]);
            assert_eq!(git_precomposes_names(root, &env), expected, "{value}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_diffs_a_decomposed_name_by_the_nfc_git_reports() {
        let tmp = nfd_stored_repo("true");
        let root = tmp.path();
        // What `git status` lists for them.
        for (path, text) in [
            (NFC_FILE.to_string(), "+file\n"),
            (format!("{NFC_DIR}/readme.txt"), "+inner\n"),
        ] {
            assert!(ensure_spelled_as_on_disk(root, &path).is_ok(), "{path}");
            let diff = untracked_diff(root, &path).unwrap();
            assert!(diff.text.contains("new file mode"), "{path}");
            assert!(diff.text.contains(text), "{path}: {}", diff.text);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_keeps_the_exact_spelling_where_git_does_not_precompose() {
        let tmp = nfd_stored_repo("false");
        let root = tmp.path();
        // Git then reports the stored NFD names; the NFC form is another spelling.
        for path in [NFC_FILE.to_string(), format!("{NFC_DIR}/readme.txt")] {
            let error = ensure_spelled_as_on_disk(root, &path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path}");
            let error = untracked_diff(root, &path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path}");
        }
        for (path, text) in [
            (NFD_FILE.to_string(), "+file\n"),
            (format!("{NFD_DIR}/readme.txt"), "+inner\n"),
        ] {
            let diff = untracked_diff(root, &path).unwrap();
            assert!(diff.text.contains(text), "{path}: {}", diff.text);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_refuses_a_decomposed_request_even_where_git_precomposes() {
        let tmp = nfd_stored_repo("true");
        let root = tmp.path();
        for path in [NFD_FILE.to_string(), format!("{NFD_DIR}/readme.txt")] {
            // It is the name as stored, so the spelling check lets it by; git
            // precomposes it and lists the NFC name, which is not the request.
            assert!(ensure_spelled_as_on_disk(root, &path).is_ok(), "{path}");
            let error = untracked_diff(root, &path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path}");
        }
    }

    // Names git lists exactly as stored (its precomposition leaves them alone)
    // although Unicode NFC changes them: U+2126 OHM SIGN, U+212B ANGSTROM SIGN,
    // U+212A KELVIN SIGN and the CJK compatibility ideographs U+F900 and U+FA10.
    #[cfg(target_os = "macos")]
    const NFC_CHANGES: [&str; 5] = [
        "10k\u{2126}.txt",
        "\u{212b}ngstrom.txt",
        "\u{212a}.txt",
        "\u{f900}.txt",
        "\u{fa10}x.txt",
    ];

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_diffs_a_name_git_lists_as_stored_although_nfc_changes_it() {
        for precompose in ["true", "false"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            test_repo::init(root);
            test_repo::git(root, &["config", "core.precomposeunicode", precompose]);
            test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
            let mut paths: Vec<String> = NFC_CHANGES.iter().map(|name| name.to_string()).collect();
            std::fs::create_dir(root.join("\u{212b}dir")).unwrap();
            paths.push("\u{212b}dir/\u{fa10}x.txt".into());
            for path in &paths {
                std::fs::write(root.join(path), "file\n").unwrap();
            }
            // What `git status` lists (what the phone sends back) is what is stored.
            let untracked = status_of(root, None).unwrap().parsed.untracked;
            for path in &paths {
                assert!(untracked.contains(path), "{path:?} {untracked:?}");
            }
            for path in &paths {
                let diff = untracked_diff(root, path)
                    .unwrap_or_else(|error| panic!("{path:?} precompose={precompose}: {error}"));
                assert!(diff.text.contains("new file mode"), "{path:?}");
                assert!(diff.text.contains("+file\n"), "{path:?}: {}", diff.text);
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_refuses_the_nfc_of_a_name_git_lists_as_stored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::git(root, &["config", "core.precomposeunicode", "true"]);
        // The rules are spelled as the names are stored: git does not precompose them.
        std::fs::write(
            root.join(".gitignore"),
            "secret-\u{2126}.txt\n\u{212b}secrets/\n",
        )
        .unwrap();
        test_repo::git(root, &["add", ".gitignore"]);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        std::fs::write(root.join("10k\u{2126}.txt"), "file\n").unwrap();
        std::fs::create_dir(root.join("\u{212b}dir")).unwrap();
        std::fs::write(root.join("\u{212b}dir/f.txt"), "inner\n").unwrap();
        std::fs::write(root.join("secret-\u{2126}.txt"), "SECRET\n").unwrap();
        std::fs::create_dir(root.join("\u{212b}secrets")).unwrap();
        std::fs::write(root.join("\u{212b}secrets/key"), "SECRET\n").unwrap();
        // The NFC of a stored name is not what git lists, though a volume that
        // folds names opens the same file under it. Git names a folder the way it
        // was asked for, so the folder's ignore rule must not be dodged either.
        for path in [
            "10k\u{3a9}.txt",
            "\u{c5}dir/f.txt",
            "secret-\u{3a9}.txt",
            "\u{c5}secrets/key",
        ] {
            assert!(ensure_spelled_as_on_disk(root, path).is_ok(), "{path:?}");
            let error = untracked_diff(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path:?}");
        }
        // The ignored files stay ignored under the names they are stored by.
        for path in ["secret-\u{2126}.txt", "\u{212b}secrets/key"] {
            let error = untracked_diff(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path:?}");
        }
        // The stored names themselves diff.
        for path in ["10k\u{2126}.txt", "\u{212b}dir/f.txt"] {
            let diff = untracked_diff(root, path).unwrap();
            assert!(diff.text.contains("new file mode"), "{path:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_keeps_an_ignored_decomposed_name_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::git(root, &["config", "core.precomposeunicode", "true"]);
        // The rules are NFC; git matches them against the precomposed names.
        std::fs::write(
            root.join(".gitignore"),
            "donn\u{e9}es/\ncaf\u{e9}-secret.txt\n",
        )
        .unwrap();
        test_repo::git(root, &["add", ".gitignore"]);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        std::fs::create_dir(root.join("donne\u{301}es")).unwrap();
        std::fs::write(root.join("donne\u{301}es/k"), "NFD-SECRET\n").unwrap();
        std::fs::write(root.join("cafe\u{301}-secret.txt"), "NFD-SECRET\n").unwrap();
        for path in ["donn\u{e9}es/k", "caf\u{e9}-secret.txt"] {
            // The NFC request reaches the stored name, so git's answer must stop it.
            assert!(ensure_spelled_as_on_disk(root, path).is_ok(), "{path}");
            let error = untracked_diff(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path}");
        }
        for path in ["donne\u{301}es/k", "cafe\u{301}-secret.txt"] {
            let error = untracked_diff(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path}");
        }
    }

    // Names as Foundation stores them: a compatibility character kept (U+F900,
    // U+2126 OHM SIGN, U+212B ANGSTROM SIGN), the rest of the component
    // decomposed. Git lists them half composed, so they are neither the
    // request nor the NFC of it.
    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_diffs_a_name_mixing_a_compatibility_character_with_a_decomposed_sequence(
    ) {
        use unicode_normalization::UnicodeNormalization;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::git(root, &["config", "core.precomposeunicode", "true"]);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        let hangul_nfd: String = "\u{d55c}\u{ae00}".nfd().collect();
        let stored = [
            format!("\u{f900}{hangul_nfd}.txt"),
            "\u{2126}cafe\u{301}.md".to_string(),
            "\u{212b}ngstro\u{308}m.txt".to_string(),
            format!("\u{f900}{hangul_nfd}/readme.txt"),
        ];
        std::fs::create_dir(root.join(format!("\u{f900}{hangul_nfd}"))).unwrap();
        for name in &stored {
            std::fs::write(root.join(name), "file\n").unwrap();
        }
        let on_disk: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(
            on_disk
                .iter()
                .any(|name| name == std::ffi::OsStr::new(&stored[0])),
            "{on_disk:?}"
        );
        // What `git status` lists, and what the phone sends back.
        let listed = [
            "\u{f900}\u{d55c}\u{ae00}.txt",
            "\u{2126}caf\u{e9}.md",
            "\u{212b}ngstr\u{f6}m.txt",
            "\u{f900}\u{d55c}\u{ae00}/readme.txt",
        ];
        let untracked = status_of(root, None).unwrap().parsed.untracked;
        for path in listed {
            assert!(
                untracked.iter().any(|name| name == path),
                "{path:?} {untracked:?}"
            );
        }
        for path in listed {
            assert!(ensure_spelled_as_on_disk(root, path).is_ok(), "{path:?}");
            let diff =
                untracked_diff(root, path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
            assert!(diff.text.contains("new file mode"), "{path:?}");
            assert!(diff.text.contains("+file\n"), "{path:?}: {}", diff.text);
        }
        // Another spelling of the same name stays another name: the stored one,
        // and the NFC of the compatibility character.
        for path in [
            stored[0].as_str(),
            stored[3].as_str(),
            "\u{8c48}\u{d55c}\u{ae00}.txt",
            "\u{3a9}caf\u{e9}.md",
        ] {
            let error = untracked_diff(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_keeps_an_ignored_mixed_name_ignored_under_every_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::git(root, &["config", "core.precomposeunicode", "true"]);
        // Rules git matches against the precomposed names.
        std::fs::write(
            root.join(".gitignore"),
            "mix-\u{2126}caf\u{e9}.md\n\u{f900}k/\n",
        )
        .unwrap();
        test_repo::git(root, &["add", ".gitignore"]);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        std::fs::write(root.join("mix-\u{2126}cafe\u{301}.md"), "SECRET\n").unwrap();
        std::fs::create_dir(root.join("\u{f900}k")).unwrap();
        std::fs::write(root.join("\u{f900}k/key"), "SECRET\n").unwrap();
        let untracked = status_of(root, None).unwrap().parsed.untracked;
        assert!(untracked.is_empty(), "{untracked:?}");
        // The stored name, git's own spelling and the canonically equivalent
        // ones the spelling layer now lets through all end at git's answer.
        for path in [
            "mix-\u{2126}cafe\u{301}.md",
            "mix-\u{2126}caf\u{e9}.md",
            "mix-\u{3a9}caf\u{e9}.md",
            "mix-\u{3a9}cafe\u{301}.md",
            "\u{f900}k/key",
            "\u{8c48}k/key",
        ] {
            let error = untracked_diff(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path:?}");
        }
    }

    #[test]
    fn unified_diff_untracked_refuses_a_decomposed_request_of_an_ignored_name_with_precompose() {
        for ignorecase in ["false", "true"] {
            let tmp = ignored_secrets_repo(ignorecase);
            let root = tmp.path();
            test_repo::git(root, &["config", "core.precomposeunicode", "true"]);
            // `données/` is stored NFC here; the NFD request is another spelling of it.
            for path in ["donn\u{e9}es/k", "donne\u{301}es/k"] {
                let error = untracked_diff(root, path).err();
                assert_eq!(
                    error.as_deref(),
                    Some(DIFF_NOT_UNTRACKED),
                    "{path:?} ignorecase={ignorecase}"
                );
            }
            // Where git precomposes, the spelling layer takes the equivalent name
            // (git's own answer above is what stops it); elsewhere it never matches.
            let spelling = ensure_spelled_as_on_disk(root, "donne\u{301}es/k");
            if cfg!(target_os = "macos") {
                assert_eq!(
                    spelling.as_deref(),
                    Ok("donn\u{e9}es/k"),
                    "ignorecase={ignorecase}"
                );
            } else {
                assert_eq!(
                    spelling.err().as_deref(),
                    Some(DIFF_NOT_UNTRACKED),
                    "ignorecase={ignorecase}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn unified_diff_untracked_follows_no_link_but_the_last_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        std::fs::write(root.join(".gitignore"), "secrets/\n").unwrap();
        test_repo::git(root, &["add", ".gitignore"]);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        std::fs::create_dir(root.join("secrets")).unwrap();
        std::fs::write(root.join("secrets/id_rsa"), "ID-RSA-PRIVATE\n").unwrap();
        std::fs::create_dir(root.join("newdir")).unwrap();
        std::fs::write(root.join("newdir/f.txt"), "untracked\n").unwrap();
        // Links inside the repository: one to an ignored folder, one to a folder
        // whose files are untracked.
        std::os::unix::fs::symlink("secrets", root.join("lnk_secrets")).unwrap();
        std::os::unix::fs::symlink("newdir", root.join("lnk_newdir")).unwrap();
        for path in ["lnk_secrets/id_rsa", "lnk_newdir/f.txt"] {
            let error = untracked_diff(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path}");
            // Refused by the spelling check itself, not left to git's own answer.
            let error = ensure_spelled_as_on_disk(root, path).err();
            assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED), "{path}");
        }
        assert!(ensure_spelled_as_on_disk(root, "lnk_newdir").is_ok());

        // A link as the last component is diffed as its link text, not followed.
        std::os::unix::fs::symlink("link-text-target", root.join("lnk_text")).unwrap();
        let diff = untracked_diff(root, "lnk_text").unwrap();
        assert!(diff.text.contains("new file mode 120000"), "{}", diff.text);
        assert!(diff.text.contains("+link-text-target"), "{}", diff.text);
        std::os::unix::fs::symlink("secrets/id_rsa", root.join("lnk_file")).unwrap();
        let diff = untracked_diff(root, "lnk_file").unwrap();
        assert!(diff.text.contains("+secrets/id_rsa"), "{}", diff.text);
        assert!(!diff.text.contains("ID-RSA-PRIVATE"));
    }

    #[test]
    fn dotgit_lookalikes_are_the_names_git_itself_refuses() {
        for name in [
            ".git",
            ".GIT",
            ".Git",
            // HFS+ drops these code points when it compares names.
            ".git\u{200c}",
            ".g\u{200d}it",
            "\u{feff}.git",
            ".gi\u{202e}t",
            ".git\u{206f}",
            // NTFS drops trailing dots and spaces, and reads `:` as a stream.
            ".git.",
            ".git ",
            ".git. . ",
            ".GIT...",
            ".git::$INDEX_ALLOCATION",
            ".git:stream",
            "git~1",
            "GIT~1",
            "Git~1 ",
            "git~1.",
            "git~1:x",
        ] {
            assert!(is_dotgit_lookalike(name), "{name:?}");
            assert_eq!(reject_git_dir(name).err().as_deref(), Some(DIFF_GIT_PATH));
            assert_eq!(
                reject_git_dir(&format!("sub/{name}/config"))
                    .err()
                    .as_deref(),
                Some(DIFF_GIT_PATH),
                "{name:?}"
            );
        }
        for name in [
            "",
            ".gitkeep",
            ".gitignore",
            ".github",
            "a.git",
            "git",
            ".gi",
            "git~",
            "git~2",
            "git~10",
            ".git~1",
            ".git\u{e9}",
            ".gitx.",
            "gitx~1",
        ] {
            assert!(!is_dotgit_lookalike(name), "{name:?}");
        }
    }

    #[test]
    fn unified_diff_refuses_a_lookalike_of_the_git_directory_for_every_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::write_and_commit(root, "tracked.txt", "TRACKED\n", "c1");
        let hash = head(root);
        for path in [
            ".git\u{200c}/config",
            ".g\u{200d}it/config",
            "sub/\u{feff}.git/config",
            ".git./config",
            ".git /config",
            ".git::$INDEX_ALLOCATION/config",
            "GIT~1/config",
            "git~1\\config",
        ] {
            let targets = [
                UnifiedDiffTarget::Worktree {
                    path,
                    untracked: true,
                },
                UnifiedDiffTarget::Worktree {
                    path,
                    untracked: false,
                },
                UnifiedDiffTarget::Staged {
                    path,
                    orig_path: None,
                },
                UnifiedDiffTarget::Staged {
                    path: "tracked.txt",
                    orig_path: Some(path),
                },
                UnifiedDiffTarget::Commit {
                    hash: &hash,
                    path,
                    orig_path: None,
                },
                UnifiedDiffTarget::Commit {
                    hash: &hash,
                    path: "tracked.txt",
                    orig_path: Some(path),
                },
            ];
            for target in targets {
                let error = unified_diff(root, target, DIFF_CAP).err();
                assert_eq!(error.as_deref(), Some(DIFF_GIT_PATH), "{path:?}");
            }
        }
    }

    #[test]
    fn unified_diff_commit_shows_the_commit_itself_despite_a_replace_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        test_repo::write_and_commit(root, "a.txt", "base\n", "base");
        let base = head(root);
        test_repo::write_and_commit(root, "a.txt", "from-A\n", "A");
        let a = head(root);
        test_repo::git(root, &["checkout", "-q", "-b", "other", &base]);
        test_repo::write_and_commit(root, "a.txt", "from-B\n", "B");
        let b = head(root);
        // Without `GIT_NO_REPLACE_OBJECTS`, `git show A` prints B.
        test_repo::git(root, &["replace", &a, &b]);

        let target = UnifiedDiffTarget::Commit {
            hash: &a,
            path: "a.txt",
            orig_path: None,
        };
        let diff = unified_diff(root, target, DIFF_CAP).unwrap();
        assert!(diff.text.contains("+from-A\n"), "{}", diff.text);
        assert!(!diff.text.contains("from-B"), "{}", diff.text);
    }

    // A tracked name whose index spelling is NFD (committed while git did not
    // precompose), read where git precomposes argv: status and the commit
    // listing report the index/tree bytes, so the diff must match them.
    #[cfg(target_os = "macos")]
    fn set_precompose(root: &Path, value: &str) {
        test_repo::git(root, &["config", "core.precomposeunicode", value]);
    }

    /// A repository whose only tracked file is `NFD_FILE` under its NFD index
    /// name, with git precomposing again afterwards.
    #[cfg(target_os = "macos")]
    fn nfd_tracked_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "false");
        std::fs::write(root.join(NFD_FILE), "a\nb\nc\n").unwrap();
        test_repo::git(root, &["add", NFD_FILE]);
        test_repo::git(root, &["commit", "-m", "base"]);
        let listed = run_git(root, &["ls-files", "-z"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(listed.stdout, format!("{NFD_FILE}\0").into_bytes());
        set_precompose(root, "true");
        tmp
    }

    #[cfg(target_os = "macos")]
    fn paths(entries: &[crate::git_status::GitFileEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.path.as_str()).collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_worktree_of_a_tracked_decomposed_name_matches_the_index() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        // status lists the index bytes; the diff takes them back as its pathspec.
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.unstaged), [NFD_FILE], "{status:?}");
        let diff = unified_diff(root, worktree(NFD_FILE), DIFF_CAP).unwrap();
        let header = format!("diff --git a/{NFD_FILE} b/{NFD_FILE}\n");
        assert!(diff.text.starts_with(&header), "{:?}", diff.text);
        assert!(diff.text.contains("\n-b\n+B\n"), "{:?}", diff.text);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_staged_of_a_tracked_decomposed_name_matches_the_index() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        // Staging it as git precomposes would add a second (NFC) index entry.
        set_precompose(root, "false");
        test_repo::git(root, &["add", NFD_FILE]);
        set_precompose(root, "true");
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.staged), [NFD_FILE], "{status:?}");
        let staged = |orig_path| UnifiedDiffTarget::Staged {
            path: NFD_FILE,
            orig_path,
        };
        for orig_path in [None, Some(NFD_FILE)] {
            let diff = unified_diff(root, staged(orig_path), DIFF_CAP).unwrap();
            let header = format!("diff --git a/{NFD_FILE} b/{NFD_FILE}\n");
            assert!(diff.text.starts_with(&header), "{:?}", diff.text);
            assert!(diff.text.contains("\n-b\n+B\n"), "{:?}", diff.text);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_commit_of_a_tracked_decomposed_name_matches_the_tree() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        let base = head(root);
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        set_precompose(root, "false");
        test_repo::git(root, &["add", NFD_FILE]);
        test_repo::git(root, &["commit", "-m", "change"]);
        set_precompose(root, "true");
        let change = head(root);
        for (hash, expected) in [(&base, "\n+a\n+b\n+c\n"), (&change, "\n-b\n+B\n")] {
            // The commit listing reports the tree bytes, which is what the diff gets.
            let detail = crate::git_log::commit_detail(root, hash).unwrap();
            let listed: Vec<&str> = detail.files.iter().map(|f| f.path.as_str()).collect();
            assert_eq!(listed, [NFD_FILE]);
            let target = UnifiedDiffTarget::Commit {
                hash,
                path: listed[0],
                orig_path: None,
            };
            let diff = unified_diff(root, target, DIFF_CAP).unwrap();
            let header = format!("diff --git a/{NFD_FILE} b/{NFD_FILE}\n");
            assert!(diff.text.starts_with(&header), "{:?}", diff.text);
            assert!(diff.text.contains(expected), "{:?}", diff.text);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_worktree_of_a_conflicted_decomposed_name_shows_the_markers() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        set_precompose(root, "false");
        let change = |branch_content: &str, message: &str| {
            std::fs::write(root.join(NFD_FILE), branch_content).unwrap();
            test_repo::git(root, &["add", NFD_FILE]);
            test_repo::git(root, &["commit", "-m", message]);
        };
        test_repo::git(root, &["checkout", "-b", "side"]);
        change("a\nTHEIRS\nc\n", "side");
        test_repo::git(root, &["checkout", "main"]);
        change("a\nOURS\nc\n", "ours");
        let merge = run_git(root, &["merge", "side"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(merge.code, 1, "{}", merge.stderr);
        set_precompose(root, "true");

        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.conflicted), [NFD_FILE], "{status:?}");
        let on_disk = std::fs::read_to_string(root.join(NFD_FILE)).unwrap();
        assert!(on_disk.contains("<<<<<<<"), "{on_disk}");
        let diff = unified_diff(root, worktree(NFD_FILE), DIFF_CAP).unwrap();
        let header = format!("diff --cc {NFD_FILE}\n");
        assert!(diff.text.starts_with(&header), "{:?}", diff.text);
        for marker in ["<<<<<<<", "OURS", "THEIRS", ">>>>>>>"] {
            assert!(diff.text.contains(marker), "{marker}: {:?}", diff.text);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_of_a_tracked_precomposed_name_keeps_working() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "true");
        test_repo::write_and_commit(root, NFC_FILE, "a\nb\nc\n", "base");
        let listed = run_git(root, &["ls-files", "-z"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(listed.stdout, format!("{NFC_FILE}\0").into_bytes());
        let header = format!("diff --git a/{NFC_FILE} b/{NFC_FILE}\n");

        std::fs::write(root.join(NFC_FILE), "a\nB\nc\n").unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.unstaged), [NFC_FILE], "{status:?}");
        let diff = unified_diff(root, worktree(NFC_FILE), DIFF_CAP).unwrap();
        assert!(diff.text.starts_with(&header), "{:?}", diff.text);
        assert!(diff.text.contains("\n-b\n+B\n"), "{:?}", diff.text);

        test_repo::git(root, &["add", NFC_FILE]);
        let staged = UnifiedDiffTarget::Staged {
            path: NFC_FILE,
            orig_path: None,
        };
        let diff = unified_diff(root, staged, DIFF_CAP).unwrap();
        assert!(diff.text.starts_with(&header), "{:?}", diff.text);
        assert!(diff.text.contains("\n-b\n+B\n"), "{:?}", diff.text);

        test_repo::git(root, &["commit", "-m", "change"]);
        let commit = UnifiedDiffTarget::Commit {
            hash: &head(root),
            path: NFC_FILE,
            orig_path: None,
        };
        let diff = unified_diff(root, commit, DIFF_CAP).unwrap();
        assert!(diff.text.starts_with(&header), "{:?}", diff.text);
        assert!(diff.text.contains("\n-b\n+B\n"), "{:?}", diff.text);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_diff_untracked_rules_and_refusals_hold_beside_a_tracked_decomposed_name() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        let untracked_nfd = "ume\u{301}.md";
        let untracked_nfc = "um\u{e9}.md";
        std::fs::write(root.join(untracked_nfd), "new\n").unwrap();
        // The tracked name diffs, while the untracked one keeps its rules: the
        // name git lists (NFC) diffs, the stored spelling of it is refused.
        assert!(unified_diff(root, worktree(NFD_FILE), DIFF_CAP)
            .unwrap()
            .text
            .contains("\n-b\n+B\n"));
        let diff = untracked_diff(root, untracked_nfc).unwrap();
        assert!(diff.text.contains("new file mode"), "{:?}", diff.text);
        let error = untracked_diff(root, untracked_nfd).err();
        assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED));
        // The tracked name is no untracked file; the git directory stays shut.
        let error = untracked_diff(root, NFD_FILE).err();
        assert_eq!(error.as_deref(), Some(DIFF_NOT_UNTRACKED));
        let hash = head(root);
        for path in [".git\u{200c}/config", "GIT~1/config"] {
            let targets = [
                worktree(path),
                UnifiedDiffTarget::Staged {
                    path,
                    orig_path: None,
                },
                UnifiedDiffTarget::Commit {
                    hash: &hash,
                    path,
                    orig_path: Some(NFD_FILE),
                },
            ];
            for target in targets {
                let error = unified_diff(root, target, DIFF_CAP).err();
                assert_eq!(error.as_deref(), Some(DIFF_GIT_PATH), "{path:?}");
            }
        }
    }

    // Names git listed, handed back to git, where it precomposes argv: reads
    // find the NFD index/tree entries, and writes act on the entry instead of
    // adding a second NFC one, while an NFC entry of a file Finder stored NFD
    // and a new Finder file (listed NFC, stored NFD) keep working.

    /// The names of the index, as git stores them.
    #[cfg(target_os = "macos")]
    fn index_names(root: &Path) -> Vec<String> {
        let listed = run_git(root, &["ls-files", "-z"], DEFAULT_TIMEOUT, &[]).unwrap();
        listed
            .stdout
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(|name| String::from_utf8(name.to_vec()).unwrap())
            .collect()
    }

    /// A git command that keeps names as they are, to set up a decomposed index entry.
    #[cfg(target_os = "macos")]
    fn git_as_stored(root: &Path, args: &[&str]) {
        let mut full = TRACKED_NAMES_AS_STORED.to_vec();
        full.extend(args);
        test_repo::git(root, &full);
    }

    /// `NFD_FILE` modified and staged under its NFD index name.
    #[cfg(target_os = "macos")]
    fn nfd_staged_repo() -> tempfile::TempDir {
        let tmp = nfd_tracked_repo();
        std::fs::write(tmp.path().join(NFD_FILE), "a\nB\nc\n").unwrap();
        git_as_stored(tmp.path(), &["add", NFD_FILE]);
        tmp
    }

    /// What Finder leaves: the file is stored under its NFD name, git precomposes
    /// and indexes it as `NFC_FILE`.
    #[cfg(target_os = "macos")]
    fn finder_tracked_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "true");
        std::fs::write(root.join(NFD_FILE), "a\nb\nc\n").unwrap();
        test_repo::git(root, &["add", NFC_FILE]);
        test_repo::git(root, &["commit", "-m", "base"]);
        assert_eq!(index_names(root), [NFC_FILE]);
        let on_disk: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        assert!(on_disk.iter().any(|name| name == NFD_FILE), "{on_disk:?}");
        tmp
    }

    /// A merge of `NFD_FILE` that conflicts: ours rewrites it, theirs rewrites
    /// it differently (`theirs` None: deletes it).
    #[cfg(target_os = "macos")]
    fn nfd_conflicted_repo(theirs: Option<&str>) -> tempfile::TempDir {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        set_precompose(root, "false");
        test_repo::git(root, &["checkout", "-b", "side"]);
        match theirs {
            Some(content) => {
                std::fs::write(root.join(NFD_FILE), content).unwrap();
                test_repo::git(root, &["add", NFD_FILE]);
            }
            None => test_repo::git(root, &["rm", "-q", "--", NFD_FILE]),
        }
        test_repo::git(root, &["commit", "-m", "side"]);
        test_repo::git(root, &["checkout", "main"]);
        std::fs::write(root.join(NFD_FILE), "a\nOURS\nc\n").unwrap();
        test_repo::git(root, &["add", NFD_FILE]);
        test_repo::git(root, &["commit", "-m", "ours"]);
        let merge = run_git(root, &["merge", "side"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(merge.code, 1, "{}", merge.stderr);
        set_precompose(root, "true");
        assert!(index_names(root).iter().all(|name| name == NFD_FILE));
        tmp
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn show_object_reads_a_decomposed_name_from_head_and_the_index() {
        let tmp = nfd_staged_repo();
        let root = tmp.path();
        let head = show_object(root, &format!("HEAD:{NFD_FILE}")).unwrap();
        assert_eq!(text(&head).as_deref(), Some("a\nb\nc\n"));
        let staged = show_object(root, &format!(":0:{NFD_FILE}")).unwrap();
        assert_eq!(text(&staged).as_deref(), Some("a\nB\nc\n"));
        // A name nothing holds is still a missing side, not an error.
        assert!(show_object(root, "HEAD:nobody\u{301}.md")
            .unwrap()
            .is_none());
        assert!(show_object(root, ":0:nobody.md").unwrap().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn show_object_still_reads_a_precomposed_entry_asked_for_by_either_spelling() {
        let tmp = finder_tracked_repo();
        let root = tmp.path();
        // The index holds NFC. The listing spells it so; the file the editor
        // opened from the directory is spelled NFD, which git precomposes.
        for name in [NFC_FILE, NFD_FILE] {
            for spec in [format!("HEAD:{name}"), format!(":0:{name}")] {
                let shown = show_object(root, &spec).unwrap();
                assert_eq!(text(&shown).as_deref(), Some("a\nb\nc\n"), "{spec}");
            }
        }
    }

    /// Adds `NFC_FILE` to the index beside the NFD entry of `NFD_FILE`, with
    /// other content: a tree that holds both spellings, which only the index can.
    #[cfg(target_os = "macos")]
    fn add_precomposed_twin(root: &Path) {
        let written = run_git_with_stdin(
            root,
            &["hash-object", "-w", "--stdin"],
            DEFAULT_TIMEOUT,
            &[],
            b"twin\n",
        )
        .unwrap();
        let oid = String::from_utf8(written.stdout)
            .unwrap()
            .trim()
            .to_string();
        let entry = format!("100644,{oid},{NFC_FILE}");
        git_as_stored(root, &["update-index", "--add", "--cacheinfo", &entry]);
        test_repo::git(root, &["commit", "-m", "twin"]);
        assert_eq!(index_names(root), [NFD_FILE, NFC_FILE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn show_object_prefers_the_exact_decomposed_entry_beside_a_precomposed_twin() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        add_precomposed_twin(root);
        for (name, expected) in [(NFD_FILE, "a\nb\nc\n"), (NFC_FILE, "twin\n")] {
            let shown = show_object(root, &format!("HEAD:{name}")).unwrap();
            assert_eq!(text(&shown).as_deref(), Some(expected), "{name}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn diff_content_of_a_decomposed_name_has_both_sides() {
        let tmp = nfd_staged_repo();
        let root = tmp.path();
        let staged = diff_content(root, NFD_FILE, true, None).unwrap();
        assert_eq!(text(&Some(staged.original)).as_deref(), Some("a\nb\nc\n"));
        assert_eq!(text(&Some(staged.modified)).as_deref(), Some("a\nB\nc\n"));

        std::fs::write(root.join(NFD_FILE), "a\nB\nC\n").unwrap();
        let unstaged = diff_content(root, NFD_FILE, false, None).unwrap();
        assert_eq!(text(&Some(unstaged.original)).as_deref(), Some("a\nB\nc\n"));
        assert_eq!(text(&Some(unstaged.modified)).as_deref(), Some("a\nB\nC\n"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn diff_content_of_a_renamed_decomposed_source_reads_its_head_side() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        git_as_stored(root, &["mv", "--", NFD_FILE, "plain.md"]);
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(status.staged[0].orig_path.as_deref(), Some(NFD_FILE));
        let diff = diff_content(root, "plain.md", true, Some(NFD_FILE)).unwrap();
        assert_eq!(text(&Some(diff.original)).as_deref(), Some("a\nb\nc\n"));
        assert_eq!(text(&Some(diff.modified)).as_deref(), Some("a\nb\nc\n"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn conflict_sides_of_a_decomposed_name_have_every_stage() {
        let tmp = nfd_conflicted_repo(Some("a\nTHEIRS\nc\n"));
        let root = tmp.path();
        let sides = conflict_sides(root, NFD_FILE).unwrap();
        assert_eq!(sides.code, "UU");
        assert_eq!(text(&sides.base).as_deref(), Some("a\nb\nc\n"));
        assert_eq!(text(&sides.ours).as_deref(), Some("a\nOURS\nc\n"));
        assert_eq!(text(&sides.theirs).as_deref(), Some("a\nTHEIRS\nc\n"));
        assert!(text(&sides.worktree).unwrap().contains("<<<<<<<"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn conflict_resolve_takes_a_side_of_a_decomposed_name() {
        for (side, expected) in [("ours", "a\nOURS\nc\n"), ("theirs", "a\nTHEIRS\nc\n")] {
            let tmp = nfd_conflicted_repo(Some("a\nTHEIRS\nc\n"));
            let root = tmp.path();
            conflict_resolve(root, &[NFD_FILE.to_string()], side).unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
                expected
            );
            let status = status_of(root, None).unwrap().parsed;
            assert!(status.conflicted.is_empty(), "{side}: {status:?}");
            // Marked resolved on the index entry itself, not on an NFC twin.
            assert_eq!(index_names(root), [NFD_FILE], "{side}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn conflict_resolve_to_a_deletion_removes_a_decomposed_name() {
        let tmp = nfd_conflicted_repo(None);
        let root = tmp.path();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.conflicted), [NFD_FILE], "{status:?}");
        conflict_resolve(root, &[NFD_FILE.to_string()], "theirs").unwrap();
        assert!(index_names(root).is_empty());
        assert!(!root.join(NFD_FILE).exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_marks_a_tracked_decomposed_entry_without_a_second_one() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        stage(root, &[NFD_FILE.to_string()]).unwrap();
        assert_eq!(index_names(root), [NFD_FILE]);
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.staged), [NFD_FILE], "{status:?}");
        assert!(status.unstaged.is_empty(), "{status:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_adds_a_new_finder_file_once_under_the_name_status_lists() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        let untracked_nfd = "ume\u{301}.md";
        let untracked_nfc = "um\u{e9}.md";
        // Finder stores the new file NFD; status, precomposing, lists it NFC.
        std::fs::write(root.join(untracked_nfd), "new\n").unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert!(
            status.untracked.iter().any(|name| name == untracked_nfc),
            "{status:?}"
        );
        stage(root, &[untracked_nfc.to_string()]).unwrap();
        assert_eq!(index_names(root), [NFD_FILE, untracked_nfc]);
        let status = status_of(root, None).unwrap().parsed;
        assert!(
            status
                .staged
                .iter()
                .any(|entry| entry.path == untracked_nfc),
            "{status:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_takes_a_decomposed_entry_and_a_new_finder_file_in_one_call() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        std::fs::write(root.join("ume\u{301}.md"), "new\n").unwrap();
        let requested = [
            NFD_FILE.to_string(),
            "um\u{e9}.md".to_string(),
            "plain.md".to_string(),
        ];
        std::fs::write(root.join("plain.md"), "plain\n").unwrap();
        stage(root, &requested).unwrap();
        assert_eq!(index_names(root), [NFD_FILE, "plain.md", "um\u{e9}.md"]);
        let status = status_of(root, None).unwrap().parsed;
        assert!(status.unstaged.is_empty(), "{status:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_unstage_and_discard_keep_working_for_a_precomposed_entry_of_a_finder_file() {
        let tmp = finder_tracked_repo();
        let root = tmp.path();
        let modify = || std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        // By the name status lists, the index entry's own.
        let requested = [NFC_FILE.to_string()];
        modify();
        stage(root, &requested).unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.staged), [NFC_FILE], "{status:?}");
        unstage(root, &requested).unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert!(status.staged.is_empty(), "{status:?}");
        assert_eq!(paths(&status.unstaged), [NFC_FILE], "{status:?}");
        discard(root, &requested, &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "a\nb\nc\n"
        );
        assert_eq!(index_names(root), [NFC_FILE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unstage_restores_a_staged_decomposed_entry() {
        let tmp = nfd_staged_repo();
        let root = tmp.path();
        unstage(root, &[NFD_FILE.to_string()]).unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert!(status.staged.is_empty(), "{status:?}");
        assert_eq!(paths(&status.unstaged), [NFD_FILE], "{status:?}");
        assert_eq!(index_names(root), [NFD_FILE]);
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "a\nB\nc\n"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unstage_of_a_rename_restores_its_decomposed_source_from_head() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        git_as_stored(root, &["mv", "--", NFD_FILE, "plain.md"]);
        assert!(!index_names(root).iter().any(|name| name == NFD_FILE));
        unstage(root, &["plain.md".to_string(), NFD_FILE.to_string()]).unwrap();
        // The source is an index entry again, under its own name and only that.
        assert_eq!(index_names(root), [NFD_FILE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unstage_on_an_unborn_branch_drops_a_decomposed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "true");
        std::fs::write(root.join(NFD_FILE), "a\n").unwrap();
        git_as_stored(root, &["add", NFD_FILE]);
        assert_eq!(index_names(root), [NFD_FILE]);
        unstage(root, &[NFD_FILE.to_string()]).unwrap();
        assert!(index_names(root).is_empty());
        assert!(root.join(NFD_FILE).exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn discard_restores_a_modified_decomposed_entry() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        discard(root, &[NFD_FILE.to_string()], &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "a\nb\nc\n"
        );
        assert_eq!(index_names(root), [NFD_FILE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_of_a_pathspec_lists_a_decomposed_entry_as_modified() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        // Alone, or beside a plain path.
        for spec in [vec![NFD_FILE], vec![NFD_FILE, "plain.md"]] {
            let spec = spec.into_iter().map(String::from).collect();
            let status = status_of(root, Some(spec)).unwrap().parsed;
            assert_eq!(paths(&status.unstaged), [NFD_FILE], "{status:?}");
            assert!(status.untracked.is_empty(), "{status:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_of_a_pathspec_keeps_working_for_finder_names() {
        let tmp = finder_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        std::fs::write(root.join("ume\u{301}.md"), "new\n").unwrap();
        // The listed name and the directory's spelling reach the NFC entry, and
        // the NFC name status lists reaches the new file.
        for name in [NFC_FILE, NFD_FILE] {
            let status = status_of(root, Some(vec![name.to_string()]))
                .unwrap()
                .parsed;
            assert_eq!(paths(&status.unstaged), [NFC_FILE], "{name}: {status:?}");
        }
        let status = status_of(root, Some(vec!["um\u{e9}.md".to_string()]))
            .unwrap()
            .parsed;
        assert_eq!(status.untracked, ["um\u{e9}.md"], "{status:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_of_a_pathspec_with_a_new_finder_file_beside_a_decomposed_entry_lists_the_file() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join("ume\u{301}.md"), "new\n").unwrap();
        let spec = vec![NFD_FILE.to_string(), "um\u{e9}.md".to_string()];
        let status = status_of(root, Some(spec)).unwrap().parsed;
        assert!(
            status.untracked.iter().any(|name| name == "um\u{e9}.md"),
            "{status:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn discard_brings_back_a_deleted_decomposed_entry() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::remove_file(root.join(NFD_FILE)).unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.unstaged), [NFD_FILE], "{status:?}");
        discard(root, &[NFD_FILE.to_string()], &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "a\nb\nc\n"
        );
        assert_eq!(index_names(root), [NFD_FILE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_records_the_deletion_of_a_decomposed_entry() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::remove_file(root.join(NFD_FILE)).unwrap();
        stage(root, &[NFD_FILE.to_string()]).unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.staged), [NFD_FILE], "{status:?}");
        assert!(index_names(root).is_empty());
    }

    #[cfg(target_os = "macos")]
    fn rollback_target(path: &str, classification: GitRollbackClassification) -> GitRollbackTarget {
        GitRollbackTarget {
            path: path.to_string(),
            classification,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_paths_restores_a_modified_and_unstages_an_added_decomposed_entry() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        let added = "ume\u{301}.md";
        std::fs::write(root.join(added), "new\n").unwrap();
        git_as_stored(root, &["add", added]);
        let targets = [
            rollback_target(
                NFD_FILE,
                GitRollbackClassification::Tracked {
                    staged_status: None,
                    unstaged_status: Some("M".into()),
                    orig_path: None,
                },
            ),
            rollback_target(
                added,
                GitRollbackClassification::Added {
                    staged_status: Some("A".into()),
                    unstaged_status: None,
                },
            ),
        ];
        let result = rollback_paths(root, &targets, true).unwrap();
        assert_eq!(result.restored, [NFD_FILE]);
        assert_eq!(result.deleted, [added]);
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "a\nb\nc\n"
        );
        assert!(!root.join(added).exists());
        assert_eq!(index_names(root), [NFD_FILE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_paths_restores_a_renamed_decomposed_source() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        git_as_stored(root, &["mv", "--", NFD_FILE, "plain.md"]);
        let target = rollback_target(
            "plain.md",
            GitRollbackClassification::Tracked {
                staged_status: Some("R".into()),
                unstaged_status: None,
                orig_path: Some(NFD_FILE.to_string()),
            },
        );
        let result = rollback_paths(root, &[target], false).unwrap();
        assert_eq!(result.restored, ["plain.md"]);
        assert_eq!(index_names(root), [NFD_FILE]);
        assert!(root.join(NFD_FILE).exists());
        assert!(!root.join("plain.md").exists());
    }

    // The twin. Where git precomposes names, a tracked file stored under a
    // decomposed name is listed twice: as its index entry and, precomposed, as an
    // "untracked" file the same directory entry reads as. The volume knows both
    // spellings as one file, so the second row is not a file of its own and acting
    // on it (clean, add, discard, rollback) would hit the tracked file.

    #[cfg(target_os = "macos")]
    const NFD_NESTED: &str = "de\u{301}/in.md";
    #[cfg(target_os = "macos")]
    const NFC_NESTED: &str = "d\u{e9}/in.md";

    /// `nfd_tracked_repo` plus `NFD_NESTED`, tracked under a decomposed folder name.
    #[cfg(target_os = "macos")]
    fn nfd_nested_tracked_repo() -> tempfile::TempDir {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        set_precompose(root, "false");
        std::fs::create_dir(root.join("de\u{301}")).unwrap();
        std::fs::write(root.join(NFD_NESTED), "n\n").unwrap();
        test_repo::git(root, &["add", NFD_NESTED]);
        test_repo::git(root, &["commit", "-m", "nested"]);
        set_precompose(root, "true");
        assert_eq!(index_names(root), [NFD_FILE, NFD_NESTED]);
        tmp
    }

    /// The rows the Git panel offers for "all": every changed path under the
    /// name status lists, tracked or untracked, as the rollback dialog classifies it.
    #[cfg(target_os = "macos")]
    fn every_listed_target(parsed: &crate::git_status::ParsedStatus) -> Vec<GitRollbackTarget> {
        let mut listed: Vec<&str> = parsed
            .staged
            .iter()
            .chain(&parsed.unstaged)
            .map(|entry| entry.path.as_str())
            .chain(parsed.untracked.iter().map(String::as_str))
            .collect();
        listed.dedup();
        listed
            .into_iter()
            .map(|path| {
                let classification = actual_rollback_classification(parsed, path)
                    .unwrap()
                    .unwrap();
                rollback_target(path, classification)
            })
            .collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_lists_no_twin_of_a_tracked_decomposed_file() {
        let tmp = nfd_nested_tracked_repo();
        let root = tmp.path();
        // Unchanged, the files are no untracked ones either.
        let status = status_of(root, None).unwrap().parsed;
        assert!(status.untracked.is_empty(), "{status:?}");
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        std::fs::write(root.join(NFD_NESTED), "N\n").unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(
            paths(&status.unstaged),
            [NFD_FILE, NFD_NESTED],
            "{status:?}"
        );
        assert!(status.untracked.is_empty(), "{status:?}");
        // The same when scoped, by the name as the index stores it (asked as
        // stored) and by the precomposed name git matches the directory entry by.
        for spec in [
            vec![NFD_FILE],
            vec![NFD_NESTED],
            vec![NFD_NESTED, NFD_FILE],
            vec![NFC_FILE],
            vec![NFC_NESTED],
            vec![NFC_NESTED, NFC_FILE],
        ] {
            let status = status_of(root, Some(spec.iter().map(|s| s.to_string()).collect()))
                .unwrap()
                .parsed;
            assert!(status.untracked.is_empty(), "{spec:?}: {status:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_still_lists_a_new_finder_file_beside_a_decomposed_entry_once() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join("ume\u{301}.md"), "new\n").unwrap();
        let listed = ["um\u{e9}.md".to_string()];
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(status.untracked, listed, "{status:?}");
        for spec in [vec!["um\u{e9}.md", NFC_FILE], vec!["um\u{e9}.md", NFD_FILE]] {
            let spec = spec.into_iter().map(String::from).collect();
            let status = status_of(root, Some(spec)).unwrap().parsed;
            assert_eq!(status.untracked, listed, "{status:?}");
        }
        // It stages once, under the name status lists.
        stage(root, &listed).unwrap();
        assert_eq!(index_names(root), [NFD_FILE, "um\u{e9}.md"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_lists_a_finder_file_with_a_precomposed_entry_as_before() {
        let tmp = finder_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        std::fs::write(root.join("ume\u{301}.md"), "new\n").unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.unstaged), [NFC_FILE], "{status:?}");
        assert_eq!(status.untracked, ["um\u{e9}.md"], "{status:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn discard_all_of_a_decomposed_entry_keeps_the_file_and_leaves_a_clean_status() {
        let tmp = nfd_nested_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        std::fs::write(root.join(NFD_NESTED), "N\n").unwrap();
        // "Discard all", as the panel sends it: every tracked row, every untracked row.
        let status = status_of(root, None).unwrap().parsed;
        let tracked: Vec<String> = status.unstaged.iter().map(|e| e.path.clone()).collect();
        discard(root, &tracked, &status.untracked).unwrap();
        for (name, content) in [(NFD_FILE, "a\nb\nc\n"), (NFD_NESTED, "n\n")] {
            assert_eq!(
                std::fs::read_to_string(root.join(name)).unwrap(),
                content,
                "{name}"
            );
        }
        assert_eq!(index_names(root), [NFD_FILE, NFD_NESTED]);
        let status = status_of(root, None).unwrap().parsed;
        assert!(
            status.staged.is_empty() && status.unstaged.is_empty() && status.untracked.is_empty(),
            "{status:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn discard_refuses_the_twin_of_a_tracked_decomposed_file_and_deletes_nothing() {
        let tmp = nfd_nested_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "local edit\n").unwrap();
        std::fs::write(root.join(NFD_NESTED), "local nested\n").unwrap();
        // A list from before the twin was hidden: the twin alone, or with its entry.
        for (tracked, untracked) in [
            (vec![], vec![NFC_FILE]),
            (vec![], vec![NFC_NESTED]),
            (vec![NFD_FILE], vec![NFC_FILE]),
        ] {
            let tracked: Vec<String> = tracked.into_iter().map(String::from).collect();
            let untracked: Vec<String> = untracked.into_iter().map(String::from).collect();
            let error = discard(root, &tracked, &untracked).unwrap_err();
            assert!(error.contains("refused"), "{untracked:?}: {error}");
            // Nothing happened, not even the restore of the tracked part.
            for (name, content) in [(NFD_FILE, "local edit\n"), (NFD_NESTED, "local nested\n")] {
                assert_eq!(
                    std::fs::read_to_string(root.join(name)).unwrap(),
                    content,
                    "{untracked:?}: {name}"
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_of_every_listed_row_keeps_a_decomposed_file() {
        let tmp = nfd_nested_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        std::fs::write(root.join(NFD_NESTED), "N\n").unwrap();
        let status = status_of(root, None).unwrap().parsed;
        let targets = every_listed_target(&status);
        let result = rollback_paths(root, &targets, true).unwrap();
        assert_eq!(result.restored, [NFD_FILE, NFD_NESTED]);
        assert!(result.deleted.is_empty(), "{result:?}");
        for (name, content) in [(NFD_FILE, "a\nb\nc\n"), (NFD_NESTED, "n\n")] {
            assert_eq!(
                std::fs::read_to_string(root.join(name)).unwrap(),
                content,
                "{name}"
            );
        }
        let status = status_of(root, None).unwrap().parsed;
        assert!(
            status.staged.is_empty() && status.unstaged.is_empty() && status.untracked.is_empty(),
            "{status:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_of_the_twin_of_a_decomposed_file_is_refused_and_deletes_nothing() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "local edit\n").unwrap();
        let twin = rollback_target(NFC_FILE, GitRollbackClassification::Untracked);
        assert!(rollback_paths(root, &[twin], true).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "local edit\n"
        );
        assert_eq!(index_names(root), [NFD_FILE]);
    }

    /// `nfd_tracked_repo` changed and added under its precomposed name too: the
    /// second index entry beside the NFD one that "stage all" used to create.
    #[cfg(target_os = "macos")]
    fn nfd_tracked_with_a_second_entry_repo() -> tempfile::TempDir {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "local edit\n").unwrap();
        test_repo::git(root, &["add", NFC_FILE]);
        assert_eq!(index_names(root), [NFD_FILE, NFC_FILE]);
        tmp
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_that_deletes_an_added_second_entry_of_a_tracked_file_is_refused_first() {
        let tmp = nfd_tracked_with_a_second_entry_repo();
        let root = tmp.path();
        let status = status_of(root, None).unwrap().parsed;
        let added = actual_rollback_classification(&status, NFC_FILE)
            .unwrap()
            .unwrap();
        assert!(matches!(added, GitRollbackClassification::Added { .. }));
        let target = rollback_target(NFC_FILE, added);
        let error = rollback_paths(root, &[target], true).unwrap_err();
        assert!(error.contains("refused"), "{error}");
        // Neither the index nor the file was touched.
        assert_eq!(index_names(root), [NFD_FILE, NFC_FILE]);
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "local edit\n"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_that_keeps_files_can_still_unstage_a_second_entry() {
        let tmp = nfd_tracked_with_a_second_entry_repo();
        let root = tmp.path();
        let status = status_of(root, None).unwrap().parsed;
        let added = actual_rollback_classification(&status, NFC_FILE)
            .unwrap()
            .unwrap();
        let result = rollback_paths(root, &[rollback_target(NFC_FILE, added)], false).unwrap();
        assert_eq!(result.preserved_untracked, [NFC_FILE]);
        // Only the duplicate entry went; the tracked file and its entry stay.
        assert_eq!(index_names(root), [NFD_FILE]);
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "local edit\n"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_all_with_the_twin_row_adds_no_second_entry() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        // The rows as the panel lists them now: the entry alone.
        let status = status_of(root, None).unwrap().parsed;
        let listed: Vec<String> = status
            .unstaged
            .iter()
            .map(|entry| entry.path.clone())
            .chain(status.untracked.iter().cloned())
            .collect();
        stage(root, &listed).unwrap();
        assert_eq!(index_names(root), [NFD_FILE]);
        // A list that still carries the twin row is refused before anything is added.
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        for requested in [vec![NFD_FILE, NFC_FILE], vec![NFC_FILE]] {
            let requested: Vec<String> = requested.into_iter().map(String::from).collect();
            let error = stage(root, &requested).unwrap_err();
            assert!(error.contains("refused"), "{requested:?}: {error}");
            assert_eq!(index_names(root), [NFD_FILE], "{requested:?}");
            let status = status_of(root, None).unwrap().parsed;
            assert!(status.staged.is_empty(), "{status:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_by_the_disk_spelling_still_adds_a_new_finder_file_once() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join("ne\u{301}w.md"), "n\n").unwrap();
        stage(root, &["ne\u{301}w.md".to_string()]).unwrap();
        assert_eq!(index_names(root), [NFD_FILE, "n\u{e9}w.md"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_unstage_and_discard_by_the_disk_spelling_keep_working_for_a_finder_entry() {
        let tmp = finder_tracked_repo();
        let root = tmp.path();
        // The directory spells the name NFD; git takes it to the precomposed entry.
        let requested = [NFD_FILE.to_string()];
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        stage(root, &requested).unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.staged), [NFC_FILE], "{status:?}");
        unstage(root, &requested).unwrap();
        discard(root, &requested, &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(NFD_FILE)).unwrap(),
            "a\nb\nc\n"
        );
        assert_eq!(index_names(root), [NFC_FILE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_takes_a_mixed_spelling_of_a_finder_entry_without_a_second_entry() {
        // Neither NFC nor NFD: git precomposes it onto the NFC entry, which is no twin.
        let mixed = "cafe\u{301}-\u{f1}.md";
        let entry = "caf\u{e9}-\u{f1}.md";
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "true");
        std::fs::write(root.join(mixed), "a\n").unwrap();
        test_repo::git(root, &["add", entry]);
        test_repo::git(root, &["commit", "-m", "base"]);
        assert_eq!(index_names(root), [entry]);
        std::fs::write(root.join(mixed), "b\n").unwrap();
        stage(root, &[mixed.to_string()]).unwrap();
        assert_eq!(index_names(root), [entry]);
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.staged), [entry], "{status:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_of_an_entry_stored_in_a_mixed_form_is_no_twin_of_itself() {
        // Neither NFC nor NFD, stored as it is: the request is the entry.
        let mixed = "cafe\u{301}-\u{f1}.md";
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "false");
        std::fs::write(root.join(mixed), "a\n").unwrap();
        test_repo::git(root, &["add", mixed]);
        test_repo::git(root, &["commit", "-m", "base"]);
        set_precompose(root, "true");
        assert_eq!(index_names(root), [mixed]);
        std::fs::write(root.join(mixed), "b\n").unwrap();
        stage(root, &[mixed.to_string()]).unwrap();
        assert_eq!(index_names(root), [mixed]);
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.staged), [mixed], "{status:?}");
        assert!(status.untracked.is_empty(), "{status:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scoped_status_with_an_ascii_directory_and_a_decomposed_name_lists_no_phantom() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        std::fs::create_dir(root.join("sub")).unwrap();
        // A Finder file in `sub` whose index entry is precomposed, and a new one.
        std::fs::write(root.join("sub/ne\u{301}e.md"), "x\n").unwrap();
        test_repo::git(root, &["add", "sub/n\u{e9}e.md"]);
        test_repo::git(root, &["commit", "-m", "finder"]);
        std::fs::write(root.join("sub/fre\u{301}sh.md"), "y\n").unwrap();
        std::fs::write(root.join(NFD_FILE), "a\nB\nc\n").unwrap();
        let full = status_of(root, None).unwrap().parsed;
        let in_sub = |parsed: &crate::git_status::ParsedStatus| -> Vec<String> {
            parsed
                .untracked
                .iter()
                .chain(parsed.unstaged.iter().map(|entry| &entry.path))
                .filter(|path| path.starts_with("sub/"))
                .cloned()
                .collect()
        };
        for spec in [vec!["sub", NFD_FILE], vec![NFD_FILE, "sub"]] {
            let spec = spec.into_iter().map(String::from).collect();
            let scoped = status_of(root, Some(spec)).unwrap().parsed;
            assert_eq!(in_sub(&scoped), in_sub(&full), "{scoped:?}");
            // A phantom is a decomposed name no entry holds listed untracked.
            assert!(
                scoped
                    .untracked
                    .iter()
                    .all(|path| unicode_normalization::is_nfc(path)),
                "{scoped:?}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scoped_status_of_a_decomposed_folder_lists_no_phantom_row() {
        // `de\u{301}/` is a folder of HEAD's tree under its decomposed name, so
        // the spec is a stored name that is no file.
        let tmp = nfd_nested_tracked_repo();
        let root = tmp.path();
        std::fs::write(root.join(NFD_NESTED), "N\n").unwrap();
        // New files Finder stored decomposed: in the folder, and beside it.
        std::fs::write(root.join("de\u{301}/ne\u{301}w.md"), "n\n").unwrap();
        std::fs::write(root.join("de\u{301}/plain.md"), "p\n").unwrap();
        std::fs::write(root.join("fre\u{301}sh.md"), "f\n").unwrap();
        std::fs::write(root.join("plain.txt"), "t\n").unwrap();
        let full = status_of(root, None).unwrap().parsed;
        let in_folder = ["d\u{e9}/n\u{e9}w.md", "d\u{e9}/plain.md"];
        let mut every = in_folder.to_vec();
        every.extend(["fr\u{e9}sh.md", "plain.txt"]);
        assert_eq!(full.untracked, every, "{full:?}");
        let folder = "de\u{301}";
        for (spec, untracked) in [
            (vec![folder], in_folder.to_vec()),
            (
                vec![folder, "plain.txt"],
                vec![in_folder[0], in_folder[1], "plain.txt"],
            ),
            (vec![folder, NFD_FILE], in_folder.to_vec()),
            (vec!["d\u{e9}"], in_folder.to_vec()),
        ] {
            let owned = spec.iter().map(|path| path.to_string()).collect();
            let scoped = status_of(root, Some(owned)).unwrap().parsed;
            // The untracked rows are the full status's, not the directory's
            // decomposed spelling of them.
            assert_eq!(scoped.untracked, untracked, "{spec:?}: {scoped:?}");
            // Every tracked row is one the full status has too (an entry under a
            // decomposed folder spec can be missing, as it is when git precomposes).
            for entry in scoped.staged.iter().chain(&scoped.unstaged) {
                assert!(
                    full.staged
                        .iter()
                        .chain(&full.unstaged)
                        .any(|row| row.path == entry.path),
                    "{spec:?}: {scoped:?}"
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tracked_twins_asks_the_index_only_for_a_name_that_can_be_spelled_another_way() {
        let tmp = nfd_nested_tracked_repo();
        let root = tmp.path();
        // A corrupt index fails every listing, which tells a listing from none.
        std::fs::write(root.join(".git/index"), "not an index").unwrap();
        // ASCII and plain CJK ideographs cannot have a twin (they are NFC and
        // NFD both): no process runs, so nothing fails.
        let free = ["a.md", "sub/b.txt", "\u{65e5}\u{672c}.md"];
        assert!(tracked_twins(root, &free, false).unwrap().is_empty());
        assert!(tracked_twins(root, &[], false).unwrap().is_empty());
        // Anything else asks: Hiragana with a voiced mark, Hangul syllables,
        // precomposed Latin, and a decomposed name too, which git may keep as it
        // is (a canonical NFD spelling it composes to nothing) beside an entry
        // that is precomposed.
        for name in [
            "\u{304c}.md",
            "\u{d55c}.md",
            NFC_FILE,
            NFC_NESTED,
            NFD_FILE,
            NFD_NESTED,
            "a\u{330}\u{301}.txt",
        ] {
            assert!(tracked_twins(root, &[name], false).is_err(), "{name}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tracked_twins_finds_the_entry_whatever_the_folder_and_file_spelling() {
        let tmp = nfd_nested_tracked_repo();
        let root = tmp.path();
        let twins = tracked_twins(
            root,
            &[NFC_FILE, NFC_NESTED, "um\u{e9}.md", "plain.md"],
            false,
        )
        .unwrap();
        assert_eq!(
            twins,
            [
                (NFC_FILE, NFD_FILE.to_string()),
                (NFC_NESTED, NFD_NESTED.to_string())
            ]
        );
        // A new file has no entry to be the twin of; an entry that is gone from
        // the directory leaves nothing to be one either.
        std::fs::remove_file(root.join(NFD_FILE)).unwrap();
        assert!(tracked_twins(root, &[NFC_FILE], false).unwrap().is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tracked_twins_can_leave_out_a_name_that_is_an_entry_itself() {
        // Two entries for the one file: each is the twin of the other, and an
        // entry itself.
        let tmp = colliding_entries_repo("A\n", "B\n", "C\n");
        let root = tmp.path();
        let both = [NFC_FILE, NFD_FILE];
        let twins = tracked_twins(root, &both, false).unwrap();
        assert_eq!(
            twins,
            [
                (NFC_FILE, NFD_FILE.to_string()),
                (NFD_FILE, NFC_FILE.to_string())
            ]
        );
        for path in both {
            assert!(tracked_twins(root, &[path], true).unwrap().is_empty());
        }
        // A name that is no entry is a twin however it is asked.
        let tmp = nfd_tracked_repo();
        let twins = tracked_twins(tmp.path(), &[NFC_FILE], true).unwrap();
        assert_eq!(twins, [(NFC_FILE, NFD_FILE.to_string())]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn is_same_file_compares_device_and_inode_not_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join(NFD_FILE), "one\n").unwrap();
        std::fs::write(root.join("other.md"), "one\n").unwrap();
        // The volume reads both spellings as one directory entry.
        assert!(is_same_file(&root.join(NFD_FILE), &root.join(NFC_FILE)));
        assert!(is_same_file(&root.join("other.md"), &root.join("other.md")));
        std::fs::hard_link(root.join("other.md"), root.join("linked.md")).unwrap();
        assert!(is_same_file(
            &root.join("other.md"),
            &root.join("linked.md")
        ));
        // Equal content and size are another file; a missing one is no file.
        assert!(!is_same_file(&root.join(NFD_FILE), &root.join("other.md")));
        assert!(!is_same_file(&root.join("none.md"), &root.join("none.md")));
    }

    #[cfg(target_os = "macos")]
    fn status_rows(
        staged: &[(&str, &str)],
        unstaged: &[(&str, &str)],
        untracked: &[&str],
    ) -> crate::git_status::ParsedStatus {
        let entries = |rows: &[(&str, &str)]| {
            rows.iter()
                .map(|(path, status)| crate::git_status::GitFileEntry {
                    path: path.to_string(),
                    orig_path: None,
                    status: status.to_string(),
                })
                .collect()
        };
        crate::git_status::ParsedStatus {
            head_oid: "head".into(),
            staged: entries(staged),
            unstaged: entries(unstaged),
            untracked: untracked.iter().map(|path| path.to_string()).collect(),
            ..Default::default()
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scope_check_rejects_the_tracked_file_that_a_request_for_its_twin_deleted() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        let none = HashSet::new();
        let check = |requested: &str, before, after| {
            ensure_literal_path_scope(
                root,
                "clean",
                &[requested.to_string()],
                &none,
                before,
                after,
            )
        };
        // `clean` of the twin took the decomposed tracked file with it.
        let before = status_rows(&[], &[(NFD_FILE, "M")], &[]);
        let after = status_rows(&[], &[(NFD_FILE, "D")], &[]);
        let error = check(NFC_FILE, &before, &after).unwrap_err();
        assert!(error.contains(NFD_FILE), "{error}");
        // Alone, with nothing listed before: the same file, gone.
        let before = status_rows(&[], &[], &[]);
        assert!(check(NFC_FILE, &before, &after).is_err());
        // The twin whose spelling git keeps is no other name for git either: the
        // entry is precomposed, git takes the request to itself, not to the entry.
        let (entry, twin) = KEPT_SPELLINGS[0];
        let before = status_rows(&[], &[(entry, "M")], &[]);
        let after = status_rows(&[], &[(entry, "D")], &[]);
        let error = check(twin, &before, &after).unwrap_err();
        assert!(error.contains(entry), "{error}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scope_check_accepts_the_row_of_the_name_git_took_a_request_to() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        let none = HashSet::new();
        let stored = HashSet::from([NFD_FILE.to_string()]);
        let request = [NFD_FILE.to_string()];
        // Unstaging a decomposed entry (it went as stored) leaves the file
        // untracked, listed precomposed.
        let before = status_rows(&[(NFD_FILE, "A")], &[], &[]);
        let after = status_rows(&[], &[], &[NFC_FILE]);
        ensure_literal_path_scope(root, "unstage", &request, &stored, &before, &after).unwrap();
        // Staging one (git took it to the precomposed name): the entry it adds.
        let before = status_rows(&[], &[], &[NFC_FILE]);
        let after = status_rows(&[(NFC_FILE, "A")], &[], &[]);
        ensure_literal_path_scope(root, "add", &request, &none, &before, &after).unwrap();
        // A precomposed entry staged under a decomposed name: its own row changes.
        let before = status_rows(&[], &[(NFC_FILE, "M")], &[]);
        let after = status_rows(&[(NFC_FILE, "M")], &[], &[]);
        ensure_literal_path_scope(root, "add", &request, &none, &before, &after).unwrap();
        // Another file's row is still outside the request.
        let before = status_rows(&[], &[], &[NFC_FILE]);
        let after = status_rows(&[(NFC_FILE, "A"), ("other.md", "A")], &[], &[]);
        assert!(ensure_literal_path_scope(root, "add", &request, &none, &before, &after).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scope_check_does_not_let_a_request_sent_as_stored_change_another_entry() {
        // Two entries for the one file: the decomposed one was restored as
        // stored, and the precomposed entry's row, clean before, now differs.
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        let request = [NFD_FILE.to_string()];
        let before = status_rows(&[], &[(NFD_FILE, "M")], &[]);
        let after = status_rows(&[], &[(NFC_FILE, "M")], &[]);
        let stored = HashSet::from([NFD_FILE.to_string()]);
        let error = ensure_literal_path_scope(root, "restore", &request, &stored, &before, &after)
            .unwrap_err();
        assert!(error.contains(NFC_FILE), "{error}");
        // Sent for git to respell, the same change is the request's own row.
        let none = HashSet::new();
        ensure_literal_path_scope(root, "restore", &request, &none, &before, &after).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scope_check_reports_another_entry_that_a_request_sent_as_stored_turned_clean() {
        // Two entries for the one file, both modified: restoring the decomposed
        // one as stored rewrites the file, and the precomposed entry's row is gone.
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        let request = [NFD_FILE.to_string()];
        let stored = HashSet::from([NFD_FILE.to_string()]);
        let before = status_rows(&[], &[(NFC_FILE, "M"), (NFD_FILE, "M")], &[]);
        let after = status_rows(&[], &[], &[]);
        let error = ensure_literal_path_scope(root, "restore", &request, &stored, &before, &after)
            .unwrap_err();
        assert!(error.contains(NFC_FILE), "{error}");
        // The same when the row turns untracked instead of going: it was a tracked
        // entry's before.
        let before = status_rows(&[(NFC_FILE, "A")], &[(NFD_FILE, "M")], &[]);
        let after = status_rows(&[], &[(NFD_FILE, "M")], &[NFC_FILE]);
        assert!(
            ensure_literal_path_scope(root, "restore", &request, &stored, &before, &after).is_err()
        );
        // Sent for git to respell, a row of the name git took the request to is the request's own.
        let none = HashSet::new();
        let before = status_rows(&[], &[(NFC_FILE, "M")], &[]);
        let after = status_rows(&[], &[], &[]);
        ensure_literal_path_scope(root, "restore", &request, &none, &before, &after).unwrap();
    }

    // The twin of a name git does not respell. Git turns a decomposed argument
    // into NFC but leaves a character NFC rewrites on its own (a singleton such
    // as U+212A KELVIN SIGN or U+2126 OHM SIGN, a CJK compatibility ideograph)
    // as it is, while the volume reads it as the plain character: the file is
    // another spelling of its tracked entry, yet the NFC form of that spelling
    // is not what git takes it to.

    /// `(index entry, the spelling the directory entry carries)`.
    #[cfg(target_os = "macos")]
    const SINGLETON_TWINS: [(&str, &str); 3] = [
        ("K.txt", "\u{212a}.txt"),
        ("\u{3a9}.txt", "\u{2126}.txt"),
        ("\u{8c48}.txt", "\u{f900}.txt"),
    ];

    /// `entry` tracked and modified, its directory entry recreated under
    /// `spelling` the way a tool that saves by replacing the file does.
    #[cfg(target_os = "macos")]
    fn singleton_twin_repo(entry: &str, spelling: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "true");
        std::fs::write(root.join(entry), "base\n").unwrap();
        std::fs::write(root.join("other.txt"), "o\n").unwrap();
        test_repo::git(root, &["add", entry, "other.txt"]);
        test_repo::git(root, &["commit", "-m", "base"]);
        std::fs::remove_file(root.join(entry)).unwrap();
        std::fs::write(root.join(spelling), "local edit\n").unwrap();
        // One file under both names, as on the volumes that fold them.
        assert_eq!(
            std::fs::read_to_string(root.join(entry)).unwrap(),
            "local edit\n"
        );
        tmp
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_lists_no_twin_of_a_tracked_file_spelled_with_a_singleton_character() {
        for (entry, spelling) in SINGLETON_TWINS {
            let tmp = singleton_twin_repo(entry, spelling);
            let root = tmp.path();
            let status = status_of(root, None).unwrap().parsed;
            assert_eq!(paths(&status.unstaged), [entry], "{entry}: {status:?}");
            assert!(status.untracked.is_empty(), "{entry}: {status:?}");
            for spec in [entry, spelling] {
                let spec = vec![spec.to_string()];
                let status = status_of(root, Some(spec.clone())).unwrap().parsed;
                assert!(status.untracked.is_empty(), "{spec:?}: {status:?}");
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tracked_twins_finds_the_entry_of_a_name_spelled_with_a_singleton_character() {
        for (entry, spelling) in SINGLETON_TWINS {
            let tmp = singleton_twin_repo(entry, spelling);
            let root = tmp.path();
            let twins = tracked_twins(root, &[spelling, "other.txt", entry], false).unwrap();
            assert_eq!(twins, [(spelling, entry.to_string())], "{entry}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn discard_stage_and_rollback_refuse_the_twin_spelled_with_a_singleton_character() {
        for (entry, spelling) in SINGLETON_TWINS {
            let tmp = singleton_twin_repo(entry, spelling);
            let root = tmp.path();
            let entries = index_names(root);
            let intact = |what: &str| {
                assert_eq!(index_names(root), entries, "{entry}: {what}");
                assert_eq!(
                    std::fs::read_to_string(root.join(entry)).unwrap(),
                    "local edit\n",
                    "{entry}: {what}"
                );
                let status = status_of(root, None).unwrap().parsed;
                assert!(status.staged.is_empty(), "{entry}: {what}: {status:?}");
            };
            let tracked = [entry.to_string()];
            let twin = [spelling.to_string()];
            // Discard of the twin alone, or after its entry (the "discard all" list).
            for (restore, clean) in [(&[][..], &twin), (&tracked[..], &twin)] {
                let error = discard(root, restore, clean).unwrap_err();
                assert!(error.contains("refused"), "{entry}: {error}");
                intact("discard");
            }
            // Stage of the twin alone, or with its entry.
            for requested in [
                vec![spelling.to_string()],
                vec![entry.to_string(), spelling.to_string()],
            ] {
                let error = stage(root, &requested).unwrap_err();
                assert!(error.contains("refused"), "{entry}: {error}");
                intact("stage");
            }
            // A list that still carries the twin as an untracked row.
            let stale = rollback_target(spelling, GitRollbackClassification::Untracked);
            assert!(rollback_paths(root, &[stale], true).is_err(), "{entry}");
            intact("rollback");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scope_check_rejects_the_tracked_file_that_a_request_for_its_singleton_twin_deleted() {
        for (entry, spelling) in SINGLETON_TWINS {
            // `clean` of the twin took the tracked file under its other spelling with it.
            let tmp = singleton_twin_repo(entry, spelling);
            let before = status_rows(&[], &[(entry, "M")], &[]);
            let after = status_rows(&[], &[(entry, "D")], &[]);
            let error = ensure_literal_path_scope(
                tmp.path(),
                "clean",
                &[spelling.to_string()],
                &HashSet::new(),
                &before,
                &after,
            )
            .unwrap_err();
            assert!(error.contains(entry), "{error}");
        }
    }

    // Names git does not compose the way NFC does. Git turns an argument or a
    // directory entry into its precomposed form pair by pair (macOS iconv
    // UTF-8-MAC): it does not reorder marks and it does not compose past a mark
    // that does not combine. So the canonical NFD name a Cocoa save leaves,
    // `a` U+0330 U+0301, is listed as it is, while its NFC form is U+00E1 U+0330.
    // A tracked entry committed in that NFC form (on Linux or Windows) is then the
    // file of a name that is NFD, and `ẹ́` (U+1EB9 U+0301), whose disk name git
    // composes only to U+00E9 U+0323, the file of a name that is neither NFC
    // nor NFD. Git takes neither spelling to the entry.

    /// `(index entry as git lists it, the spelling of the directory entry)`.
    #[cfg(target_os = "macos")]
    const KEPT_SPELLINGS: [(&str, &str); 2] = [
        ("\u{e1}\u{330}.txt", "a\u{330}\u{301}.txt"),
        ("\u{1eb9}\u{301}.txt", "\u{e9}\u{323}.txt"),
    ];

    /// `entry` tracked, its directory entry recreated under `disk` (the way a
    /// save that replaces the file does) with `content`: one file under both names.
    #[cfg(target_os = "macos")]
    fn kept_spelling_repo(entry: &str, disk: &str, content: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "true");
        std::fs::write(root.join(entry), "base\n").unwrap();
        std::fs::write(root.join("other.txt"), "o\n").unwrap();
        test_repo::git(root, &["add", "--", entry, "other.txt"]);
        test_repo::git(root, &["commit", "-m", "base"]);
        let mut entries = vec![entry.to_string(), "other.txt".to_string()];
        entries.sort();
        assert_eq!(index_names(root), entries);
        std::fs::remove_file(root.join(entry)).unwrap();
        std::fs::write(root.join(disk), content).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(entry)).unwrap(),
            content,
            "{entry}"
        );
        tmp
    }

    #[cfg(target_os = "macos")]
    fn is_clean(root: &Path) -> bool {
        let status = status_of(root, None).unwrap().parsed;
        status.staged.is_empty() && status.unstaged.is_empty() && status.untracked.is_empty()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_lists_no_twin_of_an_entry_whose_disk_spelling_git_keeps() {
        for (entry, disk) in KEPT_SPELLINGS {
            let tmp = kept_spelling_repo(entry, disk, "local edit\n");
            let root = tmp.path();
            let status = status_of(root, None).unwrap().parsed;
            assert_eq!(paths(&status.unstaged), [entry], "{entry}: {status:?}");
            assert!(status.untracked.is_empty(), "{entry}: {status:?}");
            for spec in [entry, disk] {
                let spec = vec![spec.to_string()];
                let status = status_of(root, Some(spec.clone())).unwrap().parsed;
                assert!(status.untracked.is_empty(), "{entry}: {spec:?}: {status:?}");
            }
            // Unchanged, the file is no untracked one either.
            let tmp = kept_spelling_repo(entry, disk, "base\n");
            assert!(is_clean(tmp.path()), "{entry}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn discard_all_of_an_entry_whose_disk_spelling_git_keeps_restores_it_and_leaves_a_clean_status()
    {
        for (entry, disk) in KEPT_SPELLINGS {
            let tmp = kept_spelling_repo(entry, disk, "local edit\n");
            let root = tmp.path();
            // "Discard all", as the panel sends it: every tracked row, every untracked row.
            let status = status_of(root, None).unwrap().parsed;
            let tracked: Vec<String> = status.unstaged.iter().map(|e| e.path.clone()).collect();
            discard(root, &tracked, &status.untracked).unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join(entry)).unwrap(),
                "base\n",
                "{entry}"
            );
            assert!(is_clean(root), "{entry}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_and_stage_of_every_row_of_an_entry_whose_disk_spelling_git_keeps_touch_one_file() {
        for (entry, disk) in KEPT_SPELLINGS {
            let tmp = kept_spelling_repo(entry, disk, "local edit\n");
            let root = tmp.path();
            let entries = index_names(root);
            let status = status_of(root, None).unwrap().parsed;
            // Stage all: the entry alone, no second index entry for the same file.
            let listed: Vec<String> = status
                .unstaged
                .iter()
                .map(|e| e.path.clone())
                .chain(status.untracked.iter().cloned())
                .collect();
            stage(root, &listed).unwrap();
            assert_eq!(index_names(root), entries, "{entry}");
            let staged = status_of(root, None).unwrap().parsed;
            assert_eq!(paths(&staged.staged), [entry], "{entry}: {staged:?}");
            assert!(staged.untracked.is_empty(), "{entry}: {staged:?}");
            // Rollback all, deleting what is untracked: the file is restored, not deleted.
            let targets = every_listed_target(&staged);
            let result = rollback_paths(root, &targets, true).unwrap();
            assert!(result.deleted.is_empty(), "{entry}: {result:?}");
            assert_eq!(
                std::fs::read_to_string(root.join(entry)).unwrap(),
                "base\n",
                "{entry}"
            );
            assert!(is_clean(root), "{entry}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn requests_for_the_twin_whose_spelling_git_keeps_are_refused_before_anything_changes() {
        for (entry, disk) in KEPT_SPELLINGS {
            let tmp = kept_spelling_repo(entry, disk, "local edit\n");
            let root = tmp.path();
            let entries = index_names(root);
            let intact = |what: &str| {
                assert_eq!(index_names(root), entries, "{entry}: {what}");
                assert_eq!(
                    std::fs::read_to_string(root.join(entry)).unwrap(),
                    "local edit\n",
                    "{entry}: {what}"
                );
                let status = status_of(root, None).unwrap().parsed;
                assert!(status.staged.is_empty(), "{entry}: {what}: {status:?}");
            };
            let tracked = [entry.to_string()];
            let twin = [disk.to_string()];
            // A list from before the twin was hidden: the twin alone, or after its entry.
            for (restore, clean) in [(&[][..], &twin), (&tracked[..], &twin)] {
                let error = discard(root, restore, clean).unwrap_err();
                assert!(error.contains("refused"), "{entry}: {error}");
                intact("discard");
            }
            for requested in [
                vec![disk.to_string()],
                vec![entry.to_string(), disk.to_string()],
            ] {
                let error = stage(root, &requested).unwrap_err();
                assert!(error.contains("refused"), "{entry}: {error}");
                intact("stage");
            }
            let stale = rollback_target(disk, GitRollbackClassification::Untracked);
            assert!(rollback_paths(root, &[stale], true).is_err(), "{entry}");
            intact("rollback");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn requests_by_a_disk_spelling_git_composes_to_the_name_it_lists_are_no_twin() {
        // A compatibility ideograph git keeps, then Hangul jamo it composes.
        let disk = "\u{f900}\u{1100}\u{1161}.txt";
        let listed = "\u{f900}\u{ac00}.txt";
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "true");
        std::fs::write(root.join("other.txt"), "o\n").unwrap();
        test_repo::git(root, &["add", "other.txt"]);
        test_repo::git(root, &["commit", "-m", "base"]);
        // A new file, staged by the name the directory has: one entry, as git lists it.
        std::fs::write(root.join(disk), "n\n").unwrap();
        stage(root, &[disk.to_string()]).unwrap();
        let mut entries = vec!["other.txt".to_string(), listed.to_string()];
        entries.sort();
        assert_eq!(index_names(root), entries);
        test_repo::git(root, &["commit", "-m", "add"]);
        // The tracked file: staged, unstaged and discarded by the same name.
        std::fs::write(root.join(disk), "edit\n").unwrap();
        stage(root, &[disk.to_string()]).unwrap();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.staged), [listed], "{status:?}");
        unstage(root, &[disk.to_string()]).unwrap();
        discard(root, &[disk.to_string()], &[]).unwrap();
        assert_eq!(std::fs::read_to_string(root.join(listed)).unwrap(), "n\n");
        assert_eq!(index_names(root), entries);
        assert!(is_clean(root));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn colliding_tracked_entries_keep_the_scope_check_strict() {
        // The repository was authored where names are not normalized: the index
        // holds `café.md` precomposed and decomposed, two entries and two blobs for
        // the one file APFS keeps under either name.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "false");
        std::fs::write(root.join(NFC_FILE), "nfc blob\n").unwrap();
        test_repo::git(root, &["add", "--", NFC_FILE]);
        std::fs::write(root.join(".git/nfd-blob"), "nfd blob\n").unwrap();
        let blob = run_git(
            root,
            &["hash-object", "-w", ".git/nfd-blob"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        let blob = String::from_utf8(blob.stdout).unwrap().trim().to_string();
        let cacheinfo = format!("100644,{blob},{NFD_FILE}");
        test_repo::git(root, &["update-index", "--add", "--cacheinfo", &cacheinfo]);
        test_repo::git(root, &["commit", "-m", "two entries"]);
        set_precompose(root, "true");
        assert_eq!(index_names(root), [NFD_FILE, NFC_FILE]);
        // Restoring the decomposed entry rewrites the file the precomposed entry
        // reads too, and that entry's row changes without being asked for.
        let error = discard(root, &[NFD_FILE.to_string()], &[]).unwrap_err();
        assert!(error.contains("outside the requested set"), "{error}");
        assert!(error.contains(NFC_FILE), "{error}");
    }

    /// The index of a repository made where names are not normalized holds
    /// `NFD_FILE` and `NFC_FILE` as two entries, one blob each, for the one file
    /// APFS keeps under either name. HEAD has both; the directory entry holds `disk`.
    #[cfg(target_os = "macos")]
    fn colliding_entries_repo(nfd_blob: &str, nfc_blob: &str, disk: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        test_repo::init(root);
        set_precompose(root, "false");
        for (name, content) in [(NFD_FILE, nfd_blob), (NFC_FILE, nfc_blob)] {
            std::fs::write(root.join(".git/blob"), content).unwrap();
            let blob = run_git(
                root,
                &["hash-object", "-w", ".git/blob"],
                DEFAULT_TIMEOUT,
                &[],
            )
            .unwrap();
            let blob = String::from_utf8(blob.stdout).unwrap().trim().to_string();
            let cacheinfo = format!("100644,{blob},{name}");
            test_repo::git(root, &["update-index", "--add", "--cacheinfo", &cacheinfo]);
        }
        test_repo::git(root, &["commit", "-m", "two entries"]);
        std::fs::write(root.join(NFD_FILE), disk).unwrap();
        set_precompose(root, "true");
        assert_eq!(index_names(root), [NFD_FILE, NFC_FILE]);
        tmp
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn colliding_entries_restored_as_stored_report_the_row_that_went_clean() {
        // Both entries hold "A" and the file "C": both rows are modified. The
        // decomposed entry restored as stored writes "A", and the precomposed
        // entry's row goes without being asked for.
        let tmp = colliding_entries_repo("A\n", "A\n", "C\n");
        let root = tmp.path();
        let status = status_of(root, None).unwrap().parsed;
        assert_eq!(paths(&status.unstaged), [NFD_FILE, NFC_FILE], "{status:?}");
        let error = discard(root, &[NFD_FILE.to_string()], &[]).unwrap_err();
        assert!(error.contains("outside the requested set"), "{error}");
        assert!(error.contains(NFC_FILE), "{error}");
        let tmp = colliding_entries_repo("A\n", "A\n", "C\n");
        let root = tmp.path();
        let status = status_of(root, None).unwrap().parsed;
        let classification = actual_rollback_classification(&status, NFD_FILE)
            .unwrap()
            .unwrap();
        let target = rollback_target(NFD_FILE, classification);
        let error = rollback_paths(root, &[target], false).unwrap_err();
        assert!(error.contains("outside the requested set"), "{error}");
        assert!(error.contains(NFC_FILE), "{error}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stage_of_an_entry_is_not_refused_for_a_second_entry_of_the_same_file() {
        // Two entries for the one file, both modified: staging the listed row of
        // either updates that entry and adds no third.
        for staged in [NFC_FILE, NFD_FILE] {
            let tmp = colliding_entries_repo("A\n", "B\n", "C\n");
            let root = tmp.path();
            stage(root, &[staged.to_string()]).unwrap();
            assert_eq!(index_names(root), [NFD_FILE, NFC_FILE], "{staged}");
            let status = status_of(root, None).unwrap().parsed;
            assert_eq!(paths(&status.staged), [staged], "{staged}: {status:?}");
            assert_eq!(status.unstaged.len(), 1, "{staged}: {status:?}");
            assert!(status.untracked.is_empty(), "{staged}: {status:?}");
        }
        // The state the old "stage all" left: the decomposed entry, and the
        // precomposed one added beside it.
        for requested in [vec![NFD_FILE, NFC_FILE], vec![NFC_FILE]] {
            let tmp = nfd_tracked_with_a_second_entry_repo();
            let root = tmp.path();
            let requested: Vec<String> = requested.into_iter().map(String::from).collect();
            stage(root, &requested).unwrap();
            assert_eq!(index_names(root), [NFD_FILE, NFC_FILE], "{requested:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clean_of_an_entry_that_is_a_second_entry_of_the_same_file_is_refused() {
        // `git clean` of the precomposed name deletes the file the decomposed
        // entry names, however much it is an entry itself.
        let tmp = colliding_entries_repo("A\n", "B\n", "C\n");
        let root = tmp.path();
        let error = discard(root, &[], &[NFC_FILE.to_string()]).unwrap_err();
        assert!(error.contains("refused"), "{error}");
        assert_eq!(std::fs::read_to_string(root.join(NFD_FILE)).unwrap(), "C\n");
        assert_eq!(index_names(root), [NFD_FILE, NFC_FILE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn git_spellings_are_what_git_takes_a_request_to() {
        let tmp = nfd_tracked_repo();
        let root = tmp.path();
        let nested = "de\u{301}/a\u{330}\u{301}.txt";
        let asked = [
            NFD_FILE,
            NFC_FILE,
            "a\u{330}\u{301}.txt",
            "e\u{301}\u{323}.txt",
            "\u{f900}\u{1100}\u{1161}.txt",
            "\u{212a}.txt",
            nested,
        ];
        let spelled = git_spellings(root, &asked);
        let expect = [
            (NFD_FILE, NFC_FILE),
            (NFC_FILE, NFC_FILE),
            // Kept: the mark order is canonical already and nothing combines.
            ("a\u{330}\u{301}.txt", "a\u{330}\u{301}.txt"),
            ("e\u{301}\u{323}.txt", "\u{e9}\u{323}.txt"),
            ("\u{f900}\u{1100}\u{1161}.txt", "\u{f900}\u{ac00}.txt"),
            ("\u{212a}.txt", "\u{212a}.txt"),
            (nested, "d\u{e9}/a\u{330}\u{301}.txt"),
        ];
        for (request, git) in expect {
            assert_eq!(
                spelled.get(request).map(String::as_str),
                Some(git),
                "{request}"
            );
        }
        assert_eq!(spelled.len(), expect.len());
        // Where git precomposes nothing, a request stays as it is.
        set_precompose(root, "false");
        assert_eq!(
            git_spellings(root, &[NFD_FILE])
                .get(NFD_FILE)
                .map(String::as_str),
            Some(NFD_FILE)
        );
        // A git that cannot answer (a corrupt index) takes no request to anything.
        std::fs::write(root.join(".git/index"), "not an index").unwrap();
        assert!(git_spellings(root, &[NFD_FILE]).is_empty());
        assert!(git_spellings(root, &[]).is_empty());
    }

    #[test]
    fn parsed_spellings_are_kept_only_when_they_name_the_same_path() {
        let paths = ["cafe\u{301}.md", "e\u{301}/../victim.md", "other.md"];
        // `check-attr -z`: the path, the attribute and its value, each NUL-ended.
        let output =
            "caf\u{e9}.md\0a\0unspecified\0victim.md\0a\0unspecified\0other.md\0a\0unspecified\0";
        let spelled = parse_git_spellings(&paths, output.as_bytes());
        assert_eq!(spelled.len(), 2, "{spelled:?}");
        assert_eq!(spelled["cafe\u{301}.md"], "caf\u{e9}.md");
        assert_eq!(spelled["other.md"], "other.md");
        // A path that came back as another one (git normalized it) is no spelling of the request.
        assert!(!spelled.contains_key("e\u{301}/../victim.md"));
        // Fewer records than paths: what is not answered is not taken.
        let spelled = parse_git_spellings(&paths, "caf\u{e9}.md\0a\0unspecified\0".as_bytes());
        assert_eq!(spelled.len(), 1, "{spelled:?}");
        assert!(parse_git_spellings(&paths, b"").is_empty());
    }
}
