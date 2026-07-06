use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

const RETENTION_DAYS: i64 = 14;
const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;

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
        Self { dir }
    }

    fn current_path(&self) -> PathBuf {
        let date = chrono::Local::now().format("%Y-%m-%d");
        self.dir.join(format!("yuzora-{date}.jsonl"))
    }

    pub fn write(&mut self, ev: LogEvent) {
        let mut value = serde_json::to_value(&ev).unwrap_or_default();
        if let serde_json::Value::Object(ref mut map) = value {
            map.insert(
                "timestamp".into(),
                serde_json::Value::String(chrono::Local::now().to_rfc3339()),
            );
        }
        let line = serde_json::to_string(&value).unwrap_or_default();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.current_path())
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn cleanup(&self) {
        let today = chrono::Local::now().date_naive();
        let mut files: Vec<(PathBuf, u64)> = vec![];
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if let Some(datestr) = name
                    .strip_prefix("yuzora-")
                    .and_then(|s| s.strip_suffix(".jsonl"))
                {
                    if let Ok(date) = chrono::NaiveDate::parse_from_str(datestr, "%Y-%m-%d") {
                        if (today - date).num_days() > RETENTION_DAYS {
                            let _ = std::fs::remove_file(&path);
                            continue;
                        }
                    }
                }
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push((path, size));
            }
        }
        // 總量超限：由最舊（檔名排序最小）開始刪
        files.sort();
        let mut total: u64 = files.iter().map(|(_, s)| s).sum();
        for (path, size) in files {
            if total <= MAX_TOTAL_BYTES {
                break;
            }
            let _ = std::fs::remove_file(&path);
            total -= size;
        }
    }
}

pub fn default_log_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".yuzora")
        .join("logs")
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

fn record_matches(record: &LogRecord, filters: &LogQueryFilters) -> bool {
    matches_filter(&record.level, &filters.levels)
        && matches_filter(&record.kind, &filters.kinds)
        && matches_filter(&record.source, &filters.sources)
        && filters
            .since
            .as_ref()
            .map(|since| record.timestamp >= *since)
            .unwrap_or(true)
        && filters
            .until
            .as_ref()
            .map(|until| record.timestamp <= *until)
            .unwrap_or(true)
        && filters
            .text
            .as_ref()
            .map(|text| record.event.contains(text) || record.message.contains(text))
            .unwrap_or(true)
}

pub fn query_dir(dir: &Path, filters: &LogQueryFilters) -> Vec<LogRecord> {
    let limit = filters.limit.unwrap_or(500);
    let mut records = Vec::new();
    if limit == 0 {
        return records;
    }

    for path in retained_log_files(dir) {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines().rev() {
            let Ok(record) = serde_json::from_str::<LogRecord>(line) else {
                continue;
            };
            if record_matches(&record, filters) {
                records.push(record);
                if records.len() >= limit {
                    return records;
                }
            }
        }
    }
    records
}

#[tauri::command]
pub fn log_query(filters: LogQueryFilters) -> Vec<LogRecord> {
    query_dir(&default_log_dir(), &filters)
}

pub fn sources_dir(dir: &Path) -> Vec<String> {
    let mut sources = BTreeSet::new();
    let filters = LogQueryFilters {
        limit: Some(usize::MAX),
        ..LogQueryFilters::default()
    };
    for record in query_dir(dir, &filters) {
        sources.insert(record.source);
    }
    sources.into_iter().collect()
}

#[tauri::command]
pub fn log_sources() -> Vec<String> {
    sources_dir(&default_log_dir())
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
pub fn log_export(dest: String, sanitize: bool) -> Result<String, String> {
    export_zip(&default_log_dir(), Path::new(&dest), sanitize)
}

#[tauri::command]
pub fn log_event(
    state: tauri::State<'_, std::sync::Mutex<LogSink>>,
    event: LogEvent,
) -> Result<(), String> {
    state.lock().map_err(|e| e.to_string())?.write(event);
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
    fn log_export_returns_error_for_unwritable_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("missing").join("logs.zip");

        let result = log_export(dest.to_string_lossy().to_string(), false);

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
}
