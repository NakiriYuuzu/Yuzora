// M2 Task 4: git_service core (detection + run_git + git_status command)

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// 內層以 `Arc` 共享：async command 需把「持鎖比對 + mutation」整段移進
/// `spawn_blocking`（std MutexGuard 不可跨 `.await`），closure 是 `'static`，
/// 靠 clone Arc 帶進 blocking thread（見 `with_requested_repo_blocking`）。
pub struct GitServiceState(pub std::sync::Arc<std::sync::Mutex<Option<RepoHandle>>>);

#[cfg(test)]
static GIT_SPAWN_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(crate) fn git_spawn_count() -> u64 {
    GIT_SPAWN_COUNT.load(std::sync::atomic::Ordering::SeqCst)
}

#[derive(Clone)]
pub struct RepoHandle {
    pub root: PathBuf,
}

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

pub(crate) fn parse_git_version(version: &str) -> Option<(u32, u32)> {
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
pub(crate) fn run_git_with_stdin(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(String, String)],
    stdin: &[u8],
) -> Result<GitOutput, String> {
    run_git_inner(root, args, timeout, extra_env, Some(stdin), None)
}

fn run_git_inner(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(String, String)],
    stdin_bytes: Option<&[u8]>,
    on_spawn: Option<&dyn Fn(u32)>,
) -> Result<GitOutput, String> {
    use std::io::Write;
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
    #[cfg(test)]
    GIT_SPAWN_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut child = cmd.spawn().map_err(|e| format!("git spawn failed: {e}"))?;
    if let Some(hook) = on_spawn {
        hook(child.id());
    }
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
    if let Some(data) = stdin_bytes {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(data);
        }
    }
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

/// T1（#55）：Tauri 2 同步 command 在 main thread 執行，git 子行程會凍住 UI
/// event loop → command 一律 async ＋ 把 blocking 工作丟進
/// `tauri::async_runtime::spawn_blocking`（tokio 缺 `rt` feature，不可用
/// `tokio::task::spawn_blocking`）。模式：先 lock、clone 出 root、drop guard，
/// 再 move 進 closure。
pub(crate) async fn run_blocking<T, F>(task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| format!("git blocking task failed: {e}"))?
}

/// git_detect 併發 guard（review fix）：async 化後兩個 git_detect 可真併發、
/// 完成順序不保證（sync 時代 main thread FIFO 保證 last-requested-wins）。
/// 每次 detect 進場遞增取得 generation，落地前比對——只有最新 generation 的
/// 結果可寫入，晚到的舊結果直接丟棄，杜絕快速切換 workspace 時 stale root /
/// watcher 覆蓋新 workspace（last-completed-wins 競態）。
static DETECT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// git_detect 結果落地：stale guard ＋ root/watcher 成對原子寫入。
///
/// - `counter` 目前值 ≠ 本次 `generation` → 已有更新的 detect 進場，本結果
///   過期、不落地（回傳 `Ok(false)`；watcher 隨 drop 即停）。
/// - root 與 watcher 兩個 Mutex 的寫入都發生在同一段 repo guard 內：全 crate
///   僅此處同時持兩鎖、鎖序固定 repo → watch，無死鎖之虞；杜絕兩個併發 detect
///   在「寫 root」與「寫 watcher」之間交錯出 root=A、watcher=B 的錯配。
fn commit_detect_result(
    generation: u64,
    counter: &std::sync::atomic::AtomicU64,
    repo_state: &Mutex<Option<RepoHandle>>,
    watch_state: &Mutex<Option<crate::git_watch::GitWatcher>>,
    env: &GitEnvironment,
    watcher: Option<crate::git_watch::GitWatcher>,
) -> Result<bool, String> {
    use std::sync::atomic::Ordering;
    let mut repo_guard = repo_state.lock().map_err(|e| e.to_string())?;
    if counter.load(Ordering::SeqCst) != generation {
        return Ok(false);
    }
    match env {
        GitEnvironment::Ready { root, .. } => {
            *repo_guard = Some(RepoHandle {
                root: PathBuf::from(root),
            });
            // Ready → 啟動 .git watcher；舊 debouncer 被替換即 drop 停止。
            *watch_state.lock().map_err(|e| e.to_string())? = watcher;
        }
        GitEnvironment::NotARepo | GitEnvironment::Missing { .. } => {
            // Clear the authority as well as the watcher. Leaving the previous
            // RepoHandle alive would let an in-flight request for that old root
            // pass root validation after the UI switched to a non-repository.
            *repo_guard = None;
            // 非 repo / 無 git → 清空 watch state（drop 即停）。
            *watch_state.lock().map_err(|e| e.to_string())? = None;
        }
    }
    Ok(true)
}

/// `git:state-changed` 事件 payload（#57 T3）：帶上 detect 當時的 workspace
/// 路徑，前端 listener 比對 live workspacePath 後才處理——切換 gap 內舊
/// workspace 的 .git watcher 殘留事件不得刷新新 workspace 的面板。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStateChangedEvent {
    pub workspace_root: String,
}

/// detect → watcher 建立 → State 落地的共用核心（`git_detect`／`git_bootstrap`）。
/// 整段必須在 blocking thread 執行：git 子行程與 watcher 建立本來就 blocking；
/// repo state 鎖也可能被長時操作持有（見 `with_requested_repo_blocking`，
/// push/pull 至多 120s），在 async body 直接 lock 會 park 共用的 tokio worker
/// ——鎖等待一律留在 blocking thread。
fn detect_commit_and_watch(
    app: tauri::AppHandle,
    repo_shared: &std::sync::Arc<Mutex<Option<RepoHandle>>>,
    watch_shared: &std::sync::Arc<Mutex<Option<crate::git_watch::GitWatcher>>>,
    generation: u64,
    workspace_path: &str,
) -> Result<GitEnvironment, String> {
    use tauri::Emitter;
    let env = detect_environment(Path::new(workspace_path));
    let watcher = if let GitEnvironment::Ready { ref root, .. } = env {
        let git_dir = PathBuf::from(root).join(".git");
        let event_root = workspace_path.to_string();
        Some(crate::git_watch::build_git_watcher(&git_dir, move || {
            let _ = app.emit(
                "git:state-changed",
                GitStateChangedEvent {
                    workspace_root: event_root.clone(),
                },
            );
        })?)
    } else {
        None
    };
    commit_detect_result(
        generation,
        &DETECT_GENERATION,
        repo_shared,
        watch_shared,
        &env,
        watcher,
    )?;
    // stale 時不落地但仍回傳偵測結果；前端 gitStore.detect 以同款序號
    // guard 丟棄過期 resolve（兩端各自守自己的 state）。
    Ok(env)
}

#[tauri::command]
pub async fn git_detect(
    app: tauri::AppHandle,
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    watch_state: tauri::State<'_, crate::git_watch::GitWatchState>,
    path: String,
) -> Result<GitEnvironment, String> {
    use std::sync::atomic::Ordering;
    let generation = DETECT_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let repo_shared = state.0.clone();
    let watch_shared = watch_state.0.clone();
    let trust = trust.inner().clone();
    run_blocking(move || {
        let identity = trust.require_trusted(&path)?;
        let env = detect_commit_and_watch(app, &repo_shared, &watch_shared, generation, &path)?;
        if let GitEnvironment::Ready { root, .. } = &env {
            trust.bind_session_git_root(&identity, root);
        }
        Ok(env)
    })
    .await
}

/// T3（#57）：冷開 workspace 的 git 首載單趟快照。environment 非 Ready 時
/// status/branches 為 None（前端收到 null）。Ready 落地後快照失敗時同樣為
/// None，錯誤放 `snapshot_error`（見 `bootstrap_dto`）。
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitBootstrapDto {
    pub environment: GitEnvironment,
    pub status: Option<GitStatusDto>,
    pub branches: Option<BranchList>,
    pub snapshot_error: Option<String>,
}

/// Ready 快照結果 → DTO（#57 覆核修正）。快照失敗**不整趟回 Err**：detect
/// 已把 RepoHandle 與 .git watcher 落在新 repo，此時回 Err 的話前端只會記
/// lastError、store 仍殘留前一個 workspace 的 environment/status/branches——
/// 後續 watcher/focus refresh 以舊 environment 的 ready 閘放行、卻從 Rust
/// state 讀到新 repo 的 status，把新資料填進舊 root 標頭底下（跨 workspace
/// 混血）。改回 partial DTO：environment 照常落地、status/branches 為 None、
/// 錯誤放 snapshot_error，前端據此換血＋記 lastError——等同舊流程「detect
/// 成功、refresh 失敗」的語意。
fn bootstrap_dto(
    environment: GitEnvironment,
    snapshot: Result<(GitStatusDto, BranchList), String>,
) -> GitBootstrapDto {
    match snapshot {
        Ok((status, branches)) => GitBootstrapDto {
            environment,
            status: Some(status),
            branches: Some(branches),
            snapshot_error: None,
        },
        Err(e) => GitBootstrapDto {
            environment,
            status: None,
            branches: None,
            snapshot_error: Some(e),
        },
    }
}

/// Ready 後的首載快照：status 與 branches 各自丟進 blocking pool 真併發、
/// join 後一次回齊——消除「detect 先行寫 State、status/branches 才能發」的
/// 兩趟 IPC waterfall（#57 T3）。
async fn bootstrap_ready_snapshot(root: PathBuf) -> Result<(GitStatusDto, BranchList), String> {
    let status_root = root.clone();
    let status_task = tauri::async_runtime::spawn_blocking(move || status_of(&status_root, None));
    let branches_task = tauri::async_runtime::spawn_blocking(move || branches(&root));
    let status = status_task
        .await
        .map_err(|e| format!("git blocking task failed: {e}"))??;
    let branch_list = branches_task
        .await
        .map_err(|e| format!("git blocking task failed: {e}"))??;
    Ok((status, branch_list))
}

/// 冷開 workspace 的 git 面板首載（#57 T3）：detect →（Ready 時）寫入
/// RepoHandle state、建 .git watcher，再併發跑 status‖branches 後一次回齊。
/// 細粒度 `git_status_cmd`／`git_branches` 保留給後續 refresh。Ready 落地後
/// status/branches 失敗（timeout、repo 中途被刪）→ 仍回 Ok：environment
/// 照常落地、快照為 None、錯誤放 `snapshot_error`（見 `bootstrap_dto`，
/// 與舊流程「detect 成功、refresh 失敗」同語意）。
#[tauri::command]
pub async fn git_bootstrap(
    app: tauri::AppHandle,
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    watch_state: tauri::State<'_, crate::git_watch::GitWatchState>,
    path: String,
) -> Result<GitBootstrapDto, String> {
    use std::sync::atomic::Ordering;
    let generation = DETECT_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let repo_shared = state.0.clone();
    let watch_shared = watch_state.0.clone();
    let trust = trust.inner().clone();
    let env = run_blocking(move || {
        let identity = trust.require_trusted(&path)?;
        let env = detect_commit_and_watch(app, &repo_shared, &watch_shared, generation, &path)?;
        if let GitEnvironment::Ready { root, .. } = &env {
            trust.bind_session_git_root(&identity, root);
        }
        Ok(env)
    })
    .await?;
    let root = match &env {
        GitEnvironment::Ready { root, .. } => PathBuf::from(root),
        GitEnvironment::NotARepo | GitEnvironment::Missing { .. } => {
            return Ok(GitBootstrapDto {
                environment: env,
                status: None,
                branches: None,
                snapshot_error: None,
            })
        }
    };
    let snapshot = bootstrap_ready_snapshot(root).await;
    Ok(bootstrap_dto(env, snapshot))
}

#[tauri::command]
pub async fn git_status_cmd(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    pathspec: Option<Vec<String>>,
) -> Result<GitStatusDto, String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        status_of(root, pathspec)
    })
    .await
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

fn repository_display_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

fn remote_identity_for_askpass(root: &Path) -> (Option<String>, Option<String>) {
    let upstream = run_git(
        root,
        &["rev-parse", "--abbrev-ref", "@{upstream}"],
        DEFAULT_TIMEOUT,
        &[],
    )
    .ok()
    .filter(|out| out.code == 0)
    .and_then(|out| {
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        value
            .split_once('/')
            .map(|(remote, _)| remote.to_string())
            .filter(|remote| !remote.is_empty())
    });
    let remote_name = upstream.or_else(|| {
        run_git(root, &["remote"], DEFAULT_TIMEOUT, &[])
            .ok()
            .filter(|out| out.code == 0)
            .and_then(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(ToOwned::to_owned)
            })
    });
    let Some(name) = remote_name else {
        return (None, None);
    };
    let url = run_git(root, &["remote", "get-url", &name], DEFAULT_TIMEOUT, &[])
        .ok()
        .filter(|out| out.code == 0)
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let display = match &url {
        Some(url) => Some(format!("{name} ({url})")),
        None => Some(name),
    };
    let fingerprint = url.as_ref().map(|url| {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(url.as_bytes());
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            hex.push_str(&format!("{byte:02x}"));
        }
        format!("sha256:{hex}")
    });
    (display, fingerprint)
}

fn begin_remote_askpass(
    askpass: &crate::askpass::AskpassState,
    root: &Path,
    operation: crate::askpass::AskpassOperationKind,
    background: bool,
) -> crate::askpass::AskpassOperationGuard {
    let (remote_display, remote_fingerprint) = remote_identity_for_askpass(root);
    askpass.begin_operation(crate::askpass::AskpassOperationContext {
        repository_display: repository_display_name(root),
        repository_canonical: root.to_string_lossy().into_owned(),
        remote_display,
        remote_fingerprint,
        operation,
        background,
    })
}

fn run_ok_with_askpass(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    env: &[(String, String)],
    op: &crate::askpass::AskpassOperationGuard,
) -> Result<GitOutput, String> {
    let out = run_git_inner(
        root,
        args,
        timeout,
        env,
        None,
        Some(&|pid| op.bind_root_pid(pid)),
    )?;
    if out.code != 0 {
        return Err(git_err(args.first().unwrap_or(&""), &out.stderr));
    }
    Ok(out)
}

fn status_path_fingerprint(
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

fn mutated_status_paths(
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

fn ensure_literal_path_scope(
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

fn mutate_then_assert_literal_scope(
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

/// Lexical repo-relative path check only (no filesystem resolve). Used by
/// historical object reads that must not depend on the current worktree.
pub(crate) fn validate_relative_components(value: &str, operation: &str) -> Result<(), String> {
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
pub(crate) fn validate_repo_relative_path(
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

fn validate_diff_relative_path(
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

fn remote_tracking_start_ref(root: &Path, spec: &str) -> Result<Option<String>, String> {
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

/// bytes 過分級：與 fs_service::classify_and_read / git_log::grade_object_bytes
/// 同標準，但輸入是 bytes 而非 path，且回 GradedText（無 size 欄）以符 T9
/// types.ts DiffContent 契約。UTF-16 BOM 走 encoding_rs 解碼，避免
/// from_utf8_lossy 把可讀 worktree 檔案變成亂碼。
fn grade_bytes(bytes: &[u8]) -> GradedText {
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
fn show_object(root: &Path, spec: &str) -> Result<Option<Vec<u8>>, String> {
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
fn read_worktree(root: &Path, path: &str) -> Result<Option<Vec<u8>>, String> {
    #[cfg(unix)]
    {
        return read_worktree_nofollow_unix(root, path);
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
fn c_component_name(component: &std::ffi::OsStr, label: &str) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(component.as_bytes())
        .map_err(|_| format!("git diff rejected path with interior NUL: {label}"))
}

/// Open an absolute directory by walking every component from `/` with
/// `openat(O_DIRECTORY|O_NOFOLLOW)`. Never opens the full path in one shot, so a
/// concurrent actor cannot substitute a symlink for a root path component after
/// canonicalize and before the read.
#[cfg(unix)]
fn open_absolute_dir_nofollow(absolute: &Path) -> Result<std::os::fd::OwnedFd, String> {
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
fn read_worktree_nofollow_unix(root: &Path, path: &str) -> Result<Option<Vec<u8>>, String> {
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

fn empty_text() -> GradedText {
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

fn remote_probe_inner(
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

/// Bind a mutating request to the repository snapshot the frontend acted on.
/// The state lock stays held through `operation`, making compare + mutation
/// atomic relative to `git_detect` switching the active repository.
pub(crate) fn with_requested_repo<T>(
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

/// `with_requested_repo` 的 async 包裝：整段（含持鎖比對）移進 blocking thread，
/// 保留「compare + mutation 相對 `git_detect` 切換 repo 原子」的語意——鎖不跨
/// `.await`，而是連同 mutation 一起在 blocking closure 內持有。
pub(crate) async fn with_requested_repo_blocking<T>(
    state: &GitServiceState,
    trust: &crate::workspace_trust::WorkspaceTrustState,
    requested_root: String,
    operation: impl FnOnce(&Path) -> Result<T, String> + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
{
    let shared = state.0.clone();
    let trust = trust.clone();
    run_blocking(move || {
        trust.require_trusted_git(&requested_root)?;
        with_requested_repo(&GitServiceState(shared), &requested_root, operation)
    })
    .await
}

#[tauri::command]
pub async fn git_stage(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    paths: Vec<String>,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        stage(root, &paths)
    })
    .await
}

#[tauri::command]
pub async fn git_unstage(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    paths: Vec<String>,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        unstage(root, &paths)
    })
    .await
}

#[tauri::command]
pub async fn git_discard(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    paths: Vec<String>,
    untracked: Vec<String>,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        discard(root, &paths, &untracked)
    })
    .await
}

#[tauri::command]
pub async fn git_rollback_paths(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    targets: Vec<GitRollbackTarget>,
    delete_untracked_or_added: bool,
) -> Result<GitRollbackResult, String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        rollback_paths(root, &targets, delete_untracked_or_added)
    })
    .await
}

#[tauri::command]
pub async fn git_commit_cmd(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    message: String,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        commit(root, &message)
    })
    .await
}

#[tauri::command]
pub async fn git_branches(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
) -> Result<BranchList, String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, branches).await
}

#[tauri::command]
pub async fn git_create_branch(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    name: String,
    start_point: Option<String>,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        create_branch(root, &name, start_point.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn git_checkout_detached(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    rev: String,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        checkout_detached(root, &rev)
    })
    .await
}

#[tauri::command]
pub async fn git_checkout(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    name: String,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        checkout(root, &name)
    })
    .await
}

#[tauri::command]
pub async fn git_cherry_pick(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    hash: String,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        cherry_pick(root, &hash)
    })
    .await
}

#[tauri::command]
pub async fn git_fetch_cmd(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
    background: bool,
    repository_root: String,
) -> Result<(), String> {
    let askpass = askpass.inner().clone();
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        let op = begin_remote_askpass(
            &askpass,
            root,
            crate::askpass::AskpassOperationKind::Fetch,
            background,
        );
        run_ok_with_askpass(root, &["fetch"], REMOTE_TIMEOUT, op.env(), &op).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_pull_cmd(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
    repository_root: String,
) -> Result<(), String> {
    let askpass = askpass.inner().clone();
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        let op = begin_remote_askpass(
            &askpass,
            root,
            crate::askpass::AskpassOperationKind::Pull,
            false,
        );
        let mut env = op.env().to_vec();
        env.extend(editor_true());
        run_ok_with_askpass(root, &["pull"], REMOTE_TIMEOUT, &env, &op).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_push_cmd(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
    repository_root: String,
) -> Result<(), String> {
    let askpass = askpass.inner().clone();
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        let op = begin_remote_askpass(
            &askpass,
            root,
            crate::askpass::AskpassOperationKind::Push,
            false,
        );
        run_ok_with_askpass(root, &["push"], REMOTE_TIMEOUT, op.env(), &op).map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn git_remote_probe(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
    repository_root: String,
) -> Result<String, String> {
    let askpass = askpass.inner().clone();
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        let op = begin_remote_askpass(
            &askpass,
            root,
            crate::askpass::AskpassOperationKind::Probe,
            true,
        );
        remote_probe_inner(root, op.env(), Some(&|pid| op.bind_root_pid(pid)))
    })
    .await
}

#[tauri::command]
pub async fn git_diff_content(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    path: String,
    staged: bool,
    orig_path: Option<String>,
) -> Result<DiffContent, String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        diff_content(root, &path, staged, orig_path.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn git_conflict_abort(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    op: String,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        conflict_abort(root, &op)
    })
    .await
}

#[tauri::command]
pub async fn git_conflict_continue(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    op: String,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        conflict_continue(root, &op)
    })
    .await
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
    fn parse_git_version_accepts_2_24_and_apple_suffix() {
        assert_eq!(parse_git_version("git version 2.24.0"), Some((2, 24)));
        assert_eq!(
            parse_git_version("git version 2.50.1 (Apple Git-155)"),
            Some((2, 50))
        );
        assert_eq!(parse_git_version("git version 2.23.0"), Some((2, 23)));
        assert!((2, 23) < MIN_GIT_VERSION);
        assert!((2, 24) >= MIN_GIT_VERSION);
    }

    /// #57 T3：Ready 首載快照——status 與 branches 兩個 blocking task 併發
    /// join 後一次回齊（fixture repo 上驗證兩者內容都是真的）。
    #[test]
    fn bootstrap_ready_snapshot_joins_status_and_branches() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        test_repo::write_and_commit(tmp.path(), "a.txt", "hi", "init");
        std::fs::write(tmp.path().join("b.txt"), "new").unwrap();
        let (status, branch_list) =
            tauri::async_runtime::block_on(bootstrap_ready_snapshot(tmp.path().to_path_buf()))
                .unwrap();
        assert!(
            status.parsed.untracked.iter().any(|p| p == "b.txt"),
            "untracked: {:?}",
            status.parsed.untracked
        );
        assert!(
            branch_list
                .local
                .iter()
                .any(|b| b.name == "main" && b.is_current),
            "local branches: {:?}",
            branch_list
                .local
                .iter()
                .map(|b| &b.name)
                .collect::<Vec<_>>()
        );
    }

    /// #57 T3：bootstrap 快照對 status/branches 失敗回 Err；`bootstrap_dto`
    /// 再把它映成 partial DTO（不整趟 Err，見下一個測試）。
    #[test]
    fn bootstrap_ready_snapshot_fails_whole_on_a_non_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        let result =
            tauri::async_runtime::block_on(bootstrap_ready_snapshot(tmp.path().to_path_buf()));
        assert!(result.is_err());
    }

    /// #57 覆核修正：Ready 落地後快照失敗 → partial DTO（environment 照常、
    /// status/branches 為 null、錯誤放 snapshotError）——不整趟 Err，否則前端
    /// 殘留前一個 workspace 的 git 狀態、與 Rust 端已切換的 RepoHandle 形成
    /// 跨 workspace 混血顯示。
    #[test]
    fn bootstrap_dto_keeps_environment_and_carries_snapshot_error_on_failure() {
        let dto = bootstrap_dto(
            GitEnvironment::Ready {
                root: "/w".to_string(),
                version: "2.50.1".to_string(),
            },
            Err("git status timed out".to_string()),
        );
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["environment"]["status"], "ready");
        assert!(v["status"].is_null());
        assert!(v["branches"].is_null());
        assert_eq!(v["snapshotError"], "git status timed out");
    }

    /// #57 T3：DTO 契約——camelCase、非 Ready 時 status/branches 序列化為 null。
    #[test]
    fn git_bootstrap_dto_serializes_camel_case_with_nullable_snapshot() {
        let dto = GitBootstrapDto {
            environment: GitEnvironment::NotARepo,
            status: None,
            branches: None,
            snapshot_error: None,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["environment"]["status"], "notARepo");
        assert!(v["status"].is_null());
        assert!(v["branches"].is_null());
        assert!(v["snapshotError"].is_null());
    }

    /// #57 T3：git:state-changed 事件 payload 帶 workspaceRoot（前端過濾契約）。
    #[test]
    fn git_state_changed_event_carries_workspace_root() {
        let v = serde_json::to_value(GitStateChangedEvent {
            workspace_root: "/w".to_string(),
        })
        .unwrap();
        assert_eq!(v["workspaceRoot"], "/w");
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
        let state = GitServiceState(std::sync::Arc::new(std::sync::Mutex::new(Some(
            RepoHandle {
                root: repo_b.path().to_path_buf(),
            },
        ))));

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
    fn stage_unstage_discard_rollback_treat_pathspec_magic_as_literal_filenames() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "canary.txt", "base\n", "base");
        test_repo::write_and_commit(r, "?", "old-q\n", "tracked question");
        test_repo::write_and_commit(r, ":(exclude)", "old-ex\n", "tracked exclude");

        let magic = [":(top)**", ":(glob)**", "*", "?", ":(exclude)"];
        for name in [":(top)**", ":(glob)**", "*"] {
            std::fs::write(r.join(name), format!("untracked-{name}\n")).unwrap();
        }
        std::fs::write(r.join("?"), "new-q\n").unwrap();
        std::fs::write(r.join(":(exclude)"), "new-ex\n").unwrap();
        std::fs::write(r.join("canary.txt"), "changed\n").unwrap();
        std::fs::write(r.join("other.txt"), "other\n").unwrap();

        stage(r, &[":(top)**".into()]).unwrap();
        let after_stage = status_of(r, None).unwrap().parsed;
        assert_eq!(
            after_stage
                .staged
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            vec![":(top)**"]
        );
        assert!(after_stage.untracked.iter().any(|p| p == ":(glob)**"));
        assert!(after_stage.untracked.iter().any(|p| p == "*"));
        assert!(after_stage.untracked.iter().any(|p| p == "other.txt"));
        assert!(after_stage.unstaged.iter().any(|e| e.path == "canary.txt"));
        assert!(after_stage.unstaged.iter().any(|e| e.path == "?"));

        stage(r, &[":(glob)**".into(), "*".into()]).unwrap();
        let after_second_stage = status_of(r, None).unwrap().parsed;
        assert!(after_second_stage
            .staged
            .iter()
            .any(|e| e.path == ":(glob)**"));
        assert!(after_second_stage.staged.iter().any(|e| e.path == "*"));
        assert!(after_second_stage
            .unstaged
            .iter()
            .any(|e| e.path == "canary.txt"));
        unstage(r, &[":(glob)**".into(), "*".into()]).unwrap();
        let after_unstage = status_of(r, None).unwrap().parsed;
        assert!(after_unstage.staged.iter().any(|e| e.path == ":(top)**"));
        assert!(!after_unstage.staged.iter().any(|e| e.path == ":(glob)**"));
        assert!(!after_unstage.staged.iter().any(|e| e.path == "*"));
        assert!(after_unstage.untracked.iter().any(|p| p == ":(glob)**"));
        assert!(after_unstage.untracked.iter().any(|p| p == "*"));
        assert!(after_unstage
            .unstaged
            .iter()
            .any(|e| e.path == "canary.txt"));

        discard(r, &["?".into()], &[":(glob)**".into()]).unwrap();
        assert_eq!(std::fs::read_to_string(r.join("?")).unwrap(), "old-q\n");
        assert!(!r.join(":(glob)**").exists());
        assert_eq!(
            std::fs::read_to_string(r.join("canary.txt")).unwrap(),
            "changed\n"
        );
        assert!(r.join("other.txt").exists());
        assert!(r.join("*").exists());

        let latest = status_of(r, None).unwrap().parsed;
        let exclude = latest
            .unstaged
            .iter()
            .find(|e| e.path == ":(exclude)")
            .expect(":(exclude) should still be dirty");
        rollback_paths(
            r,
            &[rollback_target(
                ":(exclude)",
                tracked_classification(None, Some(exclude.status.as_str()), None),
            )],
            false,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(r.join(":(exclude)")).unwrap(),
            "old-ex\n"
        );
        assert_eq!(
            std::fs::read_to_string(r.join("canary.txt")).unwrap(),
            "changed\n"
        );
        assert!(r.join("other.txt").exists());
        let _ = magic;
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
        create_branch(r, "side", None).unwrap();
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
    fn parses_gone_and_divergence_from_authoritative_upstream_track() {
        assert_eq!(parse_upstream_track("[gone]"), (0, 0, true));
        assert_eq!(parse_upstream_track("[ahead 3, behind 2]"), (3, 2, false));
        assert_eq!(parse_upstream_track(""), (0, 0, false));
    }

    #[test]
    fn branches_marks_a_deleted_configured_upstream_as_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        let remote_path = r.to_string_lossy().to_string();
        run_ok(
            r,
            &["remote", "add", "origin", remote_path.as_str()],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        run_ok(
            r,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        run_ok(
            r,
            &["branch", "--set-upstream-to=origin/main", "main"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        run_ok(
            r,
            &["update-ref", "-d", "refs/remotes/origin/main"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();

        let list = branches(r).unwrap();
        let main = list
            .local
            .iter()
            .find(|branch| branch.name == "main")
            .expect("main branch");
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert!(main.gone);
        assert_eq!((main.ahead, main.behind), (0, 0));
    }

    #[test]
    fn branches_lists_annotated_and_lightweight_tag_creator_dates() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        std::fs::write(r.join("a.txt"), "1").unwrap();
        run_ok(r, &["add", "a.txt"], DEFAULT_TIMEOUT, &[]).unwrap();
        run_ok(
            r,
            &["commit", "-m", "c1"],
            DEFAULT_TIMEOUT,
            &[
                ("GIT_AUTHOR_DATE".into(), "2024-01-01T12:00:00+00:00".into()),
                (
                    "GIT_COMMITTER_DATE".into(),
                    "2024-01-01T12:00:00+00:00".into(),
                ),
            ],
        )
        .unwrap();
        run_ok(r, &["tag", "release/v1.0.0"], DEFAULT_TIMEOUT, &[]).unwrap();
        run_ok(
            r,
            &["tag", "-a", "release/v2.0.0", "-m", "annotated"],
            DEFAULT_TIMEOUT,
            &[(
                "GIT_COMMITTER_DATE".into(),
                "2025-06-15T18:00:00+00:00".into(),
            )],
        )
        .unwrap();
        let list = branches(r).unwrap();
        let light = list
            .tags
            .iter()
            .find(|tag| tag.name == "release/v1.0.0")
            .expect("lightweight tag DTO");
        let annotated = list
            .tags
            .iter()
            .find(|tag| tag.name == "release/v2.0.0")
            .expect("annotated tag DTO");
        assert!(
            light.date.contains("2024-01-01"),
            "lightweight date should come from commit creatordate, got {}",
            light.date
        );
        assert!(
            annotated.date.contains("2025-06-15"),
            "annotated date should come from tagger creatordate, got {}",
            annotated.date
        );
    }

    #[test]
    fn branches_emits_stable_logical_names_when_namespaces_collide() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        create_branch(r, "origin/main", None).unwrap();
        checkout(r, "main").unwrap();
        run_ok(
            r,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        run_ok(r, &["tag", "origin/main"], DEFAULT_TIMEOUT, &[]).unwrap();
        let list = branches(r).unwrap();
        assert!(
            list.local.iter().any(|branch| branch.name == "origin/main"),
            "local logical name must stay origin/main, got {:?}",
            list.local
                .iter()
                .map(|branch| branch.name.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            list.remote.iter().any(|name| name == "origin/main"),
            "remote logical name must stay origin/main, got {:?}",
            list.remote
        );
        assert!(
            list.tags.iter().any(|tag| tag.name == "origin/main"),
            "tag logical name must stay origin/main, got {:?}",
            list.tags
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>()
        );
        checkout_detached(r, "refs/tags/origin/main").unwrap();
        assert!(status_of(r, None).unwrap().parsed.detached);
        create_branch(r, "from-remote", Some("refs/remotes/origin/main")).unwrap();
        assert!(branches(r)
            .unwrap()
            .local
            .iter()
            .any(|branch| branch.name == "from-remote" && branch.is_current));
    }

    #[test]
    fn checkout_does_not_guess_remote_tracking_names() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        run_ok(
            r,
            &["update-ref", "refs/remotes/origin/topic", "HEAD"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        let error = checkout(r, "topic").unwrap_err();
        assert!(
            !error.is_empty(),
            "--no-guess must refuse DWIM checkout of a remote-only name"
        );
        assert!(!branches(r)
            .unwrap()
            .local
            .iter()
            .any(|branch| branch.name == "topic"));
        create_branch(r, "topic", Some("refs/remotes/origin/topic")).unwrap();
        assert!(branches(r)
            .unwrap()
            .local
            .iter()
            .any(|branch| branch.name == "topic" && branch.is_current));
    }

    #[test]
    fn option_shaped_tag_is_switched_after_end_of_options() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        run_ok(
            r,
            &["update-ref", "refs/tags/-n", "HEAD"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        run_ok(
            r,
            &["update-ref", "refs/tags/--help", "HEAD"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        let listed = branches(r).unwrap();
        assert!(listed.tags.iter().any(|tag| tag.name == "-n"));
        assert!(listed.tags.iter().any(|tag| tag.name == "--help"));
        create_branch(r, "from-opt", Some("refs/tags/-n")).unwrap();
        assert!(branches(r)
            .unwrap()
            .local
            .iter()
            .any(|branch| branch.name == "from-opt" && branch.is_current));
        checkout_detached(r, "refs/tags/--help").unwrap();
        assert!(status_of(r, None).unwrap().parsed.detached);
    }

    #[test]
    fn create_branch_uses_exact_start_point_and_detached_checkout_is_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        run_ok(r, &["tag", "release/v1.0.0"], DEFAULT_TIMEOUT, &[]).unwrap();
        create_branch(r, "release/1.0.0", Some("release/v1.0.0")).unwrap();
        assert!(branches(r)
            .unwrap()
            .local
            .iter()
            .any(|branch| branch.name == "release/1.0.0" && branch.is_current));
        checkout_detached(r, "release/v1.0.0").unwrap();
        let status = status_of(r, None).unwrap();
        assert!(status.parsed.detached);
    }

    #[test]
    fn create_branch_from_remote_tracking_start_preserves_implicit_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        let start_oid = crate::git_oid::resolve_commit_oid(r, "HEAD").unwrap();
        let remote_path = r.to_string_lossy().to_string();
        run_ok(
            r,
            &["remote", "add", "origin", remote_path.as_str()],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        run_ok(
            r,
            &["update-ref", "refs/remotes/origin/topic", "HEAD"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        create_branch(r, "topic", Some("origin/topic")).unwrap();
        let list = branches(r).unwrap();
        let topic = list
            .local
            .iter()
            .find(|branch| branch.name == "topic")
            .expect("created branch");
        assert!(topic.is_current);
        assert_eq!(topic.upstream.as_deref(), Some("origin/topic"));
        let head = run_git(r, &["rev-parse", "HEAD"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            start_oid.as_str()
        );

        create_branch(r, "from-full", Some("refs/remotes/origin/topic")).unwrap();
        let listed = branches(r).unwrap();
        let from_full = listed
            .local
            .iter()
            .find(|branch| branch.name == "from-full")
            .expect("full-name branch");
        assert_eq!(from_full.upstream.as_deref(), Some("origin/topic"));

        run_ok(r, &["tag", "start-tag"], DEFAULT_TIMEOUT, &[]).unwrap();
        create_branch(r, "from-tag", Some("start-tag")).unwrap();
        let listed_after_tag = branches(r).unwrap();
        let from_tag = listed_after_tag
            .local
            .iter()
            .find(|branch| branch.name == "from-tag")
            .expect("tag start branch");
        assert!(from_tag.upstream.is_none());
    }

    #[test]
    fn renderer_revisions_reject_option_like_payloads_before_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        let sink = tmp.path().join("pwned");
        let inject = format!("--output={}", sink.display());
        let detached_err = checkout_detached(r, &inject).unwrap_err();
        assert!(
            detached_err.contains("option-like"),
            "checkout_detached: {detached_err}"
        );
        let pick_err = cherry_pick(r, &inject).unwrap_err();
        assert!(pick_err.contains("option-like"), "cherry_pick: {pick_err}");
        let branch_err = create_branch(r, "from-inject", Some(&inject)).unwrap_err();
        assert!(
            branch_err.contains("option-like"),
            "create_branch: {branch_err}"
        );
        assert!(!sink.exists(), "option revision must not write a file");
        assert!(
            !status_of(r, None).unwrap().parsed.detached,
            "rejected revision must not move HEAD"
        );
    }

    #[test]
    fn run_git_forces_literal_pathspecs_even_if_caller_tries_to_disable_them() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "canary.txt", "base\n", "base");
        std::fs::write(r.join("*"), "star\n").unwrap();
        std::fs::write(r.join("canary.txt"), "changed\n").unwrap();
        let disable = vec![("GIT_LITERAL_PATHSPECS".to_string(), "0".to_string())];
        let out = run_git(r, &["add", "--", "*"], DEFAULT_TIMEOUT, &disable).unwrap();
        assert_eq!(out.code, 0, "git add *: {}", out.stderr);
        let parsed = status_of(r, None).unwrap().parsed;
        assert!(
            parsed.staged.iter().any(|entry| entry.path == "*"),
            "literal * should be staged, got {:?}",
            parsed.staged
        );
        assert!(
            parsed
                .unstaged
                .iter()
                .any(|entry| entry.path == "canary.txt"),
            "canary must remain unstaged when * is forced literal"
        );
    }

    #[test]
    fn branches_lists_current_and_created() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        create_branch(r, "feature/x", None).unwrap();
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
        create_branch(r, "side", None).unwrap();
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

        create_branch(r, "side", None).unwrap();
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

        create_branch(r, "side", None).unwrap();
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
        create_branch(r, "side", None).unwrap();
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
        create_branch(r, "side", None).unwrap();
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
        let d = diff_content(r, "a.txt", false, None).unwrap();
        assert!(matches!(&d.original, GradedText::Full { content } if content == "one\n"));
        assert!(matches!(&d.modified, GradedText::Full { content } if content == "two\n"));
        std::fs::write(r.join("new.txt"), "n\n").unwrap();
        let d2 = diff_content(r, "new.txt", false, None).unwrap();
        assert!(matches!(&d2.original, GradedText::Full { content } if content.is_empty()));
        stage(r, &["a.txt".into()]).unwrap();
        let d3 = diff_content(r, "a.txt", true, None).unwrap();
        assert!(matches!(&d3.original, GradedText::Full { content } if content == "one\n"));
        assert!(matches!(&d3.modified, GradedText::Full { content } if content == "two\n"));
    }

    fn assert_utf16_worktree_diff(big_endian: bool) {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        let mut utf16 = if big_endian {
            vec![0xFE, 0xFF]
        } else {
            vec![0xFF, 0xFE]
        };
        for unit in "hello".encode_utf16() {
            let bytes = if big_endian {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            };
            utf16.extend_from_slice(&bytes);
        }
        std::fs::write(r.join("u.txt"), &utf16).unwrap();
        let unstaged = diff_content(r, "u.txt", false, None).unwrap();
        assert!(matches!(&unstaged.original, GradedText::Full { content } if content.is_empty()));
        assert!(matches!(&unstaged.modified, GradedText::Full { content } if content == "hello"));

        stage(r, &["u.txt".into()]).unwrap();
        let staged = diff_content(r, "u.txt", true, None).unwrap();
        assert!(matches!(&staged.original, GradedText::Full { content } if content.is_empty()));
        assert!(matches!(&staged.modified, GradedText::Full { content } if content == "hello"));
    }

    #[test]
    fn diff_content_rejects_non_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        assert!(
            matches!(diff_content(r, "../outside", false, None), Err(error) if error.contains("non-repo-relative"))
        );
        assert!(
            matches!(diff_content(r, "/tmp/outside", false, None), Err(error) if error.contains("non-repo-relative"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn diff_content_reads_internal_symlink_target_text_without_following_it() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        std::fs::write(r.join("target.txt"), "secret contents").unwrap();
        symlink("target.txt", r.join("link.txt")).unwrap();
        let diff = diff_content(r, "link.txt", false, None).unwrap();
        assert!(matches!(&diff.modified, GradedText::Full { content } if content == "target.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn diff_content_reads_external_symlink_target_text_without_following_it() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        let outside = tmp
            .path()
            .parent()
            .unwrap()
            .join("yuzora-diff-outside-secret");
        std::fs::write(&outside, "secret").unwrap();
        symlink(&outside, r.join("link.txt")).unwrap();
        let diff = diff_content(r, "link.txt", false, None).unwrap();
        assert!(
            matches!(&diff.modified, GradedText::Full { content } if content == outside.as_os_str().to_string_lossy().as_ref())
        );
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn diff_content_staged_rename_uses_orig_path_for_head() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "old.txt", "before\n", "c1");
        std::fs::rename(r.join("old.txt"), r.join("new.txt")).unwrap();
        run_ok(r, &["add", "-A"], DEFAULT_TIMEOUT, &[]).unwrap();
        let diff = diff_content(r, "new.txt", true, Some("old.txt")).unwrap();
        assert!(matches!(&diff.original, GradedText::Full { content } if content == "before\n"));
        assert!(matches!(&diff.modified, GradedText::Full { content } if content == "before\n"));
    }

    #[test]
    fn diff_content_staged_added_uses_empty_original() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "base.txt", "base\n", "base");
        std::fs::write(r.join("new.txt"), "added\n").unwrap();
        run_ok(r, &["add", "new.txt"], DEFAULT_TIMEOUT, &[]).unwrap();
        let diff = diff_content(r, "new.txt", true, None).unwrap();
        assert!(matches!(&diff.original, GradedText::Full { content } if content.is_empty()));
        assert!(matches!(&diff.modified, GradedText::Full { content } if content == "added\n"));
    }

    #[test]
    fn diff_content_unmerged_prefers_stage1_merge_base() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "conflict.txt", "base\n", "base");
        run_ok(r, &["checkout", "-b", "other"], DEFAULT_TIMEOUT, &[]).unwrap();
        std::fs::write(r.join("conflict.txt"), "theirs\n").unwrap();
        run_ok(r, &["commit", "-am", "theirs"], DEFAULT_TIMEOUT, &[]).unwrap();
        run_ok(r, &["checkout", "main"], DEFAULT_TIMEOUT, &[]).unwrap();
        std::fs::write(r.join("conflict.txt"), "ours\n").unwrap();
        run_ok(r, &["commit", "-am", "ours"], DEFAULT_TIMEOUT, &[]).unwrap();
        let merge = run_git(r, &["merge", "other"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_ne!(merge.code, 0);
        // Stage 1 = base, stage 2 = ours — original must prefer stage 1.
        let stage1 = run_git(r, &["show", ":1:conflict.txt"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(stage1.code, 0);
        assert_eq!(String::from_utf8_lossy(&stage1.stdout), "base\n");
        let stage2 = run_git(r, &["show", ":2:conflict.txt"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_eq!(stage2.code, 0);
        assert_eq!(String::from_utf8_lossy(&stage2.stdout), "ours\n");
        let diff = diff_content(r, "conflict.txt", false, None).unwrap();
        assert!(matches!(&diff.original, GradedText::Full { content } if content == "base\n"));
    }

    #[test]
    fn diff_content_unmerged_add_add_falls_back_to_ours() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "keep.txt", "keep\n", "base");
        run_ok(r, &["checkout", "-b", "other"], DEFAULT_TIMEOUT, &[]).unwrap();
        std::fs::write(r.join("both.txt"), "theirs\n").unwrap();
        run_ok(r, &["add", "both.txt"], DEFAULT_TIMEOUT, &[]).unwrap();
        run_ok(r, &["commit", "-m", "theirs add"], DEFAULT_TIMEOUT, &[]).unwrap();
        run_ok(r, &["checkout", "main"], DEFAULT_TIMEOUT, &[]).unwrap();
        std::fs::write(r.join("both.txt"), "ours\n").unwrap();
        run_ok(r, &["add", "both.txt"], DEFAULT_TIMEOUT, &[]).unwrap();
        run_ok(r, &["commit", "-m", "ours add"], DEFAULT_TIMEOUT, &[]).unwrap();
        let merge = run_git(r, &["merge", "other"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_ne!(merge.code, 0);
        // Add/add has no stage 1; fallback is stage 2 (ours).
        let stage1 = run_git(r, &["show", ":1:both.txt"], DEFAULT_TIMEOUT, &[]).unwrap();
        assert_ne!(stage1.code, 0);
        let diff = diff_content(r, "both.txt", false, None).unwrap();
        assert!(matches!(&diff.original, GradedText::Full { content } if content == "ours\n"));
    }

    #[cfg(unix)]
    #[test]
    fn read_worktree_rejects_intermediate_symlink_escape() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "a\n", "c1");
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret\n").unwrap();
        std::fs::create_dir_all(r.join("sub")).unwrap();
        // Intermediate symlink: sub/link -> outside, path sub/link/secret.txt.
        symlink(outside.path(), r.join("sub/link")).unwrap();
        let err = read_worktree(r, "sub/link/secret.txt").unwrap_err();
        assert!(
            err.contains("symlink") || err.contains("rejected"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_absolute_dir_nofollow_rejects_symlink_root_component() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        // Canonicalize the temp base first so platform prefix symlinks (e.g.
        // macOS /var → /private/var) are resolved; the intentional link is the
        // only symlink component under test.
        let base = tmp.path().canonicalize().unwrap();
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("secret.txt"), "secret\n").unwrap();
        let link = base.join("link");
        symlink(&real, &link).unwrap();
        // Path contains a symlink component. Walking with O_NOFOLLOW must reject
        // it rather than following into `real` (or any substituted target).
        let err = open_absolute_dir_nofollow(&link).unwrap_err();
        assert!(
            err.contains("symlink") || err.contains("rejected"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_absolute_dir_nofollow_opens_real_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        // Caller always feeds a canonical path (see read_worktree_nofollow_unix).
        let canonical = dir.canonicalize().unwrap();
        let fd = open_absolute_dir_nofollow(&canonical).expect("real directory must open");
        drop(fd);
    }

    #[test]
    fn diff_content_utf16_le_worktree_decodes_to_text() {
        assert_utf16_worktree_diff(false);
    }

    #[test]
    fn diff_content_utf16_be_worktree_decodes_to_text() {
        assert_utf16_worktree_diff(true);
    }

    #[test]
    fn remote_probe_unknown_without_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1", "c1");
        assert_eq!(remote_probe(r, &[]).unwrap(), "unknown");
    }

    /// T1（#55）AC1 守衛：Tauri 2 同步 command 在 main thread 執行（perf 分析既證），
    /// git 子行程會凍住 UI event loop。此測試以原始碼守護 git_service.rs 與 git_log.rs
    /// 內每個 `#[tauri::command]` 都宣告為 `pub async fn`（async command 由 async runtime
    /// 排程、不佔 main thread）。字面守衛只擋「改回同步 fn」的退化，不證明 closure 內容。
    #[test]
    fn git_commands_are_declared_async_off_the_main_thread() {
        for (name, source) in [
            ("git_service.rs", include_str!("git_service.rs")),
            ("git_log.rs", include_str!("git_log.rs")),
        ] {
            let lines: Vec<&str> = source.lines().collect();
            let mut command_count = 0usize;
            for (index, line) in lines.iter().enumerate() {
                if !line.trim_start().starts_with("#[tauri::command") {
                    continue;
                }
                let declaration = lines[index..]
                    .iter()
                    .find(|candidate| candidate.contains("fn "))
                    .unwrap_or_else(|| {
                        panic!("{name}: command attribute at line {index} has no fn declaration")
                    });
                assert!(
                    declaration.contains("pub async fn"),
                    "{name}: Tauri command must be `pub async fn` to stay off the main thread, got: {declaration}"
                );
                command_count += 1;
            }
            assert!(
                command_count >= 4,
                "{name}: expected to find Tauri commands, found {command_count}"
            );
        }
    }

    /// T1（#55）AC2：async 化後 git 讀寫可能真並發。寫操作（stage/commit）與 status 讀
    /// 併發執行必須不 panic、讀不失敗（status 走 GIT_OPTIONAL_LOCKS=0，不取鎖）、
    /// 寫入結果一致（index.lock 由 git 自身互斥）。
    #[test]
    fn concurrent_status_reads_during_stage_and_commit_writes_stay_consistent() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        test_repo::init(&root);
        test_repo::write_and_commit(&root, "base.txt", "base\n", "base");

        let writer_done = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut readers = Vec::new();
        for _ in 0..2 {
            let reader_root = root.clone();
            let done = writer_done.clone();
            let start = barrier.clone();
            readers.push(std::thread::spawn(move || {
                start.wait();
                let mut reads = 0u32;
                loop {
                    let dto = status_of(&reader_root, None)
                        .expect("concurrent status read must not fail during writes");
                    assert!(!dto.parsed.head_oid.is_empty());
                    reads += 1;
                    if done.load(Ordering::Acquire) {
                        break;
                    }
                }
                reads
            }));
        }

        barrier.wait();
        for i in 0..4 {
            let name = format!("f{i}.txt");
            std::fs::write(root.join(&name), format!("{i}\n")).unwrap();
            stage(&root, &[name]).expect("stage must succeed while status reads run");
            commit(&root, &format!("c{i}")).expect("commit must succeed while status reads run");
        }
        writer_done.store(true, Ordering::Release);
        for reader in readers {
            let reads = reader.join().expect("reader thread must not panic");
            assert!(reads > 0, "reader should have completed at least one read");
        }

        // 結果一致：工作樹乾淨、5 個 commit（base + 4）。
        let dto = status_of(&root, None).unwrap();
        assert!(dto.parsed.staged.is_empty());
        assert!(dto.parsed.unstaged.is_empty());
        assert!(dto.parsed.untracked.is_empty());
        let out = run_git(
            &root,
            &["rev-list", "--count", "HEAD"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "5");
    }

    /// review fix（#55 T1 競態）：async 化後兩個 git_detect 完成順序可反轉——
    /// 舊 workspace 的慢結果晚到時，不得覆蓋已落地的新 workspace root/watcher
    ///（generation guard），且 root 與 watcher 必須成對（單一 guard 段內寫入）。
    #[test]
    fn stale_detect_result_does_not_overwrite_newer_root_or_watcher() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let repo_old = tempfile::tempdir().unwrap();
        let repo_new = tempfile::tempdir().unwrap();
        test_repo::init(repo_old.path());
        test_repo::init(repo_new.path());

        let counter = AtomicU64::new(0);
        let repo_state: Mutex<Option<RepoHandle>> = Mutex::new(None);
        let watch_state: Mutex<Option<crate::git_watch::GitWatcher>> = Mutex::new(None);

        // 請求順序：先 detect(old)、後 detect(new)——generation 依進場順序遞增。
        let gen_old = counter.fetch_add(1, Ordering::SeqCst) + 1;
        let gen_new = counter.fetch_add(1, Ordering::SeqCst) + 1;

        // 完成順序反轉：new（快）先落地。
        let env_new = GitEnvironment::Ready {
            root: repo_new.path().to_string_lossy().into_owned(),
            version: "git version 2.50.1".into(),
        };
        let watcher_new =
            crate::git_watch::build_git_watcher(&repo_new.path().join(".git"), || {}).unwrap();
        assert!(commit_detect_result(
            gen_new,
            &counter,
            &repo_state,
            &watch_state,
            &env_new,
            Some(watcher_new),
        )
        .unwrap());
        assert!(watch_state.lock().unwrap().is_some());

        // old（慢）晚到：即使結果是 NotARepo（會清空 state）也必須被丟棄——
        // root 與 watcher 都留在 new 上。
        let applied = commit_detect_result(
            gen_old,
            &counter,
            &repo_state,
            &watch_state,
            &GitEnvironment::NotARepo,
            None,
        )
        .unwrap();
        assert!(!applied, "stale detect result must be discarded");
        assert_eq!(
            repo_state.lock().unwrap().as_ref().unwrap().root,
            repo_new.path(),
            "stale detect must not overwrite the newer repo root"
        );
        assert!(
            watch_state.lock().unwrap().is_some(),
            "stale detect must not tear down the newer watcher"
        );

        // 晚到的 Ready 結果同樣被丟棄（last-completed-wins 競態的另一半）。
        let env_old = GitEnvironment::Ready {
            root: repo_old.path().to_string_lossy().into_owned(),
            version: "git version 2.50.1".into(),
        };
        assert!(!commit_detect_result(
            gen_old,
            &counter,
            &repo_state,
            &watch_state,
            &env_old,
            None,
        )
        .unwrap());
        assert_eq!(
            repo_state.lock().unwrap().as_ref().unwrap().root,
            repo_new.path()
        );

        // 最新 generation 的 NotARepo 才可清空（原行為保留）。
        let gen_clear = counter.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(commit_detect_result(
            gen_clear,
            &counter,
            &repo_state,
            &watch_state,
            &GitEnvironment::NotARepo,
            None,
        )
        .unwrap());
        assert!(repo_state.lock().unwrap().is_none());
        assert!(watch_state.lock().unwrap().is_none());
    }

    #[test]
    fn untrusted_workspace_never_spawns_git() {
        let tmp = tempfile::tempdir().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(
            tmp.path().join("workspace-trust.json"),
        );
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        test_repo::init(&repo);
        let path = repo.to_str().unwrap().to_string();
        let before = git_spawn_count();
        let error = trust.require_trusted(&path).unwrap_err();
        assert!(
            error.contains("untrustedWorkspace"),
            "expected untrusted workspace, got {error}"
        );
        let detect_error = match detect_environment_if_trusted(&trust, &path) {
            Ok(_) => panic!("expected detect to stay closed"),
            Err(error) => error,
        };
        assert!(
            detect_error.contains("untrustedWorkspace"),
            "expected detect to stay closed, got {detect_error}"
        );
        assert_eq!(
            git_spawn_count(),
            before,
            "untrusted detect must not spawn git"
        );

        let state = GitServiceState(std::sync::Arc::new(std::sync::Mutex::new(Some(
            RepoHandle { root: repo.clone() },
        ))));
        let before = git_spawn_count();
        let wrapper_error = tauri::async_runtime::block_on(with_requested_repo_blocking(
            &state,
            &trust,
            path,
            |_| Ok(()),
        ))
        .unwrap_err();
        assert!(
            wrapper_error.contains("untrustedWorkspace"),
            "expected wrapper to stay closed, got {wrapper_error}"
        );
        assert_eq!(
            git_spawn_count(),
            before,
            "untrusted git wrapper must not spawn git"
        );
    }

    #[test]
    fn trusted_workspace_can_detect_and_run_git() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join("workspace-trust.json");
        let trust = crate::workspace_trust::WorkspaceTrustState::at(store_path);
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        test_repo::init(&repo);
        let path = repo.to_str().unwrap();
        trust.0.grant_for_tests(path);
        let env = detect_environment_if_trusted(&trust, path).unwrap();
        assert!(matches!(env, GitEnvironment::Ready { .. }));

        let state = GitServiceState(std::sync::Arc::new(std::sync::Mutex::new(Some(
            RepoHandle { root: repo.clone() },
        ))));
        tauri::async_runtime::block_on(with_requested_repo_blocking(
            &state,
            &trust,
            path.to_string(),
            |root| {
                let output = run_git(
                    root,
                    &["rev-parse", "--is-inside-work-tree"],
                    Duration::from_secs(5),
                    &[],
                )?;
                assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");
                Ok(())
            },
        ))
        .unwrap();
    }

    #[test]
    fn remote_identity_for_askpass_uses_origin_url() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        test_repo::write_and_commit(tmp.path(), "a.txt", "1", "c1");
        run_git(
            tmp.path(),
            &["remote", "add", "origin", "git@example.com:foo/bar.git"],
            DEFAULT_TIMEOUT,
            &[],
        )
        .unwrap();
        let (display, fingerprint) = remote_identity_for_askpass(tmp.path());
        let display = display.expect("origin display");
        assert!(display.contains("origin"), "{display}");
        assert!(display.contains("git@example.com:foo/bar.git"), "{display}");
        assert!(fingerprint.expect("url fingerprint").starts_with("sha256:"));
    }

    #[cfg(unix)]
    #[test]
    fn remote_askpass_operations_get_distinct_tokens() {
        let server = crate::askpass::AskpassServer::start(|_| {}).unwrap();
        let state = crate::askpass::AskpassState(Some(server));
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        let a = begin_remote_askpass(
            &state,
            tmp.path(),
            crate::askpass::AskpassOperationKind::Fetch,
            false,
        );
        let b = begin_remote_askpass(
            &state,
            tmp.path(),
            crate::askpass::AskpassOperationKind::Push,
            false,
        );
        let token_a = a
            .env()
            .iter()
            .find(|(key, _)| key == "YUZORA_ASKPASS_TOKEN")
            .map(|(_, value)| value.clone())
            .unwrap();
        let token_b = b
            .env()
            .iter()
            .find(|(key, _)| key == "YUZORA_ASKPASS_TOKEN")
            .map(|(_, value)| value.clone())
            .unwrap();
        let op_a = a
            .env()
            .iter()
            .find(|(key, _)| key == "YUZORA_ASKPASS_OPERATION")
            .map(|(_, value)| value.clone())
            .unwrap();
        let op_b = b
            .env()
            .iter()
            .find(|(key, _)| key == "YUZORA_ASKPASS_OPERATION")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert_ne!(token_a, token_b);
        assert_ne!(op_a, op_b);
        assert!(!token_a.is_empty());
        assert!(!token_b.is_empty());
    }
}

#[cfg(test)]
fn detect_environment_if_trusted(
    trust: &crate::workspace_trust::WorkspaceTrustState,
    path: &str,
) -> Result<GitEnvironment, String> {
    let _ = trust.require_trusted(path)?;
    Ok(detect_environment(Path::new(path)))
}
