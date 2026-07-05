// M2 Task 4: git_service core (detection + run_git + git_status command)

use std::path::{Path, PathBuf};
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
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .envs(extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git spawn failed: {e}"))?;
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
                let _ = child.kill();
                let _ = child.wait();
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

/// debug log：args join、code、stderr 前 200 字；不記 extra_env。
fn log_git_call(args: &[&str], code: i32, stderr: &str) {
    let mut sink = crate::logging::LogSink::new(crate::logging::default_log_dir());
    let stderr_head: String = stderr.chars().take(200).collect();
    sink.write(crate::logging::LogEvent {
        level: "debug".to_string(),
        kind: "debug".to_string(),
        source: "git_service".to_string(),
        workspace_path: None,
        event: "run_git".to_string(),
        message: format!("git {}", args.join(" ")),
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
    let mut args: Vec<&str> = vec!["status", "--porcelain=v2", "--branch", "-z"];
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

#[tauri::command]
pub fn git_stage(
    state: tauri::State<'_, GitServiceState>,
    paths: Vec<String>,
) -> Result<(), String> {
    stage(&repo_root(&state)?, &paths)
}

#[tauri::command]
pub fn git_unstage(
    state: tauri::State<'_, GitServiceState>,
    paths: Vec<String>,
) -> Result<(), String> {
    unstage(&repo_root(&state)?, &paths)
}

#[tauri::command]
pub fn git_discard(
    state: tauri::State<'_, GitServiceState>,
    paths: Vec<String>,
    untracked: Vec<String>,
) -> Result<(), String> {
    discard(&repo_root(&state)?, &paths, &untracked)
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
pub fn git_fetch_cmd(
    state: tauri::State<'_, GitServiceState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
    background: bool,
) -> Result<(), String> {
    let root = repo_root(&state)?;
    let env = askpass.env_for(background);
    run_ok(&root, &["fetch"], REMOTE_TIMEOUT, &env)?;
    Ok(())
}

#[tauri::command]
pub fn git_pull_cmd(
    state: tauri::State<'_, GitServiceState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
) -> Result<(), String> {
    let root = repo_root(&state)?;
    // pull 觸發的 merge commit 在無 TTY 下同樣需抑制 editor（見 conflict_continue 註）。
    let mut env = askpass.env_for(false);
    env.extend(editor_true());
    run_ok(&root, &["pull"], REMOTE_TIMEOUT, &env)?;
    Ok(())
}

#[tauri::command]
pub fn git_push_cmd(
    state: tauri::State<'_, GitServiceState>,
    askpass: tauri::State<'_, crate::askpass::AskpassState>,
) -> Result<(), String> {
    let root = repo_root(&state)?;
    let env = askpass.env_for(false);
    run_ok(&root, &["push"], REMOTE_TIMEOUT, &env)?;
    Ok(())
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
