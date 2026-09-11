use std::path::{Path, PathBuf};
use std::time::Duration;
pub use yuzora_host::git_service::*;

#[derive(Default)]
pub struct GitServiceState(pub std::sync::Arc<yuzora_host::git_registry::GitRegistry>);

pub(crate) async fn run_blocking<T, F>(task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| format!("git blocking task failed: {e}"))?
}

#[tauri::command]
pub async fn git_close_workspace(
    state: tauri::State<'_, GitServiceState>,
    path: String,
    generation: u64,
) -> Result<(), String> {
    let registry = state.0.clone();
    run_blocking(move || registry.close(&path, generation)).await
}

/// `git:state-changed` 事件 payload（#57 T3）：帶上 detect 當時的 workspace
/// 路徑，前端 listener 比對 live workspacePath 後才處理——切換 gap 內舊
/// workspace 的 .git watcher 殘留事件不得刷新新 workspace 的面板。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStateChangedEvent {
    pub workspace_root: String,
}

/// detect → watcher 建立 → State 落地的共用核心（`git_bootstrap`）。
/// 整段必須在 blocking thread 執行：git 子行程與 watcher 建立本來就 blocking；
/// repo state 鎖也可能被長時操作持有（見 `with_requested_repo_blocking`，
/// push/pull 至多 120s），在 async body 直接 lock 會 park 共用的 tokio worker
/// ——鎖等待一律留在 blocking thread。
fn detect_commit_and_watch(
    app: tauri::AppHandle,
    repo_shared: &std::sync::Arc<yuzora_host::git_registry::GitRegistry>,
    generation: u64,
    workspace_path: &str,
) -> Result<GitEnvironment, String> {
    use tauri::Emitter;
    let _job = repo_shared.try_job()?;
    let env = detect_environment(Path::new(workspace_path));
    let watcher = if let GitEnvironment::Ready { ref root, .. } = env {
        let event_root = workspace_path.to_string();
        Some(crate::git_watch::build_repository_watcher(
            Path::new(root),
            move || {
                let _ = app.emit(
                    "git:state-changed",
                    GitStateChangedEvent {
                        workspace_root: event_root.clone(),
                    },
                );
            },
        )?)
    } else {
        None
    };
    repo_shared.finish(workspace_path, generation, &env, watcher)?;
    // Frontend drops stale snapshots; registry generations independently protect authority.
    Ok(env)
}

fn detect_trusted_environment(
    trust: &crate::workspace_trust::WorkspaceTrustState,
    path: &str,
    detect: impl FnOnce() -> Result<GitEnvironment, String>,
) -> Result<GitEnvironment, String> {
    // A non-repository has no Git operation to authorize. Use the same
    // filesystem-only probe as the trust prompt, without spawning Git.
    if Path::new(path).is_dir() && !crate::workspace_trust::project_repo_presence(path) {
        return Ok(GitEnvironment::NotARepo);
    }
    let identity = trust.require_trusted(path)?;
    let env = detect()?;
    if let GitEnvironment::Ready { root, .. } = &env {
        trust.bind_session_git_root(&identity, root);
    }
    Ok(env)
}

fn detect_trusted_and_finish(
    trust: &crate::workspace_trust::WorkspaceTrustState,
    registry: &yuzora_host::git_registry::GitRegistry,
    generation: u64,
    path: &str,
    detect: impl FnOnce() -> Result<GitEnvironment, String>,
) -> Result<GitEnvironment, String> {
    let result = detect_trusted_environment(trust, path, detect);
    match &result {
        Ok(environment) if !matches!(environment, GitEnvironment::Ready { .. }) => {
            // The filesystem-only empty state bypasses detect_commit_and_watch.
            // Finish its generation too, releasing prior authority and watcher.
            registry.finish(path, generation, environment, None)?;
        }
        Err(_) => registry.close(path, generation)?,
        _ => {}
    }
    result
}

/// T3（#57）：冷開 workspace 的 git 首載單趟快照。environment 非 Ready 時
/// status/branches 為 None（前端收到 null）。Ready 落地後快照失敗時同樣為
/// None，錯誤放 `snapshot_error`（見 `bootstrap_dto`）。
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitBootstrapDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_generation: Option<u64>,
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
            workspace_generation: None,
            environment,
            status: Some(status),
            branches: Some(branches),
            snapshot_error: None,
        },
        Err(e) => GitBootstrapDto {
            workspace_generation: None,
            environment,
            status: None,
            branches: None,
            snapshot_error: Some(e),
        },
    }
}

/// Snapshot runs under this repository's operation lock and bounded job budget.
async fn bootstrap_ready_snapshot(
    registry: std::sync::Arc<yuzora_host::git_registry::GitRegistry>,
    root: PathBuf,
) -> Result<(GitStatusDto, BranchList), String> {
    run_blocking(move || {
        registry.with_repository(root.to_str().ok_or("git-path-not-utf8")?, |root| {
            let status = status_of(root, None)?;
            branches(root).map(|branches| (status, branches))
        })
    })
    .await
}

/// 冷開 workspace 的 git 面板首載（#57 T3）：detect →（Ready 時）寫入
/// RepoHandle state、建 .git watcher，再取得同一 repository 的 status／branches 快照。
/// 細粒度 `git_status_cmd`／`git_branches` 保留給後續 refresh。Ready 落地後
/// status/branches 失敗（timeout、repo 中途被刪）→ 仍回 Ok：environment
/// 照常落地、快照為 None、錯誤放 `snapshot_error`（見 `bootstrap_dto`，
/// 與舊流程「detect 成功、refresh 失敗」同語意）。
#[tauri::command]
pub async fn git_bootstrap(
    app: tauri::AppHandle,
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    path: String,
) -> Result<GitBootstrapDto, String> {
    let generation = state.0.begin(&path)?;
    let repo_shared = state.0.clone();
    let trust = trust.inner().clone();
    let env = run_blocking(move || {
        detect_trusted_and_finish(&trust, &repo_shared, generation, &path, || {
            detect_commit_and_watch(app, &repo_shared, generation, &path)
        })
    })
    .await?;
    let root = match &env {
        GitEnvironment::Ready { root, .. } => PathBuf::from(root),
        GitEnvironment::NotARepo | GitEnvironment::Missing { .. } => {
            return Ok(GitBootstrapDto {
                workspace_generation: Some(generation),
                environment: env,
                status: None,
                branches: None,
                snapshot_error: None,
            })
        }
    };
    let snapshot = bootstrap_ready_snapshot(state.0.clone(), root).await;
    let mut dto = bootstrap_dto(env, snapshot);
    dto.workspace_generation = Some(generation);
    Ok(dto)
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

/// Bind a mutating request to the repository snapshot the frontend acted on.
/// Each registered repository has an independent lock and filesystem identity;
/// closing its last workspace cancels queued/running jobs without affecting peers.
pub(crate) fn with_requested_repo<T>(
    state: &GitServiceState,
    requested_root: &str,
    operation: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    state.0.with_repository(requested_root, operation)
}

/// `with_requested_repo` 的 async 包裝：整段（含持鎖比對）移進 blocking thread，
/// 保留「compare + mutation 相對 `git_bootstrap` 切換 repo 原子」的語意——鎖不跨
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
    amend_head: Option<String>,
) -> Result<(), String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        commit_with_options(root, &message, amend_head.as_deref())
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

    /// Verify the owned bootstrap snapshot against a real repository.
    #[test]
    fn bootstrap_ready_snapshot_joins_status_and_branches() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        test_repo::write_and_commit(tmp.path(), "a.txt", "hi", "init");
        std::fs::write(tmp.path().join("b.txt"), "new").unwrap();
        let state = GitServiceState::default();
        bind_test_repo(&state, tmp.path());
        let (status, branch_list) = tauri::async_runtime::block_on(bootstrap_ready_snapshot(
            state.0.clone(),
            tmp.path().to_path_buf(),
        ))
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
        let state = GitServiceState::default();
        bind_test_repo(&state, tmp.path());
        let result = tauri::async_runtime::block_on(bootstrap_ready_snapshot(
            state.0.clone(),
            tmp.path().to_path_buf(),
        ));
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
            workspace_generation: None,
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

    fn bind_test_repo(state: &GitServiceState, root: &Path) -> u64 {
        let path = root.to_str().unwrap();
        let generation = state.0.begin(path).unwrap();
        state
            .0
            .finish(
                path,
                generation,
                &GitEnvironment::Ready {
                    root: path.into(),
                    version: "test".into(),
                },
                None,
            )
            .unwrap();
        generation
    }

    #[test]
    fn mutating_request_cannot_use_an_unregistered_or_closed_repository() {
        let repo_a = tempfile::tempdir().unwrap();
        let repo_b = tempfile::tempdir().unwrap();
        test_repo::init(repo_a.path());
        test_repo::init(repo_b.path());
        let state = GitServiceState::default();
        bind_test_repo(&state, repo_b.path());
        assert!(with_requested_repo(&state, repo_a.path().to_str().unwrap(), |_| Ok(())).is_err());
        let generation = bind_test_repo(&state, repo_a.path());
        std::fs::write(repo_a.path().join("same.txt"), "a\n").unwrap();
        with_requested_repo(&state, repo_a.path().to_str().unwrap(), |root| {
            stage(root, &["same.txt".into()])
        })
        .unwrap();
        assert_eq!(
            status_of(repo_a.path(), None).unwrap().parsed.staged[0].path,
            "same.txt"
        );
        state
            .0
            .close(repo_a.path().to_str().unwrap(), generation)
            .unwrap();
        assert!(with_requested_repo(&state, repo_a.path().to_str().unwrap(), |_| Ok(())).is_err());
        assert!(with_requested_repo(&state, repo_b.path().to_str().unwrap(), |_| Ok(())).is_ok());
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

    #[test]
    fn nonrepo_redetection_releases_repository_and_watcher() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("repo");
        let marker = folder.join(".git");
        std::fs::create_dir_all(&marker).unwrap();
        let path = folder.to_str().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(tmp.path().join("trust.json"));
        let registry = yuzora_host::git_registry::GitRegistry::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let token = Dropped(dropped.clone());
        let watcher = yuzora_host::git_watch::build_git_watcher(&marker, move || {
            let _ = &token;
        })
        .unwrap();
        let ready = registry.begin(path).unwrap();
        registry
            .finish(
                path,
                ready,
                &GitEnvironment::Ready {
                    root: path.into(),
                    version: "test".into(),
                },
                Some(watcher),
            )
            .unwrap();
        assert!(registry.with_repository(path, |_| Ok(())).is_ok());
        std::fs::remove_dir(&marker).unwrap();
        let generation = registry.begin(path).unwrap();
        let environment = detect_trusted_and_finish(&trust, &registry, generation, path, || {
            panic!("non-repository detection must not execute git")
        })
        .unwrap();
        assert!(matches!(environment, GitEnvironment::NotARepo));
        assert!(registry.with_repository(path, |_| Ok(())).is_err());
        assert!(
            dropped.load(Ordering::Acquire),
            "the old watcher callback must be dropped"
        );
    }

    #[test]
    fn stale_nonrepo_detection_preserves_new_repository_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("plain");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.to_str().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(tmp.path().join("trust.json"));
        let registry = yuzora_host::git_registry::GitRegistry::default();
        let stale = registry.begin(path).unwrap();
        let current = registry.begin(path).unwrap();
        registry
            .finish(
                path,
                current,
                &GitEnvironment::Ready {
                    root: path.into(),
                    version: "test".into(),
                },
                None,
            )
            .unwrap();
        assert!(matches!(
            detect_trusted_and_finish(&trust, &registry, stale, path, || {
                panic!("non-repository detection must not execute git")
            })
            .unwrap(),
            GitEnvironment::NotARepo
        ));
        assert!(registry.with_repository(path, |_| Ok(())).is_ok());
    }

    #[test]
    fn untrusted_plain_folder_detects_empty_state_without_running_git() {
        let tmp = tempfile::tempdir().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(
            tmp.path().join("workspace-trust.json"),
        );
        let folder = tmp.path().join("plain-folder");
        std::fs::create_dir(&folder).unwrap();
        let environment = detect_trusted_environment(&trust, folder.to_str().unwrap(), || {
            panic!("non-repository detection must not execute git")
        })
        .expect("a plain folder should have a non-repository empty state");
        assert!(matches!(environment, GitEnvironment::NotARepo));
    }

    #[test]
    fn untrusted_nested_repository_detection_still_requires_trust() {
        let tmp = tempfile::tempdir().unwrap();
        let trust = crate::workspace_trust::WorkspaceTrustState::at(
            tmp.path().join("workspace-trust.json"),
        );
        let repo = tmp.path().join("repo");
        let nested = repo.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(repo.join(".git"), "gitdir: ../worktree-metadata").unwrap();
        let result = detect_trusted_environment(&trust, nested.to_str().unwrap(), || {
            panic!("untrusted repository must not execute git")
        });
        match result {
            Err(error) => assert!(error.contains("untrustedWorkspace")),
            Ok(_) => panic!("repository detection must still require trust"),
        }
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
        let error = trust.require_trusted(&path).unwrap_err();
        assert!(
            error.contains("untrustedWorkspace"),
            "expected untrusted workspace, got {error}"
        );
        let detect_error = match detect_trusted_environment(&trust, &path, || {
            panic!("untrusted detect must not reach environment detection")
        }) {
            Ok(_) => panic!("expected detect to stay closed"),
            Err(error) => error,
        };
        assert!(
            detect_error.contains("untrustedWorkspace"),
            "expected detect to stay closed, got {detect_error}"
        );

        let state = GitServiceState::default();
        bind_test_repo(&state, &repo);
        let wrapper_error = tauri::async_runtime::block_on(with_requested_repo_blocking(
            &state,
            &trust,
            path,
            |_| -> Result<(), String> {
                panic!("untrusted git wrapper must not reach the repository operation")
            },
        ))
        .unwrap_err();
        assert!(
            wrapper_error.contains("untrustedWorkspace"),
            "expected wrapper to stay closed, got {wrapper_error}"
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

        let state = GitServiceState::default();
        bind_test_repo(&state, &repo);
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
    detect_trusted_environment(trust, path, || Ok(detect_environment(Path::new(path))))
}

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

pub fn configure_logging() {
    yuzora_host::git_service::set_logger(log_git_call);
}
