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
/// and exits 0: `done`, `exists`, `gone`, `refused folder|nofolder|device|rename` or
/// `refused tool mv|stat|rm|mkdir|rmdir`. Any other end leaves the entry where it was.
///
/// What is at `to` is never replaced and never followed (a link there is a name, not a folder
/// to move into). Plain `mv` cannot promise that: it takes a folder, or a link to one, at `to`
/// for the place to move into, and replaces a file. So:
/// - GNU `mv -nT` does it in one call (`renameat2` with `RENAME_NOREPLACE`) where the kernel and
///   file system have that, and `-T` keeps it from following `to` where they do not.
/// - Anywhere else the script first takes the name `to` (an empty file made the way `MAKE_SCRIPT`
///   makes one, or an empty folder for a folder), which nobody can then make, and renames over
///   that with a call that cannot take `to` for a place to move into: `perl`'s `rename`, or
///   `mv -fT` where `mv` has `-T` (BusyBox). A rename onto a name that is not a folder, or onto an
///   empty folder, is all it asks of the system. A machine with none of these (macOS has `perl`)
///   gets `refused rename`: the script does not fall back to a plain `mv`.
/// - If the rename fails, or the source is gone by then, the name taken is given back, but only
///   if it is still the one made (`device:inode`, and a folder only when it is empty).
/// - A name that is the same entry under other letters (`$3`) is renamed in place, on a file
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
    /// Nothing there can rename without taking the new name for a folder to move into.
    NoSafeRename,
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
            Problem::NoSafeRename => format!(
                "cannot {verb} {subject}: {machine} has no GNU mv, no mv with -T and no perl, one of which this needs so that nobody who can write the folder can make the rename land in another folder"
            ),
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
    NoSafeRename,
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
        ["refused", "rename"] => Some(Verdict::NoSafeRename),
        ["refused", "tool", tool] => ["mkdir", "mv", "stat", "rm", "rmdir"]
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
        Some(Verdict::NoSafeRename) => Err(Problem::NoSafeRename),
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
            ("refused rename\n", Err(Problem::NoSafeRename)),
            ("refused tool rmdir\n", Err(Problem::NoTool("rmdir"))),
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
        let told = say(&rename, Problem::NoSafeRename);
        assert!(
            told.contains("GNU mv") && told.contains("perl") && told.contains("lab"),
            "{told}"
        );
        assert_eq!(
            told,
            "cannot rename /srv/a/old.txt: lab has no perl, which this needs (or GNU mv, or an mv with -T): a plain mv could follow a link into another folder"
        );
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

    /// Whether this machine's own `mv` is GNU's, which renames without taking the name first.
    fn mv_is_gnu() -> bool {
        Command::new("mv")
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("GNU coreutils"))
            .unwrap_or(false)
    }

    fn has_perl() -> bool {
        Command::new("perl").args(["-e", "1"]).output().is_ok()
    }

    /// The ways the script runs here, each with whether it takes the new name first: on this
    /// machine's own `mv`, and on one that answers `--version` and `--help` as BSD's does, so that
    /// the script renames with `perl` (left out where there is none).
    fn ways() -> Vec<(Option<&'static str>, bool)> {
        let mut found = vec![(None, !mv_is_gnu())];
        if has_perl() {
            found.push((Some("bsd"), true));
        }
        found
    }

    fn env_for<'a>(
        path: &'a str,
        looks: Option<&'a str>,
        more: &[(&'a str, &'a str)],
    ) -> Vec<(&'a str, &'a str)> {
        let mut env = vec![("PATH", path)];
        if let Some(looks) = looks {
            env.push(("YZ_MVLOOKS", looks));
        }
        env.extend_from_slice(more);
        env
    }

    /// Stand-ins for the programs that do the rename (`mv`, and `perl` where `mv` cannot). Each runs
    /// `before` ahead of the real one (`$real`), or in place of it when `before` ends with `exit`.
    /// `before` sees the two names as `$from` and `$to`. With `YZ_MVLOOKS` set, `mv` answers
    /// `--version` and `--help` as BSD's does.
    fn rename_shims(before: &str) -> [(&'static str, String); 2] {
        let hook = format!("for a in \"$@\"; do from=$to; to=$a; done\n{before}\n");
        [
            (
                "mv",
                format!(
                    "case $1 in --version|--help) [ -z \"$YZ_MVLOOKS\" ] || {{ echo 'usage: mv [-f | -i | -n] [-hv] source target' >&2; exit 64; }} ;; esac\nreal=mv\ncase $1 in -f|-n|-T|-fT|-nT|--no-copy) {hook} ;; esac\nexec mv \"$@\""
                ),
            ),
            (
                "perl",
                format!("real=perl\ncase $1 in -e) {hook} ;; esac\nexec perl \"$@\""),
            ),
        ]
    }

    fn before_the_rename(before: &str) -> (tempfile::TempDir, String) {
        let shims = rename_shims(before);
        shimmed(&[(shims[0].0, &shims[0].1), (shims[1].0, &shims[1].1)])
    }

    /// An entry named `entry` of the kind `kind` in `dir`.
    fn entry_of_kind(dir: &Path, kind: &str) -> String {
        let path = at(dir, "entry");
        match kind {
            "file" => fs::write(&path, "mine").unwrap(),
            "link" => symlink("somewhere", &path).unwrap(),
            _ => {
                fs::create_dir(&path).unwrap();
                fs::write(dir.join("entry/x"), "x").unwrap();
            }
        }
        path
    }

    /// Whether `path` is still the entry `entry_of_kind` made.
    fn is_the_entry_of_kind(path: &str, kind: &str) -> bool {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return false;
        };
        match kind {
            "file" => meta.is_file() && fs::read_to_string(path).unwrap() == "mine",
            "link" => meta.is_symlink() && fs::read_link(path).unwrap() == Path::new("somewhere"),
            _ => meta.is_dir() && fs::read_to_string(format!("{path}/x")).unwrap() == "x",
        }
    }

    #[test]
    fn a_name_made_just_before_the_rename_is_never_replaced() {
        // The stand-in is the other maker: just before the real rename it tries to make the name,
        // the way `touch` would, and says whether it could. Where the name was taken first it cannot;
        // where the system refuses to replace (GNU), it can and the rename gives way to it.
        let (shims, path) = before_the_rename(
            r#"( set -C; echo racer > "$to" ) 2>/dev/null && echo made > "$YZ_RACE" || echo refused > "$YZ_RACE""#,
        );
        for (looks, takes_first) in ways() {
            for shell in shells() {
                let d = tempfile::tempdir().unwrap();
                let from = put(d.path(), "from.txt", b"mine");
                let to = at(d.path(), "to.txt");
                let race = at(shims.path(), "race");
                let _ = fs::remove_file(&race);
                let env = env_for(&path, looks, &[("YZ_RACE", &race)]);
                let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                let label = format!("{shell:?} {looks:?}");
                let raced = fs::read_to_string(&race).unwrap();
                if takes_first {
                    assert_eq!((got, raced.trim()), (Ok(()), "refused"), "{label}");
                    assert_eq!(fs::read_to_string(&to).unwrap(), "mine", "{label}");
                    assert_eq!(names(d.path()), ["to.txt"], "{label}");
                } else {
                    assert_eq!(
                        (got, raced.trim()),
                        (Err(Problem::Exists), "made"),
                        "{label}"
                    );
                    assert_eq!(fs::read_to_string(&to).unwrap(), "racer\n", "{label}");
                    assert_eq!(fs::read_to_string(&from).unwrap(), "mine", "{label}");
                }
            }
        }
    }

    #[test]
    fn a_folder_is_never_moved_into_or_onto_a_name_made_just_before_the_rename() {
        // The other maker gets in just before the real rename: a file, a link to nothing, a folder.
        let (_shims, path) = before_the_rename(
            r#"case $YZ_KIND in
    file) echo racer > "$to" ;;
    link) ln -s nowhere "$to" ;;
    folder) mkdir "$to" ;;
  esac"#,
        );
        for (looks, _) in ways() {
            for shell in shells() {
                for kind in ["file", "link", "folder"] {
                    let d = tempfile::tempdir().unwrap();
                    let from = at(d.path(), "from");
                    fs::create_dir(&from).unwrap();
                    fs::write(at(d.path(), "from/x"), "x").unwrap();
                    let to = at(d.path(), "to");
                    let env = env_for(&path, looks, &[("YZ_KIND", kind)]);
                    let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                    let label = format!("{shell:?} {looks:?} {kind}: {got:?}");
                    assert!(!Path::new(&to).join("from").exists(), "{label}");
                    match got {
                        Ok(()) => {
                            assert_eq!(fs::read_to_string(at(d.path(), "to/x")).unwrap(), "x");
                            assert!(!Path::new(&from).exists(), "{label}");
                        }
                        Err(Problem::Exists) => {
                            assert_eq!(fs::read_to_string(format!("{from}/x")).unwrap(), "x");
                        }
                        Err(_) => panic!("{label}"),
                    }
                }
            }
        }
    }

    #[test]
    fn a_rename_never_follows_a_name_that_is_swapped_for_a_link_to_a_folder() {
        // Someone who can write the folder takes away whatever holds the new name and puts a link
        // to a folder of the user's there, just before the rename. Nothing may end up in that folder.
        let (_shims, path) = before_the_rename(
            "rm -f -- \"$to\"; rmdir -- \"$to\" 2>/dev/null; ln -s \"$YZ_VICTIM\" \"$to\"",
        );
        for (looks, _) in ways() {
            for shell in shells() {
                for kind in ["file", "link", "folder"] {
                    let d = tempfile::tempdir().unwrap();
                    let victim = tempfile::tempdir().unwrap();
                    let from = entry_of_kind(d.path(), kind);
                    let to = at(d.path(), "renamed");
                    let held_in_victim = at(victim.path(), "entry");
                    if kind != "folder" {
                        fs::write(&held_in_victim, "precious").unwrap();
                    }
                    let env = env_for(
                        &path,
                        looks,
                        &[("YZ_VICTIM", victim.path().to_str().unwrap())],
                    );
                    let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                    let label = format!("{shell:?} {looks:?} {kind}: {got:?}");
                    if kind == "folder" {
                        assert!(names(victim.path()).is_empty(), "{label}");
                    } else {
                        assert_eq!(names(victim.path()), ["entry"], "{label}");
                        assert_eq!(
                            fs::read_to_string(&held_in_victim).unwrap(),
                            "precious",
                            "{label}"
                        );
                    }
                    match got {
                        Ok(()) => {
                            assert!(is_the_entry_of_kind(&to, kind), "{label}");
                            assert!(!Path::new(&from).exists(), "{label}");
                        }
                        Err(Problem::Exists) => {
                            assert!(is_the_entry_of_kind(&from, kind), "{label}");
                        }
                        Err(_) => panic!("{label}"),
                    }
                    assert!(
                        !names(d.path()).iter().any(|n| n.starts_with('.')),
                        "{label}: {:?}",
                        names(d.path())
                    );
                }
            }
        }
    }

    #[test]
    fn a_name_that_was_swapped_in_is_not_taken_for_the_one_that_was_held_even_with_the_same_inode_number()
     {
        // File systems hand a freed inode number to the next entry made, so a link put where the held
        // name was can have the number the held name had. A `stat` that gives every entry one
        // identity stands for that. The rename fails after the swap; the link must stay.
        let shims = rename_shims(
            "rm -f -- \"$to\"; rmdir -- \"$to\" 2>/dev/null; ln -s \"$YZ_VICTIM\" \"$to\"; exit 1",
        );
        let (_dir, path) = shimmed(&[
            (shims[0].0, &shims[0].1),
            (shims[1].0, &shims[1].1),
            (
                "stat",
                "case \"$*\" in *%d:%i*) echo 1:1 ;; *) exec stat \"$@\" ;; esac",
            ),
        ]);
        for (looks, takes_first) in ways() {
            if !takes_first {
                continue;
            }
            for shell in shells() {
                for kind in ["file", "folder"] {
                    let d = tempfile::tempdir().unwrap();
                    let victim = tempfile::tempdir().unwrap();
                    let from = entry_of_kind(d.path(), kind);
                    let to = at(d.path(), "renamed");
                    let env = env_for(
                        &path,
                        looks,
                        &[("YZ_VICTIM", victim.path().to_str().unwrap())],
                    );
                    let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                    let label = format!("{shell:?} {looks:?} {kind}: {got:?}");
                    assert_eq!(got, Err(Problem::Exists), "{label}");
                    assert_eq!(
                        fs::read_link(&to).unwrap(),
                        victim.path(),
                        "{label}: the link that took the name is gone"
                    );
                    assert!(is_the_entry_of_kind(&from, kind), "{label}");
                }
            }
        }
    }

    #[test]
    fn a_rename_whose_source_vanishes_meanwhile_gives_back_the_name_it_took_and_says_so() {
        for (what, vanish) in [
            ("moved away", "mv -- \"$from\" \"$YZ_ELSEWHERE\""),
            ("removed", "rm -r -- \"$from\""),
        ] {
            let (_shims, path) = before_the_rename(vanish);
            for (looks, _) in ways() {
                for shell in shells() {
                    for kind in ["file", "link", "folder"] {
                        let d = tempfile::tempdir().unwrap();
                        let elsewhere = tempfile::tempdir().unwrap();
                        let from = entry_of_kind(d.path(), kind);
                        let to = at(d.path(), "renamed");
                        let env = env_for(
                            &path,
                            looks,
                            &[("YZ_ELSEWHERE", elsewhere.path().to_str().unwrap())],
                        );
                        let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                        let label = format!("{shell:?} {looks:?} {kind} {what}: {got:?}");
                        assert_eq!(got, Err(Problem::Gone), "{label}");
                        assert!(
                            names(d.path()).is_empty(),
                            "{label}: left {:?}",
                            names(d.path())
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_move_that_fails_leaves_the_entry_where_it_was_and_no_empty_file_in_its_name() {
        let (_shims, path) = before_the_rename("echo \"mv: simulated: disk on fire\" >&2; exit 1");
        for (looks, _) in ways() {
            for shell in shells() {
                let d = tempfile::tempdir().unwrap();
                let from = put(d.path(), "from.txt", b"mine");
                let folder = at(d.path(), "folder");
                fs::create_dir(&folder).unwrap();
                for (src, dst) in [
                    (&from, at(d.path(), "to.txt")),
                    (&folder, at(d.path(), "to")),
                ] {
                    let env = env_for(&path, looks, &[]);
                    let got = run_under(&shell, None, &env, &Act::Move(src, &dst));
                    assert!(
                        matches!(&got, Err(Problem::Failed(w)) if w.contains("disk on fire")),
                        "{shell:?} {looks:?}: {got:?}"
                    );
                    assert!(
                        !Path::new(&dst).exists(),
                        "{shell:?} {looks:?}: a name was left behind"
                    );
                }
                assert_eq!(
                    names(d.path()),
                    ["folder", "from.txt"],
                    "{shell:?} {looks:?}"
                );
                assert_eq!(fs::read_to_string(&from).unwrap(), "mine");
            }
        }
    }

    #[test]
    fn a_move_that_is_told_to_stop_gives_the_name_back_once_it_is_over_and_says_it_was_stopped() {
        // The stand-in holds the rename up, then fails or carries on; the script is asked to end meanwhile.
        for (hold, moved) in [("exit 1", false), ("exec \"$real\" \"$@\"", true)] {
            let (shims, path) =
                before_the_rename(&format!("touch \"$YZ_STARTED\"; sleep 1; {hold}"));
            for (looks, takes_first) in ways() {
                for shell in shells() {
                    let d = tempfile::tempdir().unwrap();
                    let from = put(d.path(), "from.txt", b"mine");
                    let to = at(d.path(), "to.txt");
                    let started = at(shims.path(), "started");
                    let _ = fs::remove_file(&started);
                    let req = request(&Act::Move(&from, &to), None).unwrap();
                    let mut cmd = Command::new(shell[0]);
                    cmd.args(&shell[1..])
                        .args(&req.args)
                        .env("PATH", &path)
                        .env("YZ_STARTED", &started)
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    if let Some(looks) = looks {
                        cmd.env("YZ_MVLOOKS", looks);
                    }
                    let child = cmd.spawn().unwrap();
                    let waited = std::time::Instant::now();
                    while !Path::new(&started).exists() && waited.elapsed() < Duration::from_secs(5)
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let label = format!("{shell:?} {looks:?} moved={moved}");
                    assert!(
                        Path::new(&started).exists(),
                        "{label}: the move did not start"
                    );
                    assert_eq!(
                        Path::new(&to).exists(),
                        takes_first,
                        "{label}: whether the name was taken first"
                    );
                    // SAFETY: a signal to the child this test started.
                    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
                    let out = child.wait_with_output().unwrap();
                    assert_eq!(out.status.code(), Some(1), "{label}");
                    assert!(
                        String::from_utf8_lossy(&out.stderr).contains("may or may not"),
                        "{label}"
                    );
                    assert!(
                        out.stdout.is_empty(),
                        "{label}: no answer, since none was reached"
                    );
                    if moved {
                        assert_eq!(
                            (fs::read_to_string(&to).unwrap(), Path::new(&from).exists()),
                            ("mine".into(), false),
                            "{label}"
                        );
                    } else {
                        assert_eq!(
                            (fs::read_to_string(&from).unwrap(), Path::new(&to).exists()),
                            ("mine".into(), false),
                            "{label}: an empty file stayed in the name"
                        );
                    }
                    assert_eq!(names(d.path()).len(), 1, "{label}: {:?}", names(d.path()));
                }
            }
        }
    }

    #[test]
    fn a_machine_with_no_rename_that_leaves_a_name_alone_is_refused_not_given_a_plain_mv() {
        // A mv that has no -T and says so as BSD's does, and no perl on the path.
        let bin = tempfile::tempdir().unwrap();
        let shim = bin.path().join("mv");
        fs::write(
            &shim,
            "#!/bin/sh\ncase $1 in --version|--help) echo 'usage: mv [-f | -i | -n] [-hv] source target' >&2; exit 64 ;; esac\nexec /bin/mv \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        for tool in ["stat", "rm", "rmdir", "mkdir"] {
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
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let from = put(d.path(), "from", b"mine");
            let folder = at(d.path(), "folder");
            fs::create_dir(&folder).unwrap();
            for (src, dst) in [(&from, at(d.path(), "to")), (&folder, at(d.path(), "to2"))] {
                let got = run_under(&shell, None, &env, &Act::Move(src, &dst));
                assert_eq!(got, Err(Problem::NoSafeRename), "{shell:?}");
            }
            assert_eq!(names(d.path()), ["folder", "from"], "{shell:?}");
        }
    }

    /// A `stat` of a machine that has no `device:inode` to give: that form of the question fails there.
    const NO_IDENTITY: &str = "case \"$*\" in *%d:%i*) exit 1 ;; *) exec stat \"$@\" ;; esac";

    /// A `stat` that tells the device of what a path leads to by where that is: under `$YZ_OTHERFS` is
    /// device 2, anywhere else device 1. Like the real one it looks at a link itself (so, at the folder
    /// the link is in) unless the path goes on through it, as `link/.` does.
    fn device_by_place() -> (tempfile::TempDir, String) {
        shimmed(&[(
            "stat",
            r#"case "$*" in
  *%d:%i*) exec stat "$@" ;;
  *%d*) ;;
  *) exec stat "$@" ;;
esac
for p; do :; done
if [ -L "$p" ]; then here=$(cd "$(dirname "$p")" && pwd -P)
elif [ -d "$p" ]; then here=$(cd "$p" && pwd -P)
else here=$(cd "$(dirname "$p")" && pwd -P)
fi
case $here in "$YZ_OTHERFS"|"$YZ_OTHERFS"/*) echo 2 ;; *) echo 1 ;; esac"#,
        )])
    }

    /// `home` holds a link to a folder on another file system (`proj`, to `other/proj`) and one to a folder
    /// on the same one (`same`, to `real`); the `stat` of `device_by_place` is what tells them apart.
    struct Linked {
        _dir: tempfile::TempDir,
        home: std::path::PathBuf,
        other: std::path::PathBuf,
    }

    fn linked() -> Linked {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let (home, other) = (root.join("home"), root.join("other"));
        fs::create_dir_all(other.join("proj")).unwrap();
        fs::create_dir_all(home.join("real")).unwrap();
        symlink(other.join("proj"), home.join("proj")).unwrap();
        symlink(home.join("real"), home.join("same")).unwrap();
        Linked {
            _dir: dir,
            home,
            other,
        }
    }

    #[test]
    fn a_change_inside_a_folder_that_is_a_link_is_judged_by_the_folder_it_points_to() {
        let (_shim, path) = device_by_place();
        for shell in shells() {
            let l = linked();
            let env = [
                ("PATH", path.as_str()),
                ("YZ_OTHERFS", l.other.to_str().unwrap()),
            ];
            let run = |act: &Act| run_under(&shell, None, &env, act);
            put(&l.other, "proj/file", b"mine");
            put(&l.home, "real/file", b"two");
            let mut wrong = Vec::new();
            for (what, act, ok) in [
                (
                    "a rename in a link to a folder on another file system",
                    Act::Move(&at(&l.home, "proj/file"), &at(&l.home, "proj/file2")),
                    Ok(()),
                ),
                (
                    "a rename in a link to a folder on the same file system",
                    Act::Move(&at(&l.home, "same/file"), &at(&l.home, "same/file2")),
                    Ok(()),
                ),
                (
                    "a file made in it",
                    Act::Make(&at(&l.home, "proj/new"), Kind::File),
                    Ok(()),
                ),
                (
                    "a folder made in it",
                    Act::Make(&at(&l.home, "proj/newdir"), Kind::Folder),
                    Ok(()),
                ),
                (
                    "the link itself, renamed where it is",
                    Act::Move(&at(&l.home, "proj"), &at(&l.home, "proj-renamed")),
                    Ok(()),
                ),
            ] {
                let got = run(&act);
                if got != ok {
                    wrong.push(format!("{what}: {got:?}"));
                }
            }
            assert!(wrong.is_empty(), "{shell:?}: {wrong:#?}");
            assert_eq!(
                fs::read_to_string(at(&l.other, "proj-renamed/file2")).ok(),
                None
            );
            assert_eq!(
                fs::read_to_string(at(&l.other, "proj/file2")).unwrap(),
                "mine"
            );
            assert_eq!(
                fs::read_to_string(at(&l.home, "real/file2")).unwrap(),
                "two"
            );
            assert_eq!(
                fs::read_link(at(&l.home, "proj-renamed")).unwrap(),
                l.other.join("proj")
            );
        }
    }

    #[test]
    fn a_move_into_or_out_of_a_folder_that_is_a_link_to_another_file_system_is_refused_not_copied()
    {
        let (_shim, path) = device_by_place();
        for shell in shells() {
            let l = linked();
            let env = [
                ("PATH", path.as_str()),
                ("YZ_OTHERFS", l.other.to_str().unwrap()),
            ];
            put(&l.home, "outside", b"out");
            fs::create_dir(at(&l.home, "outdir")).unwrap();
            put(&l.home, "outdir/x", b"x");
            put(&l.other, "proj/inside", b"in");
            let mut wrong = Vec::new();
            for (what, from, to) in [
                ("a file from outside into it", "outside", "proj/arrived"),
                ("a folder from outside into it", "outdir", "proj/arriveddir"),
                ("a file out of it", "proj/inside", "left"),
            ] {
                let act = Act::Move(&at(&l.home, from), &at(&l.home, to));
                let got = run_under(&shell, None, &env, &act);
                if got != Err(Problem::OtherDevice) {
                    wrong.push(format!("{what}: {got:?}"));
                }
            }
            assert!(wrong.is_empty(), "{shell:?}: {wrong:#?}");
            assert_eq!(fs::read_to_string(at(&l.home, "outside")).unwrap(), "out");
            assert_eq!(fs::read_to_string(at(&l.home, "outdir/x")).unwrap(), "x");
            assert_eq!(names(&l.other.join("proj")), ["inside"], "{shell:?}");
            assert!(names(&l.home).iter().all(|n| n != "left"), "{shell:?}");
        }
    }

    #[test]
    fn a_stat_with_no_identity_to_give_leaves_the_name_it_cannot_tell_from_another_and_says_so() {
        // A plain failure, or an empty file or folder of someone else's put at the name that was taken:
        // where `stat` has no `device:inode` to give, nothing can say the empty entry at the name is the
        // one made, so it stays, and the answer says it does.
        let failing = rename_shims("echo \"mv: simulated: disk on fire\" >&2; exit 1");
        let swap_file = rename_shims("rm -f -- \"$to\"; : > \"$to\"; exit 1");
        let swap_dir = rename_shims("rmdir -- \"$to\"; mkdir -- \"$to\"; exit 1");
        let mut wrong = Vec::new();
        for (what, shims, folder) in [
            ("a failed rename", &failing, false),
            ("a failed rename of a folder", &failing, true),
            ("their empty file", &swap_file, false),
            ("their empty folder", &swap_dir, true),
        ] {
            let (_dir, path) = shimmed(&[
                (shims[0].0, &shims[0].1),
                (shims[1].0, &shims[1].1),
                ("stat", NO_IDENTITY),
            ]);
            for (looks, takes_first) in ways() {
                if !takes_first {
                    continue;
                }
                for shell in shells() {
                    let d = tempfile::tempdir().unwrap();
                    let from = at(d.path(), "from");
                    if folder {
                        fs::create_dir(&from).unwrap();
                    } else {
                        fs::write(&from, "mine").unwrap();
                    }
                    let to = at(d.path(), "to");
                    let env = env_for(&path, looks, &[]);
                    let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                    let said = matches!(&got, Err(Problem::Failed(w)) if w.contains(&to) && w.contains("left"));
                    let kept = fs::metadata(&to)
                        .map(|m| m.is_dir() == folder)
                        .unwrap_or(false);
                    if !said || !kept || !Path::new(&from).exists() {
                        wrong.push(format!(
                            "{shell:?} {looks:?} {what}: said={said} kept={kept} {got:?}"
                        ));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn a_move_told_to_stop_where_stat_has_no_identity_to_give_leaves_the_name_it_took_and_says_so()
    {
        let shims = rename_shims("touch \"$YZ_STARTED\"; sleep 1; exit 1");
        let (shim_dir, path) = shimmed(&[
            (shims[0].0, &shims[0].1),
            (shims[1].0, &shims[1].1),
            ("stat", NO_IDENTITY),
        ]);
        for (looks, takes_first) in ways() {
            if !takes_first {
                continue;
            }
            for shell in shells() {
                let d = tempfile::tempdir().unwrap();
                let from = put(d.path(), "from.txt", b"mine");
                let to = at(d.path(), "to.txt");
                let started = at(shim_dir.path(), "started");
                let _ = fs::remove_file(&started);
                let req = request(&Act::Move(&from, &to), None).unwrap();
                let mut cmd = Command::new(shell[0]);
                cmd.args(&shell[1..])
                    .args(&req.args)
                    .env("PATH", &path)
                    .env("YZ_STARTED", &started)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                if let Some(looks) = looks {
                    cmd.env("YZ_MVLOOKS", looks);
                }
                let child = cmd.spawn().unwrap();
                let waited = std::time::Instant::now();
                while !Path::new(&started).exists() && waited.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let label = format!("{shell:?} {looks:?}");
                assert!(
                    Path::new(&started).exists(),
                    "{label}: the move did not start"
                );
                // SAFETY: a signal to the child this test started.
                unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
                let out = child.wait_with_output().unwrap();
                let said = String::from_utf8_lossy(&out.stderr).into_owned();
                assert_eq!(out.status.code(), Some(1), "{label}");
                assert!(
                    said.contains("may or may not")
                        && said.contains("left in place")
                        && said.contains(&to),
                    "{label}: {said}"
                );
                assert_eq!(
                    fs::read_to_string(&to).unwrap(),
                    "",
                    "{label}: the name it took"
                );
                assert_eq!(fs::read_to_string(&from).unwrap(), "mine", "{label}");
            }
        }
    }

    #[test]
    fn a_stat_with_no_identity_to_give_does_not_hide_a_source_that_is_gone_behind_a_name_it_left() {
        let shims = rename_shims("mv -- \"$from\" \"$YZ_ELSEWHERE\"");
        let (_dir, path) = shimmed(&[
            (shims[0].0, &shims[0].1),
            (shims[1].0, &shims[1].1),
            ("stat", NO_IDENTITY),
        ]);
        for (looks, takes_first) in ways() {
            if !takes_first {
                continue;
            }
            for shell in shells() {
                let d = tempfile::tempdir().unwrap();
                let elsewhere = tempfile::tempdir().unwrap();
                let from = put(d.path(), "from", b"mine");
                let to = at(d.path(), "to");
                let env = env_for(
                    &path,
                    looks,
                    &[("YZ_ELSEWHERE", elsewhere.path().to_str().unwrap())],
                );
                let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                let label = format!("{shell:?} {looks:?}: {got:?}");
                assert!(
                    matches!(&got, Err(Problem::Failed(w)) if w.contains(&to) && w.contains("gone")),
                    "{label}"
                );
                assert!(Path::new(&to).exists(), "{label}");
            }
        }
    }

    /// A `mv` that answers as GNU's does (`--version`, and `--help` with or without `--no-copy`), writes
    /// the arguments of every rename it is given to `$YZ_LOG`, and carries it out with the `mv` of the
    /// machine the test runs on. With `YZ_COPIES` set it copies and removes instead, as GNU's does when
    /// the system cannot rename.
    fn gnu_looking_mv() -> (tempfile::TempDir, String) {
        shimmed(&[(
            "mv",
            r#"case $1 in
  --version) echo "mv (GNU coreutils) 9.4"; exit 0 ;;
  --help)
    echo "Usage: mv [OPTION]... [-T] SOURCE DEST"
    [ -z "$YZ_NOCOPY" ] || echo "      --no-copy                do not copy if renaming fails"
    echo "  -T, --no-target-directory    treat DEST as a normal file"
    exit 0 ;;
esac
echo "$*" >> "$YZ_LOG"
for a in "$@"; do from=$to; to=$a; done
if [ -n "$YZ_COPIES" ]; then cp -R "$from" "$to" && rm -rf "$from"; exit $?; fi
exec mv -n -- "$from" "$to""#,
        )])
    }

    /// The programs the script needs besides `mv`, in a folder of their own, so that a PATH can be made of
    /// just those and the shim of `mv`: a machine with no `perl`.
    fn only_the_basics() -> tempfile::TempDir {
        let bin = tempfile::tempdir().unwrap();
        for tool in ["stat", "rm", "rmdir", "mkdir"] {
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
        bin
    }

    #[test]
    fn a_gnu_mv_that_can_refuse_to_copy_is_told_to_and_one_that_cannot_is_not_asked_to_rename_a_folder()
     {
        let (shim, with_perl) = gnu_looking_mv();
        let basics = only_the_basics();
        let without_perl = format!("{}:{}", shim.path().display(), basics.path().display());
        let log = at(shim.path(), "log");
        for shell in shells() {
            // (does it have --no-copy, is the source a folder, is there perl, the arguments `mv` is to get)
            for (nocopy, folder, perl, want) in [
                (true, false, true, Some("--no-copy -nT -- ")),
                (true, true, true, Some("--no-copy -nT -- ")),
                (false, false, true, Some("-nT -- ")),
                (false, true, false, Some("-nT -- ")),
                (false, true, true, None),
            ] {
                if perl && want.is_none() && !has_perl() {
                    continue;
                }
                let d = tempfile::tempdir().unwrap();
                let from = at(d.path(), "from");
                if folder {
                    fs::create_dir(&from).unwrap();
                    put(d.path(), "from/x", b"x");
                } else {
                    put(d.path(), "from", b"x");
                }
                let to = at(d.path(), "to");
                let _ = fs::remove_file(&log);
                let mut env = vec![
                    (
                        "PATH",
                        if perl {
                            with_perl.as_str()
                        } else {
                            without_perl.as_str()
                        },
                    ),
                    ("YZ_LOG", log.as_str()),
                ];
                if nocopy {
                    env.push(("YZ_NOCOPY", "1"));
                }
                let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
                let label = format!("{shell:?} nocopy={nocopy} folder={folder} perl={perl}");
                assert_eq!(got, Ok(()), "{label}");
                assert!(
                    !Path::new(&from).exists() && Path::new(&to).exists(),
                    "{label}"
                );
                let logged = fs::read_to_string(&log).unwrap_or_default();
                match want {
                    Some(start) => {
                        assert_eq!(logged.trim(), format!("{start}{from} {to}"), "{label}");
                    }
                    None => assert_eq!(logged, "", "{label}: mv was asked to rename a folder"),
                }
            }
        }
    }

    #[test]
    fn a_rename_that_was_a_copy_is_not_said_to_have_been_a_replacement() {
        // A GNU mv that cannot rename and was not told to refuse copies and removes: the entry at the new
        // name is another one and the source is gone. That cannot be told from a replacement by someone
        // else, so the answer says both.
        let (shim, _) = gnu_looking_mv();
        let basics = only_the_basics();
        let path = format!("{}:{}", shim.path().display(), basics.path().display());
        let log = at(shim.path(), "log");
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let from = at(d.path(), "from");
            fs::create_dir(&from).unwrap();
            let to = at(d.path(), "to");
            let env = [
                ("PATH", path.as_str()),
                ("YZ_LOG", log.as_str()),
                ("YZ_COPIES", "1"),
            ];
            let got = run_under(&shell, None, &env, &Act::Move(&from, &to));
            assert!(
                matches!(&got, Err(Problem::Failed(w)) if w.contains("copied") && w.contains(&to)),
                "{shell:?}: {got:?}"
            );
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
            ("debian:stable-slim", "sh"),
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
