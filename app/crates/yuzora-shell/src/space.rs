//! A new Space starts in a folder someone picked (beta.1's "open a folder as a
//! Space"): the folder is resolved where the Space will live (symlinks, `~`,
//! trailing separators), named after its last part, and a Space already rooted
//! there on the same machine is shown instead of a second one. The dialog that
//! asks is the `yuzora-new-space` mod; one runs per machine at a time.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use yuzora_core::remote::{ExecRequest, run};

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

/// A path as two that name the same folder compare: no `\\?\`, no repeated
/// or trailing separators (a root keeps its own).
pub fn normalize(path: &str) -> String {
    let p = path.trim();
    let p = p.strip_prefix(r"\\?\").unwrap_or(p);
    let mut out = String::with_capacity(p.len());
    for c in p.chars() {
        let sep = c == '/' || c == '\\';
        if !(sep && out.ends_with(['/', '\\'])) {
            out.push(c);
        }
    }
    let root = |s: &str| s.len() == 1 || (s.len() == 3 && s.as_bytes()[1] == b':');
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
    let mut args = vec!["-c".to_owned(), RESOLVE.to_owned(), "sh".to_owned()];
    args.extend(folders.iter().cloned());
    let req = ExecRequest::new("sh", args).on(Some(machine)).timeout(TIMEOUT).max_output(1 << 20);
    let out = run(&req).map_err(|e| Problem::Unreachable(e.to_string()))?;
    if out.code != 0 {
        return Err(Problem::Unreachable(out.stderr_text().trim().to_owned()));
    }
    lines(&out.stdout_text(), folders.len())
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
