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
            Problem::NoTool(tool) => format!(
                "cannot {verb} {subject}: {machine} has no {tool}, which this needs"
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
        Act::Move(from, to) if usable_path(from) && usable_path(to) => {
            (MOVE_SCRIPT, vec![from, to, if other_case(from, to) { "1" } else { "0" }])
        }
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
