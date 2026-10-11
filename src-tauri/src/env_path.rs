//! GUI 啟動期 PATH／env 修正。
//!
//! macOS 從 Finder/Dock 啟動的 .app 只繼承 launchd 的預設 PATH
//! （`/usr/bin:/bin:/usr/sbin:/sbin`）與極簡 env，拿不到使用者 rc 檔（`~/.zshrc`）
//! export 的東西：PATH 上的 homebrew／nvm／bun，以及 agent 需要的憑證 env
//! （例如 pi 的 `GEOSENSE_API_KEY`／`YUUZU_API_KEY`）。前者讓工具找不到；後者讓
//! agent 子行程 `session/new` 回 "Authentication required"（release 的 Agent-Zone
//! 掛掉的根因）。此模組在啟動最早期跑一次使用者的登入互動 shell，撈回真正的 PATH
//! 併入本行程，並把 rc 檔 export、但 GUI 行程還缺的 env 以 fill-if-missing 補齊。

/// 從使用者的登入互動 shell 取回 PATH（併入本行程）與其餘 export 的 env
/// （只補本行程還沒有的，不覆蓋既有值）。unix 專用；其他平台 no-op。
///
/// 必須在任何執行緒 spawn 之前呼叫（見 `lib.rs` 呼叫點）：此函式會 `set_var`，
/// 在單執行緒階段修改行程環境才安全。
#[cfg(unix)]
pub fn fix_gui_path() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    // `-i`（interactive）才會讀 ~/.zshrc；bun 的 PATH 與 pi 的憑證 env 常只在 rc 檔
    // 設定，login 非互動 shell 讀不到——這正是要點。`-l` 讀 profile、`-c` 執行單行。
    // 輸出兩段用 marker 包夾，能從 rc 檔的雜訊輸出裡精準切出：PATH（`printf $PATH`）與
    // 整包 env（`awk ENVIRON` 以 `%c,0` 印成 `KEY=VALUE\0…`，NUL 分隔耐值裡的特殊字元）。
    let mut child = match Command::new(&shell)
        .args([
            "-ilc",
            "printf '__YZ_PATH_S__%s__YZ_PATH_E__' \"$PATH\"; \
             awk 'BEGIN{printf \"__YZ_ENV_S__\"; \
             for (k in ENVIRON) printf \"%s=%s%c\", k, ENVIRON[k], 0; \
             printf \"__YZ_ENV_E__\"}'",
        ])
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

    // Keep the bounded wait, but let a macOS child exit wake it immediately.
    // Unsupported platforms or a failed registration retain the 50ms fallback.
    #[cfg(target_os = "macos")]
    let mut exit_notification = shell_exit_notification(child.id());

    // Timeout 防呆：每次等待上限 50ms、總上限 5s，避免 rc 檔卡住時永久 block。
    let deadline = Instant::now() + Duration::from_secs(5);
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break true,
            Ok(None) => {
                if Instant::now() >= deadline {
                    break false;
                }
                #[cfg(target_os = "macos")]
                wait_for_shell_exit(&mut exit_notification, Duration::from_millis(50));
                #[cfg(not(target_os = "macos"))]
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

    // 讀成 bytes 再 lossy 轉字串：env 值理論上可能含非 UTF-8 位元組，若用
    // read_to_string 會整個 Err 而連 PATH 修正也一起放棄。lossy 保留 NUL 與 marker
    // （皆 ASCII），只把個別壞位元組換成替代字元，不影響純 ASCII 的 API key。
    let mut raw = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        if let Err(e) = pipe.read_to_end(&mut raw) {
            eprintln!("[env_path] 讀取 shell stdout 失敗，保留原 PATH: {e}");
            log_path_fix(
                "warn",
                &format!("reading shell stdout failed, PATH unchanged: {e}"),
                None,
            );
            return;
        }
    }
    let stdout = String::from_utf8_lossy(&raw);

    // ── PATH：marker 切值 + 保序去重併回 ──
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

    // ── 憑證/其他 env：補進 rc 檔 export、但本行程還沒有的變數 ──
    // fill-if-missing：只補「本行程缺」的（release 的 .app 缺 GEOSENSE_API_KEY 等），
    // 絕不覆蓋 GUI 已有的值（dev 從終端機起、值已正確）。PATH 另循上方合併故排除，
    // 其餘 shell 專屬噪音（cwd、SHLVL…）由 should_import_env 濾掉。
    if let Some(section) = extract_between(&stdout, ENV_START, ENV_END) {
        let mut imported: Vec<String> = Vec::new();
        for (key, value) in parse_env_dump(section) {
            if !should_import_env(&key) || std::env::var_os(&key).is_some() {
                continue;
            }
            std::env::set_var(&key, &value);
            imported.push(key);
        }
        imported.sort();
        if !imported.is_empty() {
            eprintln!(
                "[env_path] 從登入 shell 補入 {} 個缺漏 env：{}",
                imported.len(),
                imported.join(", ")
            );
        }
        // 只記數量與變數「名稱」（絕不記值，避免外洩憑證）。
        crate::logging::write_global(crate::logging::LogEvent {
            level: "info".to_string(),
            kind: "debug".to_string(),
            source: "env".to_string(),
            workspace_path: None,
            event: "env_import".to_string(),
            message: format!(
                "imported {} missing env var(s) from login shell",
                imported.len()
            ),
            metadata: serde_json::json!({ "count": imported.len(), "keys": imported }),
        });
    }
}

#[cfg(target_os = "macos")]
fn shell_exit_notification(pid: u32) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // The queue is private to this wait and OwnedFd closes it on every return.
    let fd = unsafe { libc::kqueue() };
    if fd < 0 {
        return None;
    }
    let queue = unsafe { OwnedFd::from_raw_fd(fd) };
    let event = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // Register only the child owned by fix_gui_path. A child that has already
    // exited can reject registration; the following try_wait still reaps it.
    let registered = unsafe {
        libc::kevent(
            queue.as_raw_fd(),
            &event,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    };
    (registered == 0).then_some(queue)
}

#[cfg(target_os = "macos")]
fn wait_for_shell_exit(queue: &mut Option<std::os::fd::OwnedFd>, duration: std::time::Duration) {
    use std::os::fd::AsRawFd;
    if let Some(notification) = queue.as_ref() {
        let timeout = libc::timespec {
            tv_sec: duration.as_secs() as libc::time_t,
            tv_nsec: duration.subsec_nanos() as libc::c_long,
        };
        let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
        // The descriptor remains owned and the output buffer has one full slot.
        let result = unsafe {
            libc::kevent(
                notification.as_raw_fd(),
                std::ptr::null(),
                0,
                event.as_mut_ptr(),
                1,
                &timeout,
            )
        };
        if result >= 0 {
            return;
        }
        *queue = None;
    }
    std::thread::sleep(duration);
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

/// ENV 傾印區段的 marker（與 probe 指令裡的字面值一致）。
#[cfg(unix)]
const ENV_START: &str = "__YZ_ENV_S__";
#[cfg(unix)]
const ENV_END: &str = "__YZ_ENV_E__";

/// 從輸出裡切出第一組 `start` … `end` 之間的內容。rc 檔可能在 marker 前後印雜訊，
/// 所以定位 marker 而非整段解析。找不到（或缺尾 marker）回 `None`；marker 內為空
/// 字串則回 `Some("")`。
#[cfg(any(unix, test))]
fn extract_between<'a>(output: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let s = output.find(start)? + start.len();
    let rest = &output[s..];
    let e = rest.find(end)?;
    Some(&rest[..e])
}

/// PATH 專用 marker 切值（委派 `extract_between`）。
#[cfg(any(unix, test))]
fn extract_marked(output: &str) -> Option<&str> {
    extract_between(output, "__YZ_PATH_S__", "__YZ_PATH_E__")
}

/// 解析 ENV 傾印區段：`KEY=VALUE\0KEY=VALUE\0…`（NUL 分隔，`awk ENVIRON` 產生）。
/// 以第一個 `=` 切 key/value，容許 value 內含 `=`；略過空段與無 key（`=…` 或無 `=`）的段。
#[cfg(any(unix, test))]
fn parse_env_dump(section: &str) -> Vec<(String, String)> {
    section
        .split('\0')
        .filter(|s| !s.is_empty())
        .filter_map(|entry| {
            let (k, v) = entry.split_once('=')?;
            if k.is_empty() {
                None
            } else {
                Some((k.to_string(), v.to_string()))
            }
        })
        .collect()
}

/// 是否該把此 env 補進 GUI 行程。排除 PATH（另循合併）與會誤導的 shell 專屬變數：
/// `PWD`／`OLDPWD`（shell 的 cwd 非 app 的）、`SHLVL`（巢狀層級）、`_`（上一個指令
/// 路徑）、`SHELL`（agent spawn 端另設）。其餘（含 API key、locale…）一律可補。
#[cfg(any(unix, test))]
fn should_import_env(key: &str) -> bool {
    !matches!(key, "PATH" | "PWD" | "OLDPWD" | "SHLVL" | "_" | "SHELL")
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

    #[test]
    fn extract_between_slices_generic_markers() {
        let out = "noise<S>hello<E>tail";
        assert_eq!(extract_between(out, "<S>", "<E>"), Some("hello"));
        assert_eq!(extract_between(out, "<S>", "<MISSING>"), None);
        assert_eq!(extract_between(out, "<NOPE>", "<E>"), None);
    }

    #[test]
    fn parse_env_dump_splits_nul_delimited_pairs() {
        // 尾端 NUL（每筆都以 \0 結尾）不應產生空 pair。
        let section = "GEOSENSE_API_KEY=sk-abc\0YUUZU_API_KEY=tok-1\0";
        assert_eq!(
            parse_env_dump(section),
            vec![
                ("GEOSENSE_API_KEY".to_string(), "sk-abc".to_string()),
                ("YUUZU_API_KEY".to_string(), "tok-1".to_string()),
            ]
        );
    }

    #[test]
    fn parse_env_dump_keeps_equals_in_value_and_skips_bad_entries() {
        // value 內含 '='（base64/url 常見）要保留；空段、無 '=' 段、無 key 段都略過。
        let section = "A=b=c\0\0NOEQ\0=novalue\0K=v\0";
        assert_eq!(
            parse_env_dump(section),
            vec![
                ("A".to_string(), "b=c".to_string()),
                ("K".to_string(), "v".to_string()),
            ]
        );
    }

    #[test]
    fn should_import_env_excludes_path_and_shell_noise() {
        for k in ["PATH", "PWD", "OLDPWD", "SHLVL", "_", "SHELL"] {
            assert!(!should_import_env(k), "{k} 應排除");
        }
        for k in [
            "GEOSENSE_API_KEY",
            "YUUZU_API_KEY",
            "LANG",
            "HOMEBREW_PREFIX",
        ] {
            assert!(should_import_env(k), "{k} 應可匯入");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_exit_notification_observes_exit_and_keeps_child_reapable() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("0.05")
            .spawn()
            .unwrap();
        let mut queue = shell_exit_notification(child.id());
        assert!(queue.is_some());
        let start = std::time::Instant::now();
        wait_for_shell_exit(&mut queue, std::time::Duration::from_secs(1));
        assert!(start.elapsed() < std::time::Duration::from_millis(900));
        assert!(child.wait().unwrap().success());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_exit_notification_preserves_timeout_then_observes_later_exit() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("0.15")
            .spawn()
            .unwrap();
        let mut queue = shell_exit_notification(child.id());
        assert!(queue.is_some());
        wait_for_shell_exit(&mut queue, std::time::Duration::from_millis(10));
        assert!(child.try_wait().unwrap().is_none());
        wait_for_shell_exit(&mut queue, std::time::Duration::from_secs(1));
        assert!(child.wait().unwrap().success());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_exit_notification_failure_keeps_the_polling_fallback() {
        assert!(shell_exit_notification(u32::MAX).is_none());
        let start = std::time::Instant::now();
        wait_for_shell_exit(&mut None, std::time::Duration::from_millis(10));
        assert!(start.elapsed() >= std::time::Duration::from_millis(10));
    }
}
