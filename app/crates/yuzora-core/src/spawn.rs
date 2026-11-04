//! Starting console programs without a console window.
//!
//! The app has no console on Windows (`windows_subsystem = "windows"`), so
//! every console program it starts (git, hc, sh, rg, powershell) opens a
//! visible window of its own unless it is started with `CREATE_NO_WINDOW`.
//! [`no_window`] is that flag; a guard test (`every_spawn_is_quiet`) keeps
//! each non-test `Command::new` in the yuzora crates on it or on a reasoned
//! allow-list.

use std::process::Command;

/// `CREATE_NO_WINDOW`: the child gets no console at all.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Makes `cmd` start without a console window on Windows; no-op elsewhere.
/// It sets the creation flags, so call it before anything that sets more
/// (those must include [`CREATE_NO_WINDOW`] themselves).
///
/// For `tokio::process::Command`, pass `cmd.as_std_mut()`.
pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn no_window_returns_the_same_command() {
        let mut cmd = Command::new("x");
        let before = std::ptr::from_mut(&mut cmd);
        assert!(std::ptr::eq(before, no_window(&mut cmd)));
    }

    /// Run by the tests below as a child of this test binary (`--ignored`
    /// keeps it out of a normal run): says whether it shares the console of its parent.
    #[test]
    #[ignore = "a child process of the Windows console tests"]
    fn console_probe() {
        #[cfg(windows)]
        println!("CONSOLE={}", if probe::shares_console() { "some" } else { "none" });
    }

    #[cfg(windows)]
    pub(crate) mod probe {
        unsafe extern "system" {
            fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
            fn AllocConsole() -> i32;
        }

        /// How many processes share this one's console (0: none). With
        /// `CREATE_NO_WINDOW` a child gets a console of its own, hidden, with
        /// only itself on it; without it, a child of a process that has a
        /// console shares that one. (`GetConsoleWindow` says nothing on CI.)
        pub fn console_sharers() -> u32 {
            let mut list = [0u32; 4];
            // SAFETY: a buffer of the length it is given.
            unsafe { GetConsoleProcessList(list.as_mut_ptr(), 4) }
        }

        pub fn console_window() -> bool {
            console_sharers() != 0
        }

        /// Said by the probe child: `some` when it shares a console.
        pub fn shares_console() -> bool {
            console_sharers() > 1
        }

        /// The test runner may have no console (CI starts it without one),
        /// and then a child without the flag would have none either. Give it
        /// one, so a child that inherits it is told apart from one that doesn't.
        pub fn own_console() {
            if !console_window() {
                // SAFETY: as above.
                unsafe { AllocConsole() };
            }
            assert!(console_window(), "this process has no console to hand on");
        }

        pub const ARGS: [&str; 5] = ["--ignored", "--exact", "spawn::tests::console_probe", "--nocapture", "--test-threads=1"];
    }

    /// A program started through `remote::run` has no console window; the
    /// same program started by hand inherits ours (so the check can fail).
    #[cfg(windows)]
    #[test]
    fn remote_run_starts_programs_without_a_console_window() {
        probe::own_console();
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        let seen = |out: String| ["some", "none"].into_iter().find(|v| out.contains(&format!("CONSOLE={v}"))).map(str::to_owned);
        let by_hand = Command::new(&exe).args(probe::ARGS).output().unwrap();
        assert_eq!(seen(String::from_utf8_lossy(&by_hand.stdout).into_owned()).as_deref(), Some("some"), "control: {by_hand:?}");
        let out = crate::remote::run(&crate::remote::ExecRequest::new(exe, probe::ARGS)).unwrap();
        assert_eq!(seen(out.stdout_text()).as_deref(), Some("none"), "{}", out.stdout_text());
    }

    #[cfg(windows)]
    #[test]
    fn no_window_alone_leaves_the_child_without_a_console() {
        probe::own_console();
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(probe::ARGS);
        let out = no_window(&mut cmd).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("CONSOLE=none"), "{out:?}");
    }

    /// The part of a source file that is not `#[cfg(test)]` code: an item
    /// under that attribute ends at the first line, at the attribute's own
    /// indentation, that starts with `}` (or at a line ending in `;`).
    fn without_tests(text: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim_start();
            if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(all(test") {
                let indent = line.len() - trimmed.len();
                i += 1;
                // Further attributes, then the item.
                while i < lines.len() && lines[i].trim_start().starts_with("#[") {
                    i += 1;
                }
                // An item that opens a block runs to the `}` at its indentation;
                // anything else ends at the first line that ends in `;`.
                let block = lines.get(i).is_some_and(|l| l.trim_end().ends_with('{'));
                while i < lines.len() {
                    let l = lines[i];
                    let t = l.trim_start();
                    let at = l.len() - t.len();
                    i += 1;
                    if block && at == indent && t.starts_with('}') || !block && l.trim_end().ends_with(';') {
                        break;
                    }
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
            i += 1;
        }
        out
    }

    /// `(name, body)` of the function that holds line `at` of `code`.
    fn enclosing_fn(code: &str, at: usize) -> (String, String) {
        let lines: Vec<&str> = code.lines().collect();
        let is_fn = |l: &str| {
            let t = l.trim_start();
            let t = t.strip_prefix("pub(super) ").or_else(|| t.strip_prefix("pub(crate) ")).or_else(|| t.strip_prefix("pub ")).unwrap_or(t);
            let t = t.strip_prefix("async ").unwrap_or(t);
            let t = t.strip_prefix("unsafe ").unwrap_or(t);
            t.starts_with("fn ")
        };
        let start = (0..=at).rev().find(|&i| is_fn(lines[i])).expect("a Command::new outside any fn");
        let indent = lines[start].len() - lines[start].trim_start().len();
        let end = (start + 1..lines.len())
            .find(|&i| lines[i].len() - lines[i].trim_start().len() == indent && lines[i].trim_start().starts_with('}'))
            .unwrap_or(lines.len() - 1);
        let name = lines[start].trim_start().split("fn ").nth(1).unwrap_or("").split(['(', '<']).next().unwrap_or("").to_owned();
        (name, lines[start..=end].join("\n"))
    }

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") && path.file_name().is_some_and(|n| n != "tests.rs") && !dir.ends_with("tests") {
                out.push(path);
            }
        }
    }

    /// Programs the app starts that need no flag, with the reason. Matched
    /// by `(file suffix, function)`.
    const ALLOWED: &[(&str, &str, &str)] = &[
        // GUI programs: they open their own window, not a console.
        ("yuzora-settings/src/lib.rs", "file_manager", "explorer / open / xdg-open are GUI file managers"),
        ("yuzora-ssh/src/js.rs", "spawn_notepad", "Notepad is a GUI program"),
        ("yuzora-files/src/fsops.rs", "reveal_impl", "explorer / open / xdg-open / dbus-send reveal in a file manager"),
        // Never run on Windows.
        ("yuzora-files/src/svg/process.rs", "start", "`prepare` sets CREATE_NO_WINDOW on Windows before the spawn"),
        ("src/yuzora/trust.rs", "in_work_tree_at", "Linux-only (`#[cfg(target_os = \"linux\")]`): the folder watcher that calls it exists nowhere else"),
        ("yuzora-core/src/migrate.rs", "sqlite_item", "/usr/bin/sqlite3 is a macOS path, the WebKit store only exists there"),
        ("yuzora-settings/src/trash.rs", "platform", "/usr/bin/trash and osascript are macOS-only; the Windows powershell call is flagged by name below"),
    ];

    #[test]
    fn every_spawn_is_quiet() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let mut files = Vec::new();
        for e in std::fs::read_dir(&crates).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("yuzora-") && e.path().join("src").is_dir() {
                sources(&e.path().join("src"), &mut files);
            }
        }
        sources(&crates.parent().unwrap().join("src/yuzora"), &mut files);
        assert!(files.len() > 40, "no sources found: {}", files.len());
        let mut seen = 0;
        let mut loud = Vec::new();
        for path in &files {
            let shown = path.display().to_string().replace('\\', "/");
            if shown.ends_with("yuzora-core/src/spawn.rs") {
                continue;
            }
            let code = without_tests(&std::fs::read_to_string(path).unwrap());
            for (n, line) in code.lines().enumerate() {
                // `Command::new` (std or tokio), not a mention in a comment.
                if !line.contains("Command::new(") || line.trim_start().starts_with("//") {
                    continue;
                }
                seen += 1;
                let (func, body) = enclosing_fn(&code, n);
                let quiet = body.contains("no_window(") || body.contains("CREATE_NO_WINDOW");
                let allowed = ALLOWED.iter().any(|(file, f, _)| shown.ends_with(file) && *f == func);
                // trash.rs::platform has a windows and two unix variants.
                let windows_trash = shown.ends_with("yuzora-settings/src/trash.rs") && body.contains("powershell");
                if !quiet && (!allowed || windows_trash) {
                    loud.push(format!("{shown}: fn {func}, line {} of the non-test code", n + 1));
                }
            }
        }
        assert!(seen >= 8, "the scan found only {seen} spawns, it is looking at the wrong place");
        assert!(loud.is_empty(), "these start a program that opens a console window on Windows (use yuzora_core::spawn::no_window, or list it in ALLOWED with the reason):\n{}", loud.join("\n"));
    }

    #[test]
    fn the_scan_skips_test_code_and_finds_the_function() {
        let text = "fn a() {\n    Command::new(\"x\");\n}\n#[cfg(test)]\nmod tests {\n    fn t() { Command::new(\"y\"); }\n}\nfn b() {}\n";
        let code = without_tests(text);
        assert!(code.contains("fn b()") && !code.contains("\"y\""));
        let at = code.lines().position(|l| l.contains("\"x\"")).unwrap();
        assert_eq!(enclosing_fn(&code, at).0, "a");
    }
}
