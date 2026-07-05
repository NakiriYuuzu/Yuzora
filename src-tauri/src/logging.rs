use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

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
}
