//! Starting console programs without a console window.
//!
//! The app has no console on Windows (`windows_subsystem = "windows"`), so
//! every console program it starts (git, hc, sh, rg, powershell) opens a
//! visible window of its own unless it is started with `CREATE_NO_WINDOW`.
//! [`no_window`] is that flag; a guard test (`every_spawn_is_quiet`) keeps
//! each non-test `Command::new` in the yuzora crates on it or on a reasoned
//! allow-list.

use std::process::Command;

/// `CREATE_NO_WINDOW`: the child gets a console of its own, without a window
/// (and so does everything it starts).
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
    /// keeps it out of a normal run): says how many processes share its console.
    #[test]
    #[ignore = "a child process of the Windows console tests"]
    fn console_probe() {
        #[cfg(windows)]
        println!("CONSOLE_SHARED={}", probe::console_sharers());
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

        /// The test runner may have no console (CI starts it without one),
        /// and then a child without the flag would have none either. Give it
        /// one, so a child that inherits it is told apart from one that doesn't.
        pub fn own_console() {
            if console_sharers() == 0 {
                // SAFETY: no arguments.
                unsafe { AllocConsole() };
            }
            assert!(console_sharers() != 0, "this process has no console to hand on");
        }

        pub const ARGS: [&str; 5] = ["--ignored", "--exact", "spawn::tests::console_probe", "--nocapture", "--test-threads=1"];

        /// The count the probe printed.
        pub fn shared(out: &str) -> Option<u32> {
            out.split("CONSOLE_SHARED=").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
        }
    }

    /// A program started through `remote::run` shares no console: it has one
    /// of its own with only itself on it (1: not none, which is
    /// `DETACHED_PROCESS`, and not shared). The same program started by hand
    /// shares ours (so the check can fail).
    #[cfg(windows)]
    #[test]
    fn remote_run_starts_programs_without_a_console_window() {
        probe::own_console();
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        let by_hand = Command::new(&exe).args(probe::ARGS).output().unwrap();
        let control = probe::shared(&String::from_utf8_lossy(&by_hand.stdout));
        assert!(control.is_some_and(|n| n >= 2), "control: {by_hand:?}");
        let out = crate::remote::run(&crate::remote::ExecRequest::new(exe, probe::ARGS)).unwrap();
        assert_eq!(probe::shared(&out.stdout_text()), Some(1), "{}", out.stdout_text());
    }

    #[cfg(windows)]
    #[test]
    fn no_window_alone_leaves_the_child_with_a_console_of_its_own() {
        probe::own_console();
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(probe::ARGS);
        let out = no_window(&mut cmd).output().unwrap();
        assert_eq!(probe::shared(&String::from_utf8_lossy(&out.stdout)), Some(1), "{out:?}");
    }

    // ---- The scan: every program the app starts either has no console window on Windows or is listed.

    /// `text` with comments, and the inside of string and char literals,
    /// replaced by spaces (newlines stay, so line numbers do).
    fn blank(text: &str) -> Vec<char> {
        let c: Vec<char> = text.chars().collect();
        let mut o = c.clone();
        let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
        let wipe = |o: &mut Vec<char>, a: usize, b: usize| {
            for ch in &mut o[a..b] {
                if *ch != '\n' {
                    *ch = ' ';
                }
            }
        };
        let mut i = 0;
        while i < c.len() {
            match c[i] {
                '/' if c.get(i + 1) == Some(&'/') => {
                    let e = (i..c.len()).find(|&k| c[k] == '\n').unwrap_or(c.len());
                    wipe(&mut o, i, e);
                    i = e;
                }
                '/' if c.get(i + 1) == Some(&'*') => {
                    let (mut depth, mut k) = (1, i + 2);
                    while k < c.len() && depth > 0 {
                        if c[k] == '/' && c.get(k + 1) == Some(&'*') {
                            depth += 1;
                            k += 2;
                        } else if c[k] == '*' && c.get(k + 1) == Some(&'/') {
                            depth -= 1;
                            k += 2;
                        } else {
                            k += 1;
                        }
                    }
                    wipe(&mut o, i, k);
                    i = k;
                }
                'r' if (i == 0 || !ident(c[i - 1]) || (c[i - 1] == 'b' && (i < 2 || !ident(c[i - 2])))) && {
                    let mut k = i + 1;
                    while c.get(k) == Some(&'#') {
                        k += 1;
                    }
                    c.get(k) == Some(&'"')
                } =>
                {
                    let hashes = c[i + 1..].iter().take_while(|&&x| x == '#').count();
                    let open = i + 1 + hashes;
                    let close: Vec<char> = std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
                    let e = (open + 1..c.len()).find(|&k| c[k..].starts_with(&close)).map_or(c.len(), |k| k + close.len());
                    wipe(&mut o, open + 1, e.saturating_sub(close.len()).max(open + 1));
                    i = e;
                }
                '"' => {
                    let mut k = i + 1;
                    while k < c.len() && c[k] != '"' {
                        k += if c[k] == '\\' { 2 } else { 1 };
                    }
                    wipe(&mut o, i + 1, k.min(c.len()));
                    i = k + 1;
                }
                '\'' => {
                    // A char literal ('{', '\n') or a lifetime ('a).
                    let end = if c.get(i + 1) == Some(&'\\') {
                        (i + 2..c.len()).find(|&k| c[k] == '\'')
                    } else if c.get(i + 2) == Some(&'\'') {
                        Some(i + 2)
                    } else {
                        None
                    };
                    if let Some(e) = end {
                        wipe(&mut o, i + 1, e);
                        i = e + 1;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        o
    }

    /// Index of the `}` closing the `{` at `open`.
    fn close_of(c: &[char], open: usize) -> usize {
        let mut depth = 0;
        for (k, &ch) in c.iter().enumerate().skip(open) {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return k;
                    }
                }
                _ => {}
            }
        }
        c.len() - 1
    }

    /// Where the item that starts at `at` ends: after its `{…}` block, or at
    /// the first `;` outside parentheses and brackets.
    fn item_end(c: &[char], at: usize) -> usize {
        let mut depth = 0;
        for k in at..c.len() {
            match c[k] {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                '{' if depth == 0 => return close_of(c, k) + 1,
                ';' if depth == 0 => return k + 1,
                _ => {}
            }
        }
        c.len()
    }

    fn starts_with_at(c: &[char], at: usize, s: &str) -> bool {
        s.chars().enumerate().all(|(k, ch)| c.get(at + k) == Some(&ch))
    }

    /// `c` without what is compiled only for tests: every item under
    /// `#[cfg(test)]` or `#[cfg(all(test, …))]`, blanked.
    fn without_tests(mut c: Vec<char>) -> Vec<char> {
        let mut i = 0;
        while i < c.len() {
            if c[i] == '#' && (starts_with_at(&c, i, "#[cfg(test)]") || starts_with_at(&c, i, "#[cfg(all(test")) {
                let mut at = (i..c.len()).find(|&k| c[k] == ']').unwrap() + 1;
                // Further attributes belong to the same item.
                loop {
                    let mut k = at;
                    while c.get(k).is_some_and(|x| x.is_whitespace()) {
                        k += 1;
                    }
                    if c.get(k) == Some(&'#') && c.get(k + 1) == Some(&'[') {
                        at = (k..c.len()).find(|&j| c[j] == ']').unwrap() + 1;
                    } else {
                        break;
                    }
                }
                let end = item_end(&c, at);
                for ch in &mut c[i..end] {
                    if *ch != '\n' {
                        *ch = ' ';
                    }
                }
                i = end;
            } else {
                i += 1;
            }
        }
        c
    }

    /// Every place `words` (identifiers joined by `::`, whitespace allowed
    /// around them) is followed by `(`: the offset of the `(`.
    fn calls(c: &[char], words: &[&str]) -> Vec<usize> {
        let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
        let mut out = Vec::new();
        'at: for i in 0..c.len() {
            if i > 0 && ident(c[i - 1]) {
                continue;
            }
            let mut k = i;
            for (n, w) in words.iter().enumerate() {
                if n > 0 {
                    while c.get(k).is_some_and(|x| x.is_whitespace()) {
                        k += 1;
                    }
                    if !starts_with_at(c, k, "::") {
                        continue 'at;
                    }
                    k += 2;
                    while c.get(k).is_some_and(|x| x.is_whitespace()) {
                        k += 1;
                    }
                }
                if !starts_with_at(c, k, w) {
                    continue 'at;
                }
                k += w.chars().count();
            }
            if c.get(k).is_some_and(|&x| ident(x)) {
                continue;
            }
            while c.get(k).is_some_and(|x| x.is_whitespace()) {
                k += 1;
            }
            if c.get(k) == Some(&'(') {
                out.push(k);
            }
        }
        out
    }

    /// The `)` closing the `(` at `open`.
    fn paren_close(c: &[char], open: usize) -> usize {
        let mut depth = 0;
        for (k, &ch) in c.iter().enumerate().skip(open) {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return k;
                    }
                }
                _ => {}
            }
        }
        c.len() - 1
    }

    /// One function: its name, the programs it starts, and how many of them
    /// it takes a console window away from.
    #[derive(Debug, PartialEq)]
    struct Spawns {
        func: String,
        starts: Vec<usize>,
        quiet: usize,
    }

    /// The `Command::new` calls of the non-test code in `text`, by the
    /// function they are in (the innermost; `<outside any fn>` for a macro or
    /// a static), with the lines they are on. Err: an alias of `Command`,
    /// which the scan cannot follow.
    fn spawns(text: &str) -> Result<Vec<Spawns>, String> {
        let c = without_tests(blank(text));
        let line = |at: usize| c[..at].iter().filter(|&&x| x == '\n').count() + 1;
        let flat: String = c.iter().collect();
        if let Some(at) = flat.find("Command as ").or_else(|| flat.find("Command  as ")) {
            return Err(format!("line {}: `Command as …` hides the spawns from the scan; use the name `Command`", line(at)));
        }
        // Function bodies: (name, `{`, `}`).
        let mut fns: Vec<(String, usize, usize)> = Vec::new();
        for i in 0..c.len() {
            if starts_with_at(&c, i, "fn ") && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_')) {
                let name: String = c[i + 3..].iter().skip_while(|x| x.is_whitespace()).take_while(|x| x.is_alphanumeric() || **x == '_').collect();
                let mut depth = 0;
                for k in i + 3..c.len() {
                    match c[k] {
                        '(' | '[' => depth += 1,
                        ')' | ']' => depth -= 1,
                        ';' if depth == 0 => break,
                        '{' if depth == 0 => {
                            fns.push((name.clone(), k, close_of(&c, k)));
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
        let innermost = |at: usize| fns.iter().enumerate().filter(|(_, (_, a, b))| *a < at && at < *b).min_by_key(|(_, (_, a, b))| b - a).map(|(n, _)| n);
        let mut out: Vec<(Option<usize>, Spawns)> = Vec::new();
        let slot = |n: Option<usize>, out: &mut Vec<(Option<usize>, Spawns)>| {
            if let Some(p) = out.iter().position(|(m, _)| *m == n) {
                return p;
            }
            let func = n.map_or("<outside any fn>".to_owned(), |n| fns[n].0.clone());
            out.push((n, Spawns { func, starts: Vec::new(), quiet: 0 }));
            out.len() - 1
        };
        for at in calls(&c, &["Command", "new"]) {
            let p = slot(innermost(at), &mut out);
            out[p].1.starts.push(line(at));
        }
        for at in calls(&c, &["no_window"]) {
            let p = slot(innermost(at), &mut out);
            out[p].1.quiet += 1;
        }
        // A hand-set `creation_flags(… CREATE_NO_WINDOW …)` counts the same.
        for at in calls(&c, &["creation_flags"]) {
            if c[at..paren_close(&c, at)].iter().collect::<String>().contains("CREATE_NO_WINDOW") {
                let p = slot(innermost(at), &mut out);
                out[p].1.quiet += 1;
            }
        }
        Ok(out.into_iter().map(|(_, s)| s).filter(|s| !s.starts.is_empty() || s.quiet > 0).collect())
    }

    /// What `text` starts without taking the window away, per function name:
    /// `(function, spawns without a flag)`. Several functions of one name
    /// (a `cfg` per platform) add up.
    fn uncovered(text: &str) -> Result<Vec<(String, usize)>, String> {
        let mut by_name: Vec<(String, usize)> = Vec::new();
        for s in spawns(text)? {
            let loud = s.starts.len().saturating_sub(s.quiet);
            match by_name.iter_mut().find(|(n, _)| *n == s.func) {
                Some((_, n)) => *n += loud,
                None => by_name.push((s.func, loud)),
            }
        }
        by_name.retain(|(_, n)| *n > 0);
        Ok(by_name)
    }

    /// The programs the app starts without the flag, with how many in each
    /// function and why that is fine: `(file suffix, function, count, why)`.
    /// A function that starts one more, or fewer, fails the scan.
    const ALLOWED: &[(&str, &str, usize, &str)] = &[
        // GUI programs open a window of their own, not a console.
        ("yuzora-settings/src/lib.rs", "reveal", 3, "open / explorer / xdg-open: file managers"),
        ("yuzora-files/src/fsops.rs", "reveal_impl", 4, "open / explorer / xdg-open / dbus-send: file managers"),
        ("app/src/links.rs", "open", 1, "explorer.exe / open / xdg-open: the default browser (upstream)"),
        // Never run on Windows (the upstream's own files are in the same list: a new spawn there shows up here).
        ("yuzora-core/src/migrate.rs", "sqlite_item", 1, "/usr/bin/sqlite3: the WebKit store only exists on macOS"),
        ("yuzora-settings/src/trash.rs", "platform", 2, "/usr/bin/trash and osascript: the macOS variant; the Windows powershell is flagged"),
        ("src/yuzora/trust.rs", "in_work_tree_at", 2, "`#[cfg(target_os = \"linux\")]`: the folder watcher that calls it exists nowhere else"),
        ("app/src/files.rs", "watch_directories", 1, "the Linux folder watcher (`mod linux`), upstream"),
        ("app/src/core_bin.rs", "login_shell_path", 1, "returns at once on Windows (`cfg!(windows)`), upstream"),
        ("app/src/update.rs", "unpack", 1, "codesign, inside `cfg!(target_os = \"macos\")`; curl and tar go through `spawn::command`"),
        ("app/src/update.rs", "prepare_macos_bundle", 3, "codesign and PlistBuddy: macOS only"),
        ("app/src/update.rs", "relaunch_expected", 1, "the `#[cfg(unix)]` variant (`sh`); the Windows one uses `spawn::command`"),
        // Delegated: checked on their own below.
        ("yuzora-files/src/svg/process.rs", "start", 1, "`prepare` sets CREATE_NO_WINDOW on Windows before the spawn (checked in prepare_sets_the_flag)"),
    ];

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "tests") {
                    sources(&path, out);
                }
            } else if path.extension().is_some_and(|x| x == "rs") && path.file_name().is_some_and(|n| n != "tests.rs") {
                out.push(path);
            }
        }
    }

    /// The yuzora crates' and the app's sources.
    fn app_sources() -> Vec<(String, String)> {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let mut files = Vec::new();
        for e in std::fs::read_dir(&crates).unwrap().flatten() {
            if e.file_name().to_string_lossy().starts_with("yuzora-") && e.path().join("src").is_dir() {
                sources(&e.path().join("src"), &mut files);
            }
        }
        sources(&crates.parent().unwrap().join("src"), &mut files);
        files
            .into_iter()
            .map(|p| (p.display().to_string().replace('\\', "/"), std::fs::read_to_string(&p).unwrap()))
            // The helper itself and the upstream's own `spawn::command`.
            .filter(|(p, _)| !p.ends_with("yuzora-core/src/spawn.rs") && !p.ends_with("app/src/spawn.rs"))
            .collect()
    }

    #[test]
    fn every_spawn_is_quiet() {
        let files = app_sources();
        assert!(files.len() > 60, "no sources found: {}", files.len());
        let (mut seen, mut loud) = (0, Vec::new());
        let mut listed = vec![0; ALLOWED.len()];
        for (path, text) in &files {
            seen += spawns(text).unwrap_or_else(|e| panic!("{path}: {e}")).iter().map(|s| s.starts.len()).sum::<usize>();
            for (func, n) in uncovered(text).unwrap() {
                match ALLOWED.iter().position(|(file, f, _, _)| path.ends_with(file) && *f == func) {
                    Some(a) if ALLOWED[a].2 == n => listed[a] += 1,
                    Some(a) => {
                        listed[a] += 1;
                        loud.push(format!("{path}: fn {func} starts {n} programs without the flag, ALLOWED says {}", ALLOWED[a].2));
                    }
                    None => loud.push(format!("{path}: fn {func} starts {n} program(s) without the flag")),
                }
            }
        }
        for (a, hits) in listed.iter().enumerate() {
            if *hits == 0 {
                loud.push(format!("ALLOWED lists {} fn {}, which starts nothing unflagged any more: remove it", ALLOWED[a].0, ALLOWED[a].1));
            }
        }
        assert!(seen >= 12, "the scan found only {seen} spawns, it is looking at the wrong place");
        assert!(loud.is_empty(), "these start a program that opens a console window on Windows (call yuzora_core::spawn::no_window on it, or list it in ALLOWED with the reason):\n{}", loud.join("\n"));
    }

    /// `svg/process.rs` starts its program in `start` and takes the window
    /// away in `prepare`: both are required.
    #[test]
    fn prepare_sets_the_flag() {
        let (_, text) = app_sources().into_iter().find(|(p, _)| p.ends_with("yuzora-files/src/svg/process.rs")).unwrap();
        let prepare = spawns(&text).unwrap().into_iter().filter(|s| s.func == "prepare").map(|s| s.quiet).sum::<usize>();
        assert!(prepare >= 1, "svg/process.rs `prepare` no longer sets CREATE_NO_WINDOW");
    }

    // ---- The scan against what it must catch: each of these used to get past it.

    fn loud(text: &str) -> Vec<(String, usize)> {
        uncovered(text).unwrap()
    }

    #[test]
    fn the_scan_skips_test_code() {
        let text = "fn a() { no_window(&mut Command::new(\"x\")); }\n#[cfg(test)]\nmod tests {\n    fn t() { Command::new(\"y\"); }\n}\n#[cfg(test)]\nfn alone() { Command::new(\"z\"); }\nfn b() { no_window(&mut Command::new(\"w\")); }\nfn c() {\n    #[cfg(test)]\n    return CAP.with(|c| Command::new(\"v\"));\n    Command::new(\"u\");\n}\n";
        assert_eq!(loud(text), vec![("c".to_owned(), 1)], "{:?}", spawns(text));
    }

    #[test]
    fn a_second_spawn_in_a_quiet_function_is_found() {
        // B1
        let text = "fn run_hc() {\n    let mut c = Command::new(\"hc\");\n    no_window(&mut c);\n    let d = Command::new(\"git\");\n}\n";
        assert_eq!(loud(text), vec![("run_hc".to_owned(), 1)]);
    }

    #[test]
    fn a_second_spawn_in_an_allowed_function_is_found() {
        // B4: ALLOWED says how many.
        let text = "fn reveal() {\n    Command::new(\"open\");\n    Command::new(\"git\");\n}\n";
        assert_eq!(loud(text), vec![("reveal".to_owned(), 2)]);
        assert_ne!(2, ALLOWED.iter().find(|a| a.1 == "reveal").unwrap().2);
    }

    #[test]
    fn a_comment_does_not_make_a_function_quiet() {
        // B2
        let text = "fn f() {\n    // CREATE_NO_WINDOW no_window(\n    /* no_window( */\n    let _s = \"no_window(\";\n    Command::new(\"git\");\n}\n";
        assert_eq!(loud(text), vec![("f".to_owned(), 1)]);
    }

    #[test]
    fn a_hand_set_flag_must_be_the_right_one() {
        let ok = "fn f() { let mut c = Command::new(\"g\"); c.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED); }";
        let wrong = "fn f() { let mut c = Command::new(\"g\"); c.creation_flags(DETACHED_PROCESS); }";
        assert!(loud(ok).is_empty());
        assert_eq!(loud(wrong), vec![("f".to_owned(), 1)]);
    }

    #[test]
    fn an_alias_of_command_is_refused() {
        // B3
        let text = "use std::process::Command as RvProc;\nfn f() { RvProc::new(\"git\"); }\n";
        assert!(spawns(text).unwrap_err().contains("Command as"));
    }

    #[test]
    fn a_one_line_function_before_a_quiet_one_is_its_own() {
        // B5
        let text = "fn rv_b5() -> Command { Command::new(\"git\") }\nfn run_hc() {\n    let mut c = Command::new(\"hc\");\n    no_window(&mut c);\n}\n";
        assert_eq!(loud(text), vec![("rv_b5".to_owned(), 1)]);
    }

    #[test]
    fn a_macro_after_a_quiet_function_is_not_part_of_it() {
        // B6
        let text = "fn run_hc() {\n    let mut c = Command::new(\"hc\");\n    no_window(&mut c);\n}\nmacro_rules! rv_git { () => { Command::new(\"git\") } }\n";
        assert_eq!(loud(text), vec![("<outside any fn>".to_owned(), 1)]);
    }

    #[test]
    fn braces_in_strings_and_chars_do_not_move_a_function_end() {
        let text = "fn a() {\n    let _ = \"}\";\n    let _ = '}';\n    let _ = r#\"} {\"#;\n    let _ = b'{';\n}\nfn b() { Command::new(\"git\"); }\n";
        assert_eq!(loud(text), vec![("b".to_owned(), 1)]);
    }

    #[test]
    fn spaces_inside_the_call_are_still_a_call() {
        assert_eq!(loud("fn f() { std::process::Command :: new (\"git\"); }"), vec![("f".to_owned(), 1)]);
        assert_eq!(loud("fn f() { tokio::process::Command::new(\"git\"); }"), vec![("f".to_owned(), 1)]);
    }
}
