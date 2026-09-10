use crate::git_service::{with_requested_repo_blocking, GitServiceState};
pub use yuzora_host::git_log::*;

// ── commands（薄包裝）────────────────────────────────────────────────────

// T1（#55）：同步 command 在 main thread 執行、git 子行程凍住 UI event loop
// → 全部 async ＋ 走 git_service::run_blocking（spawn_blocking）。repo root 取用
// `with_requested_repo_blocking` 且一律在 blocking closure 內呼叫——repo state 鎖可能
// 被長時操作（push/pull 至多 120s）持有，async body 直接 lock 會 park tokio worker。

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_log_page(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    cursor: Option<String>,
    limit: u32,
    query: Option<String>,
    author: Option<String>,
    since: Option<String>,
    until: Option<String>,
) -> Result<LogPage, String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        log_page(
            root,
            cursor.as_deref(),
            limit,
            query.as_deref(),
            author.as_deref(),
            since.as_deref(),
            until.as_deref(),
        )
    })
    .await
}

#[tauri::command]
pub async fn git_commit_detail(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    hash: String,
) -> Result<CommitDetail, String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        commit_detail(root, &hash)
    })
    .await
}

#[tauri::command]
pub async fn git_log_authors(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
) -> Result<Vec<AuthorEntry>, String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, log_authors).await
}

#[tauri::command]
pub async fn git_file_at_rev(
    state: tauri::State<'_, GitServiceState>,
    trust: tauri::State<'_, crate::workspace_trust::WorkspaceTrustState>,
    repository_root: String,
    rev: String,
    path: String,
) -> Result<FileAtRevResult, String> {
    with_requested_repo_blocking(state.inner(), trust.inner(), repository_root, move |root| {
        file_at_rev(root, &rev, &path)
    })
    .await
}
