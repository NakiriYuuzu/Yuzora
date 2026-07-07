//! GUI 啟動期 PATH 修正。
//!
//! macOS 從 Finder/Dock 啟動的 .app 只繼承 launchd 的預設 PATH
//! （`/usr/bin:/bin:/usr/sbin:/sbin`），拿不到 homebrew／nvm／bun 等安裝在使用者
//! rc 檔裡的路徑。這會讓 `lsp_service::which`、`lsp_download` 的 npm/python 偵測、
//! 以及 agent spawn 全部找不到工具。此模組在啟動最早期跑一次使用者的登入互動
//! shell，撈出真正的 PATH 併回本行程環境。

/// 從使用者的登入互動 shell 取回 PATH，併入本行程環境。unix 專用；其他平台 no-op。
///
/// 必須在任何執行緒 spawn 之前呼叫（見 `lib.rs` 呼叫點）：此函式會 `set_var("PATH")`，
/// 在單執行緒階段修改行程環境才安全。
#[cfg(unix)]
pub fn fix_gui_path() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    // `-i`（interactive）才會讀 ~/.zshrc；bun 之類的 PATH 常只在 rc 檔設定，login
    // 非互動 shell 讀不到——這正是要點。`-l` 讀 profile、`-c` 執行單行。marker 包夾
    // $PATH，讓我們能從 rc 檔的雜訊輸出裡精準切出值。
    let mut child = match Command::new(&shell)
        .args(["-ilc", "printf '__YZ_PATH_S__%s__YZ_PATH_E__' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("[env_path] shell spawn 失敗，保留原 PATH: {e}");
            log_path_fix(
                "warn",
                &format!("shell spawn failed, PATH unchanged: {e}"),
                None,
            );
            return;
        }
    };

    // Timeout 防呆：try_wait 輪詢（50ms 間隔，上限 5s），避免 rc 檔卡住（例如互動
    // prompt、等 stdin）時永久 block。用 `.output()` 會無限等待，所以手動輪詢。
    let deadline = Instant::now() + Duration::from_secs(5);
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break true,
            Ok(None) => {
                if Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                eprintln!("[env_path] shell try_wait 失敗，保留原 PATH: {e}");
                log_path_fix(
                    "warn",
                    &format!("shell try_wait failed, PATH unchanged: {e}"),
                    None,
                );
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    };

    if !exited {
        eprintln!("[env_path] shell 逾時（5s），kill 並保留原 PATH");
        log_path_fix("warn", "login shell timed out (5s), PATH unchanged", None);
        let _ = child.kill();
        let _ = child.wait();
        return;
    }

    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        if let Err(e) = pipe.read_to_string(&mut stdout) {
            eprintln!("[env_path] 讀取 shell stdout 失敗，保留原 PATH: {e}");
            log_path_fix(
                "warn",
                &format!("reading shell stdout failed, PATH unchanged: {e}"),
                None,
            );
            return;
        }
    }

    let shell_path = match extract_marked(&stdout) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => {
            eprintln!("[env_path] shell 未回傳有效 PATH，保留原 PATH");
            log_path_fix(
                "warn",
                "login shell returned no usable PATH, PATH unchanged",
                None,
            );
            return;
        }
    };

    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_paths(&shell_path, &current);
    let count = merged.split(':').filter(|e| !e.is_empty()).count();
    std::env::set_var("PATH", &merged);
    eprintln!("[env_path] PATH 已從登入 shell 修正，共 {count} 個 entry");
    log_path_fix(
        "info",
        &format!("PATH merged from login shell ({count} entries)"),
        Some(count),
    );
}

/// PATH 修正結果寫入 log 系統（source: env）——歷史上 exit 127（bunx not found）
/// 只能靠 app stderr 猜測，事後無從查證，故成敗都留紀錄。
#[cfg(unix)]
fn log_path_fix(level: &str, message: &str, entries: Option<usize>) {
    crate::logging::write_global(crate::logging::LogEvent {
        level: level.to_string(),
        kind: "debug".to_string(),
        source: "env".to_string(),
        workspace_path: None,
        event: "path_fix".to_string(),
        message: message.to_string(),
        metadata: match entries {
            Some(count) => serde_json::json!({ "entries": count }),
            None => serde_json::json!({}),
        },
    });
}

/// 非 unix 平台無此問題（Windows GUI 行程正常繼承使用者 PATH），no-op。
#[cfg(not(unix))]
pub fn fix_gui_path() {}

/// 從 shell 輸出裡切出第一組 `__YZ_PATH_S__` … `__YZ_PATH_E__` 之間的內容。
/// rc 檔可能在 marker 前後印雜訊，所以定位 marker 而非整段解析。找不到（或缺尾
/// marker）回 `None`；marker 內為空字串則回 `Some("")`，由呼叫端視為失敗。
#[cfg(any(unix, test))]
fn extract_marked(output: &str) -> Option<&str> {
    const START: &str = "__YZ_PATH_S__";
    const END: &str = "__YZ_PATH_E__";
    let start = output.find(START)? + START.len();
    let rest = &output[start..];
    let end = rest.find(END)?;
    Some(&rest[..end])
}

/// 合併 shell PATH 與原 PATH：shell 的 entries 在前，原 PATH 中不重複的附加在後。
/// 保序去重（保留各 entry 首次出現的位置），忽略空 entry，用 `:` 連接。
#[cfg(any(unix, test))]
fn merge_paths(shell_path: &str, current_path: &str) -> String {
    use std::collections::HashSet;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<&str> = Vec::new();
    for entry in shell_path
        .split(':')
        .chain(current_path.split(':'))
        .filter(|e| !e.is_empty())
    {
        if seen.insert(entry) {
            out.push(entry);
        }
    }
    out.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_marked_ignores_noise_before_and_after() {
        let out = "rc noise\nsome banner\n__YZ_PATH_S__/opt/homebrew/bin:/usr/bin__YZ_PATH_E__trailing junk\n";
        assert_eq!(extract_marked(out), Some("/opt/homebrew/bin:/usr/bin"));
    }

    #[test]
    fn extract_marked_returns_none_when_marker_missing() {
        assert_eq!(extract_marked("no markers here at all"), None);
        // start present but end missing
        assert_eq!(extract_marked("__YZ_PATH_S__/usr/bin but no end"), None);
    }

    #[test]
    fn extract_marked_empty_value_is_some_empty() {
        assert_eq!(extract_marked("__YZ_PATH_S____YZ_PATH_E__"), Some(""));
    }

    #[test]
    fn merge_paths_shell_first_then_unique_original_preserving_order() {
        let shell = "/opt/homebrew/bin:/Users/me/.bun/bin:/usr/bin";
        let current = "/usr/bin:/bin:/usr/sbin";
        assert_eq!(
            merge_paths(shell, current),
            "/opt/homebrew/bin:/Users/me/.bun/bin:/usr/bin:/bin:/usr/sbin"
        );
    }

    #[test]
    fn merge_paths_dedups_and_skips_empty_entries() {
        let shell = "/a:/b:/a"; // internal dup
        let current = ":/b:/c:"; // leading/trailing empty + dup /b
        assert_eq!(merge_paths(shell, current), "/a:/b:/c");
    }
}
