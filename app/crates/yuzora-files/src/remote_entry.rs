//! Making a file or a folder, and renaming one, on another machine:
//! `hc exec --on M -- sh -c SCRIPT`. The paths travel as arguments (never in
//! the script), and the script, on that machine, makes the change in one step
//! that fails where something is already there, so nothing is replaced and
//! nothing is left half made.
//!
//! What a new file or folder gets (its mode, its owner and group, the ACL a
//! folder hands down, the group a setgid folder gives) is whatever the machine
//! gives when its own `touch` and `mkdir` make it: the script makes it with
//! the same calls under the umask it is started with, and changes nothing after.
//! A rename keeps the very entry, so it keeps all of that.

use std::time::Duration;

use yuzora_core::remote::{ExecError, ExecOutput, ExecRequest};

use crate::fsops;

/// `$1` path, `$2` `file` or `folder`. Prints one line and exits 0: `done`, `exists`,
/// `refused folder|nofolder` or `refused tool mkdir`. Any other end leaves nothing made.
///
/// A file is made by redirecting nothing into it under `set -C` (noclobber), which asks the
/// system to create the name only if it is free; a folder by `mkdir`, which does the same.
/// So when two make the same name at once exactly one succeeds, and a link at the name, even
/// one to nothing, counts as taken.
const MAKE_SCRIPT: &str = concat!(
    include_str!("remote_entry/common.sh"),
    include_str!("remote_entry/make.sh")
);

/// `$1` from, `$2` to, `$3` `1` when the two names differ only in letter case. Prints one line
/// and exits 0: `done`, `exists`, `gone`, `refused folder|nofolder|device` or
/// `refused tool mv|stat|rm`. Any other end leaves the entry where it was.
///
/// What is at `to` is never replaced, with this much said about when that is looked at:
/// - A file, a link or anything that is not a folder first takes the name `to` (an empty file,
///   made the way `MAKE_SCRIPT` makes one), then moves over that. Nobody can make `to` between
///   the look and the move, because it is taken. If the move fails the empty file is removed
///   again, but only if it is still the one made.
/// - A folder is moved with `mv -n`, and with `-T` where `mv` has it (GNU), which asks the
///   system to move only if `to` is free (`renameat2` with `RENAME_NOREPLACE`). GNU `mv` does
///   that in one call; macOS' and BusyBox' look first and move after, so a name made in
///   between, in that sliver inside `mv`, can be missed. Afterwards the script checks that the
///   folder moved is the one at `to`.
/// - A name that is the same entry under other letters (`$3`) is moved plainly, on a file
///   system that folds case.
const MOVE_SCRIPT: &str = concat!(
    include_str!("remote_entry/common.sh"),
    include_str!("remote_entry/move.sh")
);

/// A change is small; a hung one is not worth more than this.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What the scripts print and the errors `hc` gives are short.
const MAX_OUTPUT: u64 = 16 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Folder,
}

/// What is asked of a machine.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Act<'a> {
    Make(&'a str, Kind),
    Move(&'a str, &'a str),
}

/// Why nothing was done, as the person can be told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Problem {
    /// Something is at the name (a file, a folder, a link, even one to nothing).
    Exists,
    /// What was to move is not there.
    Gone,
    /// The folder the name is to be in is not there, or is not a folder.
    NoFolder,
    /// A folder the change is in cannot be written to or entered.
    FolderNotWritable,
    /// A rename would have to copy, which a rename does not do.
    OtherDevice,
    /// A program the script runs is missing there.
    NoTool(&'static str),
    /// No `sh` there (a Windows machine).
    NoShell,
    /// Not a path `sh` there can use: it has to start with `/` and name something.
    BadPath,
    /// The machine could not be asked, or said something else: its own words.
    Failed(String),
}

impl Act<'_> {
    fn verb(&self) -> &'static str {
        match self {
            Act::Make(..) => "create",
            Act::Move(..) => "rename",
        }
    }

    /// What the person asked about.
    fn subject(&self) -> &str {
        match self {
            Act::Make(path, _) | Act::Move(path, _) => path,
        }
    }

    /// The name that is to be taken.
    fn target(&self) -> &str {
        match self {
            Act::Make(path, _) | Act::Move(_, path) => path,
        }
    }

    /// The folders the change is in, each once.
    fn folders(&self) -> Vec<&str> {
        let mut found = vec![folder_of(self.target())];
        if let Act::Move(from, _) = self {
            let source = folder_of(from);
            if !found.contains(&source) {
                found.insert(0, source);
            }
        }
        found
    }

    /// What `problem` means for this, said to the person.
    pub(crate) fn describe(&self, problem: &Problem, machine: &str) -> String {
        let (verb, subject, target) = (self.verb(), self.subject(), self.target());
        match problem {
            Problem::Exists => format!("{target} already exists"),
            Problem::Gone => format!("cannot {verb} {subject}: it is no longer there on {machine}"),
            Problem::NoFolder => format!(
                "cannot {verb} {subject}: the folder {} does not exist on {machine}",
                folder_of(target)
            ),
            Problem::FolderNotWritable => format!(
                "cannot {verb} {subject}: you may not change {} on {machine} (a name is added to or removed from a folder only by someone who can write to it and enter it)",
                self.folders().join(" and ")
            ),
            Problem::OtherDevice => format!(
                "cannot {verb} {subject}: {target} is on another file system on {machine}, and a rename does not copy"
            ),
            Problem::NoTool(tool) => {
                format!("cannot {verb} {subject}: {machine} has no {tool}, which this needs")
            }
            Problem::NoShell => format!(
                "cannot {verb} {subject}: this needs sh on {machine}, and it has none (Windows machines cannot be changed from here)"
            ),
            Problem::BadPath => format!(
                "cannot {verb} {subject}: a machine's path has to start with / and name a file or a folder (a Windows path cannot be used)"
            ),
            Problem::Failed(why) => format!("cannot {verb} {subject} on {machine}: {why}"),
        }
    }
}

/// The folder a path is in.
fn folder_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(at) => &path[..at],
    }
}

/// A path `sh` there can take: absolute, no NUL, naming something other than `.` and `..`.
fn usable_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or("");
    path.starts_with('/') && !path.contains('\0') && !matches!(name, "" | "." | "..")
}

/// `to` is `from` spelled with other letters. Whether they are one entry is for the
/// machine's file system to say, which the script asks.
fn other_case(from: &str, to: &str) -> bool {
    from != to && from.to_lowercase() == to.to_lowercase()
}

/// The request that does `act` on `on` (`None`: here, through the same script, as the tests do).
pub(crate) fn request(act: &Act, on: Option<&str>) -> Result<ExecRequest, Problem> {
    let (script, args): (&str, Vec<&str>) = match *act {
        Act::Make(path, kind) if usable_path(path) => (
            MAKE_SCRIPT,
            vec![
                path,
                match kind {
                    Kind::File => "file",
                    Kind::Folder => "folder",
                },
            ],
        ),
        Act::Move(from, to) if usable_path(from) && usable_path(to) => (
            MOVE_SCRIPT,
            vec![from, to, if other_case(from, to) { "1" } else { "0" }],
        ),
        _ => return Err(Problem::BadPath),
    };
    Ok(fsops::sh_request(on, script, &args, TIMEOUT, MAX_OUTPUT))
}

/// What the script said.
#[derive(Debug, PartialEq)]
enum Verdict {
    Done,
    Exists,
    Gone,
    NoFolder,
    FolderNotWritable,
    OtherDevice,
    NoTool(&'static str),
}

/// Reads the script's one line; anything but exactly a line of a known shape is `None`.
fn parse_verdict(stdout: &[u8]) -> Option<Verdict> {
    let line = std::str::from_utf8(stdout).ok()?.strip_suffix('\n')?;
    match line.split(' ').collect::<Vec<_>>()[..] {
        ["done"] => Some(Verdict::Done),
        ["exists"] => Some(Verdict::Exists),
        ["gone"] => Some(Verdict::Gone),
        ["refused", "nofolder"] => Some(Verdict::NoFolder),
        ["refused", "folder"] => Some(Verdict::FolderNotWritable),
        ["refused", "device"] => Some(Verdict::OtherDevice),
        ["refused", "tool", tool] => ["mkdir", "mv", "stat", "rm"]
            .into_iter()
            .find(|known| *known == tool)
            .map(Verdict::NoTool),
        _ => None,
    }
}

/// What the machine's answer means.
pub(crate) fn interpret(answer: Result<ExecOutput, ExecError>) -> Result<(), Problem> {
    let out = match answer {
        Ok(out) => out,
        Err(ExecError::Remote(m) | ExecError::Protocol(m)) if m.starts_with("cannot start sh") => {
            return Err(Problem::NoShell);
        }
        Err(e @ ExecError::TimedOut { .. }) => {
            return Err(Problem::Failed(format!(
                "{e}. It may or may not have been done: reload the folder to see"
            )));
        }
        Err(e) => return Err(Problem::Failed(e.to_string())),
    };
    if out.code != 0 {
        return Err(Problem::Failed(fsops::failure(&out, "the change")));
    }
    match parse_verdict(&out.stdout) {
        Some(Verdict::Done) => Ok(()),
        Some(Verdict::Exists) => Err(Problem::Exists),
        Some(Verdict::Gone) => Err(Problem::Gone),
        Some(Verdict::NoFolder) => Err(Problem::NoFolder),
        Some(Verdict::FolderNotWritable) => Err(Problem::FolderNotWritable),
        Some(Verdict::OtherDevice) => Err(Problem::OtherDevice),
        Some(Verdict::NoTool(tool)) => Err(Problem::NoTool(tool)),
        None => Err(Problem::Failed(
            "the answer could not be read; it may or may not have been done".into(),
        )),
    }
}

/// Does `act` on `on` (`None`: here, through the same script).
pub(crate) fn run_act(act: &Act, on: Option<&str>) -> Result<(), Problem> {
    interpret(fsops::run(request(act, on)?))
}

/// Does `act` on the machine `machine`, and tells the person if it cannot be done.
pub(crate) fn on_machine(act: Act, machine: &str) -> Result<(), String> {
    run_act(&act, Some(machine)).map_err(|problem| act.describe(&problem, machine))
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn at(dir: &Path, name: &str) -> String {
        dir.join(name).to_string_lossy().into_owned()
    }

    fn put(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let path = at(dir, name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        found
    }

    fn is_root() -> bool {
        // SAFETY: a plain syscall with no arguments.
        unsafe { libc::geteuid() == 0 }
    }

    /// The shells `hc exec ... sh` may be, here: `sh`, then `dash` and `bash --posix` where they are.
    fn shells() -> Vec<Vec<&'static str>> {
        let mut found = vec![vec!["/bin/sh"]];
        if Path::new("/bin/dash").exists() {
            found.push(vec!["/bin/dash"]);
        }
        if Path::new("/bin/bash").exists() {
            found.push(vec!["/bin/bash", "--posix"]);
        }
        found
    }

    /// Runs the script of `act` under `shell` the way `hc exec` runs it, with `umask` and `env` set.
    fn run_under(
        shell: &[&str],
        umask: Option<&str>,
        env: &[(&str, &str)],
        act: &Act,
    ) -> Result<(), Problem> {
        let req = request(act, None).unwrap();
        let wrapper = format!(
            "{}exec \"$@\"",
            umask.map(|u| format!("umask {u}; ")).unwrap_or_default()
        );
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &wrapper, "x"]).args(shell).args(&req.args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.stdin(Stdio::null()).output().unwrap();
        interpret(Ok(ExecOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        }))
    }

    fn make(shell: &[&str], path: &str, kind: Kind) -> Result<(), Problem> {
        run_under(shell, None, &[], &Act::Make(path, kind))
    }

    fn mv(shell: &[&str], from: &str, to: &str) -> Result<(), Problem> {
        run_under(shell, None, &[], &Act::Move(from, to))
    }

    /// A folder of stand-ins for the commands the script runs, ahead of the real ones.
    fn shimmed(commands: &[(&str, &str)]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let real = std::env::var("PATH").unwrap();
        for (name, body) in commands {
            let file = dir.path().join(name);
            fs::write(
                &file,
                format!("#!/bin/sh\nPATH='{real}'\nexport PATH\n{body}\n"),
            )
            .unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!("{}:{real}", dir.path().display());
        (dir, path)
    }

    /// Whether the folder is on a volume that folds letter case.
    fn folds_case(dir: &Path) -> bool {
        fs::write(dir.join("Case-Probe"), "").unwrap();
        let folds = dir.join("case-probe").exists();
        fs::remove_file(dir.join("Case-Probe")).unwrap();
        folds
    }

    fn answered(code: i32, stdout: &str, stderr: &str) -> Result<ExecOutput, ExecError> {
        Ok(ExecOutput {
            code,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    #[test]
    fn only_a_complete_answer_of_the_known_shapes_counts() {
        for (said, want) in [
            ("done\n", Ok(())),
            ("exists\n", Err(Problem::Exists)),
            ("gone\n", Err(Problem::Gone)),
            ("refused nofolder\n", Err(Problem::NoFolder)),
            ("refused folder\n", Err(Problem::FolderNotWritable)),
            ("refused device\n", Err(Problem::OtherDevice)),
            ("refused tool mv\n", Err(Problem::NoTool("mv"))),
            ("refused tool mkdir\n", Err(Problem::NoTool("mkdir"))),
        ] {
            assert_eq!(interpret(answered(0, said, "")), want, "{said:?}");
        }
        // Anything else is not an answer: the change may or may not have been made, and it says so.
        for said in [
            "",
            "done",
            "done\ndone\n",
            "done \n",
            " done\n",
            "DONE\n",
            "ok\n",
            "refused\n",
            "refused tool\n",
            "refused tool rm -rf\n",
            "refused tool cat\n",
            "refused folder extra\n",
            "exists\nrefused folder\n",
            "done\r\n",
        ] {
            let got = interpret(answered(0, said, ""));
            assert!(
                matches!(&got, Err(Problem::Failed(why)) if why.contains("may or may not")),
                "{said:?}: {got:?}"
            );
        }
        // A script that did not end well, whatever it printed first, is its own words.
        let got = interpret(answered(
            1,
            "done\n",
            "mv: cannot move 'a' to 'b': Permission denied\n",
        ));
        assert_eq!(
            got,
            Err(Problem::Failed(
                "mv: cannot move 'a' to 'b': Permission denied".into()
            ))
        );
        assert!(
            matches!(interpret(answered(127, "", "")), Err(Problem::Failed(w)) if w.contains("127"))
        );
        // hc's own words: no sh there, a machine that is not connected, one that took too long.
        let said = |e| interpret(Err(e));
        assert_eq!(
            said(ExecError::Remote(
                "cannot start sh: No such file or directory (os error 2)".into()
            )),
            Err(Problem::NoShell)
        );
        assert_eq!(
            said(ExecError::Remote("machine lab is not connected".into())),
            Err(Problem::Failed("machine lab is not connected".into()))
        );
        let late = said(ExecError::TimedOut {
            seconds: 30.0,
            machine: Some("lab".into()),
        });
        assert!(matches!(late, Err(Problem::Failed(w)) if w.contains("may or may not")));
    }

    #[test]
    fn what_cannot_be_a_path_there_is_refused_before_anything_is_sent() {
        // "offline" would answer with an error if a request were made.
        for act in [
            Act::Make("relative", Kind::File),
            Act::Make("", Kind::File),
            Act::Make("/", Kind::Folder),
            Act::Make("/a/", Kind::Folder),
            Act::Make("/a/.", Kind::Folder),
            Act::Make("/a/..", Kind::File),
            Act::Make("/a\0b", Kind::File),
            Act::Make(r"C:\a\b", Kind::File),
            Act::Move("relative", "/b"),
            Act::Move("/a", "b"),
            Act::Move("/a", "/b/"),
            Act::Move("/a", "/b/.."),
            Act::Move("/a\0", "/b"),
        ] {
            assert!(
                matches!(request(&act, Some("offline")), Err(Problem::BadPath)),
                "{act:?}"
            );
        }
        assert!(request(&Act::Make("/a/b c", Kind::File), Some("offline")).is_ok());
        assert!(request(&Act::Move("/a/b", "/a/B"), Some("lab")).is_ok());
        let said = Act::Make(r"C:\a\b", Kind::File).describe(&Problem::BadPath, "lab");
        assert!(
            said.contains("C:\\a\\b") && said.contains("start with /"),
            "{said}"
        );
    }

    #[test]
    fn each_problem_says_what_was_asked_and_where() {
        let make = Act::Make("/srv/app/new.txt", Kind::File);
        let rename = Act::Move("/srv/a/old.txt", "/srv/b/new.txt");
        let say = |act: &Act, p| act.describe(&p, "lab");
        assert_eq!(
            say(&make, Problem::Exists),
            "/srv/app/new.txt already exists"
        );
        assert_eq!(
            say(&rename, Problem::Exists),
            "/srv/b/new.txt already exists"
        );
        let told = say(&make, Problem::NoFolder);
        assert!(
            told.contains("/srv/app") && told.contains("lab") && told.contains("does not exist"),
            "{told}"
        );
        // A rename changes two folders, and either may be the one that cannot be.
        let told = say(&rename, Problem::FolderNotWritable);
        assert!(
            told.contains("/srv/a and /srv/b") && told.contains("lab"),
            "{told}"
        );
        let told = say(&make, Problem::FolderNotWritable);
        assert!(
            told.contains("/srv/app") && !told.contains(" and /srv"),
            "{told}"
        );
        assert!(say(&rename, Problem::Gone).contains("/srv/a/old.txt"));
        assert!(say(&rename, Problem::OtherDevice).contains("does not copy"));
        assert!(say(&rename, Problem::NoTool("mv")).contains("no mv"));
        assert!(say(&make, Problem::NoShell).contains("sh"));
        assert_eq!(
            say(&make, Problem::Failed("No space left on device".into())),
            "cannot create /srv/app/new.txt on lab: No space left on device"
        );
        // A file in the root folder is in "/", not in "".
        let told = Act::Make("/n.txt", Kind::File).describe(&Problem::NoFolder, "lab");
        assert!(told.contains("folder / does"), "{told}");
    }

    #[test]
    fn every_shell_makes_a_name_once_and_says_why_when_it_cannot() {
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let dir = d.path();
            let (file, folder, link) = (at(dir, "f"), at(dir, "d"), at(dir, "l"));
            assert_eq!(make(&shell, &file, Kind::File), Ok(()), "{shell:?}");
            assert_eq!(make(&shell, &folder, Kind::Folder), Ok(()), "{shell:?}");
            symlink("nowhere", &link).unwrap();
            for (path, kind) in [
                (&file, Kind::File),
                (&file, Kind::Folder),
                (&folder, Kind::File),
                (&folder, Kind::Folder),
                (&link, Kind::File),
                (&link, Kind::Folder),
            ] {
                assert_eq!(
                    make(&shell, path, kind),
                    Err(Problem::Exists),
                    "{shell:?} {path} {kind:?}"
                );
            }
            // Its folder: missing, not a folder, not to be written in or entered.
            for kind in [Kind::File, Kind::Folder] {
                assert_eq!(
                    make(&shell, &at(dir, "no/x"), kind),
                    Err(Problem::NoFolder),
                    "{shell:?}"
                );
                assert_eq!(
                    make(&shell, &at(dir, "f/x"), kind),
                    Err(Problem::NoFolder),
                    "{shell:?}"
                );
                assert_eq!(
                    make(&shell, &at(dir, "no/deeper/x"), kind),
                    Err(Problem::NoFolder),
                    "{shell:?}"
                );
            }
            if !is_root() {
                let locked = at(dir, "locked");
                fs::create_dir(&locked).unwrap();
                for mode in [0o555, 0o755 & !0o111, 0o000] {
                    fs::set_permissions(&locked, fs::Permissions::from_mode(mode)).unwrap();
                    for kind in [Kind::File, Kind::Folder] {
                        let got = make(&shell, &at(dir, "locked/x"), kind);
                        assert_eq!(
                            got,
                            Err(Problem::FolderNotWritable),
                            "{shell:?} {mode:o} {kind:?}"
                        );
                    }
                }
                fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            }
            assert_eq!(
                names(dir).iter().filter(|n| n.starts_with('.')).count(),
                0,
                "{shell:?}"
            );
        }
    }

    #[test]
    fn a_name_taken_by_a_pipe_is_taken_and_never_waited_on() {
        use std::os::unix::ffi::OsStrExt as _;
        let d = tempfile::tempdir().unwrap();
        let fifo = d.path().join("pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: a NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let path = fifo.to_string_lossy().into_owned();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let got = shells()
                .iter()
                .map(|s| make(s, &path, Kind::File))
                .collect::<Vec<_>>();
            let _ = tx.send(got);
        });
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("waited for a reader of the pipe");
        assert!(got.iter().all(|g| *g == Err(Problem::Exists)), "{got:?}");
    }

    /// The modes, owners and groups of what `touch` and `mkdir` make, under the same umask and in the same folder.
    fn what_the_machine_makes(
        umask: &str,
        dir: &Path,
        name: &str,
        folder: bool,
    ) -> (u32, u32, u32) {
        let path = at(dir, name);
        let tool = if folder { "mkdir" } else { "touch" };
        let done = Command::new("/bin/sh")
            .args(["-c", &format!("umask {umask}; {tool} \"$1\""), "x", &path])
            .status()
            .unwrap();
        assert!(done.success());
        let m = fs::metadata(&path).unwrap();
        (m.mode() & 0o7777, m.uid(), m.gid())
    }

    #[test]
    fn what_is_made_is_what_the_machines_own_touch_and_mkdir_make_under_every_umask() {
        for shell in shells() {
            for umask in ["022", "077", "002", "027", "007", "000"] {
                let d = tempfile::tempdir().unwrap();
                for (kind, folder) in [(Kind::File, false), (Kind::Folder, true)] {
                    let theirs = what_the_machine_makes(umask, d.path(), "theirs", folder);
                    let ours = at(d.path(), "ours");
                    run_under(&shell, Some(umask), &[], &Act::Make(&ours, kind)).unwrap();
                    let m = fs::metadata(&ours).unwrap();
                    assert_eq!(
                        (m.mode() & 0o7777, m.uid(), m.gid()),
                        theirs,
                        "{shell:?} umask {umask} {kind:?}"
                    );
                    fs::remove_dir_all(&ours)
                        .or_else(|_| fs::remove_file(&ours))
                        .unwrap();
                    fs::remove_dir_all(at(d.path(), "theirs"))
                        .or_else(|_| fs::remove_file(at(d.path(), "theirs")))
                        .unwrap();
                }
            }
        }
    }

    /// A group the user is in other than the one new files get.
    fn another_group() -> Option<u32> {
        // SAFETY: plain syscalls; the buffer is as long as it is said to be.
        unsafe {
            let mut groups = [0 as libc::gid_t; 64];
            let n = libc::getgroups(64, groups.as_mut_ptr());
            let own = libc::getegid();
            groups[..usize::try_from(n).ok()?]
                .iter()
                .copied()
                .find(|g| *g != own)
        }
    }

    #[test]
    fn what_a_folder_hands_down_to_the_group_is_handed_down_as_to_touch_and_mkdir() {
        let Some(group) = another_group() else { return };
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let shared = d.path().join("shared");
            fs::create_dir(&shared).unwrap();
            std::os::unix::fs::chown(&shared, None, Some(group)).unwrap();
            fs::set_permissions(&shared, fs::Permissions::from_mode(0o2775)).unwrap();
            for (kind, folder) in [(Kind::File, false), (Kind::Folder, true)] {
                let theirs = what_the_machine_makes("002", &shared, "theirs", folder);
                let ours = at(&shared, "ours");
                run_under(&shell, Some("002"), &[], &Act::Make(&ours, kind)).unwrap();
                let m = fs::metadata(&ours).unwrap();
                assert_eq!(
                    (m.mode() & 0o7777, m.uid(), m.gid()),
                    theirs,
                    "{shell:?} {kind:?}"
                );
                assert_eq!(m.gid(), theirs.2);
                fs::remove_dir_all(&ours)
                    .or_else(|_| fs::remove_file(&ours))
                    .unwrap();
                fs::remove_dir_all(at(&shared, "theirs"))
                    .or_else(|_| fs::remove_file(at(&shared, "theirs")))
                    .unwrap();
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn inheritable_entry(dir: &Path) {
        let done = Command::new("chmod")
            .args([
                "+a",
                "group:everyone allow list,search,add_file,add_subdirectory,readattr,readextattr,readsecurity,read,write,file_inherit,directory_inherit",
            ])
            .arg(dir)
            .status()
            .unwrap();
        assert!(done.success());
    }

    /// The entries of a file's ACL, without the line that names the file.
    #[cfg(target_os = "macos")]
    fn entries_of(path: &str) -> Vec<String> {
        let out = Command::new("ls").arg("-lde").arg(path).output().unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1)
            .map(|l| l.trim().to_owned())
            .collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn what_a_folder_hands_down_as_an_acl_is_handed_down_as_to_touch_and_mkdir() {
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            inheritable_entry(d.path());
            for (kind, folder) in [(Kind::File, false), (Kind::Folder, true)] {
                what_the_machine_makes("022", d.path(), "theirs", folder);
                let theirs = entries_of(&at(d.path(), "theirs"));
                assert!(
                    theirs.iter().any(|l| l.contains("everyone")),
                    "the folder hands nothing down: {theirs:?}"
                );
                let ours = at(d.path(), "ours");
                run_under(&shell, Some("022"), &[], &Act::Make(&ours, kind)).unwrap();
                assert_eq!(entries_of(&ours), theirs, "{shell:?} {kind:?}");
                fs::remove_dir_all(&ours)
                    .or_else(|_| fs::remove_file(&ours))
                    .unwrap();
                fs::remove_dir_all(at(d.path(), "theirs"))
                    .or_else(|_| fs::remove_file(at(d.path(), "theirs")))
                    .unwrap();
            }
        }
    }

    #[test]
    fn of_many_makers_of_one_name_one_makes_it() {
        for shell in shells() {
            for kind in [Kind::File, Kind::Folder] {
                let d = tempfile::tempdir().unwrap();
                let target = at(d.path(), "contested");
                let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
                let handles: Vec<_> = (0..12)
                    .map(|_| {
                        let (shell, target, barrier) =
                            (shell.clone(), target.clone(), barrier.clone());
                        std::thread::spawn(move || {
                            barrier.wait();
                            make(&shell, &target, kind)
                        })
                    })
                    .collect();
                let got: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
                assert_eq!(
                    got.iter().filter(|g| g.is_ok()).count(),
                    1,
                    "{shell:?} {kind:?}: {got:?}"
                );
                assert!(
                    got.iter()
                        .all(|g| matches!(g, Ok(()) | Err(Problem::Exists))),
                    "{got:?}"
                );
                assert_eq!(names(d.path()), ["contested"]);
            }
        }
    }

    #[test]
    fn of_many_renames_onto_one_name_one_moves_and_the_others_keep_what_they_had() {
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let target = at(d.path(), "contested");
            let sources: Vec<String> = (0..12)
                .map(|i| put(d.path(), &format!("src{i}"), format!("{i}").as_bytes()))
                .collect();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(sources.len()));
            let handles: Vec<_> = sources
                .iter()
                .map(|from| {
                    let (shell, from, target, barrier) =
                        (shell.clone(), from.clone(), target.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        mv(&shell, &from, &target)
                    })
                })
                .collect();
            let got: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert_eq!(
                got.iter().filter(|g| g.is_ok()).count(),
                1,
                "{shell:?}: {got:?}"
            );
            assert!(
                got.iter()
                    .all(|g| matches!(g, Ok(()) | Err(Problem::Exists))),
                "{got:?}"
            );
            let winner = got.iter().position(Result::is_ok).unwrap();
            assert_eq!(fs::read_to_string(&target).unwrap(), winner.to_string());
            for (i, from) in sources.iter().enumerate().filter(|(i, _)| *i != winner) {
                assert_eq!(
                    fs::read_to_string(from).unwrap(),
                    i.to_string(),
                    "{shell:?}"
                );
            }
            assert_eq!(
                names(d.path()).len(),
                sources.len(),
                "{shell:?}: one name per file, nothing extra"
            );
        }
    }

    #[test]
    fn a_rename_takes_the_name_first_so_that_nobody_can_make_it_before_the_move() {
        // The stand-in `mv` is the other maker: just before the real move it tries to make the
        // name, the way `touch` would (and says whether it could).
        let (shims, path) = shimmed(&[(
            "mv",
            r#"case $1 in -f|-n|-nT)
  last=$(eval echo \$$#)
  ( set -C; echo racer > "$last" ) 2>/dev/null && echo made > "$YZ_RACE" || echo refused > "$YZ_RACE"
esac
exec mv "$@""#,
        )]);
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let from = put(d.path(), "from.txt", b"mine");
            let to = at(d.path(), "to.txt");
            let race = at(shims.path(), "race");
            let env = [("PATH", path.as_str()), ("YZ_RACE", race.as_str())];
            assert_eq!(
                run_under(&shell, None, &env, &Act::Move(&from, &to)),
                Ok(()),
                "{shell:?}"
            );
            assert_eq!(
                fs::read_to_string(&race).unwrap().trim(),
                "refused",
                "{shell:?}: the name was free to make"
            );
            assert_eq!(fs::read_to_string(&to).unwrap(), "mine", "{shell:?}");
            assert_eq!(names(d.path()), ["to.txt"]);
        }
    }

    #[test]
    fn a_folder_is_not_moved_onto_a_name_made_just_before_the_move() {
        // The other maker gets in just before the real move: a file, a link to nothing, a folder.
        let (_shims, path) = shimmed(&[(
            "mv",
            r#"case $1 in -f|-n|-nT)
  for last; do :; done
  case $YZ_KIND in
    file) echo racer > "$last" ;;
    link) ln -s nowhere "$last" ;;
    folder) mkdir "$last" ;;
  esac
esac
exec mv "$@""#,
        )]);
        for shell in shells() {
            for kind in ["file", "link", "folder"] {
                let d = tempfile::tempdir().unwrap();
                let from = at(d.path(), "from");
                fs::create_dir(&from).unwrap();
                fs::write(at(d.path(), "from/x"), "x").unwrap();
                let to = at(d.path(), "to");
                let env = [("PATH", path.as_str()), ("YZ_KIND", kind)];
                let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                // GNU mv asks the system not to replace; the others look first, and where that is
                // not enough a folder that took the name has it moved into it: said, never silent.
                match &got {
                    Err(Problem::Exists) => {
                        assert_eq!(
                            fs::read_to_string(at(d.path(), "from/x")).unwrap(),
                            "x",
                            "{shell:?} {kind}"
                        );
                    }
                    Err(Problem::Failed(why)) if kind == "folder" => {
                        assert!(why.contains("moved into") && why.contains(&to), "{why}");
                        assert_eq!(fs::read_to_string(at(d.path(), "to/from/x")).unwrap(), "x");
                    }
                    other => panic!("{shell:?} {kind}: {other:?}"),
                }
                if kind == "file" {
                    assert_eq!(fs::read_to_string(&to).unwrap(), "racer\n");
                }
                if kind == "link" {
                    assert!(fs::symlink_metadata(&to).unwrap().is_symlink());
                }
            }
        }
    }

    #[test]
    fn a_move_that_fails_leaves_the_entry_where_it_was_and_no_empty_file_in_its_name() {
        let (_shims, path) =
            shimmed(&[("mv", r#"echo "mv: simulated: disk on fire" >&2; exit 1"#)]);
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let from = put(d.path(), "from.txt", b"mine");
            let folder = at(d.path(), "folder");
            fs::create_dir(&folder).unwrap();
            for (src, dst) in [
                (&from, at(d.path(), "to.txt")),
                (&folder, at(d.path(), "to")),
            ] {
                let got = run_under(&shell, None, &[("PATH", &path)], &Act::Move(src, &dst));
                assert!(
                    matches!(&got, Err(Problem::Failed(w)) if w.contains("disk on fire")),
                    "{shell:?}: {got:?}"
                );
                assert!(
                    !Path::new(&dst).exists(),
                    "{shell:?}: a name was left behind"
                );
            }
            assert_eq!(names(d.path()), ["folder", "from.txt"], "{shell:?}");
            assert_eq!(fs::read_to_string(&from).unwrap(), "mine");
        }
    }

    #[test]
    fn a_move_that_is_told_to_stop_gives_the_name_back_once_it_is_over_and_says_it_was_stopped() {
        // The stand-in `mv` holds the move up, then fails or carries on; the script is asked to end meanwhile.
        for (hold, moved) in [("exit 1", false), ("exec mv \"$@\"", true)] {
            let (shims, path) =
                shimmed(&[("mv", &format!("touch \"$YZ_STARTED\"; sleep 1; {hold}"))]);
            for shell in shells() {
                let d = tempfile::tempdir().unwrap();
                let from = put(d.path(), "from.txt", b"mine");
                let to = at(d.path(), "to.txt");
                let started = at(shims.path(), "started");
                let _ = fs::remove_file(&started);
                let req = request(&Act::Move(&from, &to), None).unwrap();
                let child = Command::new(shell[0])
                    .args(&shell[1..])
                    .args(&req.args)
                    .env("PATH", &path)
                    .env("YZ_STARTED", &started)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                let waited = std::time::Instant::now();
                while !Path::new(&started).exists() && waited.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(
                    Path::new(&started).exists(),
                    "{shell:?}: the move did not start"
                );
                assert!(
                    Path::new(&to).exists(),
                    "{shell:?}: the name was not taken first"
                );
                // SAFETY: a signal to the child this test started.
                unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
                let out = child.wait_with_output().unwrap();
                assert_eq!(out.status.code(), Some(1), "{shell:?}");
                assert!(
                    String::from_utf8_lossy(&out.stderr).contains("may or may not"),
                    "{shell:?}"
                );
                assert!(
                    out.stdout.is_empty(),
                    "{shell:?}: no answer, since none was reached"
                );
                if moved {
                    assert_eq!(
                        (fs::read_to_string(&to).unwrap(), Path::new(&from).exists()),
                        ("mine".into(), false)
                    );
                } else {
                    assert_eq!(
                        (fs::read_to_string(&from).unwrap(), Path::new(&to).exists()),
                        ("mine".into(), false),
                        "{shell:?}: an empty file stayed in the name"
                    );
                }
                assert_eq!(names(d.path()).len(), 1, "{shell:?}: {:?}", names(d.path()));
            }
        }
    }

    #[test]
    fn a_program_the_script_needs_is_named_when_it_is_missing() {
        // A PATH with the programs the scripts need, but one.
        let all = ["mv", "stat", "rm", "mkdir"];
        for missing in all {
            let bin = tempfile::tempdir().unwrap();
            for tool in all.iter().filter(|t| **t != missing) {
                let found = Command::new("/bin/sh")
                    .args(["-c", &format!("command -v {tool}")])
                    .output()
                    .unwrap();
                symlink(
                    String::from_utf8(found.stdout).unwrap().trim(),
                    bin.path().join(tool),
                )
                .unwrap();
            }
            let env = [("PATH", bin.path().to_str().unwrap())];
            let d = tempfile::tempdir().unwrap();
            let (from, to) = (put(d.path(), "from", b"x"), at(d.path(), "to"));
            let act = if missing == "mkdir" {
                Act::Make(&to, Kind::Folder)
            } else {
                Act::Move(&from, &to)
            };
            for shell in shells() {
                let got = run_under(&shell, None, &env, &act);
                assert_eq!(
                    got,
                    Err(Problem::NoTool(missing)),
                    "{shell:?} without {missing}"
                );
            }
            assert_eq!(names(d.path()), ["from"], "without {missing}");
        }
        // Making a file needs none of them.
        let none = tempfile::tempdir().unwrap();
        let d = tempfile::tempdir().unwrap();
        let to = at(d.path(), "to");
        let env = [("PATH", none.path().to_str().unwrap())];
        assert_eq!(
            run_under(&["/bin/sh"], None, &env, &Act::Make(&to, Kind::File)),
            Ok(())
        );
    }

    #[test]
    fn a_rename_does_not_copy_across_file_systems() {
        // Only where the temp folder and /dev/shm or another volume are different devices.
        let d = tempfile::tempdir().unwrap();
        let here = fs::metadata(d.path()).unwrap().dev();
        let Some(other) = [
            "/dev/shm",
            "/Volumes/Macintosh HD/private/tmp",
            "/System/Volumes/VM",
        ]
        .into_iter()
        .find(|p| {
            fs::metadata(p)
                .map(|m| m.dev() != here && m.permissions().mode() & 0o222 != 0)
                .unwrap_or(false)
                && tempfile::tempdir_in(p).is_ok()
        }) else {
            return;
        };
        let elsewhere = tempfile::tempdir_in(other).unwrap();
        let from = put(d.path(), "from", b"mine");
        let to = at(elsewhere.path(), "to");
        assert_eq!(mv(&["/bin/sh"], &from, &to), Err(Problem::OtherDevice));
        assert_eq!(fs::read_to_string(&from).unwrap(), "mine");
        assert!(!Path::new(&to).exists());
    }

    #[test]
    fn a_change_of_case_only_is_the_same_entry_only_where_the_system_says_so() {
        let d = tempfile::tempdir().unwrap();
        let from = put(d.path(), "readme.md", b"x");
        for shell in shells() {
            let upper = at(d.path(), "README.md");
            if folds_case(d.path()) {
                assert_eq!(mv(&shell, &from, &upper), Ok(()), "{shell:?}");
                assert_eq!(names(d.path()), ["README.md"]);
                assert_eq!(mv(&shell, &upper, &from), Ok(()), "{shell:?}");
            } else {
                // Two names: the one that is there is not replaced because it reads like the other.
                put(d.path(), "README.md", b"theirs");
                assert_eq!(mv(&shell, &from, &upper), Err(Problem::Exists), "{shell:?}");
                fs::remove_file(&upper).unwrap();
            }
            assert_eq!(names(d.path()), ["readme.md"]);
        }
        // Two links that point at one file are two entries, and are told apart by what they are.
        let target = put(d.path(), "t", b"t");
        let (a, b) = (at(d.path(), "Link"), at(d.path(), "link"));
        symlink(&target, &a).unwrap();
        if !folds_case(d.path()) {
            symlink(&target, &b).unwrap();
            assert_eq!(mv(&["/bin/sh"], &a, &b), Err(Problem::Exists));
            assert!(fs::symlink_metadata(&a).unwrap().is_symlink());
        }
    }

    /// Against a real hc, in a session and with a machine of its own (never the user's core):
    /// `YZ_REAL_HC=/tmp/x/hcx YZ_REAL_MACHINE=lab cargo test -p yuzora-files --lib -- --ignored
    /// --exact remote_entry::tests::a_real_hc_makes_and_renames_end_to_end --nocapture`.
    #[test]
    #[ignore = "needs YZ_REAL_HC and YZ_REAL_MACHINE"]
    fn a_real_hc_makes_and_renames_end_to_end() {
        let hc = std::path::PathBuf::from(std::env::var("YZ_REAL_HC").unwrap());
        let machine = std::env::var("YZ_REAL_MACHINE").unwrap();
        let on = Some(machine.as_str());
        yuzora_core::remote::set_hc_program(move || hc.clone());
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let started = std::time::Instant::now();
        let file = at(dir, "new file.txt");
        fsops::create_file(&file, on).unwrap();
        let after_file = started.elapsed();
        fsops::create_dir(&at(dir, "folder"), on).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"");
        assert!(dir.join("folder").is_dir());
        for taken in [&file, &at(dir, "folder")] {
            assert!(
                fsops::create_file(taken, on)
                    .unwrap_err()
                    .contains("already exists")
            );
            assert!(
                fsops::create_dir(taken, on)
                    .unwrap_err()
                    .contains("already exists")
            );
        }
        let renamed = at(dir, "it's \"renamed\"\nname");
        fsops::rename(&file, &renamed, on).unwrap();
        assert!(Path::new(&renamed).is_file() && !Path::new(&file).exists());
        fsops::rename(&at(dir, "folder"), &at(dir, "folder2"), on).unwrap();
        fs::write(at(dir, "other"), "theirs").unwrap();
        let refused = fsops::rename(&renamed, &at(dir, "other"), on).unwrap_err();
        assert!(refused.contains("already exists"), "{refused}");
        assert_eq!(fs::read_to_string(at(dir, "other")).unwrap(), "theirs");
        let e = fsops::create_file(&at(dir, "no/parent.txt"), on).unwrap_err();
        assert!(e.contains(&at(dir, "no")), "{e}");
        let e = fsops::rename(&at(dir, "ghost"), &at(dir, "x"), on).unwrap_err();
        assert!(e.contains("ghost"), "{e}");
        println!(
            "one create over hc: {after_file:?}; the whole run: {:?}",
            started.elapsed()
        );
        assert_eq!(names(dir), ["folder2", "it's \"renamed\"\nname", "other"]);
    }

    /// On Linux machines, in throwaway containers (`docker info` must work, and the images are
    /// pulled): the scripts as an ordinary user, against folders with default ACLs and setgid, makers
    /// and movers that race, two file systems, programs that are missing.
    /// `YZ_DOCKER=1 cargo test --locked -p yuzora-files --lib -- --ignored --exact
    /// remote_entry::tests::linux_entries_on_real_machines --nocapture`.
    #[test]
    #[ignore = "needs docker and YZ_DOCKER=1"]
    fn linux_entries_on_real_machines() {
        assert!(std::env::var("YZ_DOCKER").is_ok(), "set YZ_DOCKER=1");
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("make.sh"), MAKE_SCRIPT).unwrap();
        fs::write(dir.path().join("move.sh"), MOVE_SCRIPT).unwrap();
        fs::write(
            dir.path().join("entry.sh"),
            include_str!("../tests/docker/remote-entry.sh"),
        )
        .unwrap();
        let mut failures = Vec::new();
        for (image, shell) in [
            ("alpine:3", "sh"),
            ("ubuntu:24.04", "dash"),
            ("ubuntu:24.04", "bash --posix"),
            ("ubuntu:25.10", "sh"),
        ] {
            let out = Command::new("docker")
                .args(["run", "--rm", "-v"])
                .arg(format!("{}:/w:ro", dir.path().display()))
                .args(["--tmpfs", "/mnt/a:mode=1777", "--tmpfs", "/mnt/b:mode=1777"])
                .args([image, "sh", "/w/entry.sh", shell])
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            println!(
                "{image} ({shell})\n{text}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            if !text.contains("RESULT fails=0") {
                failures.push(format!("{image} ({shell})"));
            }
        }
        assert!(failures.is_empty(), "failing: {failures:?}");
    }
}
