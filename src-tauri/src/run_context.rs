// Run correlation（issue #40）：讓同一個 daily JSONL 能被無歧義切成多次 app run。
//
// 為什麼是 process-global 而不是 Tauri managed state（spec §3.1 寫的是後者）：
// 蓋 `run_id` 的位置是 `logging::LogSink::write`，而 Rust 端所有 log 都走
// `logging::write_global`——那條路徑拿不到 `AppHandle`，取不到 managed state。
// 因此 run context 做成 process-global 的 `OnceLock`，`run()` 仍在最早期顯式
// 初始化一次（讓 app_start 讀得到），但任何更早的寫入也不會漏掉 run_id。
//
// `current()` 是 lazily self-initialising：第一次被讀到時才產生。整個 process
// 只會產生一次，因此同一次執行寫出的每一筆 record 共用同一個 run_id——這正是
// AC 第 1 條要的性質。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// 一次 app 執行的識別資訊。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunContext {
    pub run_id: String,
    pub app_version: String,
    pub host_pid: u32,
    pub platform: String,
}

/// `run_id` 的形狀：`YYYYMMDDTHHMMSSZ-xxxxxxxx`（共 25 字元）。
///
/// 長度與字元組成是**刻意**選的，為了通過 #41 的匯出去識別化而不被改寫：
/// - 沒有 `:` → 不會被 `scrub_fingerprints` 的 `aa:bb:…` 規則吃掉；
/// - 沒有 `/`、`\`、`~` → 不會被 `scrub_paths` 當成路徑；
/// - 沒有 `.` → 不會被 `scrub_ip_literals` 當成 IP；
/// - 全長 25 < 32 → 不會被 `scrub_high_entropy_runs` 當成秘密。
///
/// 時間戳前綴讓 run 在檔案裡天然按時間排序，隨機尾碼讓同一秒內的兩次啟動
/// （例如 crash 後立即重啟）仍然可分。
fn format_run_id(timestamp: &str, nonce: u32) -> String {
    format!("{timestamp}-{nonce:08x}")
}

/// 不新增 crate 依賴：時間戳來自既有的 `chrono`，隨機碼來自既有的 `rand`
/// （`logging::process_redaction_salt` 已在用）。
fn generate_run_id() -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    format_run_id(&timestamp, rand::random::<u32>())
}

impl RunContext {
    fn detect() -> Self {
        Self {
            run_id: generate_run_id(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            host_pid: std::process::id(),
            platform: std::env::consts::OS.to_string(),
        }
    }
}

static RUN_CONTEXT: OnceLock<RunContext> = OnceLock::new();

/// 本次執行的 run context。第一次呼叫時建立，其後恆為同一個值。
pub fn current() -> &'static RunContext {
    RUN_CONTEXT.get_or_init(RunContext::detect)
}

/// 本次執行的 run id。`LogSink::write` 用它蓋每一筆 record。
pub fn current_run_id() -> &'static str {
    current().run_id.as_str()
}

// ---------------------------------------------------------------------------
// previous-run unclean shutdown
//
// 判斷「上一次執行有沒有正常收尾」需要跨 process 的持久狀態。做法是一個 marker
// 檔：啟動時寫 `clean: false`，走到 exit handler 時改寫 `clean: true`。下次啟動
// 讀到 `clean: false` 就代表上一次沒有走到 exit handler（crash／被 kill／斷電）。
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RunState {
    pub run_id: String,
    pub clean: bool,
}

pub fn run_state_path() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join(format!("yuzora-test-run-state-{}.json", std::process::id()))
    }
    #[cfg(not(test))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".yuzora")
            .join("run-state.json")
    }
}

pub fn read_run_state(path: &Path) -> Option<RunState> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str::<RunState>(&body).ok())
}

pub fn write_run_state(path: &Path, run_id: &str, clean: bool) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let body = serde_json::to_string(&RunState {
        run_id: run_id.to_string(),
        clean,
    })
    .map_err(|error| error.to_string())?;
    std::fs::write(path, body).map_err(|error| error.to_string())
}

/// 上一次執行是否沒有正常收尾。
///
/// 缺檔（第一次啟動）與壞檔一律回 `false`：那是「不知道」，不是「不乾淨」，
/// 謊報 unclean 會讓這個訊號失去診斷價值。真正的 crash 會留下一個**可讀且**
/// `clean: false` 的檔，那才是這條回 `true` 的情形。
///
/// **已知限制（latent，本次刻意不處理）**：marker 檔是每個使用者一份，多個
/// Yuzora 實例並存時會互相汙染。`RunState` 雖然存了 `run_id`，這裡卻沒有比對它
/// ——因為「上一次執行」的 run_id 本來就不等於本次的，比對不了。後果是：
/// dev build 與 release build 同時開著時，會同時出現偽陽性（讀到另一個實例還在
/// 執行中的 `clean:false`）與偽陰性（另一個實例正常結束、把檔案覆寫成
/// `clean:true`，蓋掉自己這邊真正的 crash 記錄）。本 repo 的開發流程就是
/// `bun run tauri:dev`，這個情境並不罕見。要修的話需要 per-instance 的 marker
/// （例如以 host_pid 或安裝路徑做檔名），超出 #40 的範圍。
pub fn previous_run_unclean(path: &Path) -> bool {
    matches!(read_run_state(path), Some(state) if !state.clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_is_stable_for_the_whole_process() {
        // AC 第 1 條的前提：同一次執行的每一筆 record 必須共用同一個 run_id。
        assert_eq!(current_run_id(), current_run_id());
        assert_eq!(current().run_id, current_run_id());
    }

    #[test]
    fn run_context_reports_this_process() {
        let context = current();
        assert_eq!(context.host_pid, std::process::id());
        assert_eq!(context.platform, std::env::consts::OS);
        assert_eq!(context.app_version, env!("CARGO_PKG_VERSION"));
        assert!(!context.app_version.is_empty());
    }

    #[test]
    fn generated_run_ids_differ_between_runs() {
        // 兩次「執行」（= 兩次產生）必須不同，否則 restart 後的 agent-1 仍無法區分。
        assert_ne!(generate_run_id(), generate_run_id());
    }

    #[test]
    fn run_id_shape_is_redaction_safe() {
        let id = generate_run_id();
        assert_eq!(id.len(), 25, "run_id 必須短於 32（高熵字串門檻）：{id}");
        assert!(
            !id.contains(':') && !id.contains('/') && !id.contains('\\') && !id.contains('.'),
            "run_id 不得含會觸發 fingerprint／path／IP scrubber 的字元：{id}"
        );
        let (timestamp, nonce) = id.split_once('-').expect("run_id 以 '-' 分成兩段");
        assert_eq!(timestamp.len(), 16);
        assert_eq!(nonce.len(), 8);
        assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn format_run_id_pads_small_nonces() {
        assert_eq!(
            format_run_id("20260725T101530Z", 0x1f),
            "20260725T101530Z-0000001f"
        );
    }

    #[test]
    fn previous_run_unclean_is_false_without_a_marker_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("run-state.json");
        assert!(!previous_run_unclean(&path));
    }

    #[test]
    fn previous_run_unclean_is_true_after_a_run_that_never_marked_itself_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("run-state.json");
        // 上一次執行：啟動時寫 clean=false，然後就沒有下文（crash）。
        write_run_state(&path, "run-a", false).unwrap();
        assert!(previous_run_unclean(&path));
    }

    #[test]
    fn previous_run_unclean_is_false_after_a_graceful_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("run-state.json");
        write_run_state(&path, "run-a", false).unwrap();
        write_run_state(&path, "run-a", true).unwrap();
        assert!(!previous_run_unclean(&path));
        assert_eq!(
            read_run_state(&path).unwrap(),
            RunState {
                run_id: "run-a".into(),
                clean: true
            }
        );
    }

    #[test]
    fn previous_run_unclean_is_false_for_a_corrupt_marker_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("run-state.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(read_run_state(&path).is_none());
        assert!(!previous_run_unclean(&path));
    }

    #[test]
    fn write_run_state_creates_missing_parent_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("run-state.json");
        write_run_state(&path, "run-a", false).unwrap();
        assert!(previous_run_unclean(&path));
    }
}
