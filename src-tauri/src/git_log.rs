// T2 / Phase C1: git log 管線
//
// Log tab（commit graph＋列表＋details）與 Diff modal 的資料源。拆檔慣例比照
// git_status.rs：解析邏輯抽成純函式（parse_log_records / parse_decoration /
// parse_numstat_line）供單元測試，core 函式吃 &Path 並以 git CLI 子行程實作，
// commands 是薄包裝（沿用 git_service 的 run_git / git_err 錯誤格式與 file_content
// 的 binary／tooLarge 防護）。

use crate::file_content::{
    analyze_byte_content, ByteContent, FILE_ANALYSIS_BYTES, FULL_FEATURE_MAX_BYTES, HARD_CAP_BYTES,
};
use crate::git_oid::{
    is_full_oid, is_hex_oid_prefix as is_hex_oid_prefix_len, resolve_commit_oid,
    resolve_commit_oid_optional, GitOid,
};
use crate::git_service::{
    git_err, run_git, run_git_with_stdin, validate_relative_components,
    with_requested_repo_blocking, GitServiceState,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// git_log_page 的 limit 上限（clamp）。
const MAX_LOG_LIMIT: u32 = 500;
/// git_log_authors 去重後回傳的作者數上限。
const MAX_AUTHORS: usize = 50;
/// Opaque cursor schema version. Bump when the JSON shape changes.
const LOG_CURSOR_VERSION: u8 = 2;
/// Reject oversized cursor payloads (DoS / malformed client input).
const MAX_CURSOR_BYTES: usize = 256 * 1024;
/// Hard cap on tip hashes encoded in a cursor.
const MAX_CURSOR_TIPS: usize = 10_000;
/// Minimum hex length accepted by the hash-prefix query arm.
const MIN_OID_PREFIX_LEN: usize = 4;
/// Max cursor offset Git accepts as `--skip` (signed 32-bit integer).
const MAX_CURSOR_OFFSET: u32 = i32::MAX as u32;
/// Per-process secret for opaque cursor MAC. Restarts invalidate all cursors.
static CURSOR_SECRET: LazyLock<[u8; 32]> = LazyLock::new(|| rand::random::<[u8; 32]>());

/// --format 欄位分隔（unit separator）：避免 subject 內容含分隔字元衝突。
const FIELD_SEP: char = '\x1f';
/// --format 記錄分隔（record separator）。
const RECORD_SEP: char = '\x1e';

/// Log graph 顯示所有 branch（比照 JetBrains git log）：`--all` 把 HEAD＋refs/*
/// 全數當起點，未合入當前 branch 的 local／remote／tag commit 才會入圖。
///
/// 取 `--all`＋blacklist 而非 whitelist（`--branches --remotes --tags`）：後者不含
/// HEAD，detached HEAD 需補 positional HEAD，而 positional HEAD 在空 repo 會直接
/// fatal（"ambiguous argument"），錯誤形狀與 ref-glob 選項不同。故用 `--all`（內建含
/// HEAD、空 repo exit 0）並排除機制 ref namespace——它們是工具狀態而非歷史，多數不出
/// 現在 %D decoration，會以突兀的孤立節點污染 graph（且如 refs/stash 無已知前綴，
/// decoration 會被 classify_ref 誤標成 local）：stash、notes、original（filter-branch
/// 備份，整份改寫前歷史會重複入圖）、pull（GitHub PR refspec）、wip（magit）、
/// rewritten（rebase 進行中）、replace、bisect。
/// `--exclude` 只影響其後的 ref-glob 選項，故必須排在 `--all` 之前。
///
/// `--date-order` 是 `--all` 的必要配套：`--all` 會把「指向祖先的 ref」放進初始走訪
/// 集合，commit timestamp 平手時 parent 可能先於 child 出列；前端 graphLayout 的
/// active-lanes 演算法假設 children-before-parents，違反時 parent 會被畫成孤立節點。
/// `--date-order` 維持時間降冪但保證 child 先出列。
const ALL_REFS_ARGS: [&str; 10] = [
    "--exclude=refs/stash",
    "--exclude=refs/notes/*",
    "--exclude=refs/original/*",
    "--exclude=refs/pull/*",
    "--exclude=refs/wip/*",
    "--exclude=refs/rewritten/*",
    "--exclude=refs/replace/*",
    "--exclude=refs/bisect/*",
    "--all",
    "--date-order",
];

// ── DTO ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRef {
    pub name: String,
    /// "head" | "local" | "remote" | "tag"
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogCommit {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    /// unix 秒
    pub timestamp: i64,
    /// 完整 parent hash
    pub parents: Vec<String>,
    pub refs: Vec<LogRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub commits: Vec<LogCommit>,
    pub has_more: bool,
    /// Opaque cursor for the next page. `None` when there is no further page.
    /// Encodes a fixed tip snapshot + offset so ref movement cannot shift pages.
    pub next_cursor: Option<String>,
}

/// Signed cursor payload. Clients must treat the wire token as opaque; tips and
/// offsets are never accepted unless the process-local MAC verifies.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogCursorPayload {
    v: u8,
    /// Canonical repository root fingerprint bound at issuance.
    root: String,
    /// Fingerprint of filters/query used when the cursor was issued.
    fp: String,
    tips: Vec<String>,
    offset: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFile {
    /// M/A/D/R/C/T…（diff-tree 的 raw status 首字元）
    pub status: String,
    pub path: String,
    /// rename/copy 時為來源路徑，否則 None
    pub old_path: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    /// numstat 輸出 "-" 時為 true
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    pub subject: String,
    pub body: String,
    pub author_name: String,
    pub author_email: String,
    pub timestamp: i64,
    pub parents: Vec<String>,
    pub files: Vec<CommitFile>,
    pub total_additions: u32,
    pub total_deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorEntry {
    pub name: String,
    pub email: String,
}

/// git_file_at_rev 結果：比照 fs_service::OpenFileResult 的分級形狀（content／
/// binary／tooLarge），但輸入是 git 物件 bytes 而非路徑（故無 size 欄），並多一個
/// Missing 變體表示該 rev 無此檔（不 panic、不當錯誤）。tag = "kind" 與 OpenFileResult
/// 一致，前端可共用判別邏輯。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileAtRevResult {
    Full { content: String },
    Limited { content: String },
    TooLarge,
    Binary,
    Missing,
}

// ── 純函式解析（可單元測試，不需 spawn git）─────────────────────────────

/// 解析 %D decoration（**須以 `--decorate=full` 產生**，見 log_format 的 args）。
///
/// 用 full ref path 而非短名，才能無歧義區分 local 與 remote——短名下 local branch
/// `feature/x` 與 remote `origin/x` 都含 `/`，無法純由字串判別。full 形式：
/// - `HEAD -> refs/heads/main` → `{head, HEAD}` ＋ `{local, main}`。
/// - 裸 `HEAD`（detached）→ `{head, HEAD}`。
/// - `tag: refs/tags/v1` → `{tag, v1}`。
/// - `refs/remotes/origin/x` → `{remote, origin/x}`。
/// - `refs/heads/feature/x` → `{local, feature/x}`。
///
/// decoration 以 ", " 分隔（git 慣例）。空字串回空 Vec。回傳的 name 一律為短名
/// （剝掉 refs/heads/、refs/remotes/、refs/tags/ 前綴）。
pub fn parse_decoration(deco: &str) -> Vec<LogRef> {
    let deco = deco.trim();
    if deco.is_empty() {
        return Vec::new();
    }
    let mut refs = Vec::new();
    for raw in deco.split(", ") {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(rest) = part.strip_prefix("tag: ") {
            if let Some(r) = classify_ref(rest.trim()) {
                refs.push(r);
            }
        } else if let Some((_head, target)) = part.split_once(" -> ") {
            // "HEAD -> refs/heads/main"：HEAD 本身 ＋ 指向的 ref。
            refs.push(LogRef {
                name: "HEAD".to_string(),
                kind: "head".to_string(),
            });
            if let Some(r) = classify_ref(target.trim()) {
                refs.push(r);
            }
        } else if part == "HEAD" {
            refs.push(LogRef {
                name: "HEAD".to_string(),
                kind: "head".to_string(),
            });
        } else if let Some(r) = classify_ref(part) {
            refs.push(r);
        }
    }
    refs
}

/// 由 full ref path 分類並剝短名。非 refs/ 前綴者原樣視為 local（保守）。
fn classify_ref(full: &str) -> Option<LogRef> {
    if let Some(name) = full.strip_prefix("refs/tags/") {
        Some(LogRef {
            name: name.to_string(),
            kind: "tag".to_string(),
        })
    } else if let Some(name) = full.strip_prefix("refs/remotes/") {
        // origin/HEAD 這種 symbolic ref 略過（無實際 commit ref 意義）
        if name.ends_with("/HEAD") {
            None
        } else {
            Some(LogRef {
                name: name.to_string(),
                kind: "remote".to_string(),
            })
        }
    } else if let Some(name) = full.strip_prefix("refs/heads/") {
        Some(LogRef {
            name: name.to_string(),
            kind: "local".to_string(),
        })
    } else {
        // 已是短名或未知前綴：保守當 local。
        Some(LogRef {
            name: full.to_string(),
            kind: "local".to_string(),
        })
    }
}

/// 解析 git log 自訂 --format 的輸出（欄位以 FIELD_SEP、記錄以 RECORD_SEP 分隔）。
///
/// Production log_page no longer uses this delimiter parser for identity fields.
/// Kept for unit coverage of the historical format only.
pub fn parse_log_records(raw: &str) -> Vec<LogCommit> {
    let mut commits = Vec::new();
    for record in raw.split(RECORD_SEP) {
        let record = record.trim_matches(['\n', '\r']);
        if record.is_empty() {
            continue;
        }
        let fields: Vec<&str> = record.split(FIELD_SEP).collect();
        if fields.len() < 8 {
            continue;
        }
        let parents: Vec<String> = fields[6].split_whitespace().map(String::from).collect();
        commits.push(LogCommit {
            hash: fields[0].to_string(),
            short_hash: fields[1].to_string(),
            subject: fields[2].to_string(),
            author_name: fields[3].to_string(),
            author_email: fields[4].to_string(),
            timestamp: fields[5].parse().unwrap_or(0),
            parents,
            refs: parse_decoration(fields[7]),
        });
    }
    commits
}

/// 解析一行 `git ... --numstat -z` 的 numstat（additions\tdeletions\tpath），
/// 不含 rename 的 old\0new 額外欄（rename 由 --name-status 提供，見 commit_detail）。
/// binary 檔 additions/deletions 為 "-"。回 (additions, deletions, binary, path)。
/// 格式不符回 None。
pub fn parse_numstat_line(line: &str) -> Option<(u32, u32, bool, String)> {
    let mut it = line.splitn(3, '\t');
    let a = it.next()?;
    let d = it.next()?;
    let path = it.next()?;
    if path.is_empty() {
        return None;
    }
    let binary = a == "-" || d == "-";
    let additions = if binary { 0 } else { a.parse().ok()? };
    let deletions = if binary { 0 } else { d.parse().ok()? };
    Some((additions, deletions, binary, path.to_string()))
}

// ── core（吃 &Path，spawn git）────────────────────────────────────────────

/// Snapshot eligible graph tips once (first page) and reuse them for later pages.
///
/// Includes local/remote branches + tags + detached HEAD. Mechanism refs
/// (stash/notes/original/pull/wip/rewritten/replace/bisect) are never listed by
/// the for-each-ref prefixes below, matching ALL_REFS_ARGS exclusions. Tips are
/// full object IDs, deduped and sorted for a stable cursor encoding.
fn snapshot_tips(root: &Path) -> Result<Vec<String>, String> {
    let mut tips: BTreeSet<String> = BTreeSet::new();

    let out = run_git_no_replace(
        root,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    if out.code != 0 {
        return Err(git_err("for-each-ref", &out.stderr));
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let oid = line.trim();
        if oid.is_empty() {
            continue;
        }
        if !is_full_oid(oid) {
            return Err("git for-each-ref returned a non-OID tip".to_string());
        }
        tips.insert(oid.to_string());
    }

    // Detached HEAD (and the current tip when attached) must be included so the
    // graph never drops the active commit when it is not also a named ref tip.
    if let Some(head) = resolve_commit_oid_optional(root, "HEAD")? {
        tips.insert(head.into_string());
    }

    Ok(tips.into_iter().collect())
}

/// Hex-only OID prefix for the hash query arm (never branch names / HEAD / ~).
fn is_hex_oid_prefix(value: &str) -> bool {
    is_hex_oid_prefix_len(value, MIN_OID_PREFIX_LEN)
}

fn root_fingerprint(root: &Path) -> String {
    root.canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| root.to_string_lossy().into_owned())
}

/// Normalize optional textual filters: trim; empty / whitespace-only → None.
/// Applied before both fingerprinting and execution so `None`, `Some("")`, and
/// `Some("  ")` share one canonical semantics.
fn normalize_filter(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

/// Fingerprint of already-normalized filters. Callers must pass values through
/// `normalize_filter` first so empty and absent stay equivalent.
fn filters_fingerprint(
    query: Option<&str>,
    author: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        query.unwrap_or(""),
        author.unwrap_or(""),
        since.unwrap_or(""),
        until.unwrap_or("")
    )
}

fn mac_cursor_payload(payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CURSOR_SECRET.as_slice());
    hasher.update([0u8]);
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn ct_eq_hex(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut v = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        v |= x ^ y;
    }
    v == 0
}

fn encode_cursor(
    root_fp: &str,
    filters_fp: &str,
    tips: &[String],
    offset: u32,
) -> Result<Option<String>, String> {
    if tips.is_empty() {
        return Ok(None);
    }
    if offset > MAX_CURSOR_OFFSET {
        return Err("git log cursor offset is out of range".to_string());
    }
    let payload = LogCursorPayload {
        v: LOG_CURSOR_VERSION,
        root: root_fp.to_string(),
        fp: filters_fp.to_string(),
        tips: tips.to_vec(),
        offset,
    };
    let raw = serde_json::to_string(&payload)
        .map_err(|e| format!("git log could not encode cursor: {e}"))?;
    if raw.len() > MAX_CURSOR_BYTES {
        return Err("git log cursor exceeds size limit".to_string());
    }
    let body = URL_SAFE_NO_PAD.encode(raw.as_bytes());
    let tag = mac_cursor_payload(&body);
    let token = format!("{body}.{tag}");
    if token.len() > MAX_CURSOR_BYTES {
        return Err("git log cursor exceeds size limit".to_string());
    }
    Ok(Some(token))
}

fn decode_cursor(raw: &str, root_fp: &str, filters_fp: &str) -> Result<(Vec<String>, u32), String> {
    if raw.len() > MAX_CURSOR_BYTES {
        return Err("git log cursor exceeds size limit".to_string());
    }
    let (body, tag) = raw
        .split_once('.')
        .ok_or_else(|| "git log cursor is malformed".to_string())?;
    let expected = mac_cursor_payload(body);
    if !ct_eq_hex(tag, &expected) {
        return Err("git log cursor is invalid or tampered".to_string());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(body.as_bytes())
        .map_err(|_| "git log cursor is malformed".to_string())?;
    let cursor: LogCursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| "git log cursor is malformed".to_string())?;
    if cursor.v != LOG_CURSOR_VERSION {
        return Err(format!(
            "git log cursor version {} is unsupported",
            cursor.v
        ));
    }
    if cursor.tips.len() > MAX_CURSOR_TIPS {
        return Err("git log cursor tip count exceeds limit".to_string());
    }
    if cursor.tips.is_empty() {
        return Err("git log cursor has no tips".to_string());
    }
    for tip in &cursor.tips {
        if !is_full_oid(tip) {
            return Err("git log cursor contains a non-OID tip".to_string());
        }
    }
    if cursor.root != root_fp {
        return Err("git log cursor does not match repository".to_string());
    }
    if cursor.fp != filters_fp {
        return Err("git log cursor does not match filters".to_string());
    }
    if cursor.offset > MAX_CURSOR_OFFSET {
        return Err("git log cursor offset is out of range".to_string());
    }
    Ok((cursor.tips, cursor.offset))
}

fn no_replace_env() -> Vec<(String, String)> {
    vec![("GIT_NO_REPLACE_OBJECTS".to_string(), "1".to_string())]
}

fn run_git_no_replace(root: &Path, args: &[&str]) -> Result<crate::git_service::GitOutput, String> {
    run_git(root, args, DEFAULT_TIMEOUT, &no_replace_env())
}

fn is_reachable_from_tips(root: &Path, commit: &str, tips: &[String]) -> Result<bool, String> {
    for tip in tips {
        if tip == commit {
            return Ok(true);
        }
        // exit 0 ⇒ commit is an ancestor of tip (or equal).
        let out = run_git_no_replace(root, &["merge-base", "--is-ancestor", commit, tip])?;
        if out.code == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn page_from_slice(
    commits: Vec<LogCommit>,
    has_more: bool,
    root_fp: &str,
    filters_fp: &str,
    tips: &[String],
    offset: u32,
) -> Result<LogPage, String> {
    if offset > MAX_CURSOR_OFFSET {
        return Err("git log cursor offset is out of range".to_string());
    }
    if has_more && commits.is_empty() {
        return Err("git log cursor offset is out of range".to_string());
    }
    // Bound next offset so every emitted cursor is consumable by Git `--skip`.
    let next_offset = match offset.checked_add(commits.len() as u32) {
        Some(n) if n <= MAX_CURSOR_OFFSET => n,
        Some(_) | None if has_more => {
            return Err("git log cursor offset is out of range".to_string());
        }
        Some(n) => n,
        None => return Err("git log cursor offset is out of range".to_string()),
    };
    if has_more && next_offset <= offset {
        return Err("git log cursor failed to progress".to_string());
    }
    let next_cursor = if has_more {
        encode_cursor(root_fp, filters_fp, tips, next_offset)?
    } else {
        None
    };
    Ok(LogPage {
        commits,
        has_more,
        next_cursor,
    })
}

/// Canonical date-order rank for fixed tips (child-before-parent).
fn date_order_rank(
    root: &Path,
    tips: &[String],
) -> Result<std::collections::HashMap<String, usize>, String> {
    let fmt = "%H";
    let format_arg = format!("--format={fmt}");
    let mut args: Vec<&str> = vec!["log", &format_arg, "--date-order"];
    for tip in tips {
        args.push(tip.as_str());
    }
    let out = run_git_no_replace(root, &args)?;
    if out.code != 0 {
        let stderr = out.stderr.to_lowercase();
        if stderr.contains("does not have any commits") || stderr.contains("bad default revision") {
            return Ok(std::collections::HashMap::new());
        }
        return Err(git_err("log", &out.stderr));
    }
    let mut rank = std::collections::HashMap::new();
    for (i, line) in String::from_utf8_lossy(&out.stdout).lines().enumerate() {
        let hash = line.trim();
        if hash.is_empty() {
            continue;
        }
        if !is_full_oid(hash) {
            return Err("git log returned a non-OID hash while ranking commits".to_string());
        }
        rank.entry(hash.to_string()).or_insert(i);
    }
    Ok(rank)
}

/// 一頁 commit 歷史，以固定 tip 快照分頁（opaque signed cursor），避免 refs 移動造成 skip 漏頁。
///
/// `cursor = None`：快照 heads/remotes/tags + HEAD，回第一頁。
/// `cursor = Some(...)`：沿用快照 tip 集合與 offset，ref 增刪改不得改變已開 cursor 的流。
///
/// 無 query：`git log --date-order --skip N --max-count M+1 <fixed-tips>`。
/// query（OR）：message / author / hex-hash-prefix 三集合在固定 tips 上聯集後，
/// 依固定 tip 的 date-order 排序再切頁。
pub fn log_page(
    root: &Path,
    cursor: Option<&str>,
    limit: u32,
    query: Option<&str>,
    author: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<LogPage, String> {
    if limit == 0 {
        return Err("git log limit must be greater than zero".to_string());
    }
    let limit = limit.min(MAX_LOG_LIMIT);
    // Normalize optional textual filters before fingerprint AND execution so
    // None / "" / whitespace share one canonical semantics.
    let query = normalize_filter(query);
    let author = normalize_filter(author);
    let since = normalize_filter(since);
    let until = normalize_filter(until);
    let root_fp = root_fingerprint(root);
    let filters_fp = filters_fingerprint(query, author, since, until);

    let (tips, offset) = match cursor {
        None => (snapshot_tips(root)?, 0u32),
        Some(raw) => decode_cursor(raw, &root_fp, &filters_fp)?,
    };
    if offset > MAX_CURSOR_OFFSET {
        return Err("git log cursor offset is out of range".to_string());
    }

    if tips.is_empty() {
        return Ok(LogPage {
            commits: Vec::new(),
            has_more: false,
            next_cursor: None,
        });
    }

    // 共用的過濾 flag（author 精確 / since / until）。
    let author_arg = author.map(|a| format!("--author=^{} <", regex_escape(a)));
    let since_arg = since.map(|s| format!("--since={s}"));
    let until_arg = until.map(|u| format!("--until={u}"));
    let mut filters: Vec<&str> = Vec::new();
    if let Some(a) = &author_arg {
        filters.push(a.as_str());
    }
    if let Some(s) = &since_arg {
        filters.push(s.as_str());
    }
    if let Some(u) = &until_arg {
        filters.push(u.as_str());
    }

    match query.map(str::trim).filter(|q| !q.is_empty()) {
        None => {
            let skip_arg = format!("--skip={offset}");
            let max_arg = format!("--max-count={}", limit.saturating_add(1));
            let mut args: Vec<&str> = vec!["log", "--date-order", &skip_arg, &max_arg];
            args.extend(filters.iter().copied());
            for tip in &tips {
                args.push(tip.as_str());
            }
            let fetched = run_log(root, &args)?;
            let has_more = fetched.len() > limit as usize;
            let mut commits = fetched;
            commits.truncate(limit as usize);
            page_from_slice(commits, has_more, &root_fp, &filters_fp, &tips, offset)
        }
        Some(q) => {
            // query：在固定 tips 上聯集 message/author/hex-hash，再依 date-order rank 排序。
            let mut merged: Vec<LogCommit> = Vec::new();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

            let grep_arg = format!("--grep={q}");
            // Message arm always runs (AND with common filters).
            {
                let mut args: Vec<&str> = vec!["log", "--date-order", "-i", grep_arg.as_str()];
                args.extend(filters.iter().copied());
                for tip in &tips {
                    args.push(tip.as_str());
                }
                for c in run_log(root, &args)? {
                    if seen.insert(c.hash.clone()) {
                        merged.push(c);
                    }
                }
            }
            // Author-query arm: only when no exact author filter is set.
            // Combining `--author=query` with `--author=^filter` would OR the two
            // and silently drop the exact-filter contract.
            if author.is_none() {
                let qauthor_arg = format!("--author={q}");
                let mut args: Vec<&str> = vec!["log", "--date-order", "-i", qauthor_arg.as_str()];
                args.extend(filters.iter().copied());
                for tip in &tips {
                    args.push(tip.as_str());
                }
                for c in run_log(root, &args)? {
                    if seen.insert(c.hash.clone()) {
                        merged.push(c);
                    }
                }
            }
            // Hash arm: only pure hex OID prefixes, never branch/HEAD/ancestry syntax.
            if is_hex_oid_prefix(q) {
                if let Some(full) = resolve_commit_oid_optional(root, q)? {
                    let full = full.into_string();
                    if is_reachable_from_tips(root, &full, &tips)? && seen.insert(full.clone()) {
                        // Exact commit only (`--no-walk`); never walk to a matching ancestor.
                        let mut args: Vec<&str> = vec!["log", "--no-walk", full.as_str()];
                        args.extend(filters.iter().copied());
                        for c in run_log(root, &args)? {
                            if c.hash == full {
                                merged.push(c);
                            }
                        }
                    }
                }
            }

            let rank = date_order_rank(root, &tips)?;
            merged.sort_by(|a, b| {
                let ra = rank.get(&a.hash).copied().unwrap_or(usize::MAX);
                let rb = rank.get(&b.hash).copied().unwrap_or(usize::MAX);
                ra.cmp(&rb)
                    .then_with(|| b.timestamp.cmp(&a.timestamp))
                    .then_with(|| a.hash.cmp(&b.hash))
            });

            let total = merged.len();
            if (offset as usize) > total {
                return Err("git log cursor offset is out of range".to_string());
            }
            let start = offset as usize;
            let end = start.saturating_add(limit as usize).min(total);
            let has_more = end < total;
            page_from_slice(
                merged[start..end].to_vec(),
                has_more,
                &root_fp,
                &filters_fp,
                &tips,
                offset,
            )
        }
    }
}

/// 跑一趟 git log 並解析。統一注入 `--decorate=full`：%D 才會給 full ref path
/// （refs/heads/、refs/remotes/、refs/tags/），供 parse_decoration 無歧義分類。
/// 呼叫端傳入的 args 應以 "log" 開頭，此處在其後插入 --decorate=full。
/// Always disables replace objects so fixed-snapshot pagination cannot be warped
/// by `refs/replace` introduced after the cursor was issued.
fn run_log(root: &Path, args: &[&str]) -> Result<Vec<LogCommit>, String> {
    let oids = collect_log_oids(root, args)?;
    hydrate_commits(root, &oids)
}

fn collect_log_oids(root: &Path, args: &[&str]) -> Result<Vec<GitOid>, String> {
    let mut full_args: Vec<&str> = Vec::with_capacity(args.len() + 1);
    if let Some((first, rest)) = args.split_first() {
        full_args.push(first);
        full_args.push("--format=%H");
        full_args.extend_from_slice(rest);
    } else {
        full_args.extend_from_slice(args);
    }
    let out = run_git_no_replace(root, &full_args)?;
    if out.code != 0 {
        let stderr = out.stderr.to_lowercase();
        if stderr.contains("does not have any commits") || stderr.contains("bad default revision") {
            return Ok(Vec::new());
        }
        return Err(git_err("log", &out.stderr));
    }
    let mut oids = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let hash = line.trim();
        if hash.is_empty() {
            continue;
        }
        oids.push(GitOid::parse(hash)?);
    }
    Ok(oids)
}

struct CommitMeta {
    subject: String,
    body: String,
    author_name: String,
    author_email: String,
    timestamp: i64,
    parents: Vec<String>,
}

fn hydrate_commits(root: &Path, oids: &[GitOid]) -> Result<Vec<LogCommit>, String> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let decorations = ref_decoration_map(root)?;
    let objects = cat_file_commits(root, oids)?;
    let mut commits = Vec::with_capacity(oids.len());
    for oid in oids {
        let raw = objects
            .get(oid.as_str())
            .ok_or_else(|| format!("git log missing commit object {}", oid.as_str()))?;
        let meta = parse_commit_object(raw)?;
        commits.push(LogCommit {
            hash: oid.as_str().to_string(),
            short_hash: oid.short().to_string(),
            subject: meta.subject,
            author_name: meta.author_name,
            author_email: meta.author_email,
            timestamp: meta.timestamp,
            parents: meta.parents,
            refs: decorations.get(oid.as_str()).cloned().unwrap_or_default(),
        });
    }
    Ok(commits)
}

fn ref_decoration_map(
    root: &Path,
) -> Result<std::collections::HashMap<String, Vec<LogRef>>, String> {
    let out = run_git_no_replace(
        root,
        &[
            "for-each-ref",
            "--format=%(objectname)%00%(*objectname)%00%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    if out.code != 0 {
        return Err(git_err("for-each-ref", &out.stderr));
    }
    let mut map: std::collections::HashMap<String, Vec<LogRef>> = std::collections::HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\0');
        let objectname = parts
            .next()
            .ok_or_else(|| "git for-each-ref returned a malformed record".to_string())?;
        let peeled = parts.next().unwrap_or("");
        let refname = parts
            .next()
            .ok_or_else(|| "git for-each-ref returned a malformed record".to_string())?;
        let tip = if peeled.is_empty() {
            objectname
        } else {
            peeled
        };
        if !is_full_oid(tip) {
            return Err("git for-each-ref returned a non-OID decoration target".to_string());
        }
        if let Some(parsed) = classify_ref(refname) {
            map.entry(tip.to_ascii_lowercase())
                .or_default()
                .push(parsed);
        }
    }
    if let Some(head) = resolve_commit_oid_optional(root, "HEAD")? {
        let refs = map.entry(head.as_str().to_string()).or_default();
        if !refs.iter().any(|r| r.kind == "head") {
            refs.insert(
                0,
                LogRef {
                    name: "HEAD".to_string(),
                    kind: "head".to_string(),
                },
            );
        }
    }
    Ok(map)
}

fn cat_file_commits(
    root: &Path,
    oids: &[GitOid],
) -> Result<std::collections::HashMap<String, Vec<u8>>, String> {
    if oids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let mut stdin = Vec::new();
    for oid in oids {
        stdin.extend_from_slice(oid.as_str().as_bytes());
        stdin.push(b'\n');
    }
    let out = run_git_with_stdin(
        root,
        &["cat-file", "--batch"],
        DEFAULT_TIMEOUT,
        &no_replace_env(),
        &stdin,
    )?;
    if out.code != 0 {
        return Err(git_err("cat-file", &out.stderr));
    }
    parse_cat_file_batch(&out.stdout, oids)
}

fn parse_cat_file_batch(
    stdout: &[u8],
    oids: &[GitOid],
) -> Result<std::collections::HashMap<String, Vec<u8>>, String> {
    let mut pos = 0usize;
    let mut result = std::collections::HashMap::new();
    for oid in oids {
        let nl = stdout[pos..]
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| "git cat-file: truncated batch header".to_string())?;
        let line = std::str::from_utf8(&stdout[pos..pos + nl])
            .map_err(|_| "git cat-file: non-utf8 batch header".to_string())?;
        pos += nl + 1;
        if line.ends_with(" missing") {
            return Err(format!("git log missing commit object {}", oid.as_str()));
        }
        let mut parts = line.split(' ');
        let reported = parts
            .next()
            .ok_or_else(|| "git cat-file: malformed batch header".to_string())?;
        let kind = parts
            .next()
            .ok_or_else(|| "git cat-file: malformed batch header".to_string())?;
        let size_text = parts
            .next()
            .ok_or_else(|| "git cat-file: malformed batch header".to_string())?;
        if parts.next().is_some() {
            return Err("git cat-file: malformed batch header".to_string());
        }
        if reported != oid.as_str() {
            return Err("git cat-file: oid mismatch".to_string());
        }
        if kind != "commit" {
            return Err(format!("git cat-file: expected commit, got {kind}"));
        }
        let size: usize = size_text
            .parse()
            .map_err(|_| "git cat-file: invalid object size".to_string())?;
        if pos + size > stdout.len() {
            return Err("git cat-file: truncated object".to_string());
        }
        let content = stdout[pos..pos + size].to_vec();
        pos += size;
        if pos >= stdout.len() || stdout[pos] != b'\n' {
            return Err("git cat-file: missing record terminator".to_string());
        }
        pos += 1;
        result.insert(oid.as_str().to_string(), content);
    }
    if pos != stdout.len() {
        return Err("git cat-file: unexpected trailing bytes".to_string());
    }
    Ok(result)
}

fn parse_commit_object(raw: &[u8]) -> Result<CommitMeta, String> {
    let text = String::from_utf8_lossy(raw);
    let (header, message) = text
        .split_once("\n\n")
        .ok_or_else(|| "git commit object missing header/message separator".to_string())?;
    let mut headers: Vec<String> = Vec::new();
    for line in header.split('\n') {
        if let Some(rest) = line.strip_prefix(' ') {
            let last = headers
                .last_mut()
                .ok_or_else(|| "git commit object has a dangling continuation line".to_string())?;
            last.push('\n');
            last.push_str(rest);
            continue;
        }
        headers.push(line.to_string());
    }
    let mut parents = Vec::new();
    let mut author_line = None;
    for header_line in &headers {
        if let Some(parent) = header_line.strip_prefix("parent ") {
            let parent = parent.trim();
            if !is_full_oid(parent) {
                return Err("git commit object has a malformed parent".to_string());
            }
            parents.push(parent.to_ascii_lowercase());
        } else if let Some(author) = header_line.strip_prefix("author ") {
            author_line = Some(author.to_string());
        }
    }
    let author_line = author_line.ok_or_else(|| "git commit object missing author".to_string())?;
    let (author_name, author_email, timestamp) = parse_ident_line(&author_line)?;
    let message = message.trim_end_matches(['\n', '\r']);
    let (subject, body) = match message.split_once('\n') {
        None => (message.to_string(), String::new()),
        Some((subject, rest)) => {
            let body = rest.strip_prefix('\n').unwrap_or(rest);
            (
                subject.to_string(),
                body.trim_end_matches(['\n', '\r']).to_string(),
            )
        }
    };
    Ok(CommitMeta {
        subject,
        body,
        author_name,
        author_email,
        timestamp,
        parents,
    })
}

fn parse_ident_line(line: &str) -> Result<(String, String, i64), String> {
    let lt = line
        .rfind('<')
        .ok_or_else(|| "git commit object author is missing an email".to_string())?;
    let gt = line[lt..]
        .find('>')
        .map(|idx| lt + idx)
        .ok_or_else(|| "git commit object author is missing an email terminator".to_string())?;
    let name = line[..lt].trim().to_string();
    let email = line[lt + 1..gt].to_string();
    let timestamp = line[gt + 1..]
        .split_whitespace()
        .next()
        .ok_or_else(|| "git commit object author is missing a timestamp".to_string())?
        .parse::<i64>()
        .map_err(|_| "git commit object author has a malformed timestamp".to_string())?;
    Ok((name, email, timestamp))
}

/// 轉義 regex 特殊字元（git --author 是 regex）供精確錨定 `^...$`。
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 單一 commit 的詳情。
///
/// header（subject/body/author/timestamp/parents）用 `git show -s --format=...`。
/// 檔案清單合併兩個來源（皆用 `git show --format= --first-parent`，見下方 rationale）：
///   - `--name-status --find-renames`：拿 status 與 rename 的 old_path。
///   - `--numstat`：拿 additions/deletions/binary（binary 為 "-"）。
///
/// 兩者以 new-path 為 key 對齊；`-z` 讓含空白／特殊字元的路徑安全（不轉義）。
///
/// 為何用 `git show --format= --first-parent` 而非 `diff-tree`：
///   - merge commit：以「對第一個 parent 的 diff」為準（只顯示該分支引入的變更）。
///     `diff-tree -m --first-parent` 在此情境會誤帶入其他 parent 的檔案；`git show
///     --first-parent` 正確給出 first-parent diff。
///   - root commit（無 parent）：`git show` 直接對空樹 diff（顯示所有新增檔），無需
///     額外 `--root`；`diff-tree --first-parent` 對 root 反而輸出空。
///
/// `--format=`（空）讓 show 不印 header，`-z` 下輸出即純 diff 資料（無前導分隔）。
pub fn commit_detail(root: &Path, hash: &str) -> Result<CommitDetail, String> {
    let oid = resolve_commit_oid(root, hash)?;
    let objects = cat_file_commits(root, std::slice::from_ref(&oid))?;
    let raw = objects
        .get(oid.as_str())
        .ok_or_else(|| format!("git show: missing commit object {}", oid.as_str()))?;
    let meta = parse_commit_object(raw)?;
    let subject = meta.subject;
    let author_name = meta.author_name;
    let author_email = meta.author_email;
    let timestamp = meta.timestamp;
    let parents = meta.parents;
    let body = meta.body;
    let oid_s = oid.as_str();

    // name-status（拿 status＋rename old_path），first-parent。
    let name_status = run_git(
        root,
        &[
            "show",
            "--format=",
            "--first-parent",
            "--name-status",
            "--find-renames",
            "-z",
            "--end-of-options",
            oid_s,
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if name_status.code != 0 {
        return Err(git_err("show", &name_status.stderr));
    }

    // numstat（拿 additions/deletions/binary），同條件。
    let numstat = run_git(
        root,
        &[
            "show",
            "--format=",
            "--first-parent",
            "--numstat",
            "--find-renames",
            "-z",
            "--end-of-options",
            oid_s,
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if numstat.code != 0 {
        return Err(git_err("show", &numstat.stderr));
    }

    let files = merge_file_changes(
        &String::from_utf8_lossy(&name_status.stdout),
        &String::from_utf8_lossy(&numstat.stdout),
    );
    let total_additions = files.iter().map(|f| f.additions).sum();
    let total_deletions = files.iter().map(|f| f.deletions).sum();

    Ok(CommitDetail {
        subject,
        body,
        author_name,
        author_email,
        timestamp,
        parents,
        files,
        total_additions,
        total_deletions,
    })
}

/// 解析 `--name-status -z` 輸出：以 NUL 分隔的 token 流。
/// 一般：`<status>\0<path>`；rename/copy（R/C）：`<statusNNN>\0<old>\0<new>`。
/// 回 map: new-path → (status_char, old_path)。純函式可測。
pub fn parse_name_status_z(raw: &str) -> Vec<(String, String, Option<String>)> {
    let mut tokens = raw.split('\0').filter(|t| !t.is_empty());
    let mut out = Vec::new();
    while let Some(status) = tokens.next() {
        let code = status.chars().next().unwrap_or(' ');
        if code == 'R' || code == 'C' {
            // rename/copy：接 old, new 兩個路徑
            let old = match tokens.next() {
                Some(o) => o.to_string(),
                None => break,
            };
            let new = match tokens.next() {
                Some(n) => n.to_string(),
                None => break,
            };
            out.push((code.to_string(), new, Some(old)));
        } else {
            let path = match tokens.next() {
                Some(p) => p.to_string(),
                None => break,
            };
            out.push((code.to_string(), path, None));
        }
    }
    out
}

/// 解析 `--numstat -z` 輸出。一般 token：`<add>\t<del>\t<path>`。
/// rename/copy 時 numstat 把 add/del/以及 old\0new 拆成三段：`<add>\t<del>\t`（同一
/// token 尾端無 path）＋下一 token = old ＋再下一 token = new。回 map: new-path →
/// (additions, deletions, binary)。純函式可測。
pub fn parse_numstat_z(raw: &str) -> Vec<(String, u32, u32, bool)> {
    let mut tokens = raw.split('\0').filter(|t| !t.is_empty()).peekable();
    let mut out = Vec::new();
    while let Some(tok) = tokens.next() {
        // tok 形如 "add\tdel\tpath" 或 rename 的 "add\tdel\t"（path 為空）
        let mut it = tok.splitn(3, '\t');
        let a = it.next().unwrap_or("");
        let d = it.next().unwrap_or("");
        let path_part = it.next().unwrap_or("");
        let binary = a == "-" || d == "-";
        let additions = if binary { 0 } else { a.parse().unwrap_or(0) };
        let deletions = if binary { 0 } else { d.parse().unwrap_or(0) };
        let path = if path_part.is_empty() {
            // rename：接 old, new；以 new 為 key
            let _old = tokens.next();
            match tokens.next() {
                Some(new) => new.to_string(),
                None => break,
            }
        } else {
            path_part.to_string()
        };
        out.push((path, additions, deletions, binary));
    }
    out
}

/// 合併 name-status 與 numstat（以 new-path 對齊），回 CommitFile 清單。
/// 以 name-status 為主序（保留 git 輸出順序），numstat 補 additions/deletions/binary。
pub fn merge_file_changes(name_status_raw: &str, numstat_raw: &str) -> Vec<CommitFile> {
    let statuses = parse_name_status_z(name_status_raw);
    let stats = parse_numstat_z(numstat_raw);
    let stat_map: std::collections::HashMap<&str, (u32, u32, bool)> = stats
        .iter()
        .map(|(p, a, d, b)| (p.as_str(), (*a, *d, *b)))
        .collect();
    statuses
        .into_iter()
        .map(|(status, path, old_path)| {
            let (additions, deletions, binary) = stat_map
                .get(path.as_str())
                .copied()
                .unwrap_or((0, 0, false));
            CommitFile {
                status,
                path,
                old_path,
                additions,
                deletions,
                binary,
            }
        })
        .collect()
}

/// 去重的作者清單（依出現次數降冪，上限 MAX_AUTHORS）。給 filter 的 User 下拉用。
/// 同 log_page 涵蓋所有 branch（--all），否則其他 branch 獨有的作者無法被篩選。
pub fn log_authors(root: &Path) -> Result<Vec<AuthorEntry>, String> {
    let fmt = format!("--format=%an{f}%ae", f = FIELD_SEP);
    let mut args: Vec<&str> = vec!["log", &fmt];
    args.extend(ALL_REFS_ARGS);
    let out = run_git(root, &args, DEFAULT_TIMEOUT, &[])?;
    if out.code != 0 {
        let stderr = out.stderr.to_lowercase();
        if stderr.contains("does not have any commits") || stderr.contains("bad default revision") {
            return Ok(Vec::new());
        }
        return Err(git_err("log", &out.stderr));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(dedup_authors(&text))
}

/// 純函式：解析每行 "name\x1femail"，依出現次數降冪去重（tie 保持首次出現序），上限 MAX_AUTHORS。
pub fn dedup_authors(raw: &str) -> Vec<AuthorEntry> {
    // key → (count, first_seen_idx)；order 保留首次出現序。
    let mut stats: std::collections::HashMap<(String, String), (usize, usize)> =
        std::collections::HashMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    for line in raw.lines() {
        if line.is_empty() {
            continue;
        }
        let (name, email) = match line.split_once(FIELD_SEP) {
            Some((n, e)) => (n.to_string(), e.to_string()),
            None => continue,
        };
        let key = (name, email);
        match stats.get_mut(&key) {
            Some(entry) => entry.0 += 1,
            None => {
                stats.insert(key.clone(), (1, order.len()));
                order.push(key);
            }
        }
    }
    // 依 count 降冪、first_seen_idx 升冪（stable tie-break）排序。
    order.sort_by(|a, b| {
        let (ca, ia) = stats[a];
        let (cb, ib) = stats[b];
        cb.cmp(&ca).then_with(|| ia.cmp(&ib))
    });
    order.truncate(MAX_AUTHORS);
    order
        .into_iter()
        .map(|(name, email)| AuthorEntry { name, email })
        .collect()
}

/// bytes 過分級（比照 fs_service::classify_and_read 的門檻與 binary 偵測，但輸入是
/// git 物件 bytes）。無 size 欄；回 FileAtRevResult。
///
/// UTF-16（BOM 開頭）比照 classify_and_read：encoding_rs 解碼（decode() 的 BOM 嗅探
/// 會剝除 BOM）成文字後依 byte 大小回 full/limited——歷史版本只讀，解碼後的文字對
/// diff 有用；fold 成 binary 會讓 UTF-16 檔案無法看歷史 diff，與 worktree 側可讀的
/// 行為不對稱。encoding_rs 為 lossy 解碼（不合法序列以 U+FFFD 取代、不會失敗），
/// 與 classify_and_read 同樣忽略 had_errors。
fn grade_object_bytes(bytes: &[u8]) -> FileAtRevResult {
    if bytes.len() as u64 > HARD_CAP_BYTES {
        return FileAtRevResult::TooLarge;
    }
    let sniff = &bytes[..bytes.len().min(FILE_ANALYSIS_BYTES)];
    let graded = match analyze_byte_content(sniff) {
        ByteContent::Binary => return FileAtRevResult::Binary,
        ByteContent::Utf16Le | ByteContent::Utf16Be => {
            let codec =
                if analyze_byte_content(&bytes[..bytes.len().min(2)]) == ByteContent::Utf16Be {
                    encoding_rs::UTF_16BE
                } else {
                    encoding_rs::UTF_16LE
                };
            let (cow, _, _) = codec.decode(bytes);
            cow.into_owned()
        }
        ByteContent::Text => String::from_utf8_lossy(bytes).into_owned(),
    };
    if bytes.len() as u64 > FULL_FEATURE_MAX_BYTES {
        FileAtRevResult::Limited { content: graded }
    } else {
        FileAtRevResult::Full { content: graded }
    }
}

/// 讀某 rev 下某檔的內容（`git show <rev>:<path>`），套用 file_content 的防護。
///
/// Missing 僅在「revision 可解析且 path 在該 tree 中確實不存在」時回傳。
/// invalid/missing revision、損壞物件、permission/show 失敗等一律 Err，讓前端 retry。
///
/// Path validation is lexical only — historical object reads must not depend on
/// the current worktree (e.g. a directory replaced by an external symlink).
pub fn file_at_rev(root: &Path, rev: &str, path: &str) -> Result<FileAtRevResult, String> {
    validate_relative_components(path, "file-at-rev")?;

    let full = resolve_commit_oid(root, rev)?.into_string();

    if !path_exists_at_rev(root, &full, path)? {
        return Ok(FileAtRevResult::Missing);
    }

    let spec = format!("{full}:{path}");
    let out = run_git(
        root,
        &["show", "--end-of-options", &spec, "--"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        return Err(git_err("show", &out.stderr));
    }
    Ok(grade_object_bytes(&out.stdout))
}

/// Exact path existence at `commit` via NUL-delimited `ls-tree` + literal pathspec.
/// Returns Ok(false) only when the path is verified absent; other failures are Err.
fn path_exists_at_rev(root: &Path, commit: &str, path: &str) -> Result<bool, String> {
    let out = run_git(
        root,
        &[
            "ls-tree",
            "-z",
            "--full-name",
            "--end-of-options",
            commit,
            "--",
            path,
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        return Err(git_err("ls-tree", &out.stderr));
    }
    if out.stdout.is_empty() {
        return Ok(false);
    }
    // Records: "<mode> <type> <object>\t<path>\0"
    for record in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .ok_or_else(|| "git ls-tree returned a malformed record".to_string())?;
        let returned = &record[tab + 1..];
        if returned == path.as_bytes() {
            return Ok(true);
        }
    }
    Ok(false)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_service::test_repo;
    use std::time::Duration;

    const T: Duration = Duration::from_secs(30);

    // ── 純函式解析 ──────────────────────────────────────────────────

    #[test]
    fn parse_decoration_head_local_remote_tag() {
        // full ref path 形式（--decorate=full 產生）。feature/x 是含 "/" 的 local branch，
        // 由 refs/heads/ 前綴正確判為 local（短名歧義由 full path 消除）。
        let refs = parse_decoration(
            "HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1.0, refs/heads/feature/x",
        );
        assert_eq!(
            refs,
            vec![
                LogRef {
                    name: "HEAD".into(),
                    kind: "head".into()
                },
                LogRef {
                    name: "main".into(),
                    kind: "local".into()
                },
                LogRef {
                    name: "origin/main".into(),
                    kind: "remote".into()
                },
                LogRef {
                    name: "v1.0".into(),
                    kind: "tag".into()
                },
                LogRef {
                    name: "feature/x".into(),
                    kind: "local".into()
                },
            ]
        );
    }

    #[test]
    fn parse_decoration_detached_head_and_empty() {
        assert_eq!(
            parse_decoration("HEAD"),
            vec![LogRef {
                name: "HEAD".into(),
                kind: "head".into()
            }]
        );
        assert!(parse_decoration("").is_empty());
        assert!(parse_decoration("   ").is_empty());
    }

    #[test]
    fn parse_numstat_line_text_and_binary() {
        assert_eq!(
            parse_numstat_line("3\t1\tsrc/a.rs"),
            Some((3, 1, false, "src/a.rs".to_string()))
        );
        assert_eq!(
            parse_numstat_line("-\t-\timg.png"),
            Some((0, 0, true, "img.png".to_string()))
        );
        assert_eq!(parse_numstat_line("garbage"), None);
    }

    #[test]
    fn parse_log_records_handles_special_chars() {
        let f = FIELD_SEP;
        let r = RECORD_SEP;
        let raw = format!(
            "abc123{f}abc{f}feat: \"quoted\" 中文 subject{f}Alice{f}a@x{f}1700000000{f}p1 p2{f}HEAD -> main{r}",
        );
        let commits = parse_log_records(&raw);
        assert_eq!(commits.len(), 1);
        let c = &commits[0];
        assert_eq!(c.hash, "abc123");
        assert_eq!(c.subject, "feat: \"quoted\" 中文 subject");
        assert_eq!(c.parents, vec!["p1".to_string(), "p2".to_string()]);
        assert_eq!(c.timestamp, 1700000000);
        assert_eq!(c.refs[0].kind, "head");
    }

    #[test]
    fn dedup_authors_orders_by_count_then_first_seen() {
        let f = FIELD_SEP;
        let raw = format!("Bob{f}b@x\nAlice{f}a@x\nBob{f}b@x\nBob{f}b@x\nAlice{f}a@x\n",);
        let authors = dedup_authors(&raw);
        assert_eq!(authors.len(), 2);
        // Bob 3 次 > Alice 2 次
        assert_eq!(
            authors[0],
            AuthorEntry {
                name: "Bob".into(),
                email: "b@x".into()
            }
        );
        assert_eq!(
            authors[1],
            AuthorEntry {
                name: "Alice".into(),
                email: "a@x".into()
            }
        );
    }

    #[test]
    fn merge_file_changes_aligns_status_and_numstat() {
        // modify a.txt (2/1), rename old.txt -> new.txt (0/0), binary img.png (-)
        let name_status = "M\0a.txt\0R100\0old.txt\0new.txt\0A\0img.png\0";
        let numstat = "2\t1\ta.txt\x000\t0\t\0old.txt\0new.txt\0-\t-\timg.png\0";
        let files = merge_file_changes(name_status, numstat);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!((files[0].additions, files[0].deletions), (2, 1));
        assert_eq!(files[1].status, "R");
        assert_eq!(files[1].path, "new.txt");
        assert_eq!(files[1].old_path.as_deref(), Some("old.txt"));
        assert_eq!(files[2].path, "img.png");
        assert!(files[2].binary);
    }

    // ── log_page（temp repo fixtures）────────────────────────────────

    /// 建立含 n 個 sequential commit 的 repo（c1..cn）。
    fn linear_repo(n: usize) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        for i in 1..=n {
            test_repo::write_and_commit(tmp.path(), "f.txt", &format!("v{i}\n"), &format!("c{i}"));
        }
        tmp
    }

    #[test]
    fn log_page_paginates_with_has_more() {
        let repo = linear_repo(3);
        let r = repo.path();
        let p1 = log_page(r, None, 2, None, None, None, None).unwrap();
        assert_eq!(p1.commits.len(), 2);
        assert!(p1.has_more);
        let cursor = p1
            .next_cursor
            .clone()
            .expect("page 1 should yield a cursor");
        // 預設 reverse chronological：c3, c2 先
        assert_eq!(p1.commits[0].subject, "c3");
        assert_eq!(p1.commits[1].subject, "c2");
        let p2 = log_page(r, Some(cursor.as_str()), 2, None, None, None, None).unwrap();
        assert_eq!(p2.commits.len(), 1);
        assert!(!p2.has_more);
        assert!(p2.next_cursor.is_none());
        assert_eq!(p2.commits[0].subject, "c1");
    }

    #[test]
    fn log_page_cursor_ignores_ref_moves_between_pages() {
        // Page 1 freezes tips. Creating a new tip after page 1 must not insert
        // into page 2 of the old cursor, and must not drop the original page-2 row.
        let repo = linear_repo(3);
        let r = repo.path();
        let p1 = log_page(r, None, 2, None, None, None, None).unwrap();
        let cursor = p1.next_cursor.clone().unwrap();
        let page1_hashes: Vec<_> = p1.commits.iter().map(|c| c.hash.clone()).collect();

        // Move refs: new branch tip + commit on main after the snapshot.
        run_git(r, &["branch", "extra", "HEAD~1"], T, &iso()).unwrap();
        test_repo::write_and_commit(r, "f.txt", "later\n", "later tip");

        let p2 = log_page(r, Some(cursor.as_str()), 2, None, None, None, None).unwrap();
        assert_eq!(p2.commits.len(), 1);
        assert_eq!(p2.commits[0].subject, "c1");
        // The later tip is absent from the frozen cursor stream.
        assert!(!p2.commits.iter().any(|c| c.subject == "later tip"));
        // No overlap with page 1.
        assert!(!page1_hashes.contains(&p2.commits[0].hash));

        // A fresh first page includes the later tip.
        let fresh = log_page(r, None, 10, None, None, None, None).unwrap();
        assert!(fresh.commits.iter().any(|c| c.subject == "later tip"));
    }

    #[test]
    fn log_page_rejects_malformed_and_oversized_cursors() {
        let repo = linear_repo(1);
        let r = repo.path();
        assert!(log_page(r, Some("{not-json"), 10, None, None, None, None).is_err());
        assert!(log_page(
            r,
            Some(r#"{"v":99,"tips":["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"offset":0}"#),
            10,
            None,
            None,
            None,
            None
        )
        .is_err());
        assert!(log_page(
            r,
            Some(r#"{"v":1,"tips":["not-an-oid"],"offset":0}"#),
            10,
            None,
            None,
            None,
            None
        )
        .is_err());
        let huge = format!(
            r#"{{"v":1,"tips":["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"offset":0,"pad":"{}"}}"#,
            "x".repeat(MAX_CURSOR_BYTES)
        );
        assert!(log_page(r, Some(huge.as_str()), 10, None, None, None, None).is_err());
    }

    #[test]
    fn log_page_special_chars_and_merge_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        // 建 side branch → merge，產生 merge commit（2 parents）。
        run_git(r, &["switch", "-c", "side"], T, &iso()).unwrap();
        test_repo::write_and_commit(r, "s.txt", "side\n", "side change");
        run_git(r, &["switch", "main"], T, &iso()).unwrap();
        test_repo::write_and_commit(r, "m.txt", "main\n", "「中文」\"quoted\" subject");
        // no-ff merge 確保產生 merge commit
        run_git(
            r,
            &["merge", "--no-ff", "-m", "merge side", "side"],
            T,
            &editor_iso(),
        )
        .unwrap();
        let page = log_page(r, None, 10, None, None, None, None).unwrap();
        // merge commit 為 HEAD，2 parents
        let merge = &page.commits[0];
        assert_eq!(merge.subject, "merge side");
        assert_eq!(merge.parents.len(), 2);
        // 特殊字元 subject 完整保留
        assert!(page
            .commits
            .iter()
            .any(|c| c.subject == "「中文」\"quoted\" subject"));
    }

    #[test]
    fn log_page_refs_parsing_on_real_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        run_git(r, &["branch", "second"], T, &iso()).unwrap();
        run_git(r, &["tag", "v1"], T, &iso()).unwrap();
        let page = log_page(r, None, 10, None, None, None, None).unwrap();
        let head = &page.commits[0];
        let kinds: std::collections::HashSet<&str> =
            head.refs.iter().map(|x| x.kind.as_str()).collect();
        assert!(kinds.contains("head"), "refs: {:?}", head.refs);
        assert!(kinds.contains("local"));
        assert!(kinds.contains("tag"));
        // main + second 兩個 local
        let locals: Vec<_> = head.refs.iter().filter(|x| x.kind == "local").collect();
        assert!(locals.iter().any(|x| x.name == "main"));
        assert!(locals.iter().any(|x| x.name == "second"));
    }

    #[test]
    fn log_page_includes_unmerged_branch_commits() {
        // 未合入 main 的 side branch commit 也要入圖（--all）；query 亦同。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        run_git(r, &["switch", "-c", "side"], T, &iso()).unwrap();
        test_repo::write_and_commit(r, "s.txt", "side\n", "side only");
        run_git(r, &["switch", "main"], T, &iso()).unwrap();
        test_repo::write_and_commit(r, "m.txt", "main\n", "main tip");

        let page = log_page(r, None, 10, None, None, None, None).unwrap();
        let subjects: Vec<&str> = page.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(page.commits.len(), 3, "subjects: {subjects:?}");
        assert!(subjects.contains(&"side only"));
        assert!(subjects.contains(&"main tip"));
        // side tip 帶 local branch decoration。
        let side = page
            .commits
            .iter()
            .find(|c| c.subject == "side only")
            .unwrap();
        assert!(side
            .refs
            .iter()
            .any(|x| x.kind == "local" && x.name == "side"));

        // query 也涵蓋其他 branch。
        let hit = log_page(r, None, 10, Some("side only"), None, None, None).unwrap();
        assert_eq!(hit.commits.len(), 1);
        assert_eq!(hit.commits[0].subject, "side only");
    }

    #[test]
    fn log_page_excludes_stash_commits() {
        // refs/stash 是機制 ref，--all 下須被 --exclude 擋掉，不得以孤立節點入圖。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        std::fs::write(r.join("f.txt"), "dirty\n").unwrap();
        run_git(r, &["stash"], T, &iso()).unwrap();

        let page = log_page(r, None, 10, None, None, None, None).unwrap();
        assert_eq!(
            page.commits.len(),
            1,
            "stash 不應入圖: {:?}",
            page.commits.iter().map(|c| &c.subject).collect::<Vec<_>>()
        );
        assert_eq!(page.commits[0].subject, "c1");
    }

    #[test]
    fn log_page_excludes_notes_commits() {
        // refs/notes/* 同 stash：機制 ref，不得以 "Notes added by..." commit 入圖。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        run_git(r, &["notes", "add", "-m", "a note"], T, &iso()).unwrap();

        let page = log_page(r, None, 10, None, None, None, None).unwrap();
        assert_eq!(
            page.commits.len(),
            1,
            "notes 不應入圖: {:?}",
            page.commits.iter().map(|c| &c.subject).collect::<Vec<_>>()
        );
        assert_eq!(page.commits[0].subject, "c1");
    }

    #[test]
    fn log_page_orders_children_before_parents_on_timestamp_tie() {
        // --all 會把「指向祖先的 ref」放進初始走訪集合；timestamp 平手時若無
        // --date-order，parent（被 backup 指著的 base）會先於 child（tip）出列，
        // 前端 graphLayout（假設 children-before-parents）會把 base 畫成孤立節點。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        let mut env = iso();
        for k in ["GIT_AUTHOR_DATE", "GIT_COMMITTER_DATE"] {
            env.push((k.to_string(), "2026-01-01T00:00:00 +0000".to_string()));
        }
        run_git(r, &["commit", "--allow-empty", "-m", "base"], T, &env).unwrap();
        run_git(r, &["branch", "backup"], T, &env).unwrap();
        run_git(r, &["commit", "--allow-empty", "-m", "tip"], T, &env).unwrap();

        let page = log_page(r, None, 10, None, None, None, None).unwrap();
        let subjects: Vec<&str> = page.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["tip", "base"]);
    }

    #[test]
    fn log_page_query_or_semantics() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        // 三個 commit：不同 message／作者。
        test_repo::write_and_commit(r, "a.txt", "1\n", "add feature alpha");
        // 用不同作者 commit
        std::fs::write(r.join("b.txt"), "2\n").unwrap();
        run_git(r, &["add", "b.txt"], T, &iso()).unwrap();
        run_git(
            r,
            &[
                "-c",
                "user.name=Zoe",
                "-c",
                "user.email=zoe@x",
                "commit",
                "-m",
                "unrelated work",
            ],
            T,
            &iso(),
        )
        .unwrap();
        test_repo::write_and_commit(r, "c.txt", "3\n", "another commit");

        // message 命中："alpha"
        let by_msg = log_page(r, None, 50, Some("alpha"), None, None, None).unwrap();
        assert_eq!(by_msg.commits.len(), 1);
        assert_eq!(by_msg.commits[0].subject, "add feature alpha");

        // author 命中："Zoe"
        let by_author = log_page(r, None, 50, Some("Zoe"), None, None, None).unwrap();
        assert_eq!(by_author.commits.len(), 1);
        assert_eq!(by_author.commits[0].author_name, "Zoe");

        // hash 前綴命中：取某 commit 的短 hash 前綴。
        let full = &by_msg.commits[0].hash;
        let prefix = &full[..7];
        let by_hash = log_page(r, None, 50, Some(prefix), None, None, None).unwrap();
        assert!(by_hash.commits.iter().any(|c| &c.hash == full));

        // no match → 空。
        let none = log_page(r, None, 50, Some("zzz-nomatch-zzz"), None, None, None).unwrap();
        assert!(none.commits.is_empty());
        assert!(!none.has_more);
    }

    #[test]
    fn log_page_author_exact_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1\n", "c1"); // author "t"
        std::fs::write(r.join("b.txt"), "2\n").unwrap();
        run_git(r, &["add", "b.txt"], T, &iso()).unwrap();
        run_git(
            r,
            &[
                "-c",
                "user.name=Alice",
                "-c",
                "user.email=alice@x",
                "commit",
                "-m",
                "by alice",
            ],
            T,
            &iso(),
        )
        .unwrap();
        // author "Alice" 精確 → 只 1 筆
        let page = log_page(r, None, 50, None, Some("Alice"), None, None).unwrap();
        assert_eq!(page.commits.len(), 1);
        assert_eq!(page.commits[0].author_name, "Alice");
    }

    #[test]
    fn log_page_empty_repo_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path()); // init 但無 commit
        let page = log_page(tmp.path(), None, 10, None, None, None, None).unwrap();
        assert!(page.commits.is_empty());
        assert!(!page.has_more);
    }

    #[test]
    fn log_page_limit_clamped() {
        let repo = linear_repo(2);
        // limit 超上限 → clamp 到 MAX_LOG_LIMIT，不報錯
        let page = log_page(repo.path(), None, 99999, None, None, None, None).unwrap();
        assert_eq!(page.commits.len(), 2);
        assert!(!page.has_more);
    }

    // ── commit_detail ───────────────────────────────────────────────

    #[test]
    fn commit_detail_modify_additions_deletions() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "a\nb\nc\n", "c1");
        test_repo::write_and_commit(r, "f.txt", "a\nB\nc\nd\n", "c2");
        let head = head_hash(r);
        let detail = commit_detail(r, &head).unwrap();
        assert_eq!(detail.subject, "c2");
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].path, "f.txt");
        assert_eq!(detail.files[0].status, "M");
        // 改 b->B（1 add 1 del）＋加 d（1 add）= 2 add 1 del
        assert_eq!(detail.total_additions, 2);
        assert_eq!(detail.total_deletions, 1);
        assert!(!detail.files[0].binary);
    }

    #[test]
    fn commit_detail_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "old.txt", "same content here\nline2\n", "c1");
        // rename 保留內容 → git 偵測為 rename
        std::fs::rename(r.join("old.txt"), r.join("new.txt")).unwrap();
        run_git(r, &["add", "-A"], T, &iso()).unwrap();
        run_git(r, &["commit", "-m", "rename it"], T, &iso()).unwrap();
        let detail = commit_detail(r, &head_hash(r)).unwrap();
        assert_eq!(detail.files.len(), 1);
        let f = &detail.files[0];
        assert_eq!(f.status, "R");
        assert_eq!(f.path, "new.txt");
        assert_eq!(f.old_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn commit_detail_binary_file() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "readme.txt", "hi\n", "c1");
        // 加一個 binary 檔（含 NUL）
        std::fs::write(r.join("img.bin"), b"\x00\x01\x02\x03\xff\xfe").unwrap();
        run_git(r, &["add", "img.bin"], T, &iso()).unwrap();
        run_git(r, &["commit", "-m", "add binary"], T, &iso()).unwrap();
        let detail = commit_detail(r, &head_hash(r)).unwrap();
        let bin = detail.files.iter().find(|f| f.path == "img.bin").unwrap();
        assert!(bin.binary);
        assert_eq!((bin.additions, bin.deletions), (0, 0));
    }

    #[test]
    fn commit_detail_merge_uses_first_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        run_git(r, &["switch", "-c", "side"], T, &iso()).unwrap();
        test_repo::write_and_commit(r, "s.txt", "side\n", "side");
        run_git(r, &["switch", "main"], T, &iso()).unwrap();
        test_repo::write_and_commit(r, "m.txt", "main\n", "main change");
        run_git(
            r,
            &["merge", "--no-ff", "-m", "merge side", "side"],
            T,
            &editor_iso(),
        )
        .unwrap();
        let detail = commit_detail(r, &head_hash(r)).unwrap();
        assert_eq!(detail.parents.len(), 2);
        // first-parent diff（相對 main tip）：只帶進 side 引入的 s.txt。
        assert!(detail.files.iter().any(|f| f.path == "s.txt"));
        // m.txt 已在 first parent（main tip）中，first-parent diff 不應重複列出——
        // 這是 diff-tree -m --first-parent 會誤帶、git show --first-parent 修正的重點。
        assert!(
            !detail.files.iter().any(|f| f.path == "m.txt"),
            "first-parent diff 不應含已在 first parent 的 m.txt: {:?}",
            detail.files
        );
    }

    #[test]
    fn commit_detail_body_with_field_separator_does_not_shift_fields() {
        // body 含 \x1f（FIELD_SEP）不得位移 header 欄位——body 放最後一欄＋splitn(6)
        // 的回歸測試（review Minor）。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        std::fs::write(r.join("f.txt"), "x\n").unwrap();
        run_git(r, &["add", "f.txt"], T, &iso()).unwrap();
        let body = format!("line1{}line2", FIELD_SEP);
        run_git(r, &["commit", "-m", "subj", "-m", &body], T, &iso()).unwrap();
        let detail = commit_detail(r, &head_hash(r)).unwrap();
        assert_eq!(detail.subject, "subj");
        assert_eq!(detail.author_name, "t");
        assert_eq!(detail.author_email, "t@t");
        assert!(detail.timestamp > 0);
        assert_eq!(detail.body, body, "body 應完整保留 \\x1f");
    }

    #[test]
    fn commit_detail_root_commit_shows_added_files() {
        // root commit（無 parent）：應對空樹 diff，列出所有新增檔（回歸：diff-tree
        // --first-parent 對 root 會輸出空，git show 正確處理）。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "a\nb\n", "root commit");
        let detail = commit_detail(r, &head_hash(r)).unwrap();
        assert!(detail.parents.is_empty());
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].path, "f.txt");
        assert_eq!(detail.files[0].status, "A");
        assert_eq!(detail.total_additions, 2);
        assert_eq!(detail.total_deletions, 0);
    }

    #[test]
    fn log_page_subject_with_field_and_record_separators_is_not_forged() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "base\n", "c1");
        let real = head_hash(r);
        let forged = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let subject = format!(
            "evil{f}{forged}{f}x{f}Eve{f}e@x{f}1{f}{f}{r}{forged}{f}aaaaaaa{f}forged subject{f}Eve{f}e@x{f}1{f}{f}",
            f = FIELD_SEP,
            r = RECORD_SEP
        );
        std::fs::write(r.join("f.txt"), "next\n").unwrap();
        run_git(r, &["add", "f.txt"], T, &iso()).unwrap();
        run_git(r, &["commit", "-m", &subject], T, &iso()).unwrap();

        let page = log_page(r, None, 10, None, None, None, None).unwrap();
        assert_eq!(
            page.commits.len(),
            2,
            "subjects: {:?}",
            page.commits.iter().map(|c| &c.subject).collect::<Vec<_>>()
        );
        assert!(!page.commits.iter().any(|c| c.hash == forged));
        let injected = page
            .commits
            .iter()
            .find(|c| c.hash != real)
            .expect("new commit");
        assert_eq!(injected.subject, subject);
        assert_eq!(injected.author_name, "t");
        assert_eq!(injected.author_email, "t@t");
        assert_ne!(injected.hash, forged);
        assert!(is_full_oid(&injected.hash));

        let detail = commit_detail(r, &injected.hash).unwrap();
        assert_eq!(detail.subject, subject);
        assert_eq!(detail.author_name, "t");
    }

    #[test]
    fn commit_detail_rejects_output_option_before_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "x\n", "c1");
        let sink = tmp.path().join("pwned");
        let inject = format!("--output={}", sink.display());
        let err = commit_detail(r, &inject).unwrap_err();
        assert!(
            err.contains("option-like"),
            "expected option rejection, got {err}"
        );
        assert!(
            !sink.exists(),
            "commit hash option injection must not write a file"
        );
        let err = file_at_rev(r, &inject, "f.txt").unwrap_err();
        assert!(
            err.contains("option-like"),
            "expected option rejection, got {err}"
        );
        assert!(
            !sink.exists(),
            "file_at_rev option injection must not write a file"
        );
    }

    #[test]
    fn parse_commit_object_keeps_control_characters_in_subject() {
        let raw = format!(
            "tree {:0<40}\nparent {:0<40}\nauthor Ann <a@x> 1700000000 +0000\ncommitter Ann <a@x> 1700000000 +0000\n\nsubj{f}mid{r}end\n\nbody\n",
            "b", "c", f = FIELD_SEP, r = RECORD_SEP
        );
        let meta = parse_commit_object(raw.as_bytes()).unwrap();
        assert_eq!(
            meta.subject,
            format!("subj{f}mid{r}end", f = FIELD_SEP, r = RECORD_SEP)
        );
        assert_eq!(meta.body, "body");
        assert_eq!(meta.author_name, "Ann");
        assert_eq!(meta.author_email, "a@x");
        assert_eq!(meta.timestamp, 1_700_000_000);
        assert_eq!(meta.parents.len(), 1);
    }

    // ── file_at_rev ─────────────────────────────────────────────────

    #[test]
    fn file_at_rev_returns_old_content() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "version one\n", "c1");
        let first = head_hash(r);
        test_repo::write_and_commit(r, "f.txt", "version two\n", "c2");
        // 舊 rev 取回舊內容
        match file_at_rev(r, &first, "f.txt").unwrap() {
            FileAtRevResult::Full { content } => assert_eq!(content, "version one\n"),
            other => panic!("expected Full, got {other:?}"),
        }
        // HEAD 取回新內容
        match file_at_rev(r, "HEAD", "f.txt").unwrap() {
            FileAtRevResult::Full { content } => assert_eq!(content, "version two\n"),
            other => panic!("expected Full, got {other:?}"),
        }
    }

    #[test]
    fn file_at_rev_missing_when_not_present() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "x\n", "c1");
        // 該 rev 無此檔 → Missing
        assert_eq!(
            file_at_rev(r, "HEAD", "nope.txt").unwrap(),
            FileAtRevResult::Missing
        );
        // 不存在的 rev → Err（不是 Missing）
        assert!(file_at_rev(r, "deadbeef", "f.txt").is_err());
    }

    #[test]
    fn file_at_rev_binary_protection() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        std::fs::write(r.join("img.bin"), b"\x89PNG\r\n\x1a\nbinary data").unwrap();
        run_git(r, &["add", "img.bin"], T, &iso()).unwrap();
        run_git(r, &["commit", "-m", "c1"], T, &iso()).unwrap();
        assert_eq!(
            file_at_rev(r, "HEAD", "img.bin").unwrap(),
            FileAtRevResult::Binary
        );
    }

    #[test]
    fn file_at_rev_utf16_bom_decodes_to_text() {
        // UTF-16LE BOM blob：不可走 from_utf8_lossy 亂碼、也不可誤判 binary——
        // 應以 encoding_rs 解碼（BOM 剝除）回 Full（review Important #1 回歸）。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        std::fs::write(r.join("u16.txt"), b"\xff\xfeh\x00i\x00").unwrap();
        run_git(r, &["add", "u16.txt"], T, &iso()).unwrap();
        run_git(r, &["commit", "-m", "utf16"], T, &iso()).unwrap();
        match file_at_rev(r, "HEAD", "u16.txt").unwrap() {
            FileAtRevResult::Full { content } => assert_eq!(content, "hi"),
            other => panic!("expected Full with decoded text, got {other:?}"),
        }
    }

    #[test]
    fn file_at_rev_path_not_treated_as_option() {
        // path 以 option 樣態（前綴 -）不應被 git 當 flag。此檔名在 repo 不存在 → Missing
        // （而非 git 報「unknown option」錯誤）。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "x\n", "c1");
        assert_eq!(
            file_at_rev(r, "HEAD", "--not-a-file").unwrap(),
            FileAtRevResult::Missing
        );
    }

    #[test]
    fn file_at_rev_rev_option_injection_blocked() {
        // rev 以 option 樣態（如 `--output=<file>`）不得被 git show 當成 flag——否則
        // `git show --output=/tmp/pwn` 會把輸出寫進任意路徑（任意檔案寫入原語）。
        // --end-of-options + resolve-first 路徑下 → Err，且不產生該檔（F4 回歸）。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "x\n", "c1");
        let sink = tmp.path().join("pwned");
        let inject = format!("--output={}", sink.display());
        assert!(file_at_rev(r, &inject, "f.txt").is_err());
        assert!(!sink.exists(), "rev option injection must not write a file");
    }

    #[test]
    fn log_page_rejects_limit_zero() {
        let repo = linear_repo(1);
        assert!(log_page(repo.path(), None, 0, None, None, None, None).is_err());
    }

    #[test]
    fn log_page_rejects_forged_and_cross_root_cursors() {
        let repo = linear_repo(2);
        let r = repo.path();
        let p1 = log_page(r, None, 1, None, None, None, None).unwrap();
        let cursor = p1.next_cursor.clone().unwrap();

        // Unsigned JSON tip payload must not be accepted.
        let forged = r#"{"v":2,"root":"x","fp":"","tips":["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"offset":0}"#;
        assert!(log_page(r, Some(forged), 10, None, None, None, None).is_err());

        // Tampered body with broken MAC.
        let mut parts = cursor.split('.').collect::<Vec<_>>();
        assert_eq!(parts.len(), 2);
        parts[1] = "00";
        let bad_mac = format!("{}.{}", parts[0], parts[1]);
        assert!(log_page(r, Some(bad_mac.as_str()), 10, None, None, None, None).is_err());

        // Cursor from repo A is invalid on repo B.
        let other = linear_repo(2);
        assert!(log_page(
            other.path(),
            Some(cursor.as_str()),
            10,
            None,
            None,
            None,
            None
        )
        .is_err());

        // Filter fingerprint mismatch.
        assert!(log_page(r, Some(cursor.as_str()), 10, Some("nope"), None, None, None).is_err());
    }

    #[test]
    fn log_page_ignores_replace_refs_between_pages() {
        let repo = linear_repo(4);
        let r = repo.path();
        let p1 = log_page(r, None, 2, None, None, None, None).unwrap();
        let cursor = p1.next_cursor.clone().unwrap();
        let expected_page2_subject = "c2"; // after c4,c3 page

        // Replace c2 with c1 content via refs/replace — must not warp frozen stream.
        let c2 = p1.commits.iter().find(|c| c.subject == "c3"); // just need a real hash
        let _ = c2;
        let hashes: Vec<String> = {
            let page = log_page(r, None, 10, None, None, None, None).unwrap();
            page.commits.iter().map(|c| c.hash.clone()).collect()
        };
        // hashes[0]=c4, [1]=c3, [2]=c2, [3]=c1
        let c2_hash = &hashes[2];
        let c1_hash = &hashes[3];
        // refs/replace/<old> -> <new>
        run_git(
            r,
            &["update-ref", &format!("refs/replace/{c2_hash}"), c1_hash],
            T,
            &iso(),
        )
        .unwrap();

        let p2 = log_page(r, Some(cursor.as_str()), 2, None, None, None, None).unwrap();
        assert!(
            p2.commits
                .iter()
                .any(|c| c.subject == expected_page2_subject),
            "frozen stream must keep c2, got {:?}",
            p2.commits
                .iter()
                .map(|c| c.subject.as_str())
                .collect::<Vec<_>>()
        );
        assert!(!p2.commits.is_empty());
    }

    #[test]
    fn log_page_hash_query_only_hex_prefix_and_exact_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        // Two commits; second has different author so filters can exclude the target.
        test_repo::write_and_commit(r, "f.txt", "one\n", "alpha");
        let first = head_hash(r);
        std::fs::write(r.join("f.txt"), "two\n").unwrap();
        run_git(r, &["add", "f.txt"], T, &iso()).unwrap();
        run_git(
            r,
            &[
                "-c",
                "user.name=Bob",
                "-c",
                "user.email=bob@x",
                "commit",
                "-m",
                "beta",
            ],
            T,
            &iso(),
        )
        .unwrap();
        let second = head_hash(r);

        // Branch / HEAD / ancestry expressions must not activate the hash arm.
        // Message/author arms also won't match these strings → empty result.
        assert!(log_page(r, None, 50, Some("HEAD"), None, None, None)
            .unwrap()
            .commits
            .is_empty());
        assert!(log_page(r, None, 50, Some("main"), None, None, None)
            .unwrap()
            .commits
            .is_empty());
        assert!(log_page(r, None, 50, Some("HEAD~1"), None, None, None)
            .unwrap()
            .commits
            .is_empty());

        // Exact hex prefix hits the commit.
        let prefix = &second[..8];
        let hit = log_page(r, None, 50, Some(prefix), None, None, None).unwrap();
        assert_eq!(hit.commits.len(), 1);
        assert_eq!(hit.commits[0].hash, second);

        // Author filter excludes target; matching ancestor must not be returned.
        let filtered = log_page(r, None, 50, Some(prefix), Some("t"), None, None).unwrap();
        assert!(
            filtered.commits.is_empty(),
            "must not walk to ancestor; got {:?}",
            filtered.commits
        );
        // Control: first commit hash with matching author works.
        let first_prefix = &first[..8];
        let first_hit = log_page(r, None, 50, Some(first_prefix), Some("t"), None, None).unwrap();
        assert_eq!(first_hit.commits.len(), 1);
        assert_eq!(first_hit.commits[0].hash, first);
    }

    #[test]
    fn log_page_query_preserves_child_before_parent_with_skewed_timestamps() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        // Parent commit with a *newer* author date than the child (skew).
        std::fs::write(r.join("f.txt"), "parent\n").unwrap();
        run_git(r, &["add", "f.txt"], T, &iso()).unwrap();
        let mut env_parent = iso();
        env_parent.push((
            "GIT_AUTHOR_DATE".to_string(),
            "2020-01-02T00:00:00 +0000".to_string(),
        ));
        env_parent.push((
            "GIT_COMMITTER_DATE".to_string(),
            "2020-01-02T00:00:00 +0000".to_string(),
        ));
        run_git(r, &["commit", "-m", "shared-parent"], T, &env_parent).unwrap();

        std::fs::write(r.join("f.txt"), "child\n").unwrap();
        run_git(r, &["add", "f.txt"], T, &iso()).unwrap();
        let mut env_child = iso();
        env_child.push((
            "GIT_AUTHOR_DATE".to_string(),
            "2020-01-01T00:00:00 +0000".to_string(),
        ));
        env_child.push((
            "GIT_COMMITTER_DATE".to_string(),
            "2020-01-01T00:00:00 +0000".to_string(),
        ));
        run_git(r, &["commit", "-m", "shared-child"], T, &env_child).unwrap();

        let page = log_page(r, None, 50, Some("shared"), None, None, None).unwrap();
        assert_eq!(page.commits.len(), 2);
        assert_eq!(page.commits[0].subject, "shared-child");
        assert_eq!(page.commits[1].subject, "shared-parent");
        // Child is first even though its timestamp is older.
        assert!(page.commits[0].timestamp < page.commits[1].timestamp);
    }

    #[test]
    fn log_page_normalizes_empty_and_whitespace_filters_for_cursor() {
        let repo = linear_repo(2);
        let r = repo.path();
        // First page with no author filter.
        let p1 = log_page(r, None, 1, None, None, None, None).unwrap();
        let cursor = p1.next_cursor.clone().expect("page 1 should have more");

        // Empty / whitespace author is canonicalized to None → same cursor accepted.
        let p2_empty = log_page(r, Some(cursor.as_str()), 1, None, Some(""), None, None).unwrap();
        assert_eq!(p2_empty.commits.len(), 1);
        let p2_ws = log_page(r, Some(cursor.as_str()), 1, None, Some("  \t"), None, None).unwrap();
        assert_eq!(p2_ws.commits.len(), 1);
        // Explicit None still works.
        let p2_none = log_page(r, Some(cursor.as_str()), 1, None, None, None, None).unwrap();
        assert_eq!(p2_none.commits.len(), 1);

        // A real author filter remains a distinct fingerprint.
        assert!(
            log_page(r, Some(cursor.as_str()), 1, None, Some("Alice"), None, None).is_err(),
            "non-empty author must not reuse a no-author cursor"
        );

        // Whitespace query on first page is treated as no query (full history).
        let full = log_page(r, None, 10, Some("   "), None, None, None).unwrap();
        assert_eq!(full.commits.len(), 2);
    }

    #[test]
    fn log_page_rejects_offset_above_i32_max() {
        // encode_cursor must refuse offsets Git cannot consume as --skip.
        let tips = vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()];
        assert!(encode_cursor("root", "", &tips, MAX_CURSOR_OFFSET.saturating_add(1)).is_err());
        assert!(encode_cursor("root", "", &tips, (i32::MAX as u32) + 1).is_err());
        // MAX itself is still encodable.
        assert!(encode_cursor("root", "", &tips, MAX_CURSOR_OFFSET).is_ok());

        // decode_cursor must also reject oversize offsets even if MAC is valid.
        let oversize = LogCursorPayload {
            v: LOG_CURSOR_VERSION,
            root: "root".to_string(),
            fp: String::new(),
            tips: tips.clone(),
            offset: (i32::MAX as u32) + 1,
        };
        let raw = serde_json::to_string(&oversize).unwrap();
        let body = URL_SAFE_NO_PAD.encode(raw.as_bytes());
        let tag = mac_cursor_payload(&body);
        let token = format!("{body}.{tag}");
        assert!(decode_cursor(&token, "root", "").is_err());
    }

    #[test]
    fn file_at_rev_literal_special_filenames() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        let mut lit_env = iso();
        lit_env.push(("GIT_LITERAL_PATHSPECS".to_string(), "1".to_string()));
        for name in [":(literal)kept.txt", "star*.txt", "q?.txt"] {
            std::fs::write(r.join(name), format!("content-{name}\n")).unwrap();
            // Force literal pathspecs so `:(literal)…` / `*` / `?` are filenames.
            run_git(r, &["add", "--", name], T, &lit_env).unwrap();
        }
        run_git(r, &["commit", "-m", "special names"], T, &iso()).unwrap();
        for name in [":(literal)kept.txt", "star*.txt", "q?.txt"] {
            match file_at_rev(r, "HEAD", name).unwrap() {
                FileAtRevResult::Full { content } => {
                    assert_eq!(content, format!("content-{name}\n"));
                }
                other => panic!("expected Full for {name}, got {other:?}"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_at_rev_ignores_worktree_symlink_for_historical_path() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        std::fs::create_dir_all(r.join("dir")).unwrap();
        std::fs::write(r.join("dir/f.txt"), "historical\n").unwrap();
        run_git(r, &["add", "dir/f.txt"], T, &iso()).unwrap();
        run_git(r, &["commit", "-m", "c1"], T, &iso()).unwrap();

        // Replace dir with an external symlink — filesystem validator would reject.
        std::fs::remove_dir_all(r.join("dir")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "nope\n").unwrap();
        symlink(outside.path(), r.join("dir")).unwrap();

        match file_at_rev(r, "HEAD", "dir/f.txt").unwrap() {
            FileAtRevResult::Full { content } => assert_eq!(content, "historical\n"),
            other => panic!("expected historical content, got {other:?}"),
        }
    }

    // ── log_authors ─────────────────────────────────────────────────
    // ── log_authors ─────────────────────────────────────────────────

    #[test]
    fn log_authors_dedups_across_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "a.txt", "1\n", "c1"); // author t
        test_repo::write_and_commit(r, "a.txt", "2\n", "c2"); // author t
        std::fs::write(r.join("b.txt"), "x\n").unwrap();
        run_git(r, &["add", "b.txt"], T, &iso()).unwrap();
        run_git(
            r,
            &[
                "-c",
                "user.name=Bob",
                "-c",
                "user.email=bob@x",
                "commit",
                "-m",
                "c3",
            ],
            T,
            &iso(),
        )
        .unwrap();
        let authors = log_authors(r).unwrap();
        assert_eq!(authors.len(), 2);
        // t 2 次 > Bob 1 次
        assert_eq!(authors[0].name, "t");
        assert_eq!(authors[1].name, "Bob");
    }

    #[test]
    fn log_authors_empty_repo() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path());
        assert!(log_authors(tmp.path()).unwrap().is_empty());
    }

    // ── test helpers ────────────────────────────────────────────────

    /// 隔離使用者 git 設定（比照 git_service::test_repo::isolated_env，該函式私有）。
    fn iso() -> Vec<(String, String)> {
        vec![
            ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
            ("GIT_CONFIG_SYSTEM".to_string(), "/dev/null".to_string()),
        ]
    }

    /// iso() ＋ GIT_EDITOR=true（merge --no-ff 帶 -m 不需 editor，但保險）。
    fn editor_iso() -> Vec<(String, String)> {
        let mut e = iso();
        e.push(("GIT_EDITOR".to_string(), "true".to_string()));
        e
    }

    fn head_hash(r: &Path) -> String {
        let out = run_git(r, &["rev-parse", "HEAD"], T, &iso()).unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}
