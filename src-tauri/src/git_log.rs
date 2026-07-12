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
use crate::git_service::{git_err, run_git, GitServiceState};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// git_log_page 的 limit 上限（clamp）。
const MAX_LOG_LIMIT: u32 = 500;
/// git_log_authors 去重後回傳的作者數上限。
const MAX_AUTHORS: usize = 50;

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
/// 欄位順序須與 log_format() 對齊：hash, short_hash, subject, author_name,
/// author_email, timestamp, parents(空白分隔), decoration。
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

/// log 的自訂 --format（欄位與 parse_log_records 對齊）。
fn log_format() -> String {
    // %H %h %s %an %ae %at %P %D
    format!(
        "%H{f}%h{f}%s{f}%an{f}%ae{f}%at{f}%P{f}%D{r}",
        f = FIELD_SEP,
        r = RECORD_SEP
    )
}

/// 一頁 commit 歷史（涵蓋所有 branch，見 ALL_REFS_ARGS）。
///
/// 無 query：`git log --all --skip N --max-count M+1`（多取 1 判 has_more）＋自訂
/// --format ＋ `--decorate=full`（見 run_log）。分頁順序用預設（reverse chronological /
/// commit graph 序），穩定。可選 author（精確作者，`--author=^<name> <` 錨定 name＋email
/// 定界符避免子字串誤匹配）、since/until（`--since`/`--until`）。
///
/// query（OR 語意，成本 = 多趟 git log）：query 同時比對
///   (a) commit message（`--grep`, `-i`）
///   (b) 作者（`--author`, `-i`，子字串）
///   (c) hash 前綴（`git rev-parse --verify <q>^{commit}`）
/// 做法：先各自跑一趟「全量」git log（不 skip、不 max-count，僅套 author/since/until
/// 過濾），把三個結果集合按 hash 去重、依 timestamp 降冪（tie 用原始出現序穩定）合併，
/// 最後才對合併集做 skip/limit＋has_more。取捨：query 存在時分頁對「合併後」一致（先合併
/// 再切頁），代價是每頁都重算整個聯集——commit 數大時較貴，但契約正確且簡單；Log tab 的
/// query 場景資料量有界，可接受。
pub fn log_page(
    root: &Path,
    skip: u32,
    limit: u32,
    query: Option<&str>,
    author: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<LogPage, String> {
    let limit = limit.clamp(0, MAX_LOG_LIMIT);
    let fmt = log_format();
    let format_arg = format!("--format={fmt}");

    // 共用的過濾 flag（author 精確 / since / until）。
    // git --author 是對 ident 字串 "Name <email>" 的 regex。精確作者＝把傳入值錨定到
    // name 開頭並要求其後緊接 " <"（email 定界符），避免子字串誤匹配（如 "Al" 命中
    // "Alice"）。傳入值為 User 下拉的 name 欄。
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
            // 無 query：直接 git log 分頁（多取 1 判 has_more）。
            let skip_arg = format!("--skip={skip}");
            let max_arg = format!("--max-count={}", limit.saturating_add(1));
            let mut args: Vec<&str> = vec!["log", &format_arg, &skip_arg, &max_arg];
            args.extend(ALL_REFS_ARGS);
            args.extend(filters.iter().copied());
            let commits = run_log(root, &args)?;
            Ok(paginate_fetched(commits, limit as usize))
        }
        Some(q) => {
            // query：聯集三個結果集，去重、排序，再切頁。
            let mut merged: Vec<LogCommit> = Vec::new();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

            let grep_arg = format!("--grep={q}");
            let qauthor_arg = format!("--author={q}");
            // (a) message、(b) author 各一趟全量 log（套用共用 filters）。
            for extra in [grep_arg.as_str(), qauthor_arg.as_str()] {
                let mut args: Vec<&str> = vec!["log", &format_arg, "-i", extra];
                args.extend(ALL_REFS_ARGS);
                args.extend(filters.iter().copied());
                for c in run_log(root, &args)? {
                    if seen.insert(c.hash.clone()) {
                        merged.push(c);
                    }
                }
            }
            // (c) hash 前綴：rev-parse --verify <q>^{commit}，成功則取該 commit。
            if let Some(full) = resolve_commit_prefix(root, q)? {
                if seen.insert(full.clone()) {
                    let mut args: Vec<&str> = vec!["log", &format_arg, "-1", full.as_str()];
                    args.extend(filters.iter().copied());
                    // filters 可能讓該 commit 不符（如 author 不符）→ 空結果，尊重過濾。
                    for c in run_log(root, &args)? {
                        merged.push(c);
                    }
                }
            }

            // timestamp 降冪；tie 保持插入序（sort_by_key 為 stable sort）。
            merged.sort_by_key(|c| std::cmp::Reverse(c.timestamp));

            let total = merged.len();
            let start = (skip as usize).min(total);
            let end = start.saturating_add(limit as usize).min(total);
            let has_more = end < total;
            Ok(LogPage {
                commits: merged[start..end].to_vec(),
                has_more,
            })
        }
    }
}

/// 跑一趟 git log 並解析。統一注入 `--decorate=full`：%D 才會給 full ref path
/// （refs/heads/、refs/remotes/、refs/tags/），供 parse_decoration 無歧義分類。
/// 呼叫端傳入的 args 應以 "log" 開頭，此處在其後插入 --decorate=full。
fn run_log(root: &Path, args: &[&str]) -> Result<Vec<LogCommit>, String> {
    let mut full_args: Vec<&str> = Vec::with_capacity(args.len() + 1);
    if let Some((first, rest)) = args.split_first() {
        full_args.push(first);
        full_args.push("--decorate=full");
        full_args.extend_from_slice(rest);
    } else {
        full_args.extend_from_slice(args);
    }
    let out = run_git(root, &full_args, DEFAULT_TIMEOUT, &[])?;
    if out.code != 0 {
        // 空 repo（無任何 commit）：git log 退非零並在 stderr 提示
        // "does not have any commits yet" → 視為空清單，不報錯。
        let stderr = out.stderr.to_lowercase();
        if stderr.contains("does not have any commits") || stderr.contains("bad default revision") {
            return Ok(Vec::new());
        }
        return Err(git_err("log", &out.stderr));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(parse_log_records(&text))
}

/// 從「多取 1」的結果切出本頁＋has_more。
fn paginate_fetched(mut commits: Vec<LogCommit>, limit: usize) -> LogPage {
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    LogPage { commits, has_more }
}

/// `git rev-parse --verify <q>^{commit}` → 成功回完整 hash；否則 None。
fn resolve_commit_prefix(root: &Path, q: &str) -> Result<Option<String>, String> {
    let spec = format!("{q}^{{commit}}");
    // `--end-of-options`（git ≥2.24）確保 spec 一律當 revision 而非 option——q 若以 `-`
    // 開頭（如 `--output=...`）在無此屏障時會被 git 當 flag 解析（rev option 注入）。
    let out = run_git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        return Ok(None);
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() {
        Ok(None)
    } else {
        Ok(Some(hash))
    }
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
    // header：subject \x1f an \x1f ae \x1f at \x1f parents \x1f body。
    // body（%b）刻意放最後一欄＋splitn 限制切割數：body 內容若含 \x1f 也不會位移
    // 前面的固定欄位（subject 是單行、%an/%ae 為 git ident 不含控制字元、%at/%P 為
    // git 生成，唯一可能含任意 bytes 的是 body）。
    let fmt = format!("%s{f}%an{f}%ae{f}%at{f}%P{f}%b", f = FIELD_SEP);
    let format_arg = format!("--format={fmt}");
    let head = run_git(
        root,
        &["show", "-s", &format_arg, hash],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if head.code != 0 {
        return Err(git_err("show", &head.stderr));
    }
    let head_text = String::from_utf8_lossy(&head.stdout);
    let head_text = head_text.trim_end_matches(['\n', '\r']);
    let hfields: Vec<&str> = head_text.splitn(6, FIELD_SEP).collect();
    if hfields.len() < 6 {
        return Err(format!("git show: malformed header for {hash}"));
    }
    let subject = hfields[0].to_string();
    let author_name = hfields[1].to_string();
    let author_email = hfields[2].to_string();
    let timestamp: i64 = hfields[3].trim().parse().unwrap_or(0);
    let parents: Vec<String> = hfields[4].split_whitespace().map(String::from).collect();
    let body = hfields[5].trim_end_matches(['\n', '\r']).to_string();

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
            hash,
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
            hash,
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
/// path 以 `--` 前的 `<rev>:<path>` 形式傳入 git（pathspec 內建於 revision syntax，
/// 不會被當成 option）；此外仍在 rev 與 path 之間做基本防呆。該 rev 無此檔（或 rev
/// 不存在）→ FileAtRevResult::Missing（不 panic、不當錯誤）。
pub fn file_at_rev(root: &Path, rev: &str, path: &str) -> Result<FileAtRevResult, String> {
    // `git show <rev>:<path>`：<rev>:<path> 是 git 物件語法，path 不會被解讀為 option。
    // 為保險，用 `--` 隔開位置參數（雖然 show 的 <object> 不吃 pathspec，`--` 無害且明確）。
    // `--` 只保護其後的參數；spec 本身仍可能被當 option（rev 以 `-` 開頭，如
    // `--output=/tmp/pwn` → 任意檔案寫入原語）。`--end-of-options`（git ≥2.24）把 spec
    // 一律鎖為 revision 位置參數，堵住這個注入。
    let spec = format!("{rev}:{path}");
    let out = run_git(
        root,
        &["show", "--end-of-options", &spec, "--"],
        DEFAULT_TIMEOUT,
        &[],
    )?;
    if out.code != 0 {
        // rev 或 path 不存在（"exists on disk, but not in ..."／"does not exist"／
        // "unknown revision" 等）→ Missing。
        return Ok(FileAtRevResult::Missing);
    }
    Ok(grade_object_bytes(&out.stdout))
}

// ── commands（薄包裝）────────────────────────────────────────────────────

/// 取當前 repo root（比照 git_service 的 repo_root，因該函式私有故本地重實作）。
fn repo_root(state: &tauri::State<'_, GitServiceState>) -> Result<PathBuf, String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    Ok(guard
        .as_ref()
        .ok_or_else(|| "no repository detected".to_string())?
        .root
        .clone())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn git_log_page(
    state: tauri::State<'_, GitServiceState>,
    skip: u32,
    limit: u32,
    query: Option<String>,
    author: Option<String>,
    since: Option<String>,
    until: Option<String>,
) -> Result<LogPage, String> {
    let root = repo_root(&state)?;
    log_page(
        &root,
        skip,
        limit,
        query.as_deref(),
        author.as_deref(),
        since.as_deref(),
        until.as_deref(),
    )
}

#[tauri::command]
pub fn git_commit_detail(
    state: tauri::State<'_, GitServiceState>,
    hash: String,
) -> Result<CommitDetail, String> {
    commit_detail(&repo_root(&state)?, &hash)
}

#[tauri::command]
pub fn git_log_authors(
    state: tauri::State<'_, GitServiceState>,
) -> Result<Vec<AuthorEntry>, String> {
    log_authors(&repo_root(&state)?)
}

#[tauri::command]
pub fn git_file_at_rev(
    state: tauri::State<'_, GitServiceState>,
    rev: String,
    path: String,
) -> Result<FileAtRevResult, String> {
    file_at_rev(&repo_root(&state)?, &rev, &path)
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
        let p1 = log_page(r, 0, 2, None, None, None, None).unwrap();
        assert_eq!(p1.commits.len(), 2);
        assert!(p1.has_more);
        // 預設 reverse chronological：c3, c2 先
        assert_eq!(p1.commits[0].subject, "c3");
        assert_eq!(p1.commits[1].subject, "c2");
        let p2 = log_page(r, 2, 2, None, None, None, None).unwrap();
        assert_eq!(p2.commits.len(), 1);
        assert!(!p2.has_more);
        assert_eq!(p2.commits[0].subject, "c1");
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
        let page = log_page(r, 0, 10, None, None, None, None).unwrap();
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
        let page = log_page(r, 0, 10, None, None, None, None).unwrap();
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

        let page = log_page(r, 0, 10, None, None, None, None).unwrap();
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
        let hit = log_page(r, 0, 10, Some("side only"), None, None, None).unwrap();
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

        let page = log_page(r, 0, 10, None, None, None, None).unwrap();
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

        let page = log_page(r, 0, 10, None, None, None, None).unwrap();
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

        let page = log_page(r, 0, 10, None, None, None, None).unwrap();
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
        let by_msg = log_page(r, 0, 50, Some("alpha"), None, None, None).unwrap();
        assert_eq!(by_msg.commits.len(), 1);
        assert_eq!(by_msg.commits[0].subject, "add feature alpha");

        // author 命中："Zoe"
        let by_author = log_page(r, 0, 50, Some("Zoe"), None, None, None).unwrap();
        assert_eq!(by_author.commits.len(), 1);
        assert_eq!(by_author.commits[0].author_name, "Zoe");

        // hash 前綴命中：取某 commit 的短 hash 前綴。
        let full = &by_msg.commits[0].hash;
        let prefix = &full[..7];
        let by_hash = log_page(r, 0, 50, Some(prefix), None, None, None).unwrap();
        assert!(by_hash.commits.iter().any(|c| &c.hash == full));

        // no match → 空。
        let none = log_page(r, 0, 50, Some("zzz-nomatch-zzz"), None, None, None).unwrap();
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
        let page = log_page(r, 0, 50, None, Some("Alice"), None, None).unwrap();
        assert_eq!(page.commits.len(), 1);
        assert_eq!(page.commits[0].author_name, "Alice");
    }

    #[test]
    fn log_page_empty_repo_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        test_repo::init(tmp.path()); // init 但無 commit
        let page = log_page(tmp.path(), 0, 10, None, None, None, None).unwrap();
        assert!(page.commits.is_empty());
        assert!(!page.has_more);
    }

    #[test]
    fn log_page_limit_clamped() {
        let repo = linear_repo(2);
        // limit 超上限 → clamp 到 MAX_LOG_LIMIT，不報錯
        let page = log_page(repo.path(), 0, 99999, None, None, None, None).unwrap();
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
        // 不存在的 rev → Missing（不 panic）
        assert_eq!(
            file_at_rev(r, "deadbeef", "f.txt").unwrap(),
            FileAtRevResult::Missing
        );
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
        // --end-of-options 屏障下 spec 一律當 revision → Missing，且不產生該檔（F4 回歸）。
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        test_repo::init(r);
        test_repo::write_and_commit(r, "f.txt", "x\n", "c1");
        let sink = tmp.path().join("pwned");
        let inject = format!("--output={}", sink.display());
        assert_eq!(
            file_at_rev(r, &inject, "f.txt").unwrap(),
            FileAtRevResult::Missing
        );
        assert!(!sink.exists(), "rev option injection must not write a file");
    }

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
