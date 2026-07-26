use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const RETENTION_DAYS: i64 = 14;
const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;
const VALID_LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];
/// `app_lifecycle` 是 #40 新增的 kind（app_start／graceful_exit）。Logs pane 的
/// `LOG_KIND_OPTIONS` 與 i18n 的 kind 說明字串必須同步——三處是同一份語意。
const VALID_KINDS: [&str; 4] = ["debug", "user_action", "audit", "app_lifecycle"];

const LEVEL_DEBUG: u8 = 0;
const LEVEL_INFO: u8 = 1;
const LEVEL_WARN: u8 = 2;
const LEVEL_ERROR: u8 = 3;

/// 事件 level 的排序權重。未知 level 視為 info——預設門檻（info）下仍會落盤，
/// 不會因為打錯 level 字串而被靜默丟棄。
fn level_rank(level: &str) -> u8 {
    match level {
        "debug" => LEVEL_DEBUG,
        "info" => LEVEL_INFO,
        "warn" => LEVEL_WARN,
        "error" => LEVEL_ERROR,
        _ => LEVEL_INFO,
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LogEvent {
    pub level: String,
    pub kind: String,
    pub source: String,
    pub workspace_path: Option<String>,
    pub event: String,
    pub message: String,
    pub metadata: serde_json::Value,
}

pub struct LogSink {
    dir: PathBuf,
    last_cleanup: Option<NaiveDate>,
    min_level: u8,
    /// `None` = 用 process-global run context（生產路徑）。只有測試會覆寫，
    /// 好在同一個檔案裡模擬兩次 app run。
    run_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LogRecord {
    pub timestamp: String,
    /// 本筆 record 所屬的 app run（#40）。
    ///
    /// **`Option` 本身就是向後相容的關鍵**（serde 對缺席的 `Option` 欄位自動填
    /// `None`，不需要 `#[serde(default)]`）：#40 之前寫下的 record 沒有這個欄位，
    /// 若宣告成必填 `String`，`query_dir` 的 `from_str::<LogRecord>` 會對每一行
    /// 歷史資料失敗。`query_dir` 是逐行 `let Ok(record) = … else { continue }`，
    /// 所以後果是那些行被**逐筆靜默跳過**（不是整批失敗），但結果一樣：使用者
    /// 在 Logs pane 再也看不到 #40 之前的任何資料。
    /// `null` 的語意是「pre-#40，無法歸屬到任何一次執行」。
    pub run_id: Option<String>,
    pub level: String,
    pub kind: String,
    pub source: String,
    pub workspace_path: Option<String>,
    pub event: String,
    pub message: String,
    pub metadata: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct LogQueryFilters {
    pub since: Option<String>,
    pub until: Option<String>,
    pub levels: Option<Vec<String>>,
    pub kinds: Option<Vec<String>>,
    pub sources: Option<Vec<String>>,
    pub text: Option<String>,
    pub limit: Option<usize>,
}

impl LogSink {
    pub fn new(dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).ok();
        Self {
            dir,
            last_cleanup: None,
            min_level: LEVEL_DEBUG,
            run_id: None,
        }
    }

    /// 綁定固定 run id 的 sink。測試用來在同一個 daily file 裡模擬多次 app run；
    /// 生產路徑一律用 `new`，由 process-global run context 供值。
    #[cfg(test)]
    pub fn with_run_id(dir: PathBuf, run_id: &str) -> Self {
        Self {
            run_id: Some(run_id.to_string()),
            ..Self::new(dir)
        }
    }

    fn current_path(&self) -> PathBuf {
        let date = chrono::Local::now().format("%Y-%m-%d");
        self.dir.join(format!("yuzora-{date}.jsonl"))
    }

    pub fn write(&mut self, ev: LogEvent) {
        // 每日首筆寫入時補跑 cleanup（放在門檻判斷之前：嚴格門檻下仍會清理，
        // retention／size 上限在長時間不重啟下也會生效）
        let today = Local::now().date_naive();
        if self.last_cleanup != Some(today) {
            self.cleanup();
            self.last_cleanup = Some(today);
        }
        // 寫入端門檻：低於 min_level 的事件不落盤（例：預設 info 時的 git debug 探測）
        //
        // **已知限制（latent，本次刻意不處理）**：門檻在補 run_id 之前就 return，
        // 而 #40 的三個錨點（`app_start`／`graceful_exit`／`diagnostics.sample`）
        // 都是 `level: "info"`。使用者若把 log level 調成 warn 或 error，run 分組
        // 本身還能用（warn/error record 仍帶 run_id），但 `run-summary.json` 會
        // 失去 `app_version`／`host_pid`／`platform`／`previous_run_unclean` 與
        // 所有 long-task／lag／perf 峰值——摘要會退化成只有起訖時間與筆數。
        // 要修的話得讓 app_lifecycle 與 diagnostics 豁免門檻，那是另一個決策。
        if level_rank(&ev.level) < self.min_level {
            return;
        }
        let Ok(mut value) = serde_json::to_value(&ev) else {
            return; // 序列化失敗：整筆跳過，不寫壞行
        };
        if let serde_json::Value::Object(ref mut map) = value {
            // UTC：RFC3339 的 UTC 字串時間序 = 字典序，且跨時區／DST 不受影響
            map.insert(
                "timestamp".into(),
                serde_json::Value::String(Utc::now().to_rfc3339()),
            );
            // run_id 與 timestamp 同樣由寫入端統一補上（#40 §3.1）——因此 46 個
            // `logUserAction` 呼叫點與所有 Rust 端 `write_global` 呼叫點都不必改。
            map.insert(
                "run_id".into(),
                serde_json::Value::String(
                    self.run_id
                        .clone()
                        .unwrap_or_else(|| crate::run_context::current_run_id().to_string()),
                ),
            );
        }
        let Ok(line) = serde_json::to_string(&value) else {
            return;
        };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.current_path())
        {
            // 單次 write_all（含換行）：多寫者併發 append 時避免 torn line
            let _ = f.write_all(format!("{line}\n").as_bytes());
        }
    }

    pub fn set_min_level(&mut self, level: &str) {
        self.min_level = level_rank(level);
    }

    pub fn cleanup(&self) {
        self.cleanup_with_limits(RETENTION_DAYS, MAX_TOTAL_BYTES);
    }

    fn cleanup_with_limits(&self, retention_days: i64, max_total_bytes: u64) {
        let today = Local::now().date_naive();
        let mut files: Vec<(PathBuf, u64)> = vec![];
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // 只管理 yuzora-YYYY-MM-DD.jsonl；其他檔案（legacy sqlite 等）一律不動
                let Some(date) = name
                    .strip_prefix("yuzora-")
                    .and_then(|s| s.strip_suffix(".jsonl"))
                    .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                else {
                    continue;
                };
                // >= : 含今日共保留 retention_days 個檔
                if (today - date).num_days() >= retention_days {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                if date == today {
                    continue; // 今日檔正在寫入，不列入 size 清理候選
                }
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push((path, size));
            }
        }
        // 總量超限：由最舊（檔名排序最小）開始刪
        files.sort();
        let mut total: u64 = files.iter().map(|(_, s)| s).sum();
        for (path, size) in files {
            if total <= max_total_bytes {
                break;
            }
            let _ = std::fs::remove_file(&path);
            total -= size;
        }
    }
}

static GLOBAL_SINK: OnceLock<Mutex<LogSink>> = OnceLock::new();

fn global_sink() -> &'static Mutex<LogSink> {
    GLOBAL_SINK.get_or_init(|| Mutex::new(LogSink::new(global_log_dir())))
}

/// 全行程共享 sink 的目錄。cfg(test) 重導到 tempdir——cargo test 走到的任何生產
/// 路徑（run_git 等）都不會汙染 ~/.yuzora/logs。
fn global_log_dir() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join(format!("yuzora-test-logs-{}", std::process::id()))
    }
    #[cfg(not(test))]
    {
        default_log_dir()
    }
}

/// 單一共享 sink 的寫入口。Rust 端所有 log（git／ssh／process／acp／env）與前端
/// log_event 都走這裡，不再各自 new LogSink。
pub fn write_global(ev: LogEvent) {
    if let Ok(mut sink) = global_sink().lock() {
        sink.write(ev);
    }
}

/// 啟動期清理（lib.rs 呼叫一次）；其後由每日首筆寫入觸發。
pub fn cleanup_global() {
    if let Ok(sink) = global_sink().lock() {
        sink.cleanup();
    }
}

/// 將字串中所有 `scheme://user[:pass]@host` 的 userinfo 遮蔽為 `<redacted>`，
/// 供 git args 等可能含 credentials 的內容入 log 前使用。
pub fn mask_url_userinfo(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find("://") {
        let after_scheme = pos + 3;
        out.push_str(&rest[..after_scheme]);
        let tail = &rest[after_scheme..];
        // userinfo 只會出現在 authority 段（下一個 '/'、'?'、'#'、空白之前）
        let authority_end = tail
            .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace())
            .unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        if let Some(at) = authority.rfind('@') {
            out.push_str("<redacted>");
            out.push_str(&authority[at..]);
        } else {
            out.push_str(authority);
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}

/// 連線失敗（SSH/SFTP/DB）的統一落盤事件。level=warn（在預設門檻 info 下會落盤）。
/// 只記 host/port/user + 原因；host 與 reason 先過 mask_url_userinfo，避免呼叫端
/// 誤把含憑證的連線字串（如 postgres://user:pass@host）帶進來造成外洩。
pub fn connect_failure_event(
    source: &str,
    host: &str,
    port: u16,
    user: &str,
    reason: &str,
) -> LogEvent {
    let host = mask_url_userinfo(host);
    let reason = mask_url_userinfo(reason);
    LogEvent {
        level: "warn".to_string(),
        kind: "debug".to_string(),
        source: source.to_string(),
        workspace_path: None,
        event: "connect_failed".to_string(),
        message: format!("{source} connection to {user}@{host}:{port} failed: {reason}"),
        metadata: serde_json::json!({ "host": host, "port": port, "user": user }),
    }
}

// ---------------------------------------------------------------------------
// App lifecycle 事件（#40 §3.1）
//
// 這兩筆是把 daily JSONL 切成多次 run 的錨點：`app_start` 標出一次執行的起點與
// 環境，`graceful_exit` 標出它有沒有正常收尾。兩者都是 `level=info`，在預設門檻
// 下必定落盤。
// ---------------------------------------------------------------------------

pub const APP_LIFECYCLE_KIND: &str = "app_lifecycle";
pub const APP_START_EVENT: &str = "app_start";
pub const GRACEFUL_EXIT_EVENT: &str = "graceful_exit";

pub fn app_start_event(
    context: &crate::run_context::RunContext,
    previous_unclean: bool,
) -> LogEvent {
    LogEvent {
        level: "info".to_string(),
        kind: APP_LIFECYCLE_KIND.to_string(),
        source: "app".to_string(),
        workspace_path: None,
        event: APP_START_EVENT.to_string(),
        message: format!(
            "app run {} started (version {}, pid {}, {})",
            context.run_id, context.app_version, context.host_pid, context.platform
        ),
        metadata: serde_json::json!({
            "app_version": context.app_version,
            "host_pid": context.host_pid,
            "platform": context.platform,
            "previous_run_unclean": previous_unclean,
        }),
    }
}

pub fn graceful_exit_event(context: &crate::run_context::RunContext) -> LogEvent {
    LogEvent {
        level: "info".to_string(),
        kind: APP_LIFECYCLE_KIND.to_string(),
        source: "app".to_string(),
        workspace_path: None,
        event: GRACEFUL_EXIT_EVENT.to_string(),
        message: format!("app run {} reached the exit handler", context.run_id),
        metadata: serde_json::json!({
            "app_version": context.app_version,
            "host_pid": context.host_pid,
            "platform": context.platform,
        }),
    }
}

/// 啟動期一次性動作：判斷上一次執行是否 unclean、把本次標成「執行中」、寫下
/// `app_start`。回傳 previous-run 的判定，供呼叫端（`lib.rs`）測試／診斷使用。
pub fn record_app_start() -> bool {
    let context = crate::run_context::current();
    let state_path = crate::run_context::run_state_path();
    let previous_unclean = crate::run_context::previous_run_unclean(&state_path);
    let _ = crate::run_context::write_run_state(&state_path, &context.run_id, false);
    write_global(app_start_event(context, previous_unclean));
    previous_unclean
}

/// 走到 exit handler 的**第一步**：寫下 `graceful_exit` log record。
///
/// 刻意**不**在這裡標記 marker 檔——見 `mark_run_clean`。
pub fn record_graceful_exit() {
    write_global(graceful_exit_event(crate::run_context::current()));
}

/// 關機清理**全部跑完之後**才把 marker 檔標成 clean。
///
/// 兩者拆開是必要的：`graceful_exit` 記錄的是「有走到 exit handler」，而 marker
/// 檔記錄的是「有乾淨地結束」，這是兩件事。若在清理之前就標 clean，下面這條序列
/// 會讓 unclean 訊號永久失效：
///
///   ⌘Q → marker 立刻 clean → `pty kill_all` / `ssh kill_all` / DB shutdown 其中
///   一步 hang（只有 DB 那段有 timeout）→ 使用者強制結束 → 下次啟動誤判為 clean。
///
/// issue #40 的原始症狀就是 Windows `AppHangB1`，而「關閉時卡住」正是最可能落在
/// 這條路上的情境——那也正是最需要 `previous_run_unclean` 為真的時候。
pub fn mark_run_clean() {
    let context = crate::run_context::current();
    let _ = crate::run_context::write_run_state(
        &crate::run_context::run_state_path(),
        &context.run_id,
        true,
    );
}

pub fn default_log_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".yuzora")
        .join("logs")
}

fn log_config_path() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join(format!("yuzora-test-logging-{}.json", std::process::id()))
    }
    #[cfg(not(test))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".yuzora")
            .join("logging.json")
    }
}

/// 讀持久化的 min level；缺檔、壞 JSON、或非 VALID_LEVELS 值一律回 "info"。
pub fn read_log_level_from(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("minLevel")
                .and_then(|l| l.as_str())
                .map(|s| s.to_string())
        })
        .filter(|l| VALID_LEVELS.contains(&l.as_str()))
        .unwrap_or_else(|| "info".to_string())
}

pub fn write_log_level_to(path: &Path, level: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let body = serde_json::json!({ "minLevel": level }).to_string();
    std::fs::write(path, body).map_err(|e| e.to_string())
}

/// 套用 level 到全域共享 sink（write_global 走的那個）。
pub fn set_min_level_global(level: &str) {
    if let Ok(mut sink) = global_sink().lock() {
        sink.set_min_level(level);
    }
}

/// 啟動期讀持久化設定並套用（lib.rs 呼叫一次）。無設定檔時 = info。
pub fn apply_persisted_log_level() {
    set_min_level_global(&read_log_level_from(&log_config_path()));
}

pub(crate) fn retained_log_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|rd| rd.flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("yuzora-"))
                .and_then(|name| name.strip_suffix(".jsonl"))
                .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                .is_some()
        })
        .collect();
    files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    files
}

fn matches_filter(value: &str, filters: &Option<Vec<String>>) -> bool {
    filters
        .as_ref()
        .map(|items| items.iter().any(|item| item == value))
        .unwrap_or(true)
}

/// 查詢用時間篩選。寫入端是 RFC3339（新資料 UTC、歷史資料含 +08:00 等 offset），
/// 前端可能送 datetime-local（無時區）或純日期——一律 parse 成 DateTime 比較，
/// 不做字典序。parse 不出的篩選值視為未設定。
fn parse_query_time(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return local_to_utc(naive);
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0).and_then(local_to_utc);
    }
    None
}

/// 無時區的輸入視為本地時間（Logs pane 的使用情境）。
fn local_to_utc(naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.with_timezone(&Utc))
}

struct TimeBounds {
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}

impl TimeBounds {
    fn from(filters: &LogQueryFilters) -> Self {
        Self {
            since: filters.since.as_deref().and_then(parse_query_time),
            until: filters.until.as_deref().and_then(parse_query_time),
        }
    }

    fn contains(&self, timestamp: &str) -> bool {
        if self.since.is_none() && self.until.is_none() {
            return true;
        }
        // 有時間篩選但 record timestamp parse 不出 → 視為不符合
        let Ok(ts) = DateTime::parse_from_rfc3339(timestamp) else {
            return false;
        };
        let ts = ts.with_timezone(&Utc);
        self.since.map(|since| ts >= since).unwrap_or(true)
            && self.until.map(|until| ts <= until).unwrap_or(true)
    }
}

fn record_matches(record: &LogRecord, filters: &LogQueryFilters, bounds: &TimeBounds) -> bool {
    matches_filter(&record.level, &filters.levels)
        && matches_filter(&record.kind, &filters.kinds)
        && matches_filter(&record.source, &filters.sources)
        && bounds.contains(&record.timestamp)
        && filters
            .text
            .as_ref()
            .map(|text| {
                record.event.contains(text)
                    || record.message.contains(text)
                    // git stderr 等診斷內容在 metadata，一併納入搜尋
                    || serde_json::to_string(&record.metadata)
                        .map(|meta| meta.contains(text))
                        .unwrap_or(false)
            })
            .unwrap_or(true)
}

pub fn query_dir(dir: &Path, filters: &LogQueryFilters) -> Vec<LogRecord> {
    let limit = filters.limit.unwrap_or(500);
    let mut records = Vec::new();
    if limit == 0 {
        return records;
    }
    let bounds = TimeBounds::from(filters);

    // 檔案由新到舊；檔內順序掃描，ring buffer 只留該檔最後（最新）remaining 筆。
    // 整體結果 newest-first，記憶體 O(limit)、不整檔載入。
    for path in retained_log_files(dir) {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let remaining = limit - records.len();
        let mut newest: VecDeque<LogRecord> = VecDeque::new();
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            let Ok(record) = serde_json::from_str::<LogRecord>(&line) else {
                continue;
            };
            if record_matches(&record, filters, &bounds) {
                if newest.len() == remaining {
                    newest.pop_front();
                }
                newest.push_back(record);
            }
        }
        records.extend(newest.into_iter().rev());
        if records.len() >= limit {
            return records;
        }
    }
    records
}

#[tauri::command]
pub async fn log_query(filters: LogQueryFilters) -> Result<Vec<LogRecord>, String> {
    // 同步 command 會在 main thread 掃檔（最壞近 100MB）→ async + spawn_blocking
    tauri::async_runtime::spawn_blocking(move || query_dir(&default_log_dir(), &filters))
        .await
        .map_err(|err| err.to_string())
}

pub fn sources_dir(dir: &Path) -> Vec<String> {
    let mut sources: BTreeSet<String> = BTreeSet::new();
    for path in retained_log_files(dir) {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            // 只取 source 欄位，不建整批 LogRecord
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if let Some(source) = value.get("source").and_then(|v| v.as_str()) {
                if !sources.contains(source) {
                    sources.insert(source.to_string());
                }
            }
        }
    }
    sources.into_iter().collect()
}

#[tauri::command]
pub async fn log_sources() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| sources_dir(&default_log_dir()))
        .await
        .map_err(|err| err.to_string())
}

fn current_username() -> Option<String> {
    std::env::var("USER")
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::env::var("USERNAME")
                .ok()
                .filter(|name| !name.is_empty())
        })
        .or_else(|| {
            dirs::home_dir()
                .and_then(|home| home.file_name().map(|name| name.to_owned()))
                .and_then(|name| name.to_str().map(|name| name.to_string()))
                .filter(|name| !name.is_empty())
        })
}

fn sanitize_line(line: &str, home: Option<&str>, username: Option<&str>) -> String {
    let mut sanitized = line.to_string();
    if let Some(home) = home.filter(|value| !value.is_empty()) {
        sanitized = sanitized.replace(home, "~");
    }
    if let Some(username) = username.filter(|value| !value.is_empty()) {
        sanitized = sanitized
            .replace(&format!("/Users/{username}/"), "/Users/<user>/")
            .replace(&format!(r#"\Users\{username}\"#), r#"\Users\<user>\"#)
            .replace(&format!(r#"\\Users\\{username}\\"#), r#"\\Users\\<user>\\"#);
    }
    sanitized
}

// ---------------------------------------------------------------------------
// 匯出時去識別化（issue #41）
//
// 立場：原始資料留在本機 log（診斷需要），只有「匯出／複製」這道出口做 redaction。
// 逐行 schema-aware：先 parse JSON，parse 不出的行一律 fail-closed（輸出佔位記錄，
// 不輸出原文），parse 得出的行遞迴走訪每個 string value 套 scrubber 鏈。
// ---------------------------------------------------------------------------

/// 匯出時 redaction 的統計摘要。前端據此顯示「保留 vs 移除」的資料類型與計數。
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct SanitizeSummary {
    pub paths: u32,
    pub hosts: u32,
    pub usernames: u32,
    pub fingerprints: u32,
    pub secrets: u32,
    pub unparseable_lines: u32,
}

/// log_export 的回傳：目的路徑 + sanitize 摘要（sanitize=false 時 summary=None）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LogExportResult {
    pub path: String,
    pub summary: Option<SanitizeSummary>,
}

/// 走訪過程只有共享參考（redact_line 的簽章是 &Redactor），計數用 Cell 累加。
#[derive(Default)]
struct RedactCounters {
    paths: Cell<u32>,
    hosts: Cell<u32>,
    usernames: Cell<u32>,
    fingerprints: Cell<u32>,
    secrets: Cell<u32>,
    unparseable_lines: Cell<u32>,
}

fn bump(counter: &Cell<u32>) {
    counter.set(counter.get().saturating_add(1));
}

/// 對 path/host/user/fingerprint 套更強規則的 key（key 名本身不改）。
const PATH_KEYS: [&str; 7] = [
    "workspace_path",
    "workspace",
    "cwd",
    "path",
    "command",
    "server_path",
    "shell",
];
const HOST_KEYS: [&str; 2] = ["host", "address"];
const USER_KEYS: [&str; 2] = ["user", "username"];

/// 識別字 run 的掃描上限。最長的秘密 key 名是 `connectionstring`（16），實際 key
/// 連前後綴（`AZURE_STORAGE_ACCOUNT_KEY`）也遠短於此。
const MAX_SECRET_KEY_CHARS: usize = 64;

/// 秘密欄位名的**子字串**。用包含式比對而非精確相等——實際 key 名多是
/// `GEOSENSE_API_KEY`、`refresh_token`、`db_password` 這種前後綴組合。
const SECRET_KEY_PARTS: [&str; 15] = [
    "authorization",
    "api_key",
    "api-key",
    "apikey",
    "password",
    "passwd",
    "credential",
    "secret",
    "token",
    // Azure 連線字串（本專案有 MSSQL／Azure 使用情境，driver 的錯誤原文會原封進 log）
    "accountkey",
    "account_key",
    "sharedaccess",
    "connectionstring",
    "clientid",
    "client_id",
];

/// key 名雖然含秘密子字串、值卻是診斷資訊的欄位。改成「以 word 邊界比對」救不了
/// 這一類——`firstToken` 的 `Token` 本來就是 camelCase 的獨立 word，而 camelCase
/// 切詞又不能省（否則 `accessToken`、`apiToken` 這種 key 會漏遮），只能逐一列外。
/// `firstToken`／`firstTokenOnPath` 是 `agent_process` 診斷 exit 127 的一組欄位，
/// 遮掉 `firstToken` 會讓兩者一起失去意義。
const SECRET_KEY_ALLOWLIST: [&str; 2] = ["firsttoken", "firsttokenonpath"];

/// key 名（JSON key 或 `key=value` 的 key）是否屬於秘密欄位。
fn is_secret_key_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if SECRET_KEY_ALLOWLIST.contains(&lower.as_str()) {
        return false;
    }
    lower == "pwd" || SECRET_KEY_PARTS.iter().any(|part| lower.contains(part))
}

/// `Authorization:` / `Proxy-Authorization:` 這類欄位——值的開頭是 auth scheme
/// 而非憑證本體。
fn is_authorization_key(name: &str) -> bool {
    name.to_ascii_lowercase().contains("authorization")
}

/// `Authorization:` 後可能出現的 scheme。scheme 本身是欄位語法（保留有診斷價值），
/// 真正的憑證在它後面——`Basic` 的 base64 解開就是 `user:password`。
const AUTH_SCHEMES: [&str; 5] = ["bearer", "basic", "digest", "negotiate", "ntlm"];

/// scheme 前綴的長度（含其後的空白）；不是 scheme 就回 0。
fn auth_scheme_prefix_len(value: &str) -> usize {
    for scheme in AUTH_SCHEMES {
        let Some(head) = value.get(..scheme.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(scheme) {
            continue;
        }
        let tail = &value[scheme.len()..];
        let spaces = tail.len() - tail.trim_start_matches(' ').len();
        if spaces > 0 {
            return scheme.len() + spaces;
        }
    }
    0
}

/// 已知憑證前綴——形狀明確，不必再看字元組成。
const SECRET_PREFIXES: [&str; 12] = [
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "github_pat_",
    "sk-",
    "AKIA",
    "eyJ",
];

pub struct Redactor {
    salt: Vec<u8>,
    home: Option<String>,
    username: Option<String>,
    counters: RedactCounters,
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

/// process 生命週期內共用的 salt：Copy 與 Export（以及連按兩次 Copy）對同一個
/// host/path 產生相同 hash，判讀者才能把剪貼簿內容與 zip 對起來。每次 app 啟動
/// 重新隨機 → 跨 bundle 仍不可關聯。
///
/// 取捨（有意識偏離 spec 的「每次匯出一組新 salt」）：為了滿足 AC7（Copy 與
/// Export 語意一致）只能共用 salt。代價是同一次 app 執行中匯出的兩份 bundle 可以
/// 互相關聯——桌面 app 常數週不重啟，這條 salt 的壽命因此很長。
static REDACTION_SALT: OnceLock<[u8; 16]> = OnceLock::new();

fn process_redaction_salt() -> &'static [u8] {
    REDACTION_SALT.get_or_init(|| rand::random::<u128>().to_le_bytes())
}

impl Redactor {
    /// 同一個 process 內共用 salt（見 REDACTION_SALT）：同值可關聯，跨 bundle 不可
    /// 關聯，也無法用字典反查。
    pub fn new() -> Self {
        Self::with_salt(process_redaction_salt())
    }

    /// salt 可注入——測試要斷言確定的 HASH8 就靠這個。
    pub fn with_salt(salt: &[u8]) -> Self {
        Self {
            salt: salt.to_vec(),
            home: dirs::home_dir().map(|home| home.to_string_lossy().to_string()),
            username: current_username(),
            counters: RedactCounters::default(),
        }
    }

    pub fn summary(&self) -> SanitizeSummary {
        SanitizeSummary {
            paths: self.counters.paths.get(),
            hosts: self.counters.hosts.get(),
            usernames: self.counters.usernames.get(),
            fingerprints: self.counters.fingerprints.get(),
            secrets: self.counters.secrets.get(),
            unparseable_lines: self.counters.unparseable_lines.get(),
        }
    }

    /// HASH8 = SHA-256(salt ‖ value) 前 8 個 hex。
    fn hash8(&self, value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.salt);
        hasher.update(value.as_bytes());
        hasher
            .finalize()
            .iter()
            .take(4)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// 逐行 redaction 入口。fail-closed：parse 不出的行不輸出原文。
pub fn redact_line(line: &str, redactor: &Redactor) -> String {
    // 既有行為（home → `~`、/Users/<name>/ → /Users/<user>/）先套用，之後才進
    // 路徑 scrubber。
    let prepared = sanitize_line(line, redactor.home.as_deref(), redactor.username.as_deref());
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&prepared) else {
        bump(&redactor.counters.unparseable_lines);
        return unparseable_placeholder(line.len());
    };
    match serde_json::to_string(&redact_value(&value, None, redactor)) {
        Ok(rendered) => rendered,
        Err(_) => {
            bump(&redactor.counters.unparseable_lines);
            unparseable_placeholder(line.len())
        }
    }
}

fn unparseable_placeholder(bytes: usize) -> String {
    format!(r#"{{"redacted":"unparseable-log-line","bytes":{bytes}}}"#)
}

fn redact_value(
    value: &serde_json::Value,
    key: Option<&str>,
    redactor: &Redactor,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(redact_string(text, key, redactor))
        }
        // array 沿用父層 key（例：`"paths": ["/a/b"]`）
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| redact_value(item, key, redactor))
                .collect(),
        ),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(name, child)| {
                    (
                        name.clone(),
                        redact_value(child, Some(name.as_str()), redactor),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn redact_string(input: &str, key: Option<&str>, redactor: &Redactor) -> String {
    // fingerprint 先於 secrets：SHA256:<base64> 否則會被高熵字串規則吃成
    // `<redacted>`，失去同一主機金鑰的關聯性。
    let stage = scrub_fingerprints(input, redactor);
    let stage = scrub_secrets(&stage, redactor);
    if let Some(key) = key {
        if let Some(strong) = scrub_by_key(&stage, key, redactor) {
            return strong;
        }
    }
    let stage = scrub_user_at_host(&stage, redactor);
    // URL：scheme 後的 authority 交給 host scrubber，路徑段留給 scrub_paths
    let stage = scrub_urls(&stage, redactor);
    let stage = scrub_paths(&stage, redactor);
    scrub_ip_literals(&stage, redactor)
}

/// 特定 key 的更強規則：整個 value 當成單一 path／host／user／fingerprint 處理，
/// 不受一般掃描的分隔字元限制（例：含空白的 `/Users/me/My Documents/a.txt`）。
fn scrub_by_key(value: &str, key: &str, redactor: &Redactor) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    // metadata 直接掛 `password` / `token` / `GEOSENSE_API_KEY` 之類的 key 時
    // 整個值移除，不留 hash
    if is_secret_key_name(key) {
        bump(&redactor.counters.secrets);
        return Some("<redacted>".to_string());
    }
    if value.contains("<path:") || value.contains("<host:") {
        return None;
    }
    if key == "fingerprint" {
        if value.contains("<fp:") {
            return None;
        }
        bump(&redactor.counters.fingerprints);
        return Some(format!("<fp:{}>", redactor.hash8(value)));
    }
    if HOST_KEYS.contains(&key) {
        // 帶 scheme／路徑的 address（postgres://…）交給一般鏈處理
        if value.contains("://") || value.contains('/') || value.contains('@') {
            return None;
        }
        // `db.internal:5432` 這種帶 port 的 address：port 是診斷價值要保留，
        // 而且 host class 必須看不含 port 的 host 才會判對。
        let (host, port) = split_host_port(value);
        return Some(format!("{}{port}", scrub_host_token(host, redactor)));
    }
    if USER_KEYS.contains(&key) {
        if value.contains("<user:") {
            return None;
        }
        bump(&redactor.counters.usernames);
        return Some(format!("<user:{}>", redactor.hash8(value)));
    }
    if PATH_KEYS.contains(&key) && path_start_prefix(value).is_some() {
        return Some(replace_path_span(value, redactor));
    }
    None
}

// --- scrubber：SSH fingerprint ---------------------------------------------

fn scrub_fingerprints(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        let rest = &input[idx..];
        if let Some(len) = fingerprint_span_len(rest) {
            bump(&redactor.counters.fingerprints);
            out.push_str(&format!("<fp:{}>", redactor.hash8(&rest[..len])));
            idx += len;
            continue;
        }
        let ch = rest.chars().next().unwrap_or('\u{0}');
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

/// `SHA256:<base64>`、`MD5:<hex:hex…>`，以及裸的 `aa:bb:…`（≥6 組雙位 hex）。
fn fingerprint_span_len(rest: &str) -> Option<usize> {
    for prefix in ["SHA256:", "sha256:"] {
        if let Some(tail) = rest.strip_prefix(prefix) {
            let len = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
                .map(char::len_utf8)
                .sum::<usize>();
            if len >= 20 {
                return Some(prefix.len() + len);
            }
        }
    }
    for prefix in ["MD5:", "md5:"] {
        if let Some(tail) = rest.strip_prefix(prefix) {
            if let Some(len) = colon_hex_span_len(tail, 6) {
                return Some(prefix.len() + len);
            }
        }
    }
    colon_hex_span_len(rest, 6)
}

/// `aa:bb:cc…` 形式的長度（每組剛好兩個 hex，至少 `groups` 組）。
fn colon_hex_span_len(rest: &str, groups: usize) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut idx = 0usize;
    let mut count = 0usize;
    loop {
        if idx + 2 > bytes.len()
            || !bytes[idx].is_ascii_hexdigit()
            || !bytes[idx + 1].is_ascii_hexdigit()
        {
            break;
        }
        // 第三個 hex → 不是雙位分組
        if bytes.get(idx + 2).is_some_and(u8::is_ascii_hexdigit) {
            break;
        }
        count += 1;
        idx += 2;
        if bytes.get(idx) == Some(&b':') {
            idx += 1;
        } else {
            break;
        }
    }
    (count >= groups).then_some(idx)
}

// --- scrubber：secrets ------------------------------------------------------

fn scrub_secrets(input: &str, redactor: &Redactor) -> String {
    let stage = scrub_private_key_blocks(input, redactor);
    let stage = scrub_bearer_tokens(&stage, redactor);
    let stage = scrub_secret_assignments(&stage, redactor);
    scrub_high_entropy_runs(&stage, redactor)
}

fn scrub_private_key_blocks(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("-----BEGIN") {
        let after = &rest[start..];
        let Some(header_end) = after.find("KEY-----") else {
            break;
        };
        let end = after
            .find("-----END")
            .and_then(|pos| after[pos..].rfind("-----").map(|tail| pos + tail + 5))
            .unwrap_or(after.len())
            .max(header_end + "KEY-----".len());
        bump(&redactor.counters.secrets);
        out.push_str(&rest[..start]);
        out.push_str("<redacted>");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn scrub_bearer_tokens(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        let rest = &input[idx..];
        // `rest.get(..6)` 而非 `rest[..6]`：非 ASCII 內容（`→`、中文、emoji）在
        // byte index 6 未必是 char boundary，直接切片會 panic。
        let matched = rest
            .get(..6)
            .is_some_and(|head| head.eq_ignore_ascii_case("bearer"))
            && rest.as_bytes().get(6) == Some(&b' ')
            && is_word_boundary(char_before(input, idx));
        if matched {
            let value = rest[7..].trim_start();
            let value_len = value
                .chars()
                .take_while(|c| !is_secret_terminator(*c))
                .map(char::len_utf8)
                .sum::<usize>();
            if value_len > 0 {
                bump(&redactor.counters.secrets);
                out.push_str(&rest[..6]);
                out.push_str(" <redacted>");
                idx += 7 + (rest.len() - 7 - value.len()) + value_len;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or('\u{0}');
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

fn scrub_secret_assignments(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        let rest = &input[idx..];
        if let Some((key_len, value_len)) = secret_assignment_span(rest, char_before(input, idx)) {
            bump(&redactor.counters.secrets);
            out.push_str(&rest[..key_len]);
            out.push_str("<redacted>");
            idx += key_len + value_len;
            continue;
        }
        let ch = rest.chars().next().unwrap_or('\u{0}');
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

/// 回傳 (要原樣保留的前綴長度＝key + 分隔符 + 引號, 要遮蔽的 value 長度)。
fn secret_assignment_span(rest: &str, prev: Option<char>) -> Option<(usize, usize)> {
    if !is_word_boundary(prev) {
        return None;
    }
    // 先整個識別字取出來（`GEOSENSE_API_KEY`、`refresh_token`…）再判斷是不是秘密
    // 欄位；長度用 char 累加，非 ASCII 內容不會切在 char boundary 中間。
    // `take(MAX_SECRET_KEY_CHARS)`：`.` 同時是識別字字元與 word boundary，沒有上限
    // 時 `a.a.a…` 這種輸入會讓每個位置都掃過整段 run（O(n²)）。
    let key_len = rest
        .chars()
        .take(MAX_SECRET_KEY_CHARS)
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .map(char::len_utf8)
        .sum::<usize>();
    if key_len == 0 || !is_secret_key_name(&rest[..key_len]) {
        return None;
    }
    let mut cursor = key_len;
    let bytes = rest.as_bytes();
    while matches!(bytes.get(cursor), Some(b' ' | b'"' | b'\'')) {
        cursor += 1;
    }
    let separator = *bytes.get(cursor)?;
    if !matches!(separator, b'=' | b':') {
        return None;
    }
    cursor += 1;
    while matches!(bytes.get(cursor), Some(b' ' | b'"' | b'\'')) {
        cursor += 1;
    }
    // `Authorization: Basic <base64>`：值的第一個 token 是 scheme，遮它等於放走真正
    // 的憑證（`Basic` 的 base64 解開就是 `user:password`）。scheme 原樣保留、從它
    // 後面開始遮，形狀與 scrub_bearer_tokens 一致。
    if is_authorization_key(&rest[..key_len]) {
        cursor += auth_scheme_prefix_len(&rest[cursor..]);
    }
    let value_len = rest[cursor..]
        .chars()
        .take_while(|c| !is_secret_terminator(*c))
        .map(char::len_utf8)
        .sum::<usize>();
    if value_len == 0 {
        return None;
    }
    // `=` 是明確的賦值，一律遮蔽。`:` 同時是散文的標點，需要再分辨：
    if separator == b':' {
        let value = &rest[cursor..cursor + value_len];
        // 純數字一律視為計數（`tokens: 1523` 這種 usage 統計），任何欄位名都不遮。
        if value.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        // 值形狀的啟發式只套用在**散文也會用到的欄位名**上。`password: hunter`
        // 的 key 已經被認出是憑證欄位，卻因為值「短且全小寫」而被放走——輸出仍
        // 標示為「已去識別化」，密碼卻是原文（P1）。明確的憑證欄位一律遮，值再
        // 普通也一樣：漏遮的代價遠大於多遮一個字。
        if secret_key_is_prose_ambiguous(&rest[..key_len]) && !looks_like_secret_value(value) {
            return None;
        }
    }
    Some((cursor, value_len))
}

/// 散文也會用到的秘密欄位詞。`merge feat/secret: hotfix for prod outage` 的
/// `secret` 是分支名的一部分，不是欄位名——只有這一類才保留「值不像憑證就放過」
/// 的判斷。`password`／`credential`／`api_key` 等在實務上不會出現在散文裡。
const PROSE_AMBIGUOUS_SECRET_PARTS: [&str; 1] = ["secret"];

/// key 名是否**只**因為散文常用詞而被判為秘密欄位。只要命中任何一個明確的憑證
/// 詞（`db_password` 命中 `password`），就不算模稜兩可，值形狀不再有否決權。
fn secret_key_is_prose_ambiguous(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower == "pwd" {
        return false;
    }
    SECRET_KEY_PARTS
        .iter()
        .filter(|part| lower.contains(*part))
        .all(|part| PROSE_AMBIGUOUS_SECRET_PARTS.contains(part))
}

/// 純數字（`tokens: 42` 這類計數）與純小寫單字不算憑證；其餘（混大小寫／含數字
/// 或符號／長度 ≥ 16）視為憑證。
fn looks_like_secret_value(value: &str) -> bool {
    if value.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    value.len() >= 16 || !value.chars().all(|c| c.is_ascii_lowercase())
}

/// 高熵連續字串（API key、JWT、base64 blob、hex digest）。run 的字元集含 `.` 與
/// `/`——base64 字母表本來就有 `/`，JWT 是 `.` 分段——被判定為非秘密的 run 原樣輸出，
/// 因此後面的路徑／host scrubber 仍照常運作。
fn scrub_high_entropy_runs(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        let rest = &input[idx..];
        let run_len = rest
            .chars()
            .take_while(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '+' | '=' | '_' | '-' | '.' | '/')
            })
            .map(char::len_utf8)
            .sum::<usize>();
        if run_len > 0 && is_high_entropy(&rest[..run_len]) {
            bump(&redactor.counters.secrets);
            out.push_str("<redacted>");
            idx += run_len;
            continue;
        }
        if run_len > 0 {
            out.push_str(&rest[..run_len]);
            idx += run_len;
            continue;
        }
        let ch = rest.chars().next().unwrap_or('\u{0}');
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

fn is_high_entropy(run: &str) -> bool {
    // 已知前綴（xoxb- / ghp_ / sk-proj- / AKIA / JWT 的 eyJ）長度就足以判定
    if run.len() >= 20 && SECRET_PREFIXES.iter().any(|prefix| run.starts_with(prefix)) {
        return true;
    }
    if run.len() < 32 {
        return false;
    }
    // 絕對路徑交給 path scrubber（保留 basename 的診斷價值）
    if run.starts_with('/') {
        return false;
    }
    // 以 - _ . / 切段後逐段判斷：JWT 三段、`ghp_…` 的尾段、snake_case 事件名都在此收斂
    run.split(['-', '_', '.', '/']).any(segment_is_high_entropy) || is_base64_blob(run)
}

/// 單一分段是否高熵。門檻 24 字元——snake_case 的英文單字幾乎不會這麼長。
fn segment_is_high_entropy(segment: &str) -> bool {
    // base64 的 `=` padding 不影響熵，但會讓「全為英數」的判斷整段失效
    // （base64url 的 `-`／`_` 切段後，padding 會留在最後一段）→ 先剝掉。
    let segment = segment.trim_end_matches('=');
    if segment.len() < 24 || !segment.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    // git commit SHA 白名單：support bundle 的核心用途是把現象對回程式碼版本。
    // 掛在 secret key／assignment 上的 SHA 仍會被那兩條規則先吃掉。
    if is_git_sha_shape(segment) {
        return false;
    }
    let has_digit = segment.chars().any(|c| c.is_ascii_digit());
    let has_upper = segment.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = segment.chars().any(|c| c.is_ascii_lowercase());
    let all_hex = segment.chars().all(|c| c.is_ascii_hexdigit());
    has_digit && (all_hex || has_upper || has_lower)
}

/// git object id 的形狀：縮寫 7–12 位或完整 40 位純小寫 hex。中間的 13–39 位不放行
/// ——診斷價值極低，卻正好涵蓋 32 位小寫 hex 的 Twilio auth token、Mailchimp key 與
/// 一票 MD5-based session/API key。
fn is_git_sha_shape(segment: &str) -> bool {
    let len = segment.len();
    ((7..=12).contains(&len) || len == 40)
        && segment
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// base64 blob（標準字母表）。不要求出現 `+`／`/`——標準 base64 有約 1/3 的機率
/// 一個都不含。相對路徑因為要求大小寫與數字齊全而不會誤中。
fn is_base64_blob(run: &str) -> bool {
    let core = run.trim_end_matches('=');
    core.len() >= 32
        && core
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/'))
        && core.chars().any(|c| c.is_ascii_digit())
        && core.chars().any(|c| c.is_ascii_uppercase())
        && core.chars().any(|c| c.is_ascii_lowercase())
}

fn is_secret_terminator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | '&' | ',' | ';' | '}' | ']' | ')' | '<'
        )
}

fn is_word_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => !(c.is_ascii_alphanumeric() || c == '_' || c == '-'),
    }
}

fn char_before(input: &str, idx: usize) -> Option<char> {
    input[..idx].chars().next_back()
}

// --- scrubber：user@host:port ----------------------------------------------

fn scrub_user_at_host(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut emitted = 0usize;
    for (at, ch) in input.char_indices() {
        if ch != '@' || at < emitted {
            continue;
        }
        let user_start = emitted
            + input[emitted..at]
                .char_indices()
                .rev()
                .take_while(|(_, c)| is_host_char(*c))
                .last()
                .map(|(offset, _)| offset)
                .unwrap_or(at - emitted);
        let Some(host_len) = host_span_len(&input[at + 1..]) else {
            continue;
        };
        if user_start == at {
            continue;
        }
        let host_end = at + 1 + host_len;
        let port_len = port_span_len(&input[host_end..]);
        out.push_str(&input[emitted..user_start]);
        // 已被 mask_url_userinfo 處理過的 `<redacted>@host` 不再重複包裝
        if char_before(input, user_start) == Some('>') {
            out.push_str(&input[user_start..at]);
        } else {
            bump(&redactor.counters.usernames);
            out.push_str(&format!(
                "<user:{}>",
                redactor.hash8(&input[user_start..at])
            ));
        }
        out.push('@');
        out.push_str(&scrub_host_token(&input[at + 1..host_end], redactor));
        out.push_str(&input[host_end..host_end + port_len]);
        emitted = host_end + port_len;
        // scp 形式的 git remote（`git@host:group/repo.git`）：`:` 之後是 repo 路徑而
        // 非 port，敏感度與 URL 的路徑段相同 → 整段 hash。
        if port_len == 0 {
            if let Some(len) = scp_path_span_len(&input[emitted..]) {
                bump(&redactor.counters.paths);
                out.push(':');
                out.push_str(&format!(
                    "<path:{}>",
                    redactor.hash8(&input[emitted + 1..emitted + len])
                ));
                emitted += len;
            }
        }
    }
    out.push_str(&input[emitted..]);
    out
}

fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
}

/// scp 形式 remote 的 `:group/repo.git` 長度（含開頭的 `:`）。`:` 後必須直接接路徑
/// 字元——`git@host: Permission denied` 這種散文冒號後面是空白，不算路徑。
fn scp_path_span_len(rest: &str) -> Option<usize> {
    let tail = rest.strip_prefix(':')?;
    let head = tail.chars().next()?;
    if !(head.is_ascii_alphanumeric() || matches!(head, '/' | '~' | '_' | '.')) {
        return None;
    }
    let len = tail
        .chars()
        .take_while(|c| !is_path_terminator(*c))
        .map(char::len_utf8)
        .sum::<usize>();
    (len > 0).then_some(len + 1)
}

fn host_span_len(rest: &str) -> Option<usize> {
    if rest.starts_with('[') {
        return rest.find(']').map(|pos| pos + 1);
    }
    let len = rest
        .chars()
        .take_while(|c| is_host_char(*c))
        .map(char::len_utf8)
        .sum::<usize>();
    (len > 0).then_some(len)
}

fn port_span_len(rest: &str) -> usize {
    if !rest.starts_with(':') {
        return 0;
    }
    let digits = rest[1..].chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        0
    } else {
        digits + 1
    }
}

/// 切出 `host` 與 `:port` 尾段。未加中括號的 IPv6（`fd00::1`）不切，避免把位址
/// 的最後一組當成 port。
fn split_host_port(value: &str) -> (&str, &str) {
    let authority_end = value.rfind(']').map(|pos| pos + 1).unwrap_or(0);
    let Some(offset) = value[authority_end..].rfind(':') else {
        return (value, "");
    };
    let idx = authority_end + offset;
    let port = &value[idx + 1..];
    let bare_ipv6 = authority_end == 0 && value[..idx].contains(':');
    if port.is_empty() || bare_ipv6 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return (value, "");
    }
    (&value[..idx], &value[idx..])
}

fn scrub_host_token(host: &str, redactor: &Redactor) -> String {
    bump(&redactor.counters.hosts);
    format!(
        "<host:{}:{}>",
        redactor.hash8(host),
        host_class(host.trim_start_matches('[').trim_end_matches(']'))
    )
}

/// loopback / private / public——保留網段類別的診斷價值，不保留位址本身。
fn host_class(host: &str) -> &'static str {
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower == "::1" || lower.starts_with("127.") {
        return "loopback";
    }
    if let Some(octets) = ipv4_octets(&lower) {
        return match octets {
            [10, ..] => "private",
            [172, second, ..] if (16..=31).contains(&second) => "private",
            [192, 168, ..] => "private",
            [169, 254, ..] => "private",
            _ => "public",
        };
    }
    if lower.contains(':') {
        // IPv6：unique-local (fc00::/7) 與 link-local (fe80::/10)
        if lower.starts_with("fc") || lower.starts_with("fd") || lower.starts_with("fe8") {
            return "private";
        }
        return "public";
    }
    if !lower.contains('.')
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower.ends_with(".lan")
        || lower.ends_with(".home")
    {
        return "private";
    }
    "public"
}

fn ipv4_octets(host: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut parts = host.split('.');
    for slot in octets.iter_mut() {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse::<u8>().ok()?;
    }
    parts.next().is_none().then_some(octets)
}

// --- scrubber：URL -----------------------------------------------------------

/// scheme 的掃描上限（IANA 已註冊的最長 scheme 也遠短於此）。
const MAX_URL_SCHEME_CHARS: usize = 32;

/// `scheme://host[:port]` 的 authority 段。`file:///Users/…` 沒有 authority，
/// scheme 原樣輸出後由 scrub_paths 接手（`//` 之後的 `/` 是路徑起點，因此
/// `file://` 與裸路徑會得到同一個 hash）。
fn scrub_urls(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        let rest = &input[idx..];
        if let Some(scheme_len) = url_scheme_len(rest, char_before(input, idx)) {
            out.push_str(&rest[..scheme_len]);
            idx += scheme_len;
            // `file:///…` 沒有 authority——`//` 之後的 `/` 是路徑起點，交給 scrub_paths
            if input[idx..].starts_with('/') {
                continue;
            }
            // scrub_user_at_host 比這裡早跑，userinfo／host 這時可能已經是
            // `<user:…>@<host:…>`。原樣抄過這些 marker（不能整段跳過，否則後面的
            // 路徑段永遠輪不到 hash）。
            if let Some(len) = redacted_userinfo_len(&input[idx..]) {
                out.push_str(&input[idx..idx + len]);
                idx += len;
            }
            // host span 不含 `:port`，port 原樣留在後面（診斷價值）
            if let Some(len) = redaction_marker_len(&input[idx..], "<host:") {
                out.push_str(&input[idx..idx + len]);
                idx += len;
            } else {
                let Some(host_len) = host_span_len(&input[idx..]) else {
                    continue;
                };
                out.push_str(&scrub_host_token(&input[idx..idx + host_len], redactor));
                idx += host_len;
            }
            let tail = &input[idx..];
            let port_len = port_span_len(tail);
            out.push_str(&tail[..port_len]);
            idx += port_len;
            // URL 的路徑段（私有 repo 名之類）整段 hash——這裡沒有 basename 的診斷價值
            let tail = &input[idx..];
            if tail.starts_with('/') {
                let len = tail
                    .chars()
                    .take_while(|c| !is_path_terminator(*c))
                    .map(char::len_utf8)
                    .sum::<usize>();
                bump(&redactor.counters.paths);
                out.push_str(&format!("<path:{}>", redactor.hash8(&tail[..len])));
                idx += len;
            }
            continue;
        }
        let ch = rest.chars().next().unwrap_or('\u{0}');
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

/// 前一階段留下的 marker（`<host:…>`、`<user:…>`）長度。
fn redaction_marker_len(rest: &str, tag: &str) -> Option<usize> {
    if !rest.starts_with(tag) {
        return None;
    }
    rest.find('>').map(|pos| pos + 1)
}

/// 已被 scrub_user_at_host 換成 `<user:…>@` 的 userinfo 長度（含 `@`）。
fn redacted_userinfo_len(rest: &str) -> Option<usize> {
    let len = redaction_marker_len(rest, "<user:")?;
    rest[len..].starts_with('@').then_some(len + 1)
}

/// `scheme://` 的長度（含 `://`）。scheme 只認 ASCII 字母開頭的常見形式。
fn url_scheme_len(rest: &str, prev: Option<char>) -> Option<usize> {
    if !is_word_boundary(prev) {
        return None;
    }
    // `take(MAX_URL_SCHEME_CHARS)`：`.` 與 `+` 同時是 scheme 字元與 word boundary，
    // 沒有上限時 `a.a.a…` 這種輸入會讓每個位置都掃過整段 run（O(n²)）。
    let len = rest
        .chars()
        .take(MAX_URL_SCHEME_CHARS)
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        .map(char::len_utf8)
        .sum::<usize>();
    let scheme = rest.get(..len)?;
    if len == 0 || !scheme.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    rest[len..].starts_with("://").then_some(len + 3)
}

// --- scrubber：路徑起點 ------------------------------------------------------

/// 路徑起點前綴長度：verbatim UNC / verbatim drive / UNC / drive letter / POSIX，
/// 外加 home-relative 的 `~/`。
///
/// **`~/` 這一條是必要的，不是方便性**：`redact_line` 會先跑 `sanitize_line` 把
/// home 前綴換成 `~`，之後才進 scrubber。若這裡只認絕對路徑，家目錄底下的東西
/// 就會整個逸出——包括 `PATH_KEYS` 欄位（`workspace_path`／`cwd`／`command`…）
/// 因為守門條件失敗而**完全不進 scrubber**，以及訊息內文裡的 `~/Clients/Acme/…`。
/// 兩者都會原封留在標示為「已去識別化」的輸出中，而路徑遮蔽計數還回報 0。
///
/// 只認 `~/` 與 `~\`（長度 2），不認裸 `~`——後者是散文常見字元（`~5 分鐘`）。
fn path_start_prefix(rest: &str) -> Option<usize> {
    if rest.starts_with(r"\\?\UNC\") {
        return Some(8);
    }
    if rest.starts_with(r"\\?\") {
        return Some(4);
    }
    if rest.starts_with(r"\\") {
        return Some(2);
    }
    let bytes = rest.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        return Some(3);
    }
    if rest.starts_with("~/") || rest.starts_with(r"~\") {
        return Some(2);
    }
    if bytes.first() == Some(&b'/') {
        return Some(1);
    }
    None
}

fn is_path_terminator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\''
                | '`'
                | '|'
                | '*'
                | ','
                | ';'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '='
                | '?'
                // 已經產生的 marker（`<host:…>`）不併進路徑 span
                | '<'
                | '>'
        )
}

fn is_path_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\''
                        | '`'
                        | '='
                        // `file:///Users/…`：`//` 之後的 `/` 就是路徑起點
                        | '/'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | ','
                        | ';'
                        | '|'
                        | '<'
                        | '>'
                        | '*'
                )
        }
    }
}

fn scrub_paths(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    let mut prev: Option<char> = None;
    while idx < input.len() {
        let rest = &input[idx..];
        // `file:///Users/…`：`//` 中間那個 `/` 不是路徑起點，往後挪一格才會與裸路徑
        // 得到同一個 hash。
        let boundary = is_path_boundary(prev) && !(prev == Some('/') && rest.starts_with("//"));
        if let Some(prefix) = path_start_prefix(rest).filter(|_| boundary) {
            let mut end = prefix;
            while end < rest.len() {
                let ch = rest[end..].chars().next().unwrap_or('\u{0}');
                if is_path_terminator(ch) {
                    break;
                }
                end += ch.len_utf8();
            }
            // 句末標點不算路徑的一部分（`… /a/b/c.txt: not found`）
            while end > prefix && matches!(rest.as_bytes()[end - 1], b'.' | b':') {
                end -= 1;
            }
            if end > prefix || prefix >= 3 {
                let span = &rest[..end];
                out.push_str(&replace_path_span(span, redactor));
                prev = span.chars().next_back();
                idx += end;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or('\u{0}');
        out.push(ch);
        prev = Some(ch);
        idx += ch.len_utf8();
    }
    out
}

/// `PATH` 這種 `:` 分隔的多路徑逐段處理，否則整串只會得到一個 hash（S8）。
fn replace_path_span(span: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(span.len());
    let mut start = 0usize;
    for (idx, ch) in span.char_indices() {
        if ch != ':'
            || idx <= start
            || !span[idx + 1..].starts_with('/')
            || is_drive_colon(span, idx)
        {
            continue;
        }
        out.push_str(&replace_single_path(&span[start..idx], redactor));
        out.push(':');
        start = idx + 1;
    }
    out.push_str(&replace_single_path(&span[start..], redactor));
    out
}

/// `C:/…`、`\\?\C:/…` 的磁碟機冒號不是路徑分隔符。
fn is_drive_colon(span: &str, idx: usize) -> bool {
    let mut before = span[..idx].chars().rev();
    let Some(letter) = before.next() else {
        return false;
    };
    letter.is_ascii_alphabetic() && matches!(before.next(), None | Some('\\') | Some('/'))
}

/// 目錄段換 `<path:HASH8>`，basename 保留——可判斷是否同一路徑、看得到檔名。
fn replace_single_path(span: &str, redactor: &Redactor) -> String {
    bump(&redactor.counters.paths);
    let Some(pos) = span.rfind(['/', '\\']) else {
        return format!("<path:{}>", redactor.hash8(span));
    };
    let separator = span[pos..].chars().next().unwrap_or('/');
    let directory = if pos == 0 { &span[..1] } else { &span[..pos] };
    let basename = &span[pos + separator.len_utf8()..];
    if basename.is_empty() {
        return format!("<path:{}>", redactor.hash8(span));
    }
    format!("<path:{}>{separator}{basename}", redactor.hash8(directory))
}

// --- scrubber：裸 IP literal -------------------------------------------------

fn scrub_ip_literals(input: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(input.len());
    let mut idx = 0usize;
    let mut prev: Option<char> = None;
    while idx < input.len() {
        let rest = &input[idx..];
        if is_ip_boundary(prev) {
            if let Some(len) = ip_literal_span_len(rest) {
                let span = &rest[..len];
                out.push_str(&scrub_host_token(span, redactor));
                prev = span.chars().next_back();
                idx += len;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or('\u{0}');
        out.push(ch);
        prev = Some(ch);
        idx += ch.len_utf8();
    }
    out
}

fn is_ip_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => !(c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_')),
    }
}

/// IPv4 dotted-quad，或 IPv6 literal（含 `fd00::1`、`::1` 這種壓縮形式）。
fn ip_literal_span_len(rest: &str) -> Option<usize> {
    let v4_len = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .count();
    if v4_len > 0
        && ipv4_octets(&rest[..v4_len]).is_some()
        && !rest[v4_len..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
    {
        return Some(v4_len);
    }
    let v6_len = rest
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == ':')
        .count();
    if is_ipv6_literal(&rest[..v6_len])
        && !rest[v6_len..].starts_with(|c: char| c.is_ascii_alphanumeric())
    {
        return Some(v6_len);
    }
    None
}

/// 完整（8 組）或壓縮（單一 `::`）的 IPv6。RFC3339 時間戳是 3 組非空、沒有 `::`，
/// 因此不會被誤判。
fn is_ipv6_literal(candidate: &str) -> bool {
    if candidate.matches("::").count() > 1 || !candidate.contains(':') {
        return false;
    }
    let groups: Vec<&str> = candidate.split(':').collect();
    if groups.len() > 8 || groups.iter().any(|group| group.len() > 4) {
        return false;
    }
    let filled = groups.iter().filter(|group| !group.is_empty()).count();
    if candidate.contains("::") {
        (1..=7).contains(&filled)
    } else {
        groups.len() == 8 && filled == 8
    }
}

pub fn export_zip_with_summary(
    dir: &Path,
    dest: &Path,
    sanitize: bool,
) -> Result<LogExportResult, String> {
    let file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let redactor = sanitize.then(Redactor::new);
    // sanitize 模式本來就逐行走過每一個檔，順手把 run-summary 摺出來——不必為了
    // 摘要再讀第二遍。raw 模式維持 `io::copy` 的位元組原樣複製（不做 UTF-8 行
    // 重組，避免壞掉的檔案在匯出時反而失敗），因此仍需要第二遍串流讀取。
    // 兩條路徑的記憶體都是 O(run 數)。
    let mut summary_builder = sanitize.then(crate::run_summary::RunSummaryBuilder::new);

    for path in retained_log_files(dir) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "invalid log file name".to_string())?
            .to_string();
        archive
            .start_file(name, options)
            .map_err(|e| e.to_string())?;
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(file);
        if let Some(redactor) = redactor.as_ref() {
            let mut line = String::new();
            loop {
                line.clear();
                let bytes = reader.read_line(&mut line).map_err(|e| e.to_string())?;
                if bytes == 0 {
                    break;
                }
                let had_newline = line.ends_with('\n');
                if had_newline {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                // log 檔中間的空行不是 malformed record，直接略過（不計 unparseable）
                if line.trim().is_empty() {
                    continue;
                }
                // 摘要摺的是**去識別化之前**的原文：run_id 等欄位在這裡才是原值。
                if let Some(builder) = summary_builder.as_mut() {
                    builder.absorb_line(&line);
                }
                let redacted = redact_line(&line, redactor);
                archive
                    .write_all(redacted.as_bytes())
                    .map_err(|e| e.to_string())?;
                if had_newline {
                    archive.write_all(b"\n").map_err(|e| e.to_string())?;
                }
            }
        } else {
            std::io::copy(&mut reader, &mut archive).map_err(|e| e.to_string())?;
        }
    }

    // run-summary.json（#40 §3.6）：與 log 檔同一個 bundle、同一條 sanitize 出口。
    // 放在 log 檔之後寫，摘要因此涵蓋 bundle 內所有 daily file。
    // sanitize 模式的摘要已在上面那一遍摺好；raw 模式（io::copy）沒有逐行走過，
    // 才需要這第二遍串流讀取。
    let entries = match summary_builder {
        Some(builder) => builder.finish(),
        None => crate::run_summary::build_run_summary(dir),
    };
    let summary_line = crate::run_summary::render_run_summary(&entries);
    let summary_line = match redactor.as_ref() {
        Some(redactor) => redact_line(&summary_line, redactor),
        None => summary_line,
    };
    archive
        .start_file("run-summary.json", options)
        .map_err(|e| e.to_string())?;
    archive
        .write_all(summary_line.as_bytes())
        .map_err(|e| e.to_string())?;

    archive.finish().map_err(|e| e.to_string())?;
    Ok(LogExportResult {
        path: dest.to_string_lossy().to_string(),
        summary: redactor.map(|redactor| redactor.summary()),
    })
}

pub fn export_zip(dir: &Path, dest: &Path, sanitize: bool) -> Result<String, String> {
    export_zip_with_summary(dir, dest, sanitize).map(|result| result.path)
}

#[tauri::command]
pub async fn log_export(dest: String, sanitize: bool) -> Result<LogExportResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        export_zip_with_summary(&default_log_dir(), Path::new(&dest), sanitize)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Copy rows 走的 redaction 出口——與 export 共用 redact_line，語意一致。
#[tauri::command]
pub fn log_sanitize_lines(lines: Vec<String>) -> Result<Vec<String>, String> {
    let redactor = Redactor::new();
    Ok(lines
        .iter()
        .map(|line| redact_line(line, &redactor))
        .collect())
}

#[tauri::command]
pub fn log_event(event: LogEvent) -> Result<(), String> {
    // 值收斂：Logs pane 的篩選是固定清單，未知值永遠篩不出來 → 直接拒絕
    if !VALID_LEVELS.contains(&event.level.as_str()) {
        return Err(format!("invalid log level: {}", event.level));
    }
    if !VALID_KINDS.contains(&event.kind.as_str()) {
        return Err(format!("invalid log kind: {}", event.kind));
    }
    write_global(event);
    Ok(())
}

#[tauri::command]
pub fn get_log_level() -> String {
    read_log_level_from(&log_config_path())
}

#[tauri::command]
pub fn set_log_level(level: String) -> Result<(), String> {
    if !VALID_LEVELS.contains(&level.as_str()) {
        return Err(format!("invalid log level: {level}"));
    }
    // 持鎖序列化：寫檔 + 套用記憶體門檻在同一把鎖下完成，避免並發呼叫
    // 造成「持久化值」與「生效門檻」分歧。
    let mut sink = global_sink()
        .lock()
        .map_err(|_| "log sink unavailable".to_string())?;
    write_log_level_to(&log_config_path(), &level)?;
    sink.set_min_level(&level);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(msg: &str) -> LogEvent {
        LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "test".into(),
            workspace_path: None,
            event: "unit_test".into(),
            message: msg.into(),
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn write_appends_jsonl_line_with_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        sink.write(ev("hello"));
        sink.write(ev("world"));
        let files: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(files.len(), 1);
        let content = std::fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
        let lines: Vec<_> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["message"], "hello");
        assert!(parsed["timestamp"].as_str().unwrap().contains("T"));
    }

    #[test]
    fn cleanup_removes_files_older_than_retention() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("yuzora-2020-01-01.jsonl");
        std::fs::write(&old, "{}\n").unwrap();
        let sink = LogSink::new(tmp.path().to_path_buf());
        sink.cleanup();
        assert!(!old.exists());
    }

    #[test]
    fn query_filters_and_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        for (k, s, m) in [
            ("debug", "lsp", "a"),
            ("audit", "acp", "b"),
            ("debug", "acp", "c"),
        ] {
            sink.write(LogEvent {
                level: "info".into(),
                kind: k.into(),
                source: s.into(),
                workspace_path: None,
                event: "e".into(),
                message: m.into(),
                metadata: serde_json::json!({}),
            });
        }
        let f = LogQueryFilters {
            kinds: Some(vec!["debug".into()]),
            sources: Some(vec!["acp".into()]),
            since: None,
            until: None,
            levels: None,
            text: None,
            limit: Some(10),
        };
        let got = query_dir(tmp.path(), &f);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "c");
        assert!(got[0].timestamp.contains("T"));
    }

    #[test]
    fn sources_returns_distinct_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        for source in ["lsp", "acp", "lsp"] {
            sink.write(LogEvent {
                level: "info".into(),
                kind: "debug".into(),
                source: source.into(),
                workspace_path: None,
                event: "unit_test".into(),
                message: "source".into(),
                metadata: serde_json::json!({}),
            });
        }

        let got = sources_dir(tmp.path());

        assert_eq!(got, vec!["acp".to_string(), "lsp".to_string()]);
    }

    #[test]
    fn export_zip_contains_today_file_and_sanitizes_home_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("logs.zip");
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp/yuzora-home"));
        let home_text = home.to_string_lossy().to_string();
        let username = home
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("yuzora-user")
            .to_string();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        sink.write(LogEvent {
            level: "info".into(),
            kind: "debug".into(),
            source: "test".into(),
            workspace_path: Some(format!("{home_text}/workspace")),
            event: "unit_test".into(),
            message: format!("{home_text}/workspace owned by {username}"),
            metadata: serde_json::json!({}),
        });

        let exported = export_zip(tmp.path(), &dest, true).unwrap();

        assert_eq!(exported, dest.to_string_lossy());
        let file = std::fs::File::open(&dest).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d");
        let mut entry = archive.by_name(&format!("yuzora-{today}.jsonl")).unwrap();
        let mut content = String::new();
        use std::io::Read;
        entry.read_to_string(&mut content).unwrap();
        // home 前綴換成 `~` 之後**還要**進 path scrubber。原本這裡斷言
        // `content.contains("~/workspace")`，等於把「home-relative 路徑整段逸出」
        // 這個洩漏鎖成預期行為（P1）；現在改成斷言它確實被 hash 掉。
        assert!(
            !content.contains("~/workspace"),
            "home-relative 路徑未經 scrubber：{content}"
        );
        assert!(content.contains("<path:"), "應產生 path hash：{content}");
        assert!(content.contains(&format!("owned by {username}")));
        assert!(!content.contains(&home_text));
    }

    // --- #40 run correlation ------------------------------------------------

    fn records_in(dir: &Path) -> Vec<LogRecord> {
        let mut records = query_dir(dir, &LogQueryFilters::default());
        records.reverse(); // query_dir 回 newest-first；斷言用寫入順序比較好讀
        records
    }

    #[test]
    fn write_stamps_the_same_run_id_on_every_record_of_one_run() {
        // AC 第 1 條的前提。若 run_id 改成「每次寫入重新產生」，這裡會紅。
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        sink.write(ev("a"));
        sink.write(ev("b"));
        sink.write(ev("c"));

        let records = records_in(tmp.path());
        assert_eq!(records.len(), 3);
        let run_id = records[0].run_id.clone().expect("run_id 必須被寫入端補上");
        assert!(!run_id.is_empty());
        assert!(
            records
                .iter()
                .all(|record| record.run_id.as_deref() == Some(run_id.as_str())),
            "同一次 run 的每一筆 record 必須共用同一個 run_id：{records:?}"
        );
        assert_eq!(run_id, crate::run_context::current_run_id());
    }

    #[test]
    fn one_daily_file_splits_into_multiple_runs_by_run_id() {
        // AC 第 1 條：兩次 LogSink 生命週期寫進同一個 daily file，讀回後可依
        // run_id 無歧義分組。若 run_id 被寫成常數，這裡會塌成一組而變紅。
        let tmp = tempfile::tempdir().unwrap();
        let mut first = LogSink::with_run_id(tmp.path().to_path_buf(), "20260725T010000Z-aaaaaaaa");
        first.write(ev("run one first"));
        first.write(ev("run one second"));
        drop(first);

        let mut second =
            LogSink::with_run_id(tmp.path().to_path_buf(), "20260725T020000Z-bbbbbbbb");
        second.write(ev("run two first"));
        drop(second);

        let records = records_in(tmp.path());
        assert_eq!(records.len(), 3, "三筆必須全落在同一個 daily file");
        let files: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(files.len(), 1, "測試前提：只有一個 daily file");

        let mut grouped: std::collections::BTreeMap<String, Vec<&LogRecord>> =
            std::collections::BTreeMap::new();
        for record in &records {
            grouped
                .entry(record.run_id.clone().expect("run_id"))
                .or_default()
                .push(record);
        }
        assert_eq!(grouped.len(), 2, "必須恰好分成兩個 run");
        assert_eq!(grouped["20260725T010000Z-aaaaaaaa"].len(), 2);
        assert_eq!(grouped["20260725T020000Z-bbbbbbbb"].len(), 1);
    }

    #[test]
    fn agent_ids_reused_across_runs_stay_distinguishable_by_run_id() {
        // AC 第 3 條：restart 後 agent id 會重用（同一個 `agent-1`），加上 run_id
        // 之後 `run_id + local id` 才是唯一識別。
        let tmp = tempfile::tempdir().unwrap();
        let agent_event = |run: &str| LogEvent {
            source: "acp".into(),
            event: "acp_spawn".into(),
            metadata: serde_json::json!({ "id": "agent-1" }),
            ..ev(run)
        };
        let mut first = LogSink::with_run_id(tmp.path().to_path_buf(), "20260725T010000Z-aaaaaaaa");
        first.write(agent_event("first run"));
        drop(first);
        let mut second =
            LogSink::with_run_id(tmp.path().to_path_buf(), "20260725T020000Z-bbbbbbbb");
        second.write(agent_event("second run"));
        drop(second);

        let records = records_in(tmp.path());
        let local_ids: Vec<_> = records
            .iter()
            .map(|record| record.metadata.get("id").and_then(|v| v.as_str()).unwrap())
            .collect();
        assert_eq!(local_ids, vec!["agent-1", "agent-1"], "local id 確實重用");
        let global_ids: std::collections::BTreeSet<String> = records
            .iter()
            .map(|record| format!("{}/agent-1", record.run_id.clone().unwrap()))
            .collect();
        assert_eq!(global_ids.len(), 2, "run_id + local id 必須是唯一的");
    }

    #[test]
    fn pre_issue_40_records_without_run_id_still_parse() {
        // 向後相容：run_id 若宣告成必填 String，歷史資料會整批消失在 Logs pane。
        let tmp = tempfile::tempdir().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d");
        let legacy = serde_json::json!({
            "timestamp": "2026-07-01T00:00:00+00:00",
            "level": "info",
            "kind": "debug",
            "source": "test",
            "workspace_path": null,
            "event": "legacy",
            "message": "written before issue #40",
            "metadata": {}
        });
        std::fs::write(
            tmp.path().join(format!("yuzora-{today}.jsonl")),
            format!("{legacy}\n"),
        )
        .unwrap();

        let records = records_in(tmp.path());
        assert_eq!(records.len(), 1, "沒有 run_id 的歷史 record 必須仍讀得回來");
        assert_eq!(records[0].run_id, None);
        assert_eq!(records[0].event, "legacy");
    }

    #[test]
    fn app_start_event_carries_run_identity_and_previous_run_verdict() {
        let context = crate::run_context::RunContext {
            run_id: "20260725T010000Z-aaaaaaaa".into(),
            app_version: "9.9.9".into(),
            host_pid: 4242,
            platform: "windows".into(),
        };
        let event = app_start_event(&context, true);
        assert_eq!(event.kind, APP_LIFECYCLE_KIND);
        assert_eq!(event.event, APP_START_EVENT);
        assert_eq!(event.level, "info", "必須在預設門檻（info）下落盤");
        assert_eq!(event.metadata["app_version"], "9.9.9");
        assert_eq!(event.metadata["host_pid"], 4242);
        assert_eq!(event.metadata["platform"], "windows");
        assert_eq!(event.metadata["previous_run_unclean"], true);

        let clean = app_start_event(&context, false);
        assert_eq!(clean.metadata["previous_run_unclean"], false);
    }

    #[test]
    fn graceful_exit_event_is_a_loggable_app_lifecycle_record() {
        let context = crate::run_context::RunContext {
            run_id: "20260725T010000Z-aaaaaaaa".into(),
            app_version: "9.9.9".into(),
            host_pid: 4242,
            platform: "macos".into(),
        };
        let event = graceful_exit_event(&context);
        assert_eq!(event.kind, APP_LIFECYCLE_KIND);
        assert_eq!(event.event, GRACEFUL_EXIT_EVENT);
        assert_eq!(event.level, "info");
        assert!(event.message.contains("20260725T010000Z-aaaaaaaa"));
        // log_event 的值收斂必須認得這個 kind，否則前端／未來呼叫端會被拒。
        assert!(VALID_KINDS.contains(&event.kind.as_str()));
    }

    #[test]
    fn run_id_survives_export_redaction_unchanged() {
        // #41 的 redactor 必須把 run_id 視為非敏感：它是隨機碼，不含環境資訊。
        let run_id = crate::run_context::current_run_id();
        let line = serde_json::json!({
            "timestamp": "2026-07-25T01:00:00+00:00",
            "run_id": run_id,
            "level": "info",
            "kind": "debug",
            "source": "test",
            "workspace_path": null,
            "event": "unit_test",
            "message": "hello",
            "metadata": {}
        })
        .to_string();
        let redacted = redact_line(&line, &redactor());
        let value: serde_json::Value = serde_json::from_str(&redacted).unwrap();
        assert_eq!(
            value["run_id"].as_str(),
            Some(run_id),
            "run_id 必須原值保留，否則匯出後就無法把現象對回某一次 run"
        );
    }

    #[test]
    fn export_bundle_contains_a_run_summary_for_every_run_in_the_logs() {
        // AC §3.6：run-summary.json 必須真的掛在既有的匯出流程上。
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("logs.zip");
        let mut first = LogSink::with_run_id(tmp.path().to_path_buf(), "20260725T010000Z-aaaaaaaa");
        first.write(ev("run one"));
        drop(first);
        let mut second =
            LogSink::with_run_id(tmp.path().to_path_buf(), "20260725T020000Z-bbbbbbbb");
        second.write(ev("run two"));
        drop(second);

        for sanitize in [false, true] {
            export_zip(tmp.path(), &dest, sanitize).unwrap();
            let file = std::fs::File::open(&dest).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let mut entry = archive.by_name("run-summary.json").unwrap_or_else(|_| {
                panic!("bundle 必須含 run-summary.json（sanitize={sanitize}）")
            });
            let mut content = String::new();
            use std::io::Read;
            entry.read_to_string(&mut content).unwrap();
            let entries: Vec<crate::run_summary::RunSummaryEntry> =
                serde_json::from_str(&content).unwrap();
            let ids: Vec<&str> = entries.iter().map(|e| e.run_id.as_str()).collect();
            assert_eq!(
                ids,
                vec!["20260725T010000Z-aaaaaaaa", "20260725T020000Z-bbbbbbbb"],
                "sanitize={sanitize} 時 run_id 必須原值保留"
            );
            assert!(entries.iter().all(|e| e.record_count == 1));
        }
    }

    #[test]
    fn export_bundle_run_summary_is_identical_for_raw_and_sanitized_paths() {
        // sanitize 路徑在寫 zip 的同一遍就把摘要摺出來（不重讀檔案），raw 路徑
        // 走第二遍串流。兩者必須產生完全相同的摘要，否則單次讀取的最佳化就改變
        // 了行為。
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = LogSink::with_run_id(tmp.path().to_path_buf(), "20260725T010000Z-aaaaaaaa");
        sink.write(ev("one"));
        sink.write(ev("two"));
        drop(sink);
        let mut second =
            LogSink::with_run_id(tmp.path().to_path_buf(), "20260725T020000Z-bbbbbbbb");
        second.write(ev("three"));
        drop(second);

        let read_summary = |sanitize: bool| {
            let dest = tmp.path().join(format!("logs-{sanitize}.zip"));
            export_zip(tmp.path(), &dest, sanitize).unwrap();
            let file = std::fs::File::open(&dest).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let mut entry = archive.by_name("run-summary.json").unwrap();
            let mut content = String::new();
            use std::io::Read;
            entry.read_to_string(&mut content).unwrap();
            serde_json::from_str::<Vec<crate::run_summary::RunSummaryEntry>>(&content).unwrap()
        };

        let raw = read_summary(false);
        let sanitized = read_summary(true);
        assert_eq!(raw, sanitized);
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0].record_count, 2);
        assert_eq!(raw[1].record_count, 1);
    }

    #[test]
    fn graceful_exit_record_does_not_by_itself_mark_the_run_clean() {
        // #40：「有走到 exit handler」與「有乾淨地結束」是兩件事。若 record 一併
        // 標記 marker，關閉時卡住再被強制結束就會誤判為 clean——那正是 AppHangB1
        // 最需要 previous_run_unclean 的情境。
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("run-state.json");
        crate::run_context::write_run_state(&path, "run-a", false).unwrap();

        // graceful_exit 的 log record 不碰 marker 檔。
        let context = crate::run_context::RunContext {
            run_id: "run-a".into(),
            app_version: "9.9.9".into(),
            host_pid: 1,
            platform: "macos".into(),
        };
        let _ = graceful_exit_event(&context);
        assert!(
            crate::run_context::previous_run_unclean(&path),
            "只寫 graceful_exit record 不得把 marker 翻成 clean"
        );

        // 真正翻轉的是清理跑完之後的那一步。
        crate::run_context::write_run_state(&path, "run-a", true).unwrap();
        assert!(!crate::run_context::previous_run_unclean(&path));
    }

    #[test]
    fn export_zip_returns_error_for_unwritable_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("missing").join("logs.zip");

        let result = export_zip(tmp.path(), &dest, false);

        assert!(result.is_err());
    }

    #[test]
    fn sanitize_line_masks_username_only_in_user_path_segments() {
        let line = r#"{"message":"theme me metadata","workspace_path":"/Users/me/project","windows":"C:\\Users\\me\\project","metadata":{"theme":"me"}}"#;

        let sanitized = sanitize_line(line, None, Some("me"));

        assert!(sanitized.contains(r#""message":"theme me metadata""#));
        assert!(sanitized.contains(r#""metadata":{"theme":"me"}"#));
        assert!(sanitized.contains(r#""workspace_path":"/Users/<user>/project""#));
        assert!(sanitized.contains(r#""windows":"C:\\Users\\<user>\\project""#));
    }

    /// 固定 salt——HASH8 才有確定值可斷言。
    fn redactor() -> Redactor {
        Redactor::with_salt(b"yuzora-issue-41-fixed-salt")
    }

    fn redacted_message(message: &str, redactor: &Redactor) -> String {
        let line = serde_json::json!({ "message": message }).to_string();
        let value: serde_json::Value = serde_json::from_str(&redact_line(&line, redactor)).unwrap();
        value["message"].as_str().unwrap().to_string()
    }

    /// AC 1：任意 Windows drive / UNC / verbatim / macOS / Linux 絕對路徑都去識別化，
    /// 且一律保留 basename。
    #[test]
    fn redact_line_scrubs_absolute_paths_on_every_platform_shape() {
        let redactor = redactor();
        for (input, secret, basename) in [
            (r"opened D:\Work\proj\main.rs", r"D:\Work\proj", "main.rs"),
            (
                r"unc \\fileserver\share\logs\a.txt",
                r"\\fileserver\share\logs",
                "a.txt",
            ),
            (
                r"verbatim \\?\C:\Users\alice\ws\x.txt",
                r"\\?\C:\Users\alice\ws",
                "x.txt",
            ),
            (
                r"verbatim unc \\?\UNC\fileserver\share\y.txt",
                r"\\?\UNC\fileserver\share",
                "y.txt",
            ),
            (
                "mac /Users/alice/project/app.ts",
                "/Users/alice/project",
                "app.ts",
            ),
            ("linux /home/bob/src/lib.rs", "/home/bob/src", "lib.rs"),
            (
                "slash drive E:/build/out/app.exe",
                "E:/build/out",
                "app.exe",
            ),
        ] {
            let got = redacted_message(input, &redactor);
            assert!(
                !got.contains(secret),
                "目錄段仍外洩：{input} → {got}（不應含 {secret}）"
            );
            assert!(got.contains(basename), "basename 應保留：{input} → {got}");
            assert!(got.contains("<path:"), "應標記為 path：{input} → {got}");
        }
    }

    /// AC 1／AC 2：路徑 key 的更強規則——含空白的絕對路徑整段當一個 path 處理。
    #[test]
    fn redact_line_treats_path_keys_as_whole_value_paths() {
        let redactor = redactor();
        let line = r#"{"workspace_path":"/Users/alice/My Documents/ws","metadata":{"cwd":"/Users/alice/My Documents/ws"}}"#;

        let got = redact_line(line, &redactor);
        let value: serde_json::Value = serde_json::from_str(&got).unwrap();

        assert!(!got.contains("alice"), "使用者目錄仍外洩：{got}");
        assert!(!got.contains("My Documents"), "目錄段仍外洩：{got}");
        // 同一路徑在同一份 bundle 內 hash 相同 → 可關聯
        assert_eq!(value["workspace_path"], value["metadata"]["cwd"]);
        assert!(value["workspace_path"].as_str().unwrap().contains("/ws"));
    }

    /// AC 2：SSH/DB address、username、host/IP、fingerprint 遮罩但保留可關聯 hash。
    #[test]
    fn redact_line_masks_endpoints_but_keeps_correlatable_hashes() {
        let redactor = redactor();
        let line = r#"{"message":"ssh alice@10.0.0.5:22 fingerprint SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCD","metadata":{"host":"db.internal","user":"alice","fingerprint":"MD5:aa:bb:cc:dd:ee:ff","address":"10.0.0.5"}}"#;

        let got = redact_line(line, &redactor);
        let value: serde_json::Value = serde_json::from_str(&got).unwrap();

        for secret in [
            "alice",
            "10.0.0.5",
            "db.internal",
            "abcdefghijklmnopqrstuvwxyz",
            "aa:bb:cc:dd:ee:ff",
        ] {
            assert!(!got.contains(secret), "{secret} 仍外洩：{got}");
        }
        assert!(value["metadata"]["host"]
            .as_str()
            .unwrap()
            .ends_with(":private>"));
        assert!(value["metadata"]["user"]
            .as_str()
            .unwrap()
            .starts_with("<user:"));
        assert!(value["metadata"]["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("<fp:"));
        // port 是診斷價值，保留
        assert!(got.contains(":22 "), "port 應保留：{got}");
    }

    /// AC 2：同一輸入兩次 → 同 hash；不同輸入 → 不同 hash；不同 salt → 不可跨 bundle 關聯。
    #[test]
    fn redaction_hashes_are_deterministic_per_salt_and_unique_per_value() {
        let redactor = redactor();
        let host = |name: &str| {
            redact_line(
                &serde_json::json!({ "metadata": { "host": name } }).to_string(),
                &redactor,
            )
        };

        assert_eq!(host("db.internal"), host("db.internal"));
        assert_ne!(host("db.internal"), host("db2.internal"));

        let rotated = Redactor::with_salt(b"a-different-export-salt");
        assert_ne!(
            host("db.internal"),
            redact_line(
                &serde_json::json!({ "metadata": { "host": "db.internal" } }).to_string(),
                &rotated,
            )
        );
        // HASH8 = 8 個 hex
        let value: serde_json::Value = serde_json::from_str(&host("db.internal")).unwrap();
        let marked = value["metadata"]["host"].as_str().unwrap().to_string();
        let digest = marked
            .trim_start_matches("<host:")
            .split(':')
            .next()
            .unwrap();
        assert_eq!(digest.len(), 8);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// AC 2：host class 保留網段類別，不保留位址。一律走 redact_line，確保偵測層
    /// 真的把這些形狀認出來（含帶 port 與壓縮型 IPv6）。
    #[test]
    fn redact_line_labels_host_ranges_through_the_detection_layer() {
        let redactor = redactor();
        for (host, class) in [
            ("127.0.0.1", "loopback"),
            ("localhost", "loopback"),
            ("::1", "loopback"),
            ("10.1.2.3", "private"),
            ("172.16.0.9", "private"),
            ("172.32.0.9", "public"),
            ("192.168.1.5", "private"),
            ("fd00::1", "private"),
            ("fe80::abcd", "private"),
            ("db.internal", "private"),
            ("example.com", "public"),
            ("8.8.8.8", "public"),
            // 帶 port 的 DB/SSH address 是最常見形狀，class 不能被 port 帶偏
            ("192.168.1.9:5432", "private"),
            ("db.internal:5432", "private"),
        ] {
            let line = serde_json::json!({ "metadata": { "host": host } }).to_string();
            let got = redact_line(&line, &redactor);
            let value: serde_json::Value = serde_json::from_str(&got).unwrap();
            let marked = value["metadata"]["host"].as_str().unwrap();
            let (bare, port) = split_host_port(host);
            assert!(
                marked.starts_with("<host:") && marked.contains(&format!(":{class}>")),
                "host key {host} 應標成 {class}：{marked}"
            );
            assert!(!marked.contains(bare), "位址仍外洩：{marked}");
            assert!(marked.ends_with(port), "port 應保留：{marked}");
        }

        // 訊息內的裸 IP literal 同樣要被偵測層抓到（含壓縮型 IPv6）
        for (message, class) in [
            ("connect to 10.1.2.3 refused", "private"),
            ("listen on ::1 ready", "loopback"),
            ("peer fe80::abcd unreachable", "private"),
            ("upstream 8.8.8.8 timeout", "public"),
        ] {
            let got = redacted_message(message, &redactor);
            assert!(
                got.contains(&format!(":{class}>")),
                "message 內的 IP 未被偵測：{message} → {got}"
            );
        }
    }

    /// AC 3：token、Authorization、password、private key material 一律不出現，且不留 hash。
    #[test]
    fn redact_line_removes_secret_material_without_leaving_a_hash() {
        let redactor = redactor();
        let line = r#"{"message":"Authorization: Bearer eyJhbGciOiJIUzI1NiJ9 token=abc123XYZdef456 password=hunter2 api_key='k-9Zx' -----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----","metadata":{"password":"hunter2","token":"abc123XYZdef456"}}"#;

        let got = redact_line(line, &redactor);

        for secret in [
            "eyJhbGciOiJIUzI1NiJ9",
            "abc123XYZdef456",
            "hunter2",
            "k-9Zx",
            "b3BlbnNzaC1rZXktdjEAAAAA",
            "PRIVATE KEY",
        ] {
            assert!(!got.contains(secret), "{secret} 仍外洩：{got}");
        }
        assert!(
            got.contains("<redacted>"),
            "秘密應被 <redacted> 取代：{got}"
        );
    }

    /// AC 3：常見服務的 token 形狀（全小寫、含 `/` 的 base64、JWT、前綴型）都要被吃掉。
    #[test]
    fn redact_line_removes_common_credential_token_shapes() {
        let redactor = redactor();
        // 這些全是合成的假憑證，但形狀夠真，secret scanner（GitHub push protection
        // 與 GitGuardian）會把原始碼裡的連續字面值當成外洩憑證而擋下 push／讓 check
        // 變紅。`concat!` 在編譯期組回完全相同的字串，因此**測試行為與斷言一字未變**，
        // 只是原始碼裡不再出現連續的 token 形狀。
        //
        // 刻意不使用 scanner 的 unblock／allowlist：那會把這些字串永久加進 repo 的
        // 白名單，等於為了測試 fixture 而降低往後對真實外洩的偵測力。
        for secret in [
            concat!("xoxb", "-2345678901-2345678901-abcdefghijklmnopqrstuvwx"),
            concat!("ghp", "_abcdefghijklmnopqrstuvwxyz012345"),
            concat!("eyJhbGciOiJIUzI1NiJ9", ".eyJ1c2VyIjoiYWxpY2UifQ.sig"),
            concat!("AKIA", "IOSFODNN7EXAMPLE"),
            "ab/cdEF12ghIJ34klMN56opQR78stUV90wx+/abcdef=",
        ] {
            let got = redacted_message(&format!("adapter stderr: {secret} rejected"), &redactor);
            assert!(!got.contains(secret), "{secret} 仍外洩：{got}");
            assert!(got.contains("<redacted>"), "{secret} 應被移除：{got}");
        }

        // 專案實際使用的憑證變數名——key 名比對必須是包含式。值刻意挑「看不出是憑證」
        // 的形狀（無已知前綴、非高熵），只有 key 名這條規則能救。
        let embedded = redacted_message(
            r#"env {"GEOSENSE_API_KEY":"geosense-live-abcdefghij"}"#,
            &redactor,
        );
        assert!(
            !embedded.contains("geosense-live"),
            "GEOSENSE_API_KEY 值仍外洩：{embedded}"
        );

        let keyed = redact_line(
            r#"{"metadata":{"GEOSENSE_API_KEY":"geosense-live-abcdefghij","YUUZU_API_TOKEN":"tok-live-abcdefghij"}}"#,
            &redactor,
        );
        assert!(
            !keyed.contains("geosense-live"),
            "metadata key 未命中：{keyed}"
        );
        assert!(!keyed.contains("tok-live"), "metadata key 未命中：{keyed}");
    }

    /// B3-R／AC 3：**不含 `+`／`/`** 的 padded base64——標準 base64 約 1/3 的機率長
    /// 這個樣子，含 `+/` 的樣本會矇混過關，所以這裡刻意只挑不含的。
    #[test]
    fn redact_line_removes_padded_base64_credentials_without_plus_or_slash() {
        let redactor = redactor();
        // base64("alice:supersecretpassword1234567890")
        let basic = "YWxpY2U6c3VwZXJzZWNyZXRwYXNzd29yZDEyMzQ1Njc4OTA=";
        // Azure Storage 的 AccountKey 形狀（64 bytes → 88 字元）
        let account_key =
            "AgkQFx4lLDM6QUhPVl1ka3J5gIeOlZyjqrG4v8bN1Nvi6fD3AwoRGB8mLTQ7QklQV15lbHN6gYiPlp2kq7K5wA==";
        for sample in [basic, account_key] {
            assert!(
                !sample.contains(['+', '/']) && sample.ends_with('='),
                "樣本必須是不含 +／的 padded base64，否則守不到這個形狀：{sample}"
            );
        }

        // `Authorization: Basic <blob>`：spec §3.2 明列「Authorization: 後值」，
        // 只遮掉 `Basic` 這個字等於原封不動放走可 base64 解回明文的憑證。
        let auth = redacted_message(&format!("Authorization: Basic {basic}"), &redactor);
        assert!(!auth.contains(basic), "Basic 憑證仍外洩：{auth}");
        assert!(
            auth.contains("Basic <redacted>"),
            "scheme 應保留、憑證應被遮：{auth}"
        );

        // driver 錯誤原文帶出來的 Azure 連線字串（mask_url_userinfo 不認 `AccountKey=`）
        let conn = redacted_message(
            &format!(
                "connect failed: DefaultEndpointsProtocol=https;AccountName=acmestore;AccountKey={account_key};EndpointSuffix=core.windows.net"
            ),
            &redactor,
        );
        assert!(!conn.contains(account_key), "AccountKey 仍外洩：{conn}");
        assert!(
            conn.contains("AccountKey=<redacted>"),
            "AccountKey 應被遮：{conn}"
        );

        // 裸的 blob（沒有 key 名可依靠）也要被熵規則吃掉
        let bare = redacted_message(&format!("adapter stderr: {basic} rejected"), &redactor);
        assert!(!bare.contains(basic), "裸 base64 仍外洩：{bare}");
    }

    /// S-new-1：`firstToken`／`firstTokenOnPath` 是診斷 exit 127 的一組欄位，
    /// key 名含 `token` 但值不是憑證——被遮掉這一組就完全失去意義。
    #[test]
    fn redact_line_keeps_first_token_diagnostic_field() {
        let redactor = redactor();

        let got = redact_line(
            r#"{"message":"spawn failed","metadata":{"id":"pi","firstToken":"bunx","firstTokenOnPath":false}}"#,
            &redactor,
        );
        let value: serde_json::Value = serde_json::from_str(&got).unwrap();

        assert_eq!(value["metadata"]["firstToken"], "bunx");
        assert_eq!(value["metadata"]["firstTokenOnPath"], false);
        // 真正的憑證 key 不受影響
        let secret = redact_line(
            r#"{"metadata":{"accessToken":"abcdefghij0123456789"}}"#,
            &redactor,
        );
        assert!(secret.contains("<redacted>"), "憑證 key 反被放行：{secret}");
    }

    /// S7-R：32 位小寫 hex（Twilio auth token／Mailchimp key／MD5-based session id）
    /// 不在 git SHA 白名單內；縮寫與完整 SHA 才保留。
    #[test]
    fn redact_line_redacts_hex_credentials_outside_the_git_sha_shape() {
        let redactor = redactor();

        // `-us1` 後綴與前面的 32 位 hex 連在一起就是 Mailchimp key 的形狀，會被
        // GitHub push protection 擋下；`concat!` 編譯期組回同一個字串，行為不變。
        let got = redacted_message(
            concat!(
                "twilio 8f14e45fceea167a5a36dedd4bea2543 ",
                "mailchimp abcdef0123456789abcdef0123456789",
                "-us1"
            ),
            &redactor,
        );

        assert!(
            !got.contains("8f14e45fceea167a5a36dedd4bea2543"),
            "32 位 hex token 仍外洩：{got}"
        );
        assert!(
            !got.contains("abcdef0123456789abcdef0123456789"),
            "Mailchimp key 仍外洩：{got}"
        );
        // 白名單範圍（縮寫 7–12 位、完整 40 位）照舊保留
        let shas = redacted_message(
            "checkout a1b2c3d 3f2a1b4c5d6e 3f2a1b4c5d6e7f8091a2b3c4d5e6f708192a3b4c",
            &redactor,
        );
        assert_eq!(
            shas,
            "checkout a1b2c3d 3f2a1b4c5d6e 3f2a1b4c5d6e7f8091a2b3c4d5e6f708192a3b4c"
        );
    }

    /// S3-R：ssh／scp 形式的 git remote（私有 repo 最常見的寫法）路徑段也要 hash。
    #[test]
    fn redact_line_scrubs_repo_paths_in_urls_with_userinfo() {
        let redactor = redactor();

        for remote in [
            "ssh://git@gitlab.internal.acme:22/platform/billing-secrets.git",
            "git@gitlab.internal.acme:platform/billing-secrets.git",
        ] {
            let got = redacted_message(&format!("git remote add origin {remote}"), &redactor);
            assert!(!got.contains("billing-secrets"), "repo 路徑仍外洩：{got}");
            assert!(!got.contains("acme"), "host 仍外洩：{got}");
            assert!(got.contains("<path:"), "路徑段應 hash：{got}");
        }

        // `git@host: Permission denied` 的散文冒號不是路徑
        let prose = redacted_message("git@gitlab.internal.acme: Permission denied", &redactor);
        assert!(
            prose.ends_with(": Permission denied"),
            "散文冒號被誤判成路徑：{prose}"
        );
    }

    /// S7：git commit SHA 是 support bundle 對回版本的依據，白名單保留；
    /// 但掛在 secret key 底下的同樣字串仍必須移除。
    #[test]
    fn redact_line_keeps_git_commit_shas_but_not_secret_values() {
        let redactor = redactor();
        let sha = "3f2a1b4c5d6e7f8091a2b3c4d5e6f708192a3b4c";
        let line = format!(
            r#"{{"message":"checkout {sha} (a1b2c3d)","metadata":{{"commit":"{sha}","token":"{sha}"}}}}"#
        );

        let got = redact_line(&line, &redactor);
        let value: serde_json::Value = serde_json::from_str(&got).unwrap();

        assert!(got.contains(sha), "commit SHA 應保留：{got}");
        assert!(got.contains("a1b2c3d"), "short SHA 應保留：{got}");
        assert_eq!(value["metadata"]["commit"], sha);
        assert_eq!(value["metadata"]["token"], "<redacted>");
    }

    /// S-new-2：`.` 同時是識別字／scheme 字元與 word boundary，沒有 run 上限時
    /// `secret_assignment_span` 與 `url_scheme_len` 會在每個位置重掃整段（O(n²)）。
    /// `log_event` 對前端送來的 message 沒有長度上限，這條必須是線性的。
    #[test]
    fn redact_line_stays_linear_on_long_separator_runs() {
        let redactor = redactor();
        let started = std::time::Instant::now();

        let got = redacted_message(&".".repeat(20_000), &redactor);

        let elapsed = started.elapsed();
        assert_eq!(got.len(), 20_000);
        // 修正前：20 000 個 `.` 約 21 秒（debug）；修正後 O(n)，門檻留足機器差異
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "20 000 個 `.` 花了 {elapsed:?}，退化成 O(n²)"
        );
    }

    /// S9：`:` 也是散文的標點，不能把 commit message 的內容當成憑證吃掉。
    #[test]
    fn redact_line_keeps_prose_after_a_colon_intact() {
        let redactor = redactor();

        let got = redacted_message("merge feat/secret: hotfix for prod outage", &redactor);

        assert_eq!(got, "merge feat/secret: hotfix for prod outage");
    }

    /// P1：明確的憑證欄位不能因為「值短且全小寫」而被放走。修正前 `password: hunter`
    /// 的 key 已被認出是憑證欄位，卻被值形狀啟發式否決，原文留在標示為「已去識別化」
    /// 的輸出裡。
    #[test]
    fn redact_line_redacts_explicit_credential_fields_whatever_the_value_looks_like() {
        let redactor = redactor();

        for (line, secret) in [
            ("db login password: hunter", "hunter"),
            ("connect passwd: swordfish", "swordfish"),
            ("header api_key: abcdef", "abcdef"),
            ("cfg credential: letmein", "letmein"),
            ("pwd: opensesame", "opensesame"),
        ] {
            let got = redacted_message(line, &redactor);
            assert!(!got.contains(secret), "{secret} 仍外洩：{got}");
            assert!(got.contains("<redacted>"), "{line} 應被遮蔽：{got}");
        }
    }

    /// 上一條的反向護欄：usage 統計這類純數字值，任何欄位名都不能遮掉，否則
    /// `tokens: 1523` 這種診斷資訊會消失。
    #[test]
    fn redact_line_keeps_numeric_counters_after_a_colon() {
        let redactor = redactor();

        let got = redacted_message("usage tokens: 1523 total_tokens: 4096", &redactor);

        assert_eq!(got, "usage tokens: 1523 total_tokens: 4096");
    }

    /// P1：`redact_line` 會先把 home 前綴換成 `~` 才進 scrubber。修正前 scrubber
    /// 只認絕對路徑，家目錄底下的目錄名（專案／客戶代號）因此原封輸出，而且路徑
    /// 遮蔽計數還回報 0。
    #[test]
    fn redact_line_scrubs_home_relative_paths_in_messages() {
        let redactor = redactor();

        let got = redacted_message(
            "opened ~/Clients/AcmeCorp/secret-project/main.rs",
            &redactor,
        );

        assert!(!got.contains("AcmeCorp"), "客戶目錄名仍外洩：{got}");
        assert!(!got.contains("secret-project"), "專案目錄名仍外洩：{got}");
        assert!(got.contains("<path:"), "應產生 path hash：{got}");
        // 與絕對路徑一致：basename 保留（診斷需要檔名），目錄段才 hash。
        assert!(got.contains("main.rs"), "檔名不該被吃掉：{got}");
        assert!(
            redactor.counters.paths.get() > 0,
            "路徑遮蔽計數必須反映這次遮蔽"
        );
    }

    /// 同一個洞的欄位版：`PATH_KEYS` 的守門條件是「值看起來像路徑起點」，`~/` 不被
    /// 認得時整個欄位**完全不進 scrubber**。#40 之後 `workspace_path` 每筆 user
    /// action 都帶真實值，這條路徑因此變成日常會踩到的。
    #[test]
    fn redact_line_scrubs_home_relative_path_fields() {
        let redactor = redactor();
        let line = r#"{"timestamp":"t","level":"info","kind":"user_action","source":"ui","workspace_path":"~/Clients/AcmeCorp/secret-project","event":"e","message":"m","metadata":{}}"#;

        let got = redact_line(line, &redactor);

        assert!(!got.contains("AcmeCorp"), "中間目錄名仍外洩：{got}");
        assert!(got.contains("<path:"), "應產生 path hash：{got}");
        // basename 仍保留，與絕對路徑的既有行為一致（`<path:hash>/basename`，見
        // `replace_single_path`；UI 的說明也是「保留檔名」）。對 `workspace_path`
        // 這種值本身就是目錄的欄位，basename 等於專案名——**要不要連它一起遮是
        // 獨立的產品決策**，不在本次修正範圍內，這裡明確釘住現況以免日後誤以為
        // 已經處理過。
        assert!(
            got.contains("/secret-project"),
            "basename 的現況應維持不變：{got}"
        );
    }

    /// 裸 `~` 是散文字元（`~5 分鐘`），不能被當成路徑起點。
    #[test]
    fn redact_line_leaves_a_bare_tilde_alone() {
        let redactor = redactor();

        let got = redacted_message("retry in ~5 minutes ~ done", &redactor);

        assert_eq!(got, "retry in ~5 minutes ~ done");
    }

    /// AC 4：JSON escaped Windows path 與 nested metadata（含陣列）都要覆蓋。
    #[test]
    fn redact_line_walks_json_escaped_paths_and_nested_metadata() {
        let redactor = redactor();
        // 檔案裡的實際位元組就是 JSON escaped 形式：C:\\Users\\alice\\ws
        let line = r#"{"message":"cwd C:\\Users\\alice\\ws","metadata":{"nested":{"deep":{"server_path":"C:\\Program Files\\pylsp\\pylsp.exe"},"list":["/opt/tools/bin/agent","\\\\fileserver\\share\\a.log"]}}}"#;

        let got = redact_line(line, &redactor);
        let value: serde_json::Value = serde_json::from_str(&got).unwrap();

        assert!(!got.contains("alice"), "escaped Windows path 仍外洩：{got}");
        assert!(!got.contains("Program Files"), "nested key 未處理：{got}");
        assert!(!got.contains("fileserver"), "陣列元素未處理：{got}");
        assert!(value["metadata"]["nested"]["deep"]["server_path"]
            .as_str()
            .unwrap()
            .ends_with("pylsp.exe"));
        assert!(value["metadata"]["nested"]["list"][0]
            .as_str()
            .unwrap()
            .ends_with("/agent"));
    }

    /// B1：非 ASCII 內容（`→`、中文、日文、emoji）不得 panic——`lsp_service` 的
    /// `spawn {} → {}` 是每個啟動過 LSP 的使用者當日 log 必有的形狀。
    #[test]
    fn redact_line_handles_non_ascii_content_without_panicking() {
        let redactor = redactor();

        // 生產必觸發的真實形狀（byte index 6 落在 `→` 中間）
        let real = redact_line(
            r#"{"message":"spawn rust-analyzer → /Users/a/ws"}"#,
            &redactor,
        );
        assert!(!real.contains("/Users/a/ws"), "路徑仍外洩：{real}");
        assert!(real.contains("rust-analyzer"), "訊息應保留：{real}");

        for message in [
            "spawn rust-analyzer → /Users/a/ws",
            "ab認",
            "無法開啟 /Users/愛麗絲/專案/主控台.rs：權限不足",
            "サーバー起動 → C:\\ユーザー\\alice\\ワークスペース\\main.rs",
            "🚀 deploy → bearer トークン失効 token=abc123XYZdef456",
            "認證失敗：password=秘密のことば",
        ] {
            let got = redacted_message(message, &redactor);
            assert!(!got.contains("愛麗絲"), "中文使用者目錄仍外洩：{got}");
            assert!(!got.contains("abc123XYZdef456"), "token 仍外洩：{got}");
            assert!(!got.contains("秘密のことば"), "password 仍外洩：{got}");
        }

        // 整條 scrubber 鏈都不得對 byte index 做未檢查的切片：用確定性 LCG 拼出
        // 混雜多位元組字元的訊息，只要有一處切在 char boundary 中間就會 panic。
        let pool: Vec<char> = "→認あ🚀ü/\\:@.-_= \"'abzAZ09[]{}<>".chars().collect();
        let mut seed = 0x2026_0725u64;
        for _ in 0..2000 {
            let message: String = (0..24)
                .map(|_| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    pool[(seed >> 33) as usize % pool.len()]
                })
                .collect();
            let _ = redacted_message(&message, &redactor);
        }
    }

    /// B2：`file://` URI 與裸路徑必須得到同一個 hash——同一行內不能一個遮一個不遮。
    #[test]
    fn redact_line_scrubs_file_uris_consistently_with_bare_paths() {
        let redactor = redactor();

        let got = redacted_message(
            "opened /Users/alice/ws/agent at file:///Users/alice/ws/agent",
            &redactor,
        );

        assert!(!got.contains("/Users/alice"), "URI 形式仍外洩：{got}");
        assert!(got.contains("file://<path:"), "URI 應保留 scheme：{got}");
        let marker = got
            .split("file://")
            .nth(1)
            .and_then(|tail| tail.split(' ').next())
            .unwrap()
            .to_string();
        assert_eq!(
            got.matches(&marker).count(),
            2,
            "裸路徑與 URI 應同 hash：{got}"
        );

        let windows = redacted_message("file:///D:/Work/AcmeSecret/src/main.rs", &redactor);
        assert!(
            !windows.contains("AcmeSecret"),
            "URI 目錄段仍外洩：{windows}"
        );
        assert!(windows.ends_with("main.rs"), "basename 應保留：{windows}");
    }

    /// S3：內網 git host 與私有 repo 名（URL 的 host 與路徑段）都要去識別化。
    #[test]
    fn redact_line_scrubs_http_url_hosts_and_paths() {
        let redactor = redactor();

        let got = redacted_message(
            "fetch https://gitlab.internal.acme-corp.net/platform/billing-secrets.git failed",
            &redactor,
        );

        assert!(!got.contains("acme-corp"), "host 仍外洩：{got}");
        assert!(!got.contains("billing-secrets"), "repo 路徑仍外洩：{got}");
        assert!(got.contains("https://<host:"), "scheme 應保留：{got}");
    }

    /// S1／S2：`command` 與 `workspace` 也是路徑 key，且同一路徑跨 key 同 hash。
    #[test]
    fn redact_line_treats_command_and_workspace_as_path_keys() {
        let redactor = redactor();
        let line = r#"{"workspace_path":"/Users/alice/ws","metadata":{"workspace":"/Users/alice/ws","command":"C:\\Program Files\\Acme Tools\\pylsp.exe --stdio"}}"#;

        let got = redact_line(line, &redactor);
        let value: serde_json::Value = serde_json::from_str(&got).unwrap();

        assert_eq!(
            value["workspace_path"], value["metadata"]["workspace"],
            "同一路徑跨 key 應同 hash：{got}"
        );
        assert!(!got.contains("Acme Tools"), "command 目錄段仍外洩：{got}");
        assert!(
            value["metadata"]["command"]
                .as_str()
                .unwrap()
                .ends_with("pylsp.exe --stdio"),
            "command 的執行檔與參數應保留：{got}"
        );
    }

    /// S8：`PATH` 這類 `:` 分隔的多路徑要逐段 hash，不能壓成一個 token。
    #[test]
    fn redact_line_splits_colon_separated_path_lists() {
        let redactor = redactor();

        let got = redacted_message("PATH=/Users/alice/.bun/bin:/usr/local/bin", &redactor);

        assert!(!got.contains("alice"), "第一段仍外洩：{got}");
        assert!(!got.contains("/usr/local"), "第二段未被處理：{got}");
        assert_eq!(got.matches("<path:").count(), 2, "應逐段 hash：{got}");
        assert_eq!(got.matches("/bin").count(), 2, "basename 應保留：{got}");
    }

    /// AC 5：malformed / non-JSON 行 fail-closed——不輸出原文，只留佔位記錄。
    #[test]
    fn redact_line_falls_back_to_a_placeholder_for_unparseable_lines() {
        let redactor = redactor();
        for line in [
            "adapter stderr: cannot open /Users/alice/ws/secret.pem",
            r#"{"message":"truncated"#,
            "",
        ] {
            let got = redact_line(line, &redactor);
            assert!(
                !got.contains("alice") && !got.contains("truncated"),
                "fail-closed 失效，原文外洩：{line} → {got}"
            );
            let value: serde_json::Value = serde_json::from_str(&got).unwrap();
            assert_eq!(value["redacted"], "unparseable-log-line");
            assert_eq!(value["bytes"], line.len());
        }
        assert_eq!(redactor.summary().unparseable_lines, 3);
    }

    /// AC 6：export 回傳結構化 summary（各類型計數 + unparseable 行數）。
    #[test]
    fn export_zip_with_summary_reports_redaction_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d");
        std::fs::write(
            tmp.path().join(format!("yuzora-{today}.jsonl")),
            concat!(
                r#"{"message":"ssh alice@10.0.0.5:22 /Users/alice/ws/app.ts","metadata":{"fingerprint":"MD5:aa:bb:cc:dd:ee:ff","password":"hunter2"}}"#,
                "\nadapter stderr: boom\n"
            ),
        )
        .unwrap();
        let dest = tmp.path().join("logs.zip");

        let result = export_zip_with_summary(tmp.path(), &dest, true).unwrap();

        assert_eq!(result.path, dest.to_string_lossy());
        let summary = result.summary.expect("sanitize=true 應回 summary");
        assert_eq!(summary.paths, 1);
        assert_eq!(summary.hosts, 1);
        assert_eq!(summary.usernames, 1);
        assert_eq!(summary.fingerprints, 1);
        assert_eq!(summary.secrets, 1);
        assert_eq!(summary.unparseable_lines, 1);

        // sanitize=false 沒有 summary
        let raw = export_zip_with_summary(tmp.path(), &tmp.path().join("raw.zip"), false).unwrap();
        assert!(raw.summary.is_none());
    }

    /// AC 7：Copy（log_sanitize_lines）與 Export 走同一條 redaction，語意一致。
    #[test]
    fn log_sanitize_lines_matches_export_redaction_semantics() {
        let tmp = tempfile::tempdir().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d");
        let line = r#"{"message":"ssh alice@10.0.0.5:22 /Users/alice/ws/app.ts","metadata":{"password":"hunter2"}}"#;
        std::fs::write(
            tmp.path().join(format!("yuzora-{today}.jsonl")),
            format!("{line}\nadapter stderr: boom\n"),
        )
        .unwrap();
        let dest = tmp.path().join("logs.zip");
        export_zip_with_summary(tmp.path(), &dest, true).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).unwrap();
        let mut exported = String::new();
        {
            use std::io::Read;
            archive
                .by_name(&format!("yuzora-{today}.jsonl"))
                .unwrap()
                .read_to_string(&mut exported)
                .unwrap();
        }

        let copied =
            log_sanitize_lines(vec![line.to_string(), "adapter stderr: boom".to_string()]).unwrap();

        for secret in ["alice", "10.0.0.5", "hunter2", "boom"] {
            assert!(!exported.contains(secret), "export 外洩 {secret}");
            assert!(
                !copied.iter().any(|row| row.contains(secret)),
                "copy 外洩 {secret}"
            );
        }
        // S6：salt 是 process 生命週期共用的，剪貼簿與 zip 內必須是同一組 hash
        assert_eq!(exported.trim_end_matches('\n'), copied.join("\n"));
        // 連按兩次 Copy 也要一致
        assert_eq!(
            copied,
            log_sanitize_lines(vec![line.to_string(), "adapter stderr: boom".to_string()]).unwrap()
        );
    }

    /// N5：log 檔中間的空行不是 malformed record，不該產生佔位記錄或灌大計數。
    #[test]
    fn export_zip_skips_blank_lines_instead_of_counting_them_as_unparseable() {
        let tmp = tempfile::tempdir().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d");
        std::fs::write(
            tmp.path().join(format!("yuzora-{today}.jsonl")),
            "{\"message\":\"first\"}\n\n   \n{\"message\":\"second\"}\n",
        )
        .unwrap();
        let dest = tmp.path().join("logs.zip");

        let result = export_zip_with_summary(tmp.path(), &dest, true).unwrap();

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).unwrap();
        let mut exported = String::new();
        {
            use std::io::Read;
            archive
                .by_name(&format!("yuzora-{today}.jsonl"))
                .unwrap()
                .read_to_string(&mut exported)
                .unwrap();
        }
        assert_eq!(result.summary.unwrap().unparseable_lines, 0);
        assert!(!exported.contains("unparseable-log-line"), "{exported}");
        assert_eq!(exported.lines().count(), 2, "{exported}");
    }

    #[test]
    fn query_time_filters_compare_chronologically_across_offsets() {
        // A（+08:00）字典序比 B（+00:00）大，但時間上比 B 早 4 小時——
        // 字典序比較會做出相反判斷，這裡固定 chronological 語意。
        let tmp = tempfile::tempdir().unwrap();
        let make = |ts: &str, msg: &str| {
            serde_json::json!({
                "timestamp": ts,
                "level": "info",
                "kind": "debug",
                "source": "test",
                "workspace_path": null,
                "event": "e",
                "message": msg,
                "metadata": {}
            })
            .to_string()
        };
        std::fs::write(
            tmp.path().join("yuzora-2026-01-02.jsonl"),
            format!(
                "{}\n{}\n",
                make("2026-01-02T00:00:00+08:00", "earlier"), // = 2026-01-01T16:00Z
                make("2026-01-01T20:00:00+00:00", "later"),   // = 2026-01-01T20:00Z
            ),
        )
        .unwrap();

        let got = query_dir(
            tmp.path(),
            &LogQueryFilters {
                since: Some("2026-01-01T18:00:00+00:00".into()),
                limit: Some(10),
                ..LogQueryFilters::default()
            },
        );

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "later");
    }

    #[test]
    fn query_limit_keeps_newest_matches_within_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        for msg in ["a", "b", "c"] {
            sink.write(ev(msg));
        }

        let got = query_dir(
            tmp.path(),
            &LogQueryFilters {
                limit: Some(2),
                ..LogQueryFilters::default()
            },
        );

        // newest-first：限 2 筆時應回最新的 c、b
        assert_eq!(
            got.iter().map(|r| r.message.as_str()).collect::<Vec<_>>(),
            vec!["c", "b"]
        );
    }

    #[test]
    fn query_text_filter_matches_metadata_content() {
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        sink.write(LogEvent {
            level: "debug".into(),
            kind: "debug".into(),
            source: "git_service".into(),
            workspace_path: None,
            event: "run_git".into(),
            message: "git push".into(),
            metadata: serde_json::json!({ "stderr": "fatal: remote rejected" }),
        });
        sink.write(ev("unrelated"));

        let got = query_dir(
            tmp.path(),
            &LogQueryFilters {
                text: Some("remote rejected".into()),
                limit: Some(10),
                ..LogQueryFilters::default()
            },
        );

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "git push");
    }

    #[test]
    fn cleanup_size_purge_skips_non_log_files_and_today() {
        let tmp = tempfile::tempdir().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let old = tmp.path().join("yuzora-2026-01-01.jsonl");
        let todays = tmp.path().join(format!("yuzora-{today}.jsonl"));
        let legacy = tmp.path().join("yuzora-logs.sqlite");
        std::fs::write(&old, vec![b'x'; 64]).unwrap();
        std::fs::write(&todays, vec![b'x'; 64]).unwrap();
        std::fs::write(&legacy, vec![b'x'; 64]).unwrap();
        let sink = LogSink::new(tmp.path().to_path_buf());

        // retention 放寬到不觸發、size 上限壓到 1 byte：只有非今日的 dated 檔可被刪
        sink.cleanup_with_limits(100_000, 1);

        assert!(!old.exists(), "非今日 dated 檔應被 size 清理刪除");
        assert!(todays.exists(), "今日檔不可刪");
        assert!(legacy.exists(), "非 yuzora-*.jsonl 檔不可刪");
    }

    #[test]
    fn cleanup_retention_boundary_keeps_exactly_retention_days() {
        let tmp = tempfile::tempdir().unwrap();
        let today = chrono::Local::now().date_naive();
        let at = |days_ago: i64| {
            tmp.path().join(format!(
                "yuzora-{}.jsonl",
                (today - chrono::Duration::days(days_ago)).format("%Y-%m-%d")
            ))
        };
        std::fs::write(at(RETENTION_DAYS), "{}\n").unwrap(); // 第 15 天（含今日）→ 刪
        std::fs::write(at(RETENTION_DAYS - 1), "{}\n").unwrap(); // 第 14 天 → 留
        let sink = LogSink::new(tmp.path().to_path_buf());

        sink.cleanup();

        assert!(!at(RETENTION_DAYS).exists());
        assert!(at(RETENTION_DAYS - 1).exists());
    }

    #[test]
    fn write_global_redirects_to_temp_dir_under_tests() {
        write_global(ev("global-sink-probe"));

        let dir = global_log_dir();
        assert!(dir.starts_with(std::env::temp_dir()));
        let today = chrono::Local::now().format("%Y-%m-%d");
        let content = std::fs::read_to_string(dir.join(format!("yuzora-{today}.jsonl"))).unwrap();
        assert!(content.contains("global-sink-probe"));
    }

    #[test]
    fn log_event_rejects_unknown_level_and_kind() {
        let mut bad_level = ev("x");
        bad_level.level = "verbose".into();
        assert!(log_event(bad_level).is_err());

        let mut bad_kind = ev("x");
        bad_kind.kind = "telemetry".into();
        assert!(log_event(bad_kind).is_err());

        assert!(log_event(ev("valid")).is_ok());
    }

    #[test]
    fn mask_url_userinfo_redacts_credentials_only() {
        assert_eq!(
            mask_url_userinfo("git remote add origin https://user:tok3n@github.com/a/b.git"),
            "git remote add origin https://<redacted>@github.com/a/b.git"
        );
        assert_eq!(
            mask_url_userinfo("clone https://github.com/a/b.git"),
            "clone https://github.com/a/b.git"
        );
        assert_eq!(
            mask_url_userinfo("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
        assert_eq!(
            mask_url_userinfo("push https://x@h/a and https://y:z@h2/b"),
            "push https://<redacted>@h/a and https://<redacted>@h2/b"
        );
    }

    #[test]
    fn connect_failure_event_masks_credentials_in_reason() {
        let ev = connect_failure_event(
            "db",
            "dbhost",
            5432,
            "app",
            "cannot connect to postgres: postgres://app:s3cr3t@dbhost:5432/db",
        );
        let blob = format!("{} {}", ev.message, ev.metadata);
        assert!(!blob.contains("s3cr3t"), "密碼不可出現在事件中");
        assert!(blob.contains("<redacted>"), "userinfo 應被遮蔽");
    }

    #[test]
    fn connect_failure_event_shape() {
        let ev = connect_failure_event("ssh", "example.com", 22, "alice", "認證失敗");
        assert_eq!(ev.level, "warn");
        assert_eq!(ev.source, "ssh");
        assert_eq!(ev.event, "connect_failed");
        assert_eq!(ev.metadata["host"], "example.com");
        assert_eq!(ev.metadata["port"], 22);
        assert_eq!(ev.metadata["user"], "alice");
        // 不得含任何密碼欄位
        assert!(ev.metadata.get("password").is_none());
    }

    #[test]
    fn query_time_filters_include_exact_boundary_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let timestamp = "2026-01-02T03:04:05+00:00";
        let record = serde_json::json!({
            "timestamp": timestamp,
            "level": "info",
            "kind": "debug",
            "source": "test",
            "workspace_path": null,
            "event": "unit_test",
            "message": "boundary",
            "metadata": {}
        });
        std::fs::write(
            tmp.path().join("yuzora-2026-01-02.jsonl"),
            format!("{record}\n"),
        )
        .unwrap();

        let got = query_dir(
            tmp.path(),
            &LogQueryFilters {
                since: Some(timestamp.into()),
                until: Some(timestamp.into()),
                limit: Some(10),
                ..LogQueryFilters::default()
            },
        );

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].timestamp, timestamp);
    }

    #[test]
    fn write_drops_events_below_min_level() {
        let tmp = tempfile::tempdir().unwrap();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        sink.set_min_level("info");

        let mut debug_ev = ev("dropped");
        debug_ev.level = "debug".into();
        sink.write(debug_ev); // 低於 info → 丟棄
        sink.write(ev("kept")); // ev() 是 info → 寫入

        let files: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        let content = std::fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
        assert!(content.contains("kept"));
        assert!(!content.contains("dropped"));
    }

    #[test]
    fn write_below_threshold_still_triggers_daily_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("yuzora-2020-01-01.jsonl");
        std::fs::write(&old, "{}\n").unwrap();
        let mut sink = LogSink::new(tmp.path().to_path_buf());
        sink.set_min_level("error");
        // 一筆被門檻丟棄的 debug 事件仍應觸發當日 cleanup
        let mut debug_ev = ev("dropped");
        debug_ev.level = "debug".into();
        sink.write(debug_ev);
        assert!(!old.exists(), "被丟棄的寫入仍應觸發 retention cleanup");
    }

    #[test]
    fn level_rank_orders_levels() {
        assert!(level_rank("debug") < level_rank("info"));
        assert!(level_rank("info") < level_rank("warn"));
        assert!(level_rank("warn") < level_rank("error"));
        assert_eq!(level_rank("unknown"), level_rank("info"));
    }

    #[test]
    fn log_level_config_round_trips_and_defaults_to_info() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("logging.json");

        // 缺檔 → 預設 info
        assert_eq!(read_log_level_from(&path), "info");

        write_log_level_to(&path, "debug").unwrap();
        assert_eq!(read_log_level_from(&path), "debug");

        // 非法值 → 退回 info
        std::fs::write(&path, r#"{"minLevel":"loud"}"#).unwrap();
        assert_eq!(read_log_level_from(&path), "info");
    }
}
