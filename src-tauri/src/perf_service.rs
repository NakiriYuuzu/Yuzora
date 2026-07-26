// F1 performance monitor: sample the app process *and* every descendant it owns
// (ACP wrappers, Pi/Claude agents, terminal shells, LSP servers, …).
//
// A persistent `System` lives in Tauri managed state so successive
// `perf_snapshot` calls (driven by the 2s frontend poll) are spaced far enough
// apart to satisfy sysinfo's minimum CPU update interval.
//
// 聚合方式走 OS 的 parent → child 關係（#22），而不是各子系統的 registry：
// Yuzora 的子行程分散在五個 module，其中 ACP 與 process_service 沒有常駐可讀的
// pid 欄位；而且 Pi 這類 agent 是「wrapper → 真正的 agent」兩層結構，registry
// 只知道第一層。改由 process table 做 BFS 可以自動涵蓋孫層以下，也不必動任何
// 子系統。
//
// 已知限制（best-effort，本設計不處理）：Windows 上若子行程被 re-parent（例如
// conhost 的特殊情形）就會逸出這棵樹；macOS 的 WKWebView WebContent/GPU helper
// 住在 XPC 之下，同樣不是 Yuzora 的 descendant，因此不計入。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use sysinfo::{get_current_pid, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// The persistent `System`, refreshed once per `perf_snapshot`. CPU% is a delta
/// from the previous refresh, so the first sample after startup reads 0.
pub struct PerfState(pub Mutex<System>);

#[derive(serde::Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PerfSnapshot {
    /// App 本體 + 所有 Yuzora-owned descendants 的總和。
    ///
    /// CPU 沿用 sysinfo 的既有語意：單一 process 的 `cpu_usage()` 是相對於
    /// 「一顆核心」的百分比，可超過 100；總量定義為所有成員該值的**算術和**，
    /// 因此在多核心機器上可以超過 100 × 核心數。刻意不除以核心數，以免改動既有
    /// 顯示值的量級。
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    /// 只有 host process 自己——UI 需要能區分「App 本體」與總量。
    pub app_cpu_percent: f32,
    pub app_memory_bytes: u64,
    /// 被計入的 descendant 數（0 = 只有 App 本體）。
    pub descendant_count: u32,
    /// WebView renderer/GPU helper 的小計（#40 §3.3）。Windows 上是
    /// `msedgewebview2.exe` 那組，macOS 上是 `com.apple.WebKit.*` helper。
    pub webview_cpu_percent: f32,
    pub webview_memory_bytes: u64,
    pub webview_count: u32,
    /// #22 定義的 Yuzora-owned descendants **扣掉** webview 那組：ACP wrapper、
    /// agent、terminal shell、LSP server⋯，以及分類不出來的 process。
    ///
    /// 不變式：`app + webview + managed_tools == 總量`、
    /// `webview_count + managed_tools_count == descendant_count`。分類判斷不到的
    /// process 落在 managed_tools，因此**分類失敗不會讓總量失真**。
    pub managed_tools_cpu_percent: f32,
    pub managed_tools_memory_bytes: u64,
    pub managed_tools_count: u32,
}

/// descendant 的分類結果。`Unknown` 不是第三個小計桶——它會被算進
/// `managed_tools`（見 `PerfSnapshot` 的不變式）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessClass {
    Webview,
    Unknown,
}

/// WebView renderer/GPU helper 的 process name 標記（小寫比對）。
///
/// macOS 的 WKWebView helper 住在 XPC 之下、不是 Yuzora 的 descendant（見檔頭
/// 的已知限制），所以在 macOS 上這組通常一個都比對不到——分類因此是 best-effort，
/// 比對不到就是 `Unknown`、落進 managed_tools，總量不受影響。
const WEBVIEW_NAME_MARKERS: [&str; 4] = [
    // Windows：WebView2 的 renderer/GPU/utility 全部叫這個名字
    "msedgewebview2",
    // macOS
    "com.apple.webkit.webcontent",
    "com.apple.webkit.gpu",
    "wkwebview",
];

/// 純函式：只看 process name 判斷是不是 WebView helper。
pub fn classify_process(name: &str) -> ProcessClass {
    let lower = name.to_ascii_lowercase();
    if WEBVIEW_NAME_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return ProcessClass::Webview;
    }
    ProcessClass::Unknown
}

/// 從 process table 取出的一列，只留聚合需要的四個欄位。抽成獨立 struct 是為了
/// 讓 `aggregate_tree` 成為純函式，測試不必真的開行程。
#[derive(Clone, Debug, PartialEq)]
struct ProcessRow {
    pid: Pid,
    parent: Option<Pid>,
    cpu_percent: f32,
    memory_bytes: u64,
    /// OS 回報的 process name，只用於 `classify_process`。
    name: String,
}

/// 從 `rows` 聚合 `root` 的整棵子樹；`root` 不在 rows 裡時回 `None`。
///
/// BFS 搭配 visited set，因此同一個 pid 只會被計入一次（重複列、parent 指回
/// 祖先的環狀關係都不會重複計算或無限迴圈）。
fn aggregate_tree(rows: &[ProcessRow], root: Pid) -> Option<PerfSnapshot> {
    let mut children: HashMap<Pid, Vec<&ProcessRow>> = HashMap::new();
    let mut root_row: Option<&ProcessRow> = None;
    for row in rows {
        if row.pid == root && root_row.is_none() {
            root_row = Some(row);
        }
        if let Some(parent) = row.parent {
            children.entry(parent).or_default().push(row);
        }
    }
    let root_row = root_row?;

    let mut visited: HashSet<Pid> = HashSet::new();
    visited.insert(root);
    let mut queue: VecDeque<Pid> = VecDeque::new();
    queue.push_back(root);

    let mut cpu_percent = root_row.cpu_percent;
    let mut memory_bytes = root_row.memory_bytes;
    let mut descendant_count: u32 = 0;
    let mut webview_cpu_percent = 0.0f32;
    let mut webview_memory_bytes = 0u64;
    let mut webview_count: u32 = 0;

    while let Some(pid) = queue.pop_front() {
        let Some(kids) = children.get(&pid) else {
            continue;
        };
        for kid in kids {
            if !visited.insert(kid.pid) {
                continue;
            }
            cpu_percent += kid.cpu_percent;
            memory_bytes = memory_bytes.saturating_add(kid.memory_bytes);
            descendant_count = descendant_count.saturating_add(1);
            // 只分出 webview 一組，managed_tools 由「總量 − app − webview」導出：
            // 兩個小計因此在算術上永遠加得回總量，分類判錯也只會讓某個 process
            // 換一個桶，不會讓總量失真。
            if classify_process(&kid.name) == ProcessClass::Webview {
                webview_cpu_percent += kid.cpu_percent;
                webview_memory_bytes = webview_memory_bytes.saturating_add(kid.memory_bytes);
                webview_count = webview_count.saturating_add(1);
            }
            queue.push_back(kid.pid);
        }
    }

    Some(PerfSnapshot {
        cpu_percent,
        memory_bytes,
        app_cpu_percent: root_row.cpu_percent,
        app_memory_bytes: root_row.memory_bytes,
        descendant_count,
        webview_cpu_percent,
        webview_memory_bytes,
        webview_count,
        managed_tools_cpu_percent: cpu_percent - root_row.cpu_percent - webview_cpu_percent,
        managed_tools_memory_bytes: memory_bytes
            .saturating_sub(root_row.memory_bytes)
            .saturating_sub(webview_memory_bytes),
        managed_tools_count: descendant_count.saturating_sub(webview_count),
    })
}

/// Refresh 整張 process table 並映射成 `ProcessRow`。
///
/// `ProcessesToUpdate::All` 比單一 PID 昂貴，但只在 2 秒一次的 poll 執行，而且
/// refresh kind 僅要 cpu + memory（不取 cmdline/env）。第二個參數 `true` 會把已
/// 結束的 process 移出 table，因此 process exit 後下一次 sample 自然不再計入。
fn collect_rows(system: &mut System) -> Vec<ProcessRow> {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessRow {
            pid: *pid,
            parent: process.parent(),
            cpu_percent: process.cpu_usage(),
            memory_bytes: process.memory(),
            name: process.name().to_string_lossy().into_owned(),
        })
        .collect()
}

fn sample(system: &mut System, pid: Pid) -> Option<PerfSnapshot> {
    let rows = collect_rows(system);
    aggregate_tree(&rows, pid)
}

/// 必須是 `async`：sysinfo 在 macOS 上對每個 process 無條件執行
/// `sysctl(KERN_PROCARGS2)`（把完整 argv + environ 複製進 userspace），
/// `ProcessRefreshKind` 只決定「要不要留下來」、擋不住「讀取」，所以
/// `ProcessesToUpdate::All` 一次約 10 ms（本機 652 個 process 實測 8.2–10.4 ms，
/// 單一 PID 版本只要 0.15–0.23 ms）。同步 command 走 tauri 的 Blocking
/// execution context，在 macOS/wry 下 inline 跑在主執行緒，等於每 2 秒吃掉超過
/// 半個 60 fps frame budget 並全程持有 `PerfState` 的 Mutex；標成 async 才會被
/// 丟到 async runtime 的執行緒上。
#[tauri::command(async)]
pub fn perf_snapshot(state: tauri::State<'_, PerfState>) -> Result<Option<PerfSnapshot>, String> {
    let pid = get_current_pid().map_err(|e| e.to_string())?;
    let mut system = state.0.lock().map_err(|e| e.to_string())?;
    Ok(sample(&mut system, pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, parent: Option<u32>, cpu: f32, memory: u64) -> ProcessRow {
        named_row(pid, parent, cpu, memory, "")
    }

    fn named_row(pid: u32, parent: Option<u32>, cpu: f32, memory: u64, name: &str) -> ProcessRow {
        ProcessRow {
            pid: Pid::from_u32(pid),
            parent: parent.map(Pid::from_u32),
            cpu_percent: cpu,
            memory_bytes: memory,
            name: name.to_string(),
        }
    }

    /// 兩個小計必須永遠加得回總量——分類是否判對都一樣（#40 §3.3）。
    fn assert_totals_are_conserved(snapshot: &PerfSnapshot) {
        assert_eq!(
            snapshot.app_memory_bytes
                + snapshot.webview_memory_bytes
                + snapshot.managed_tools_memory_bytes,
            snapshot.memory_bytes,
            "memory 小計必須加得回總量"
        );
        assert!(
            (snapshot.app_cpu_percent
                + snapshot.webview_cpu_percent
                + snapshot.managed_tools_cpu_percent
                - snapshot.cpu_percent)
                .abs()
                < 1e-4,
            "cpu 小計必須加得回總量"
        );
        assert_eq!(
            snapshot.webview_count + snapshot.managed_tools_count,
            snapshot.descendant_count,
            "分類計數必須加得回 descendant_count"
        );
    }

    /// 起一個活得夠久、會被測試自己殺掉的子行程。CI 的 `cargo test` 只在 macOS
    /// 跑，Windows 分支只需要能編譯（`cargo check --all-targets` 三平台都跑）。
    ///
    /// Windows 側刻意**不經過 `cmd /C`**：那樣直接子行程會是 cmd.exe、ping 變成
    /// 孫層，而 `Child::kill()` 是 `TerminateProcess`（只殺單一 process、不殺整
    /// 棵樹），測試會留下活 60 秒的孤兒 ping.exe。直接 spawn ping 才殺得掉。
    fn spawn_child() -> std::process::Child {
        #[cfg(windows)]
        {
            std::process::Command::new("ping")
                .args(["-n", "60", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .expect("spawn child process")
        }
        #[cfg(not(windows))]
        {
            std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("spawn child process")
        }
    }

    #[test]
    fn sample_reports_memory_and_finite_cpu_for_own_process() {
        let pid = get_current_pid().unwrap();
        let mut system = System::new();
        // First refresh primes the CPU delta baseline; the second, after a delay
        // past the minimum update interval, yields a real percentage.
        let _ = sample(&mut system, pid);
        std::thread::sleep(std::time::Duration::from_millis(250));
        let snapshot = sample(&mut system, pid).expect("own process is alive");
        assert!(snapshot.memory_bytes > 0, "memory should be > 0");
        // `>= 0.0` is false for NaN, so this also rules out a garbage CPU reading.
        assert!(
            snapshot.cpu_percent >= 0.0,
            "cpu should be non-negative and finite"
        );
    }

    #[test]
    fn sample_returns_none_for_missing_pid() {
        let mut system = System::new();
        // A PID that is overwhelmingly unlikely to exist must return None, not panic.
        assert!(sample(&mut system, Pid::from_u32(u32::MAX)).is_none());
    }

    #[test]
    fn aggregate_tree_returns_none_when_root_is_absent() {
        let rows = [row(1, None, 1.0, 100)];
        assert!(aggregate_tree(&rows, Pid::from_u32(999)).is_none());
    }

    #[test]
    fn aggregate_tree_reports_zero_descendants_for_a_lone_process() {
        // parent-only：總量必須等於 app 自身，descendant_count = 0。
        let rows = [row(10, Some(1), 12.5, 4_096), row(20, Some(1), 99.0, 8_192)];
        let snapshot = aggregate_tree(&rows, Pid::from_u32(10)).expect("root is present");
        assert_eq!(
            snapshot,
            PerfSnapshot {
                cpu_percent: 12.5,
                memory_bytes: 4_096,
                app_cpu_percent: 12.5,
                app_memory_bytes: 4_096,
                descendant_count: 0,
                webview_cpu_percent: 0.0,
                webview_memory_bytes: 0,
                webview_count: 0,
                managed_tools_cpu_percent: 0.0,
                managed_tools_memory_bytes: 0,
                managed_tools_count: 0,
            }
        );
    }

    #[test]
    fn aggregate_tree_sums_grandchildren() {
        // 10 → 20 → 30，另有一棵無關的樹（40 → 41）不得被計入。
        // 對應 Pi 的「ACP wrapper → 真正的 agent」兩層結構。
        let rows = [
            row(10, Some(1), 1.0, 100),
            row(20, Some(10), 2.0, 200),
            row(30, Some(20), 4.0, 400),
            row(40, Some(1), 8.0, 800),
            row(41, Some(40), 16.0, 1_600),
        ];
        let snapshot = aggregate_tree(&rows, Pid::from_u32(10)).expect("root is present");
        assert_eq!(snapshot.descendant_count, 2);
        assert_eq!(snapshot.memory_bytes, 700);
        assert_eq!(snapshot.app_memory_bytes, 100);
        // AC 第 5 條：總量就是各成員 cpu_usage() 的算術和。
        assert_eq!(snapshot.cpu_percent, 1.0 + 2.0 + 4.0);
        assert_eq!(snapshot.app_cpu_percent, 1.0);
    }

    #[test]
    fn aggregate_tree_counts_each_pid_once() {
        // 同一個 pid 出現兩列（例如 process table 重複），只能計一次。
        let rows = [
            row(10, Some(1), 1.0, 100),
            row(20, Some(10), 2.0, 200),
            row(20, Some(10), 2.0, 200),
        ];
        let snapshot = aggregate_tree(&rows, Pid::from_u32(10)).expect("root is present");
        assert_eq!(snapshot.descendant_count, 1);
        assert_eq!(snapshot.memory_bytes, 300);
    }

    #[test]
    fn aggregate_tree_terminates_on_a_parent_cycle() {
        // 10 → 20 → 30 → 10：最後一段指回 root。visited set 必須讓 BFS 收斂，
        // 且 root 不會被重複計入。
        let rows = [
            row(10, Some(30), 1.0, 100),
            row(20, Some(10), 2.0, 200),
            row(30, Some(20), 4.0, 400),
        ];
        let snapshot = aggregate_tree(&rows, Pid::from_u32(10)).expect("root is present");
        assert_eq!(snapshot.descendant_count, 2);
        assert_eq!(snapshot.memory_bytes, 700);
    }

    #[test]
    fn aggregate_tree_drops_a_process_that_has_exited() {
        // 每次 sample 都重建整棵樹，因此 process exit（= 從 process table 消失）
        // 之後，同一個 root 的總量自然回落。
        let alive = [row(10, Some(1), 1.0, 100), row(20, Some(10), 2.0, 200)];
        let exited = [row(10, Some(1), 1.0, 100)];
        let before = aggregate_tree(&alive, Pid::from_u32(10)).expect("root is present");
        let after = aggregate_tree(&exited, Pid::from_u32(10)).expect("root is present");
        assert_eq!(before.descendant_count, 1);
        assert_eq!(after.descendant_count, 0);
        assert_eq!(after.memory_bytes, after.app_memory_bytes);
    }

    #[test]
    fn classify_process_recognises_windows_webview2_helpers() {
        // 原症狀（issue #40）：6 個 WebView descendants 合計約 542 MB。
        assert_eq!(
            classify_process("msedgewebview2.exe"),
            ProcessClass::Webview
        );
        assert_eq!(
            classify_process("MSEdgeWebView2.exe"),
            ProcessClass::Webview
        );
    }

    #[test]
    fn classify_process_recognises_macos_webkit_helpers() {
        for name in [
            "com.apple.WebKit.WebContent",
            "com.apple.WebKit.WebContent.Development",
            "com.apple.WebKit.GPU",
            "WKWebView",
        ] {
            assert_eq!(
                classify_process(name),
                ProcessClass::Webview,
                "{name} 應被判為 webview"
            );
        }
    }

    #[test]
    fn classify_process_leaves_yuzora_managed_tools_unclassified() {
        // Yuzora 自己的子行程與其他不相干的 process 都不是 webview。
        for name in [
            "",
            "node",
            "bun",
            "pi",
            "claude",
            "zsh",
            "rust-analyzer",
            "git",
            "msedge.exe",
            "com.apple.WebKit.Networking",
        ] {
            assert_eq!(
                classify_process(name),
                ProcessClass::Unknown,
                "{name} 不應被判為 webview"
            );
        }
    }

    #[test]
    fn aggregate_tree_splits_descendants_into_webview_and_managed_tools() {
        // 10 = host；20/21 = WebView renderer + GPU；30 = ACP wrapper、31 = 真正的
        // agent（孫層），兩者都歸 managed_tools。
        let rows = [
            named_row(10, Some(1), 1.0, 100, "yuzora"),
            named_row(20, Some(10), 2.0, 200, "msedgewebview2.exe"),
            named_row(21, Some(10), 4.0, 400, "msedgewebview2.exe"),
            named_row(30, Some(10), 8.0, 800, "node"),
            named_row(31, Some(30), 16.0, 1_600, "pi"),
        ];
        let snapshot = aggregate_tree(&rows, Pid::from_u32(10)).expect("root is present");
        assert_eq!(snapshot.descendant_count, 4);
        assert_eq!(snapshot.webview_count, 2);
        assert_eq!(snapshot.webview_memory_bytes, 600);
        assert_eq!(snapshot.webview_cpu_percent, 6.0);
        assert_eq!(snapshot.managed_tools_count, 2);
        assert_eq!(snapshot.managed_tools_memory_bytes, 2_400);
        assert_eq!(snapshot.managed_tools_cpu_percent, 24.0);
        assert_totals_are_conserved(&snapshot);
    }

    #[test]
    fn aggregate_tree_keeps_totals_exact_when_nothing_can_be_classified() {
        // macOS 上 WKWebView helper 不在 Yuzora 的 process tree 裡，分類會全數落空。
        // 這種情況下總量仍必須完全等於 #22 的聚合結果——分類失敗不得讓總量失真。
        let rows = [
            row(10, Some(1), 1.0, 100),
            row(20, Some(10), 2.0, 200),
            row(30, Some(20), 4.0, 400),
        ];
        let snapshot = aggregate_tree(&rows, Pid::from_u32(10)).expect("root is present");
        assert_eq!(snapshot.memory_bytes, 700);
        assert_eq!(snapshot.cpu_percent, 7.0);
        assert_eq!(snapshot.webview_count, 0);
        assert_eq!(snapshot.webview_memory_bytes, 0);
        assert_eq!(snapshot.managed_tools_count, 2);
        assert_eq!(snapshot.managed_tools_memory_bytes, 600);
        assert_totals_are_conserved(&snapshot);
    }

    #[test]
    fn aggregate_tree_classifies_webview_helpers_nested_under_a_broker() {
        // Windows 的 WebView2 是「broker → renderer/GPU」兩層；孫層的 helper 必須
        // 也被算進 webview 小計，broker 本身留在 managed_tools。
        let rows = [
            named_row(10, Some(1), 1.0, 100, "yuzora.exe"),
            named_row(20, Some(10), 2.0, 200, "msedgewebview2.exe"),
            named_row(21, Some(20), 4.0, 400, "msedgewebview2.exe"),
        ];
        let snapshot = aggregate_tree(&rows, Pid::from_u32(10)).expect("root is present");
        assert_eq!(snapshot.webview_count, 2);
        assert_eq!(snapshot.webview_memory_bytes, 600);
        assert_eq!(snapshot.managed_tools_count, 0);
        assert_totals_are_conserved(&snapshot);
    }

    #[test]
    fn collect_rows_populates_process_names() {
        // 分類完全依賴 name；若 refresh kind 沒帶回 name，整個 §3.3 就是死碼。
        let pid = get_current_pid().unwrap();
        let mut system = System::new();
        let rows = collect_rows(&mut system);
        let own = rows
            .iter()
            .find(|row| row.pid == pid)
            .expect("own process is in the table");
        assert!(
            !own.name.is_empty(),
            "process name must be populated by the cpu+memory refresh kind"
        );
    }

    #[test]
    fn sample_counts_a_real_child_process_and_drops_it_after_exit() {
        let pid = get_current_pid().unwrap();
        let mut system = System::new();
        // 先 prime 一次 CPU delta baseline。
        let _ = sample(&mut system, pid);

        let mut child = spawn_child();
        let child_pid = Pid::from_u32(child.id());
        std::thread::sleep(std::time::Duration::from_millis(300));

        let rows_alive = collect_rows(&mut system);
        let sampled = sample(&mut system, pid);

        // 先收掉子行程再斷言：`Child` 的 Drop 不會 kill，任何 panic 都會讓
        // `sleep 60` 活得比測試行程久。
        let _ = child.kill();
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let rows_after = collect_rows(&mut system);

        let snapshot = sampled.expect("own process is alive");

        // 直接子行程必須出現在 process table，且 parent 就是測試行程本身。
        assert!(
            rows_alive
                .iter()
                .any(|r| r.pid == child_pid && r.parent == Some(pid)),
            "spawned child should be visible as a direct child of the test process"
        );
        // 其他測試可能同時開行程，所以只斷言「至少一個」而非精確數量。
        assert!(
            snapshot.descendant_count >= 1,
            "descendant_count should count the spawned child, got {}",
            snapshot.descendant_count
        );
        assert!(
            snapshot.memory_bytes > snapshot.app_memory_bytes,
            "total memory {} should exceed app-only memory {}",
            snapshot.memory_bytes,
            snapshot.app_memory_bytes
        );
        // exit 後下一次 refresh 就把它移出 process table，用量隨之消失。
        assert!(
            !rows_after.iter().any(|r| r.pid == child_pid),
            "exited child should be gone from the next refresh"
        );
    }
}
