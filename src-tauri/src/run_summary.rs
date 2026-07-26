// `run-summary.json`（issue #40 §3.6）：把 daily JSONL 依 `run_id` 摺成每次 app
// run 一列的摘要，隨 Logs 匯出 bundle 一起送出。使用者回報 Windows UI hang 時，
// 可以用這裡的起訖時間與 WER report 的 timestamp 對齊，再用 run_id 回到原始
// JSONL 找細節。
//
// 資料來源全部是**既有的 log record**，本模組不新增任何計數器：
// - app 生命週期 → `logging::app_start_event` / `graceful_exit_event`
// - long-task／event-loop lag／terminal queue／perf → 前端 PerfBridge 週期寫下的
//   `diagnostics.sample`（metadata key 的契約見下方常數）
// - ACP process churn → `agent_process` 既有的 `acp_spawn` / `acp_kill` / `acp_exit`

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::logging::{
    retained_log_files, LogRecord, APP_LIFECYCLE_KIND, APP_START_EVENT, GRACEFUL_EXIT_EVENT,
};

/// 前端 `src/features/logs/diagnostics.ts` 寫入的 event 名。兩側必須一致，否則
/// run-summary 的 stall 欄位會永遠是 0——`diagnostics_event_name_matches_frontend`
/// 測試負責釘住這個契約。
pub const DIAGNOSTICS_SAMPLE_EVENT: &str = "diagnostics.sample";

fn number(metadata: &serde_json::Value, key: &str) -> Option<f64> {
    metadata.get(key).and_then(serde_json::Value::as_f64)
}

/// 一次 app run 的摘要。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RunSummaryEntry {
    pub run_id: String,
    /// 該 run 在本 bundle 中最早／最晚一筆 record 的 timestamp（RFC3339 UTC）。
    pub started_at: String,
    pub ended_at: String,
    pub record_count: u64,
    pub app_version: Option<String>,
    pub host_pid: Option<u64>,
    pub platform: Option<String>,
    /// `app_start` metadata 的 `previous_run_unclean`；沒有 `app_start` 時為 None
    /// （例如 bundle 只涵蓋了該 run 的後半段）。
    pub previous_run_unclean: Option<bool>,
    /// 這次 run 有沒有寫下 `graceful_exit`。false = 尚未結束或非正常收尾。
    pub graceful_exit: bool,
    pub long_task_count: u64,
    pub long_task_total_ms: f64,
    pub long_task_max_ms: f64,
    pub event_loop_lag_max_ms: f64,
    pub perf_peak_memory_bytes: u64,
    pub perf_peak_cpu_percent: f64,
    pub perf_peak_webview_memory_bytes: u64,
    pub terminal_dropped_bytes: u64,
    pub perf_sampling_failures: u64,
    pub acp_spawns: u64,
    pub acp_exits: u64,
    pub error_count: u64,
}

impl RunSummaryEntry {
    fn new(run_id: String, timestamp: String) -> Self {
        Self {
            run_id,
            started_at: timestamp.clone(),
            ended_at: timestamp,
            record_count: 0,
            app_version: None,
            host_pid: None,
            platform: None,
            previous_run_unclean: None,
            graceful_exit: false,
            long_task_count: 0,
            long_task_total_ms: 0.0,
            long_task_max_ms: 0.0,
            event_loop_lag_max_ms: 0.0,
            perf_peak_memory_bytes: 0,
            perf_peak_cpu_percent: 0.0,
            perf_peak_webview_memory_bytes: 0,
            terminal_dropped_bytes: 0,
            perf_sampling_failures: 0,
            acp_spawns: 0,
            acp_exits: 0,
            error_count: 0,
        }
    }

    fn absorb(&mut self, record: &LogRecord) {
        self.record_count = self.record_count.saturating_add(1);
        // 字典序比較對 RFC3339 UTC 字串成立，但歷史資料可能帶 +08:00 offset，
        // 因此 min/max 只在能 parse 時才更新（parse 不出就沿用既有邊界）。
        if is_earlier(&record.timestamp, &self.started_at) {
            self.started_at = record.timestamp.clone();
        }
        if is_earlier(&self.ended_at, &record.timestamp) {
            self.ended_at = record.timestamp.clone();
        }
        if record.level == "error" {
            self.error_count = self.error_count.saturating_add(1);
        }
        match (record.kind.as_str(), record.event.as_str()) {
            (APP_LIFECYCLE_KIND, APP_START_EVENT) => {
                self.app_version = record
                    .metadata
                    .get("app_version")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);
                self.host_pid = record.metadata.get("host_pid").and_then(|v| v.as_u64());
                self.platform = record
                    .metadata
                    .get("platform")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);
                self.previous_run_unclean = record
                    .metadata
                    .get("previous_run_unclean")
                    .and_then(serde_json::Value::as_bool);
            }
            (APP_LIFECYCLE_KIND, GRACEFUL_EXIT_EVENT) => self.graceful_exit = true,
            _ => {}
        }
        match record.event.as_str() {
            "acp_spawn" => self.acp_spawns = self.acp_spawns.saturating_add(1),
            // exit 與 kill 都是「這個 agent process 沒了」，churn 看的是這個。
            "acp_exit" | "acp_kill" => self.acp_exits = self.acp_exits.saturating_add(1),
            DIAGNOSTICS_SAMPLE_EVENT => self.absorb_diagnostics(&record.metadata),
            _ => {}
        }
    }

    /// `diagnostics.sample` 的欄位分三類，聚合方式不同：
    ///
    /// - **增量（delta）**：`long_task_*` 與 `sampling_*` 都是「距上次回報以來」
    ///   的量（前端分別由 `telemetry.drain()` 與 since-last-report 計數器產生），
    ///   因此累加後等於實際次數。**不可以**餵滾動視窗的當下組成進來——視窗
    ///   60 s 而回報間隔最短 10 s，那樣同一次失敗最多會被計 6 次。
    /// - **峰值（peak）**：`*_max_ms`、`perf_*` 是該視窗的瞬時值，取最大值。
    /// - **單調累計（cumulative）**：`terminal_dropped_bytes` 是跨 session 的高
    ///   水位總和（前端已處理 session 關閉後的倒退），取最大值。
    fn absorb_diagnostics(&mut self, metadata: &serde_json::Value) {
        if let Some(value) = number(metadata, "long_task_count") {
            self.long_task_count = self.long_task_count.saturating_add(value.max(0.0) as u64);
        }
        if let Some(value) = number(metadata, "long_task_total_ms") {
            self.long_task_total_ms += value.max(0.0);
        }
        if let Some(value) = number(metadata, "sampling_failures") {
            self.perf_sampling_failures = self
                .perf_sampling_failures
                .saturating_add(value.max(0.0) as u64);
        }
        for (key, slot) in [
            ("long_task_max_ms", &mut self.long_task_max_ms),
            ("event_loop_lag_max_ms", &mut self.event_loop_lag_max_ms),
            ("perf_cpu_percent", &mut self.perf_peak_cpu_percent),
        ] {
            if let Some(value) = number(metadata, key) {
                if value > *slot {
                    *slot = value;
                }
            }
        }
        for (key, slot) in [
            ("perf_memory_bytes", &mut self.perf_peak_memory_bytes),
            (
                "perf_webview_memory_bytes",
                &mut self.perf_peak_webview_memory_bytes,
            ),
        ] {
            if let Some(value) = number(metadata, key) {
                let value = value.max(0.0) as u64;
                if value > *slot {
                    *slot = value;
                }
            }
        }
        // dropped bytes 是前端已做成單調遞增的高水位總和（含已關閉的 session），
        // 取最大值而非累加——後續取樣本來就包含先前的量，累加會膨脹。
        if let Some(value) = number(metadata, "terminal_dropped_bytes") {
            let value = value.max(0.0) as u64;
            if value > self.terminal_dropped_bytes {
                self.terminal_dropped_bytes = value;
            }
        }
    }
}

/// `a` 是否嚴格早於 `b`。兩邊都 parse 得出才比較；否則回 false（保守，不動邊界）。
fn is_earlier(a: &str, b: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(a),
        chrono::DateTime::parse_from_rfc3339(b),
    ) {
        (Ok(left), Ok(right)) => left < right,
        _ => false,
    }
}

/// 增量摺疊器：把 log record 一筆一筆摺進 per-run 摘要。
///
/// 記憶體是 **O(run 數)**，與 log 大小無關：每一行 parse 完就摺進對應的 run 並
/// 丟掉，不保留 `LogRecord`（其中的 `serde_json::Value` metadata 在記憶體裡通常
/// 是原文的數倍）。log 總量上限是 `MAX_TOTAL_BYTES` = 100 MB 再加上無上限的今日
/// 檔，全量載入會在「已經在 hang 的機器上按 Export bundle」時造成數百 MB 的尖峰
/// ——同一個模組的 `logging::query_dir` 早就為此改用 ring buffer，這裡不該退回去。
#[derive(Default)]
pub struct RunSummaryBuilder {
    runs: HashMap<String, RunSummaryEntry>,
}

impl RunSummaryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 摺入一筆 record。
    ///
    /// 沒有 `run_id` 的 record（#40 之前寫下的）一律丟棄——它們無法歸屬到任何一次
    /// 執行，硬塞進一個 `"unknown"` 桶只會產生看起來像 run、實際上橫跨數週的假資料。
    pub fn absorb_record(&mut self, record: &LogRecord) {
        let Some(run_id) = record.run_id.as_deref().filter(|id| !id.is_empty()) else {
            return;
        };
        self.runs
            .entry(run_id.to_string())
            .or_insert_with(|| RunSummaryEntry::new(run_id.to_string(), record.timestamp.clone()))
            .absorb(record);
    }

    /// 摺入一行 JSONL。parse 不出的行靜默跳過（壞行不該中斷整份摘要）。
    pub fn absorb_line(&mut self, line: &str) {
        if let Ok(record) = serde_json::from_str::<LogRecord>(line) {
            self.absorb_record(&record);
        }
    }

    /// 依 `started_at` 由舊到新排序後輸出。
    pub fn finish(self) -> Vec<RunSummaryEntry> {
        let mut entries: Vec<RunSummaryEntry> = self.runs.into_values().collect();
        entries.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.run_id.cmp(&b.run_id))
        });
        entries
    }
}

/// 把一批已在記憶體裡的 record 摺成 per-run 摘要，依 `started_at` 由舊到新排序。
/// 掃目錄請用 `build_run_summary`（串流，不需要先把 record 收集起來）。
pub fn summarize_records(records: &[LogRecord]) -> Vec<RunSummaryEntry> {
    let mut builder = RunSummaryBuilder::new();
    for record in records {
        builder.absorb_record(record);
    }
    builder.finish()
}

/// 掃 log 目錄下所有保留中的 daily file，逐行串流摺疊。
///
/// 記憶體 O(run 數)：讀一行、摺一行、丟一行，任何時刻只有一筆 `LogRecord` 存活。
pub fn build_run_summary(dir: &Path) -> Vec<RunSummaryEntry> {
    let mut builder = RunSummaryBuilder::new();
    for path in retained_log_files(dir) {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            builder.absorb_line(&line);
        }
    }
    builder.finish()
}

/// bundle 內 `run-summary.json` 的內容。
///
/// **單行 compact JSON array**（而不是 pretty-print）：#41 的 `redact_line` 是
/// 逐行 fail-closed 的——pretty-print 後每一行都不是合法 JSON，整份摘要會被換成
/// 佔位符。單行陣列讓 sanitize 出口原封沿用既有 redaction 契約。
pub fn render_run_summary(entries: &[RunSummaryEntry]) -> String {
    serde_json::to_string(entries).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(run_id: Option<&str>, timestamp: &str, event: &str) -> LogRecord {
        LogRecord {
            timestamp: timestamp.to_string(),
            run_id: run_id.map(str::to_string),
            level: "info".into(),
            kind: "debug".into(),
            source: "test".into(),
            workspace_path: None,
            event: event.to_string(),
            message: String::new(),
            metadata: serde_json::json!({}),
        }
    }

    fn app_start(run_id: &str, timestamp: &str, unclean: bool) -> LogRecord {
        LogRecord {
            kind: APP_LIFECYCLE_KIND.into(),
            metadata: serde_json::json!({
                "app_version": "0.0.3",
                "host_pid": 4242,
                "platform": "macos",
                "previous_run_unclean": unclean,
            }),
            ..record(Some(run_id), timestamp, APP_START_EVENT)
        }
    }

    fn diagnostics(run_id: &str, timestamp: &str, metadata: serde_json::Value) -> LogRecord {
        LogRecord {
            metadata,
            ..record(Some(run_id), timestamp, DIAGNOSTICS_SAMPLE_EVENT)
        }
    }

    #[test]
    fn summary_splits_records_into_one_entry_per_run() {
        // AC 第 1 條：同一份資料裡的兩次執行必須各自成組，且不互相汙染。
        let records = [
            app_start("run-a", "2026-07-25T01:00:00+00:00", false),
            record(Some("run-a"), "2026-07-25T01:05:00+00:00", "acp_spawn"),
            app_start("run-b", "2026-07-25T02:00:00+00:00", true),
            record(Some("run-b"), "2026-07-25T02:05:00+00:00", "acp_spawn"),
            record(Some("run-b"), "2026-07-25T02:06:00+00:00", "acp_exit"),
        ];
        let summary = summarize_records(&records);
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0].run_id, "run-a");
        assert_eq!(summary[0].record_count, 2);
        assert_eq!(summary[0].acp_spawns, 1);
        assert_eq!(summary[0].acp_exits, 0);
        assert_eq!(summary[0].previous_run_unclean, Some(false));
        assert_eq!(summary[1].run_id, "run-b");
        assert_eq!(summary[1].acp_spawns, 1);
        assert_eq!(summary[1].acp_exits, 1);
        assert_eq!(summary[1].previous_run_unclean, Some(true));
    }

    #[test]
    fn summary_records_start_end_and_identity_from_app_lifecycle_events() {
        let records = [
            app_start("run-a", "2026-07-25T01:00:00+00:00", false),
            record(Some("run-a"), "2026-07-25T01:30:00+00:00", "anything"),
            LogRecord {
                kind: APP_LIFECYCLE_KIND.into(),
                ..record(
                    Some("run-a"),
                    "2026-07-25T02:00:00+00:00",
                    GRACEFUL_EXIT_EVENT,
                )
            },
        ];
        let summary = summarize_records(&records);
        assert_eq!(summary[0].started_at, "2026-07-25T01:00:00+00:00");
        assert_eq!(summary[0].ended_at, "2026-07-25T02:00:00+00:00");
        assert_eq!(summary[0].app_version.as_deref(), Some("0.0.3"));
        assert_eq!(summary[0].host_pid, Some(4242));
        assert_eq!(summary[0].platform.as_deref(), Some("macos"));
        assert!(summary[0].graceful_exit);
    }

    #[test]
    fn summary_marks_a_run_without_graceful_exit() {
        let records = [app_start("run-a", "2026-07-25T01:00:00+00:00", false)];
        assert!(!summarize_records(&records)[0].graceful_exit);
    }

    #[test]
    fn summary_skips_records_without_a_run_id() {
        // pre-#40 的歷史資料不得被塞進任何一次 run。
        let records = [
            record(None, "2026-07-01T00:00:00+00:00", "legacy"),
            record(Some(""), "2026-07-01T00:00:01+00:00", "legacy"),
            record(Some("run-a"), "2026-07-25T01:00:00+00:00", "modern"),
        ];
        let summary = summarize_records(&records);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].run_id, "run-a");
        assert_eq!(summary[0].record_count, 1);
    }

    #[test]
    fn summary_accumulates_counts_and_peaks_from_diagnostics_samples() {
        let records = [
            diagnostics(
                "run-a",
                "2026-07-25T01:00:00+00:00",
                serde_json::json!({
                    "long_task_count": 3,
                    "long_task_total_ms": 210.5,
                    "long_task_max_ms": 120.0,
                    "event_loop_lag_max_ms": 40.0,
                    "perf_cpu_percent": 30.0,
                    "perf_memory_bytes": 500_000_000u64,
                    "perf_webview_memory_bytes": 400_000_000u64,
                    "terminal_dropped_bytes": 1_000,
                    "sampling_failures": 1,
                }),
            ),
            diagnostics(
                "run-a",
                "2026-07-25T01:01:00+00:00",
                serde_json::json!({
                    "long_task_count": 2,
                    "long_task_total_ms": 100.0,
                    "long_task_max_ms": 90.0,
                    "event_loop_lag_max_ms": 900.0,
                    "perf_cpu_percent": 10.0,
                    "perf_memory_bytes": 300_000_000u64,
                    "perf_webview_memory_bytes": 200_000_000u64,
                    "terminal_dropped_bytes": 4_000,
                    "sampling_failures": 2,
                }),
            ),
        ];
        let summary = summarize_records(&records);
        // count 類累加
        assert_eq!(summary[0].long_task_count, 5);
        assert!((summary[0].long_task_total_ms - 310.5).abs() < 1e-9);
        assert_eq!(summary[0].perf_sampling_failures, 3);
        // peak 類取最大值，不因為後一筆較小而被覆蓋
        assert!((summary[0].long_task_max_ms - 120.0).abs() < 1e-9);
        assert!((summary[0].event_loop_lag_max_ms - 900.0).abs() < 1e-9);
        assert!((summary[0].perf_peak_cpu_percent - 30.0).abs() < 1e-9);
        assert_eq!(summary[0].perf_peak_memory_bytes, 500_000_000);
        assert_eq!(summary[0].perf_peak_webview_memory_bytes, 400_000_000);
        // 累計型指標取最大值而非累加
        assert_eq!(summary[0].terminal_dropped_bytes, 4_000);
    }

    #[test]
    fn summary_counts_error_level_records() {
        let records = [
            LogRecord {
                level: "error".into(),
                ..record(Some("run-a"), "2026-07-25T01:00:00+00:00", "boom")
            },
            record(Some("run-a"), "2026-07-25T01:00:01+00:00", "fine"),
        ];
        assert_eq!(summarize_records(&records)[0].error_count, 1);
    }

    #[test]
    fn summary_is_ordered_oldest_run_first_regardless_of_input_order() {
        let records = [
            record(Some("run-b"), "2026-07-25T05:00:00+00:00", "x"),
            record(Some("run-a"), "2026-07-25T01:00:00+00:00", "x"),
        ];
        let summary = summarize_records(&records);
        assert_eq!(
            summary
                .iter()
                .map(|e| e.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["run-a", "run-b"]
        );
    }

    #[test]
    fn build_run_summary_reads_a_daily_log_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d");
        let path = tmp.path().join(format!("yuzora-{today}.jsonl"));
        let lines = [
            serde_json::to_string(&app_start("run-a", "2026-07-25T01:00:00+00:00", false)).unwrap(),
            // 壞行必須被跳過而不是中斷整份摘要
            "{ not json".to_string(),
            serde_json::to_string(&record(
                Some("run-a"),
                "2026-07-25T01:01:00+00:00",
                "acp_spawn",
            ))
            .unwrap(),
        ];
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        let summary = build_run_summary(tmp.path());
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].run_id, "run-a");
        assert_eq!(summary[0].record_count, 2);
        assert_eq!(summary[0].acp_spawns, 1);
    }

    #[test]
    fn render_run_summary_is_a_single_line_of_valid_json() {
        // #41 的 redact_line 逐行 fail-closed，摘要必須是**一行**合法 JSON。
        let rendered = render_run_summary(&summarize_records(&[app_start(
            "run-a",
            "2026-07-25T01:00:00+00:00",
            false,
        )]));
        assert!(!rendered.contains('\n'), "run-summary 必須是單行");
        let parsed: Vec<RunSummaryEntry> = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].run_id, "run-a");
    }

    #[test]
    fn render_run_summary_is_an_empty_array_when_there_are_no_runs() {
        assert_eq!(render_run_summary(&[]), "[]");
    }

    #[test]
    fn builder_folds_incrementally_without_retaining_records() {
        // 增量摺疊必須與「先收集再摺」等價——差別只在記憶體，不在結果。
        let records = [
            app_start("run-a", "2026-07-25T01:00:00+00:00", false),
            record(Some("run-a"), "2026-07-25T01:05:00+00:00", "acp_spawn"),
            record(Some("run-b"), "2026-07-25T02:00:00+00:00", "acp_spawn"),
        ];
        let mut builder = RunSummaryBuilder::new();
        for record in &records {
            builder.absorb_record(record);
        }
        assert_eq!(builder.finish(), summarize_records(&records));
    }

    #[test]
    fn builder_absorb_line_skips_unparseable_lines() {
        let mut builder = RunSummaryBuilder::new();
        builder.absorb_line("{ not json");
        builder.absorb_line("");
        builder.absorb_line(
            &serde_json::to_string(&record(Some("run-a"), "2026-07-25T01:00:00+00:00", "x"))
                .unwrap(),
        );
        let entries = builder.finish();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].record_count, 1);
    }

    #[test]
    fn sampling_failures_are_treated_as_per_report_deltas_not_window_snapshots() {
        // 契約：前端送的是「距上次回報以來」的增量，因此 Rust 端累加後 = 實際次數。
        // 若前端改回送滾動視窗的當下組成（30 polls），這裡的累加會嚴重灌水
        // ——視窗 60 s vs 回報間隔最短 10 s，同一次失敗最多被計 6 次。
        let records = [
            diagnostics(
                "run-a",
                "2026-07-25T01:00:00+00:00",
                serde_json::json!({ "sampling_failures": 1, "sampling_attempts": 5 }),
            ),
            diagnostics(
                "run-a",
                "2026-07-25T01:00:10+00:00",
                serde_json::json!({ "sampling_failures": 2, "sampling_attempts": 5 }),
            ),
        ];
        // 實際失敗 3 次（1 + 2），不是 window 重疊後的 6 次。
        assert_eq!(summarize_records(&records)[0].perf_sampling_failures, 3);
    }
}
