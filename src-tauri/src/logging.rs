use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const RETENTION_DAYS: i64 = 14;
const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;
const VALID_LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];
const VALID_KINDS: [&str; 3] = ["debug", "user_action", "audit"];

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
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LogRecord {
    pub timestamp: String,
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

fn retained_log_files(dir: &Path) -> Vec<PathBuf> {
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

pub fn export_zip(dir: &Path, dest: &Path, sanitize: bool) -> Result<String, String> {
    let file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let home = dirs::home_dir().map(|home| home.to_string_lossy().to_string());
    let username = current_username();

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
        if sanitize {
            let home = home.as_deref();
            let username = username.as_deref();
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
                let sanitized = sanitize_line(&line, home, username);
                archive
                    .write_all(sanitized.as_bytes())
                    .map_err(|e| e.to_string())?;
                if had_newline {
                    archive.write_all(b"\n").map_err(|e| e.to_string())?;
                }
            }
        } else {
            std::io::copy(&mut reader, &mut archive).map_err(|e| e.to_string())?;
        }
    }

    archive.finish().map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn log_export(dest: String, sanitize: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        export_zip(&default_log_dir(), Path::new(&dest), sanitize)
    })
    .await
    .map_err(|err| err.to_string())?
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
        assert!(content.contains("~/workspace"));
        assert!(content.contains(&format!("owned by {username}")));
        assert!(!content.contains(&home_text));
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
