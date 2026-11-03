//! A new Space starts in a folder someone picked (beta.1's "open a folder as a
//! Space"): the folder is resolved where the Space will live (symlinks, `~`,
//! trailing separators), named after its last part, and a Space already rooted
//! there on the same machine is shown instead of a second one. The dialog that
//! asks is the `yuzora-new-space` mod; one runs per machine at a time.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use yuzora_core::remote::{ExecError, ExecRequest, run};

/// How long resolving folders on a machine may take.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Run on a machine with the folders as `$1…`: each one's real path, or an
/// empty line when it isn't a folder there. `~` is the machine's home, and so
/// is where relative paths start. The paths are arguments, never code.
pub const RESOLVE: &str = r#"cd -- "$HOME" 2>/dev/null
for p do
  case $p in "" | "~") p=$HOME ;; "~/"*) p=$HOME/${p#"~/"} ;; esac
  (CDPATH= cd -- "$p" 2>/dev/null && pwd -P) || echo
done"#;

/// RESOLVE for a machine without `sh` (Windows), run by its PowerShell as one
/// line. `-Command` reads extra arguments as code, so the folders go into
/// `@(FOLDERS)` as single-quoted strings, where nothing expands. Links are not
/// followed there.
pub const RESOLVE_PS: &str = r#"$ErrorActionPreference = 'SilentlyContinue';
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false } catch {};
Set-Location -LiteralPath $HOME;
foreach ($p in @(FOLDERS)) {
  if ($p -eq '' -or $p -eq '~') { $p = $HOME } elseif ($p.StartsWith('~/') -or $p.StartsWith('~\')) { $p = Join-Path $HOME ($p.Substring(2)) };
  $i = Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue;
  $f = if ($i -and $i.PSIsContainer) { $i.FullName } else { '' };
  if ($f.Length -gt 3) { $f = $f.TrimEnd('\', '/') };
  [Console]::Out.Write($f + [char]10)
}"#;

/// PowerShell's single quotes, the curly ones too (as `programs::powershell_line`).
const PS_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];

/// `powershell.exe`'s arguments for RESOLVE_PS with `folders`.
fn powershell_args(folders: &[String]) -> Vec<String> {
    let quote = |f: &String| {
        let mut s = String::from("'");
        for ch in f.chars() {
            if PS_QUOTES.contains(&ch) {
                s.push(ch);
            }
            s.push(ch);
        }
        s + "'"
    };
    let list = folders.iter().map(quote).collect::<Vec<_>>().join(",");
    let script = RESOLVE_PS.lines().map(str::trim).collect::<Vec<_>>().join(" ").replacen("FOLDERS", &list, 1);
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"].into_iter().map(String::from).chain([script]).collect()
}

/// Where a new Space for a folder would start, and the Space already there.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plan {
    pub folder: String,
    pub name: String,
    pub existing: Option<String>,
}

/// Why there is no plan.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// Nothing there, or not a folder.
    NotAFolder,
    /// The machine didn't answer (or answered something else): why.
    Unreachable(String),
}

impl Problem {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Problem::NotAFolder => serde_json::json!({ "error": "not_a_folder" }),
            Problem::Unreachable(why) => serde_json::json!({ "error": "unreachable", "detail": why }),
        }
    }
}

/// A folder's last name: `/Users/me/api/` and `C:\work\api` are `api`; `/` is
/// itself and `C:\` is `C:`.
pub fn folder_name(path: &str) -> String {
    let p = normalize(path);
    match p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => p,
    }
}

/// A path as two that name the same folder compare: no `\\?\` (`\\?\UNC\host\share`
/// is `\\host\share`), no repeated or trailing separators (a root keeps its own, a
/// share its leading `\\`).
pub fn normalize(path: &str) -> String {
    let p = path.trim();
    let (share, p) = match p.strip_prefix(r"\\?\") {
        Some(rest) => rest.strip_prefix(r"UNC\").map_or((false, rest), |s| (true, s)),
        None => p.strip_prefix(r"\\").map_or((false, p), |s| (true, s)),
    };
    let mut out = String::from(if share { r"\\" } else { "" });
    for c in p.chars() {
        let sep = c == '/' || c == '\\';
        if !(sep && out.ends_with(['/', '\\'])) {
            out.push(c);
        }
    }
    let root = |s: &str| s.len() == 1 || (s.len() == 3 && s.as_bytes()[1] == b':') || (share && s.len() == 2);
    while out.ends_with(['/', '\\']) && !root(&out) {
        out.pop();
    }
    out
}

/// The Space rooted at `folder`: `spaces` are `(id, folder as created, that folder resolved)`.
pub fn existing(folder: &str, spaces: &[(String, String, Option<String>)]) -> Option<String> {
    let want = normalize(folder);
    spaces.iter().find(|(_, raw, resolved)| normalize(resolved.as_deref().unwrap_or(raw)) == want).map(|(id, _, _)| id.clone())
}

/// Each folder's real path on `on` (None: here), or None where it isn't a folder.
pub fn resolve(folders: &[String], on: Option<&str>) -> Result<Vec<Option<String>>, Problem> {
    let Some(machine) = on.filter(|m| !m.is_empty()) else {
        return Ok(folders.iter().map(|f| resolve_here(f)).collect());
    };
    let ask = |program: &str, args: Vec<String>| run(&ExecRequest::new(program, args).on(Some(machine)).timeout(TIMEOUT).max_output(1 << 20));
    let mut args = vec!["-c".to_owned(), RESOLVE.to_owned(), "sh".to_owned()];
    args.extend(folders.iter().cloned());
    let out = match ask("sh", args) {
        // No `sh` there (Windows): its PowerShell, as for terminals there. When
        // that can't start either, sh's error says more.
        Err(ExecError::Remote(why)) => ask("powershell.exe", powershell_args(folders)).map_err(|e| match e {
            ExecError::Remote(_) => Problem::Unreachable(why),
            e => Problem::Unreachable(e.to_string()),
        })?,
        r => r.map_err(|e| Problem::Unreachable(e.to_string()))?,
    };
    if out.code != 0 {
        return Err(Problem::Unreachable(out.stderr_text().trim().to_owned()));
    }
    let text = out.stdout_text();
    lines(text.strip_prefix('\u{feff}').unwrap_or(&text), folders.len())
}

/// The script's answer: one line per folder.
fn lines(stdout: &str, count: usize) -> Result<Vec<Option<String>>, Problem> {
    let list: Vec<Option<String>> = stdout.strip_suffix('\n').unwrap_or(stdout).split('\n').map(|l| Some(l.to_owned()).filter(|l| !l.is_empty())).collect();
    if list.len() != count && !(count == 0 && stdout.is_empty()) {
        return Err(Problem::Unreachable(format!("expected {count} lines, got {}", list.len())));
    }
    Ok(if count == 0 { Vec::new() } else { list })
}

fn resolve_here(folder: &str) -> Option<String> {
    let home = yuzora_core::paths::home_dir();
    let f = folder.trim();
    let path = match f {
        "" | "~" => home,
        _ => match f.strip_prefix("~/").or_else(|| f.strip_prefix("~\\")) {
            Some(rest) => home.join(rest),
            None => home.join(f), // an absolute path replaces home
        },
    };
    let real = std::fs::canonicalize(path).ok().filter(|p| p.is_dir())?;
    Some(normalize(&real.to_string_lossy()))
}

/// The plan for a new Space at `folder` on `on`, given the Spaces on that
/// machine as `(id, folder)`.
pub fn plan(folder: &str, spaces: &[(String, String)], on: Option<&str>) -> Result<Plan, Problem> {
    let mut asked = vec![folder.to_owned()];
    asked.extend(spaces.iter().map(|(_, f)| f.clone()));
    let mut found = resolve(&asked, on)?.into_iter();
    let real = found.next().flatten().ok_or(Problem::NotAFolder)?;
    let known: Vec<_> = spaces.iter().zip(found).map(|((id, raw), r)| (id.clone(), raw.clone(), r)).collect();
    Ok(Plan { name: folder_name(&real), existing: existing(&real, &known), folder: real })
}

/// Machines with a new-Space dialog open ("" is this computer), and since when.
static BUSY: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
/// A dialog that never said it closed stops counting after this.
const STALE: Duration = Duration::from_secs(600);

/// Starts a new Space on `machine`: false while another is being made there.
pub fn begin(machine: &str) -> bool {
    let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    busy.retain(|(_, since)| since.elapsed() < STALE);
    if busy.iter().any(|(m, _)| m == machine) {
        return false;
    }
    busy.push((machine.to_owned(), Instant::now()));
    true
}

/// That Space is made (or abandoned).
pub fn end(machine: &str) {
    BUSY.lock().unwrap_or_else(|e| e.into_inner()).retain(|(m, _)| m != machine);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("yuzora-space-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(std::fs::canonicalize(dir).unwrap())
        }
        fn path(&self, rest: &str) -> String {
            self.0.join(rest).to_string_lossy().into_owned()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn names_a_folder_by_its_last_part() {
        assert_eq!(folder_name("/Users/me/api"), "api");
        assert_eq!(folder_name("/Users/me/api/"), "api");
        assert_eq!(folder_name("/srv//my app//"), "my app");
        assert_eq!(folder_name(r"C:\work\api"), "api");
        assert_eq!(folder_name(r"\\?\C:\work\api\"), "api");
        assert_eq!(folder_name("/"), "/");
        assert_eq!(folder_name(r"C:\"), "C:");
        assert_eq!(folder_name(""), "");
    }

    #[test]
    fn normalizes_separators_but_keeps_roots() {
        assert_eq!(normalize("/a//b/"), "/a/b");
        assert_eq!(normalize("  /a/b///  "), "/a/b");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("//"), "/");
        assert_eq!(normalize(r"\\?\C:\a\b\"), r"C:\a\b");
        assert_eq!(normalize(r"C:\"), r"C:\");
        // A network share keeps its leading \\ (canonicalize gives \\?\UNC\host\share).
        assert_eq!(normalize(r"\\?\UNC\nas\share\proj\"), r"\\nas\share\proj");
        assert_eq!(normalize(r"\\nas\share\\proj"), r"\\nas\share\proj");
        assert_eq!(folder_name(r"\\?\UNC\nas\share"), "share");
        let spaces = vec![("sp1".to_owned(), r"\\nas\share\proj".to_owned(), None)];
        assert_eq!(existing(r"\\?\UNC\nas\share\proj", &spaces), Some("sp1".into()));
    }

    #[test]
    fn finds_the_space_rooted_there() {
        let spaces = vec![
            ("sp1".to_owned(), "/srv/api/".to_owned(), None),
            ("sp2".to_owned(), "/tmp/web".to_owned(), Some("/private/tmp/web".to_owned())),
        ];
        assert_eq!(existing("/srv/api", &spaces), Some("sp1".into()));
        assert_eq!(existing("/private/tmp/web", &spaces), Some("sp2".into()));
        assert_eq!(existing("/srv/api/v2", &spaces), None);
        assert_eq!(existing("/srv", &spaces), None);
    }

    #[test]
    fn plans_here_through_symlinks_and_sees_the_space_already_there() {
        let s = Scratch::new("plan");
        std::fs::create_dir_all(s.0.join("my app")).unwrap();
        std::fs::write(s.0.join("note.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(s.0.join("my app"), s.0.join("link")).unwrap();
        let real = s.path("my app");

        let fresh = plan(&format!("{real}/"), &[], None).unwrap();
        assert_eq!(fresh, Plan { folder: real.clone(), name: "my app".into(), existing: None });

        let spaces = vec![("sp9".to_owned(), real.clone())];
        assert_eq!(plan(&real, &spaces, None).unwrap().existing, Some("sp9".into()));
        #[cfg(unix)]
        {
            let via = plan(&s.path("link"), &spaces, None).unwrap();
            assert_eq!((via.folder.as_str(), via.existing.as_deref()), (real.as_str(), Some("sp9")));
        }

        assert_eq!(plan(&s.path("note.txt"), &[], None), Err(Problem::NotAFolder));
        assert_eq!(plan(&s.path("missing"), &[], None), Err(Problem::NotAFolder));
        let home = std::fs::canonicalize(yuzora_core::paths::home_dir()).unwrap();
        assert_eq!(plan("~", &[], None).unwrap().folder, normalize(&home.to_string_lossy()));
    }

    // RESOLVE runs on another machine through `hc exec`; here it runs in sh
    // the same way, with a scratch HOME.
    #[cfg(unix)]
    fn resolve_in_sh(home: &Path, folders: &[&str]) -> Vec<Option<String>> {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(RESOLVE)
            .arg("sh")
            .args(folders)
            .env("HOME", home)
            .env("CDPATH", home)
            .output()
            .unwrap();
        assert!(out.status.success());
        lines(&String::from_utf8_lossy(&out.stdout), folders.len()).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn the_machine_script_resolves_and_never_runs_a_path() {
        let s = Scratch::new("remote");
        std::fs::create_dir_all(s.0.join("src/my app")).unwrap();
        std::fs::create_dir_all(s.0.join("$(touch pwned)")).unwrap();
        std::os::unix::fs::symlink(s.0.join("src"), s.0.join("link")).unwrap();
        let home = s.0.to_string_lossy().into_owned();
        let got = resolve_in_sh(&s.0, &["~", "", "~/src/my app/", "src", "link/my app", "/nonexistent-yuzora", "$(touch pwned)", "`touch pwned2`"]);
        assert_eq!(
            got,
            vec![
                Some(home.clone()),
                Some(home.clone()),
                Some(format!("{home}/src/my app")),
                Some(format!("{home}/src")),
                Some(format!("{home}/src/my app")),
                None,
                Some(format!("{home}/$(touch pwned)")),
                None,
            ]
        );
        assert!(!s.0.join("pwned").exists() && !s.0.join("pwned2").exists(), "a path ran as code");
    }

    #[test]
    fn the_powershell_script_holds_folders_as_quoted_text() {
        let args = powershell_args(&["it's ‘q’".into(), "$(calc)".into()]);
        assert_eq!(args[..4], ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
        assert_eq!(args.len(), 5);
        assert!(args[4].contains("@('it''s ‘‘q’’','$(calc)')"), "{}", args[4]);
        assert!(!args[4].contains(['\n', '"', '`']), "one line, nothing that expands: {}", args[4]);
        assert!(powershell_args(&[])[4].contains("@()"));
    }

    // RESOLVE_PS runs on Windows machines through `hc exec`; here it runs in
    // pwsh (CI's runners have it) the same way, with a scratch HOME.
    #[cfg(unix)]
    #[test]
    fn the_powershell_script_resolves_and_never_runs_a_path() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let Some(pwsh) = std::env::split_paths(&path).map(|d| d.join("pwsh")).find(|p| p.is_file()) else {
            eprintln!("skipped: no pwsh on PATH");
            return;
        };
        let s = Scratch::new("powershell");
        for dir in ["src/my app", "$(New-Item pwned)", "it's ‘q’"] {
            std::fs::create_dir_all(s.0.join(dir)).unwrap();
        }
        let home = s.0.to_string_lossy().into_owned();
        let folders: Vec<String> = ["~", "", "~/src/my app/", "src", "/nonexistent-yuzora", "$(New-Item pwned)", "it's ‘q’"].map(String::from).into();
        let out = std::process::Command::new(pwsh).args(powershell_args(&folders)).env("HOME", &s.0).current_dir(&s.0).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            lines(&String::from_utf8_lossy(&out.stdout), folders.len()).unwrap(),
            vec![
                Some(home.clone()),
                Some(home.clone()),
                Some(format!("{home}/src/my app")),
                Some(format!("{home}/src")),
                None,
                Some(format!("{home}/$(New-Item pwned)")),
                Some(format!("{home}/it's ‘q’")),
            ]
        );
        assert!(!s.0.join("pwned").exists(), "a path ran as code");
    }

    #[test]
    fn reads_one_line_per_folder() {
        assert_eq!(lines("/a\n\n/b\n", 3).unwrap(), vec![Some("/a".into()), None, Some("/b".into())]);
        assert_eq!(lines("", 0).unwrap(), Vec::<Option<String>>::new());
        assert!(matches!(lines("/a\n", 2), Err(Problem::Unreachable(_))));
    }

    #[test]
    fn one_new_space_per_machine_at_a_time() {
        assert!(begin("test-lab"));
        assert!(!begin("test-lab"));
        assert!(begin("test-other"));
        end("test-lab");
        assert!(begin("test-lab"));
        end("test-lab");
        end("test-other");
    }
}
