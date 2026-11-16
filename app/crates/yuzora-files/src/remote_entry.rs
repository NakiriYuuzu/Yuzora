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
//!
//! A copy is made in a scratch folder of its own beside the names and renamed in, by the rename
//! script, once it is whole. Moving to the trash asks the machine's own trash first.

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
///   file system have that, and `-T` keeps it from following `to` where they do not. Where the
///   system cannot rename at all (EXDEV: a folder out of an image layer of an overlay file system,
///   or across a bind mount, where the device is the same) GNU `mv` copies; `--no-copy`, where
///   `mv --help` lists it (9.4), makes it fail instead. A GNU `mv` without it is not asked to
///   rename a folder when there is `perl`: that goes the way below.
/// - Anywhere else the script first takes the name `to` (an empty file made the way `MAKE_SCRIPT`
///   makes one, or an empty folder for a folder), which nobody can then make, and renames over
///   that with a call that cannot take `to` for a place to move into: `perl`'s `rename`, or
///   `mv -fT` where `mv` has `-T` (BusyBox). A rename onto a name that is not a folder, or onto an
///   empty folder, is all it asks of the system. A machine with none of these (macOS has `perl`)
///   gets `refused rename`: the script does not fall back to a plain `mv`.
/// - If the rename fails, or the source is gone by then, the name taken is given back, but only
///   if it is still the one made (`device:inode`, and a folder only when it is empty). Where
///   `stat` gives no `device:inode` nothing can say that, so the name stays and the answer says so.
/// - A name that is the same entry under other letters (`$3`) is renamed in place, on a file
///   system that folds case.
/// - The device of the folder `to` is in is read through `/.`: `stat` alone looks at a link, and
///   a link to a folder is that folder to a rename.
const MOVE_SCRIPT: &str = concat!(
    include_str!("remote_entry/common.sh"),
    include_str!("remote_entry/move.sh")
);

/// `$1` from, `$2` into, `$3` the name, `$4` and `$5` its stem and extension as a file's, `$6` 32
/// lowercase hex for the scratch folder, `$7` [`MOVE_SCRIPT`], then the names in `into` that no copy
/// may take. Prints one line and exits 0: `done file|folder K` (the copy is in `into` under the
/// name [`fsops::copy_name`] gives for K), `gone`, `refused nofolder|folder|inside|rename`,
/// `refused open J` (the Jth of those names was the next free one) or `refused tool …`. Any other
/// end leaves no copy in `into`.
///
/// `cp -RP` makes the copy (a link stays a link, a new entry gets the mode `cp` gives it) as
/// `.yuzora-save-<hex>/<hex>` beside the names, in a folder only its owner can enter, which the script
/// goes into and checks before it changes anything there (`cd -P`, then the entry made, empty and its
/// own): someone who may write `into` cannot have the copy made, or anything cleared, in a folder of
/// theirs or of the person's. The rename script then moves it to the first free name, never over what
/// is there. `into` is looked up by its path once, when the script enters it: the names, the rename
/// and the scratch folder's removal go through the scratch folder's `..`, so `into` swapped for a link
/// meanwhile gets the copy put nowhere else. A copy killed before it is done (the timeout) cannot
/// clear that folder, which the tree does not list: [`CLEAR_SCRIPT`] does.
const COPY_SCRIPT: &str = concat!(
    include_str!("remote_entry/common.sh"),
    include_str!("remote_entry/copy.sh")
);

/// `$1` into, `$2` the hex of a copy's scratch folder there. Prints `done` when nothing of that copy
/// is left there, else `left`; `left` too when `into` cannot be entered (renamed away, or shut), since
/// the copy may still be in it. It removes `.yuzora-save-<hex>` only when that is a folder of the
/// person's, not a link, and the folder entered; and in it only `<hex>`, which only [`COPY_SCRIPT`]
/// makes there: a folder put at that name by someone else keeps what it holds.
const CLEAR_SCRIPT: &str = concat!(
    include_str!("remote_entry/common.sh"),
    include_str!("remote_entry/clear.sh")
);

/// `$1` path. Prints one line and exits 0: `done`, `gone`, `refused folder`, `refused device` (the
/// entry is on another disk than the trash), `refused tool trash` (nothing here could take it to a
/// trash) or `refused tool <name>` (a tool it needs, such as `od`, is missing). Any other end leaves
/// it where it was.
///
/// `gio trash` first, then a `trash` command, then the freedesktop trash by hand (its
/// `.trashinfo` is written first, under a name nobody has, then the entry is moved next to it),
/// and only on the disk the entry is on: a move to another disk would copy it. The entry's folder
/// is looked up by its path once, when the script enters it; every later step names the entry in
/// the folder entered.
const TRASH_SCRIPT: &str = concat!(
    include_str!("remote_entry/common.sh"),
    include_str!("remote_entry/trash.sh")
);

/// A change is small; a hung one is not worth more than this.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A copy reads and writes everything it copies.
const COPY_TIMEOUT: Duration = Duration::from_secs(120);

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
    /// `from` into the folder `into`, under the first free name; never under one of the names in
    /// the list, where a document is still open.
    Copy(&'a str, &'a str, &'a [String]),
    Trash(&'a str),
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
    /// A folder would go into itself.
    IntoItself,
    /// The name a copy would take is where a document is still open: the index of that name.
    StillOpen(usize),
    /// Nothing there can take it to a trash.
    NoTrash,
    /// The machine could not be asked, or said something else: its own words.
    Failed(String),
}

impl Act<'_> {
    /// What could not be done, as a sentence starts.
    fn head(&self) -> String {
        match self {
            Act::Make(path, _) => format!("cannot create {path}"),
            Act::Move(from, _) => format!("cannot rename {from}"),
            Act::Copy(from, ..) => format!("cannot copy {from}"),
            Act::Trash(path) => format!("cannot move {path} to the trash"),
        }
    }

    /// The name that is to be taken, or what is to go.
    fn target(&self) -> &str {
        match self {
            Act::Make(path, _) | Act::Move(_, path) | Act::Copy(_, path, _) | Act::Trash(path) => {
                path
            }
        }
    }

    /// The folder a name is taken in or given up from.
    fn folder(&self) -> &str {
        match self {
            Act::Copy(_, into, _) => into,
            _ => folder_of(self.target()),
        }
    }

    /// The folders the change is in, each once.
    fn folders(&self) -> Vec<&str> {
        let mut found = vec![self.folder()];
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
        let (head, target) = (self.head(), self.target());
        match problem {
            Problem::Exists => format!("{target} already exists"),
            Problem::Gone => format!("{head}: it is no longer there on {machine}"),
            Problem::NoFolder => format!(
                "{head}: the folder {} does not exist on {machine}",
                self.folder()
            ),
            Problem::FolderNotWritable => format!(
                "{head}: you may not change {} on {machine} (a name is added to or removed from a folder only by someone who can write to it and enter it)",
                self.folders().join(" and ")
            ),
            Problem::OtherDevice => format!(
                "{head}: {target} is on another file system on {machine}, and a rename does not copy"
            ),
            Problem::NoTool(tool) => format!("{head}: {machine} has no {tool}, which this needs"),
            Problem::NoSafeRename => format!(
                "{head}: {machine} has no perl, which this needs (or GNU mv, or an mv with -T): a plain mv could follow a link into another folder"
            ),
            Problem::NoShell => format!(
                "{head}: this needs sh on {machine}, and it has none (Windows machines cannot be changed from here)"
            ),
            Problem::BadPath => format!(
                "{head}: a machine's path has to start with / and name a file or a folder (a Windows path cannot be used)"
            ),
            Problem::IntoItself => format!("{head} into {target}: a folder cannot go into itself"),
            Problem::StillOpen(at) => {
                let name = match self {
                    Act::Copy(_, into, held) => held.get(*at).map(|n| fsops::join(into, n)),
                    _ => None,
                };
                format!(
                    "{} is still open (its file was deleted): close it first",
                    name.as_deref().unwrap_or(target)
                )
            }
            Problem::NoTrash => format!(
                "{head}: nothing on {machine} can take it to a trash (no gio or trash command that does, and no trash folder on its disk that can be written)"
            ),
            Problem::Failed(why) => format!("{head} on {machine}: {why}"),
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
    let (script, timeout, args): (&str, Duration, Vec<String>) = match *act {
        Act::Make(path, kind) if usable_path(path) => (
            MAKE_SCRIPT,
            TIMEOUT,
            vec![
                path.into(),
                match kind {
                    Kind::File => "file",
                    Kind::Folder => "folder",
                }
                .into(),
            ],
        ),
        Act::Move(from, to) if usable_path(from) && usable_path(to) => (
            MOVE_SCRIPT,
            TIMEOUT,
            vec![
                from.into(),
                to.into(),
                if other_case(from, to) { "1" } else { "0" }.into(),
            ],
        ),
        Act::Copy(from, into, held) => {
            return copy_request(from, into, held, &fsops::scratch_id(), on);
        }
        Act::Trash(path) if usable_path(path) => (TRASH_SCRIPT, TIMEOUT, vec![path.into()]),
        _ => return Err(Problem::BadPath),
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Ok(fsops::sh_request(on, script, &args, timeout, MAX_OUTPUT))
}

/// The request for a copy whose scratch folder is named after `id` (32 lowercase hex).
fn copy_request(
    from: &str,
    into: &str,
    held: &[String],
    id: &str,
    on: Option<&str>,
) -> Result<ExecRequest, Problem> {
    if !(usable_path(from) && into.starts_with('/') && !into.contains('\0')) {
        return Err(Problem::BadPath);
    }
    let name = from.rsplit('/').next().unwrap_or(from);
    let (stem, ext) = fsops::split_ext(name);
    let mut args = vec![from, into, name, stem, ext, id, MOVE_SCRIPT];
    args.extend(held.iter().map(String::as_str));
    Ok(fsops::sh_request(
        on,
        COPY_SCRIPT,
        &args,
        COPY_TIMEOUT,
        MAX_OUTPUT,
    ))
}

/// What the script said.
#[derive(Debug, PartialEq)]
enum Verdict {
    Done,
    /// A copy is done: whether it was a folder (named whole), and which free name it took.
    Copied(bool, u32),
    Exists,
    Gone,
    NoFolder,
    FolderNotWritable,
    OtherDevice,
    NoSafeRename,
    IntoItself,
    /// The Jth held name (from 1) was the next free one.
    Open(usize),
    NoTrash,
    NoTool(&'static str),
}

/// The programs a script names when one is missing.
const TOOLS: [&str; 12] = [
    "mkdir", "mv", "stat", "rm", "rmdir", "cp", "chmod", "ls", "id", "od", "date", "trash",
];

/// Reads the script's one line; anything but exactly a line of a known shape is `None`.
fn parse_verdict(stdout: &[u8]) -> Option<Verdict> {
    let line = std::str::from_utf8(stdout).ok()?.strip_suffix('\n')?;
    fn number<T: std::str::FromStr>(n: &str) -> Option<T> {
        n.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| n.parse().ok())?
    }
    match line.split(' ').collect::<Vec<_>>()[..] {
        ["done"] => Some(Verdict::Done),
        ["done", "file", k] => Some(Verdict::Copied(false, number(k)?)),
        ["done", "folder", k] => Some(Verdict::Copied(true, number(k)?)),
        ["exists"] => Some(Verdict::Exists),
        ["gone"] => Some(Verdict::Gone),
        ["refused", "nofolder"] => Some(Verdict::NoFolder),
        ["refused", "folder"] => Some(Verdict::FolderNotWritable),
        ["refused", "device"] => Some(Verdict::OtherDevice),
        ["refused", "rename"] => Some(Verdict::NoSafeRename),
        ["refused", "inside"] => Some(Verdict::IntoItself),
        ["refused", "open", j] => Some(Verdict::Open(number(j).filter(|j| *j > 0)?)),
        ["refused", "tool", "trash"] => Some(Verdict::NoTrash),
        ["refused", "tool", tool] => TOOLS
            .into_iter()
            .find(|known| *known == tool)
            .map(Verdict::NoTool),
        _ => None,
    }
}

/// What a line that is not the one hoped for means.
fn problem(verdict: Verdict) -> Problem {
    match verdict {
        Verdict::Exists => Problem::Exists,
        Verdict::Gone => Problem::Gone,
        Verdict::NoFolder => Problem::NoFolder,
        Verdict::FolderNotWritable => Problem::FolderNotWritable,
        Verdict::OtherDevice => Problem::OtherDevice,
        Verdict::NoSafeRename => Problem::NoSafeRename,
        Verdict::IntoItself => Problem::IntoItself,
        Verdict::Open(j) => Problem::StillOpen(j - 1),
        Verdict::NoTrash => Problem::NoTrash,
        Verdict::NoTool(tool) => Problem::NoTool(tool),
        Verdict::Done | Verdict::Copied(..) => unreadable(),
    }
}

fn unreadable() -> Problem {
    Problem::Failed("the answer could not be read; it may or may not have been done".into())
}

/// What the machine said, when it said one of the known lines.
fn read(answer: Result<ExecOutput, ExecError>) -> Result<Verdict, Problem> {
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
    parse_verdict(&out.stdout).ok_or_else(unreadable)
}

/// What the machine's answer to a change means.
pub(crate) fn interpret(answer: Result<ExecOutput, ExecError>) -> Result<(), Problem> {
    match read(answer)? {
        Verdict::Done => Ok(()),
        other => Err(problem(other)),
    }
}

/// What the machine's answer to a copy means: whether a folder was copied, and the free name taken.
fn interpret_copy(answer: Result<ExecOutput, ExecError>) -> Result<(bool, u32), Problem> {
    match read(answer)? {
        Verdict::Copied(folder, k) => Ok((folder, k)),
        other => Err(problem(other)),
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

/// Copies `from` into `into` on `machine` and gives the path of the copy.
pub(crate) fn copy_on_machine(
    from: &str,
    into: &str,
    held: &[String],
    machine: &str,
) -> Result<String, String> {
    copy_by(from, into, held, Some(machine), fsops::run)
        .map_err(|problem| Act::Copy(from, into, held).describe(&problem, machine))
}

/// A copy by [`COPY_SCRIPT`], each request run by `run`, as the path of the copy. A copy stopped before
/// it is done (the timeout, a signal, the line to the machine lost) leaves its scratch folder:
/// [`CLEAR_SCRIPT`] is sent after it, and when that cannot clear it the answer says where a partial
/// copy may be.
pub(crate) fn copy_by(
    from: &str,
    into: &str,
    held: &[String],
    on: Option<&str>,
    mut run: impl FnMut(ExecRequest) -> Result<ExecOutput, ExecError>,
) -> Result<String, Problem> {
    let id = fsops::scratch_id();
    let answer = run(copy_request(from, into, held, &id, on)?);
    // Whether the script may have been stopped before its trap ran: at the timeout, by a signal
    // (`hc` gives no exit code, read as -1), or with the line to the machine lost during the call
    // (the machine then kills it). Only a request that never left needs no clear.
    let killed = match &answer {
        Ok(out) => out.code < 0,
        Err(ExecError::Spawn(_)) => false,
        Err(ExecError::Remote(m)) => !never_sent(m),
        Err(_) => true,
    };
    let name = from.rsplit('/').next().unwrap_or(from);
    match interpret_copy(answer) {
        Ok((folder, k)) => Ok(fsops::join(into, &fsops::copy_name(name, folder, k))),
        Err(Problem::Failed(why)) if killed => {
            let clear = fsops::sh_request(on, CLEAR_SCRIPT, &[into, &id], TIMEOUT, MAX_OUTPUT);
            if matches!(read(run(clear)), Ok(Verdict::Done)) {
                return Err(Problem::Failed(why));
            }
            let scratch = fsops::join(into, &format!("{}{id}", fsops::SCRATCH_PREFIX));
            Err(Problem::Failed(format!(
                "{why}. A partial copy may be left in {scratch}, a folder the tree does not show"
            )))
        }
        Err(problem) => Err(problem),
    }
}

/// Whether `hc` says a request never reached the machine, so nothing ran there. hc 0.1.21 answers
/// `<machine> is unreachable: cannot reach …` when it cannot open the line, and `lost the line to
/// <machine> during the call` when the line goes once the request is sent; both with the same
/// `"kind": "unavailable"` and exit status 5, so only its words tell them apart.
fn never_sent(message: &str) -> bool {
    message.contains(" is unreachable: ")
        || message.ends_with("is not connected")
        || message.starts_with("cannot start sh")
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
    fn exec_under(
        shell: &[&str],
        umask: Option<&str>,
        env: &[(&str, &str)],
        act: &Act,
    ) -> ExecOutput {
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
        ExecOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        }
    }

    fn run_under(
        shell: &[&str],
        umask: Option<&str>,
        env: &[(&str, &str)],
        act: &Act,
    ) -> Result<(), Problem> {
        interpret(Ok(exec_under(shell, umask, env, act)))
    }

    /// A copy under `shell`, as the path of the copy.
    fn copy_under(shell: &[&str], env: &[(&str, &str)], act: &Act) -> Result<String, Problem> {
        let Act::Copy(from, into, _) = *act else {
            panic!("not a copy: {act:?}")
        };
        let name = from.rsplit('/').next().unwrap();
        interpret_copy(Ok(exec_under(shell, None, env, act)))
            .map(|(folder, k)| fsops::join(into, &fsops::copy_name(name, folder, k)))
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
            ("refused tool cp\n", Err(Problem::NoTool("cp"))),
            ("refused tool trash\n", Err(Problem::NoTrash)),
            ("refused inside\n", Err(Problem::IntoItself)),
        ] {
            assert_eq!(interpret(answered(0, said, "")), want, "{said:?}");
        }
        // A copy says which free name it took, and which held name stopped it.
        for (said, want) in [
            ("done file 0\n", Ok((false, 0))),
            ("done folder 12\n", Ok((true, 12))),
            ("refused open 1\n", Err(Problem::StillOpen(0))),
            ("refused open 3\n", Err(Problem::StillOpen(2))),
            ("gone\n", Err(Problem::Gone)),
        ] {
            assert_eq!(interpret_copy(answered(0, said, "")), want, "{said:?}");
        }
        for said in [
            "done\n",
            "done file\n",
            "done file x\n",
            "done file -1\n",
            "done file 1 2\n",
            "done dir 1\n",
            "refused open 0\n",
            "refused open\n",
        ] {
            let got = interpret_copy(answered(0, said, ""));
            assert!(
                matches!(&got, Err(Problem::Failed(why)) if why.contains("may or may not")),
                "{said:?}: {got:?}"
            );
        }
        // A change that answers as a copy did is not done.
        assert!(matches!(
            interpret(answered(0, "done file 1\n", "")),
            Err(Problem::Failed(_))
        ));
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
            Act::Copy("relative", "/b", &[]),
            Act::Copy("/a", "b", &[]),
            Act::Copy("/a/..", "/b", &[]),
            Act::Copy("/a", "/b\0", &[]),
            Act::Trash("relative"),
            Act::Trash("/"),
            Act::Trash(r"C:\a"),
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
        let held = ["b.txt".to_owned()];
        let copy = Act::Copy("/srv/a/b.txt", "/srv/c", &held);
        assert_eq!(
            say(&copy, Problem::Gone),
            "cannot copy /srv/a/b.txt: it is no longer there on lab"
        );
        assert!(say(&copy, Problem::NoFolder).contains("the folder /srv/c does not exist on lab"));
        let told = say(&copy, Problem::FolderNotWritable);
        assert!(told.contains("you may not change /srv/c on lab"), "{told}");
        assert_eq!(
            say(&copy, Problem::IntoItself),
            "cannot copy /srv/a/b.txt into /srv/c: a folder cannot go into itself"
        );
        assert_eq!(
            say(&copy, Problem::StillOpen(0)),
            "/srv/c/b.txt is still open (its file was deleted): close it first"
        );
        let trash = Act::Trash("/srv/a/b.txt");
        assert_eq!(
            say(&trash, Problem::Gone),
            "cannot move /srv/a/b.txt to the trash: it is no longer there on lab"
        );
        assert!(
            say(&trash, Problem::FolderNotWritable).contains("you may not change /srv/a on lab")
        );
        assert!(say(&trash, Problem::NoTrash).contains("nothing on lab can take it to a trash"));
        assert_eq!(
            say(&trash, Problem::Failed("mv: busy".into())),
            "cannot move /srv/a/b.txt to the trash on lab: mv: busy"
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

    /// A PATH of only these programs (found where `sh` finds them), and stand-ins written as `sh`
    /// that see the same PATH: nothing else here, and so no trash of this machine, can be reached.
    fn only(tools: &[&str], stand_ins: &[(&str, &str)]) -> tempfile::TempDir {
        let bin = tempfile::tempdir().unwrap();
        for tool in tools {
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
        for (name, body) in stand_ins {
            let file = bin.path().join(name);
            let own = bin.path().display();
            fs::write(&file, format!("#!/bin/sh\nPATH='{own}'\n{body}\n")).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    /// Every entry under `dir`, by path: its kind, mode and bytes, or where a link points.
    fn tree(dir: &Path) -> Vec<(String, String)> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
            for e in fs::read_dir(dir).unwrap() {
                let path = e.unwrap().path();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let meta = fs::symlink_metadata(&path).unwrap();
                let mode = meta.mode() & 0o7777;
                let what = if meta.file_type().is_symlink() {
                    format!("link {}", fs::read_link(&path).unwrap().display())
                } else if meta.is_dir() {
                    walk(root, &path, out);
                    format!("folder {mode:o}")
                } else {
                    format!("file {mode:o} {:?}", fs::read(&path).unwrap())
                };
                out.push((rel, what));
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// A folder of what a copy meets: modes, depth, links of three kinds, names the shell would read.
    fn sample(dir: &Path) -> String {
        let src = dir.join("src.d");
        fs::create_dir_all(src.join("one/two/three")).unwrap();
        fs::write(src.join("one/two/three/deep.txt"), "deep").unwrap();
        fs::write(src.join("plain.txt"), "plain").unwrap();
        fs::write(src.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(src.join("it's \"q\" $HOME\ntwo.txt"), "odd").unwrap();
        fs::create_dir(src.join("private")).unwrap();
        fs::write(src.join("private/key"), "k").unwrap();
        fs::set_permissions(src.join("private"), fs::Permissions::from_mode(0o700)).unwrap();
        symlink("plain.txt", src.join("to-file")).unwrap();
        symlink("one", src.join("to-folder")).unwrap();
        symlink("nowhere", src.join("dangling")).unwrap();
        src.to_string_lossy().into_owned()
    }

    fn umask_here() -> String {
        let out = Command::new("/bin/sh")
            .args(["-c", "umask"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// What a folder holds that a copy may have left: scratch folders.
    fn scratch_left(dir: &Path) -> Vec<String> {
        names(dir)
            .into_iter()
            .filter(|n| n.starts_with(fsops::SCRATCH_PREFIX))
            .collect()
    }

    #[test]
    fn every_shell_copies_as_this_machine_does_and_names_the_copy_the_same() {
        let d = tempfile::tempdir().unwrap();
        let src = sample(d.path());
        let plain = format!("{src}/plain.txt");
        let odd = format!("{src}/it's \"q\" $HOME\ntwo.txt");
        for shell in shells() {
            let here = tempfile::tempdir().unwrap();
            let there = tempfile::tempdir().unwrap();
            let into = |t: &tempfile::TempDir| t.path().to_string_lossy().into_owned();
            // Into another folder: the name itself; a folder with all it holds, as it is.
            let mine = fsops::copy(&src, &into(&here), None, &[]).unwrap();
            let theirs = copy_under(&shell, &[], &Act::Copy(&src, &into(&there), &[])).unwrap();
            assert_eq!(mine, format!("{}/src.d", into(&here)), "{shell:?}");
            assert_eq!(theirs, format!("{}/src.d", into(&there)), "{shell:?}");
            assert_eq!(
                tree(Path::new(&mine)),
                tree(Path::new(&theirs)),
                "{shell:?}"
            );
            if umask_here() == "0022" {
                assert_eq!(tree(Path::new(&src)), tree(Path::new(&mine)), "{shell:?}");
            }
            // Beside itself: "<name> copy", a folder whole, a file keeping its extension.
            for (from, want) in [
                (src.clone(), format!("{src} copy")),
                (plain.clone(), format!("{src}/plain copy.txt")),
                (odd.clone(), format!("{src}/it's \"q\" $HOME\ntwo copy.txt")),
            ] {
                let folder = from.rsplit_once('/').unwrap().0.to_owned();
                let got = copy_under(&shell, &[], &Act::Copy(&from, &folder, &[])).unwrap();
                assert_eq!(got, want, "{shell:?}");
                assert_eq!(
                    fs::symlink_metadata(&got).unwrap().is_dir(),
                    fs::symlink_metadata(&from).unwrap().is_dir()
                );
                let again = fsops::copy(&from, &folder, None, &[]).unwrap();
                assert!(
                    again.ends_with(" copy 2") || again.contains(" copy 2."),
                    "{again}"
                );
                for made in [&got, &again] {
                    if Path::new(made).is_dir() {
                        fs::remove_dir_all(made).unwrap();
                    } else {
                        fs::remove_file(made).unwrap();
                    }
                }
            }
            assert!(scratch_left(d.path()).is_empty() && scratch_left(Path::new(&src)).is_empty());
        }
    }

    #[test]
    fn a_name_is_split_the_same_here_and_there() {
        let d = tempfile::tempdir().unwrap();
        for (name, folder, want) in [
            ("a.txt", false, "a copy.txt"),
            ("archive.tar.gz", false, "archive.tar copy.gz"),
            (".env", false, ".env copy"),
            (".env.local", false, ".env copy.local"),
            ("Makefile", false, "Makefile copy"),
            ("v1.2", true, "v1.2 copy"),
            ("a.", false, "a copy."),
        ] {
            let path = d.path().join(name);
            if folder {
                fs::create_dir(&path).unwrap();
            } else {
                fs::write(&path, name).unwrap();
            }
            let from = path.to_string_lossy().into_owned();
            let into = d.path().to_string_lossy().into_owned();
            for shell in shells() {
                let got = copy_under(&shell, &[], &Act::Copy(&from, &into, &[])).unwrap();
                assert_eq!(got, format!("{into}/{want}"), "{shell:?} {name}");
                let _ = fs::remove_file(&got).or_else(|_| fs::remove_dir(&got));
            }
            assert_eq!(
                fsops::copy(&from, &into, None, &[]).unwrap(),
                format!("{into}/{want}")
            );
        }
    }

    #[test]
    fn a_copy_takes_the_next_free_name_and_never_replaces_anything() {
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let dir = d.path().to_string_lossy().into_owned();
            let a = put(d.path(), "a.txt", b"mine");
            put(d.path(), "a copy.txt", b"one");
            fs::create_dir(d.path().join("a copy 2.txt")).unwrap();
            symlink("nowhere", d.path().join("a copy 3.txt")).unwrap();
            let got = copy_under(&shell, &[], &Act::Copy(&a, &dir, &[])).unwrap();
            assert_eq!(got, format!("{dir}/a copy 4.txt"), "{shell:?}");
            assert_eq!(fs::read(&got).unwrap(), b"mine");
            assert_eq!(fs::read(d.path().join("a copy.txt")).unwrap(), b"one");
            assert_eq!(
                fs::read_dir(d.path().join("a copy 2.txt")).unwrap().count(),
                0
            );
            assert!(
                fs::symlink_metadata(d.path().join("a copy 3.txt"))
                    .unwrap()
                    .is_symlink()
            );
            assert_eq!(
                names(d.path()),
                [
                    "a copy 2.txt",
                    "a copy 3.txt",
                    "a copy 4.txt",
                    "a copy.txt",
                    "a.txt"
                ]
            );
        }
    }

    #[test]
    fn a_copy_that_cannot_be_made_is_refused_as_here_and_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let src = sample(d.path());
        let empty = tempfile::tempdir().unwrap();
        let into = empty.path().to_string_lossy().into_owned();
        let plain = format!("{src}/plain.txt");
        let held = ["plain.txt".to_owned()];
        let ghost = format!("{src}/ghost");
        let missing = format!("{into}/no/where");
        let deeper = format!("{src}/one/two");
        let mut cases: Vec<(Act, Problem)> = vec![
            (Act::Copy(&plain, &into, &held), Problem::StillOpen(0)),
            (Act::Copy(&ghost, &into, &[]), Problem::Gone),
            (Act::Copy(&plain, &missing, &[]), Problem::NoFolder),
            (Act::Copy(&src, &src, &[]), Problem::IntoItself),
            (Act::Copy(&src, &deeper, &[]), Problem::IntoItself),
        ];
        let locked = format!("{}/locked", d.path().display());
        if !is_root() {
            fs::create_dir(&locked).unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
            cases.push((Act::Copy(&plain, &locked, &[]), Problem::FolderNotWritable));
        }
        for shell in shells() {
            for (act, want) in &cases {
                let got = copy_under(&shell, &[], act);
                assert_eq!(got.as_ref(), Err(want), "{shell:?} {act:?}");
            }
        }
        // This machine says the same.
        for (act, want) in &cases {
            let Act::Copy(from, into, held) = *act else {
                unreachable!()
            };
            assert_eq!(
                fsops::copy(from, into, None, held),
                Err(act.describe(want, "this machine")),
                "{act:?}"
            );
        }
        // A held name taken on disk is not the one the copy takes.
        put(empty.path(), "plain.txt", b"theirs");
        let got = copy_under(&shells()[0], &[], &Act::Copy(&plain, &into, &held)).unwrap();
        assert_eq!(got, format!("{into}/plain copy.txt"));
        assert_eq!(names(empty.path()), ["plain copy.txt", "plain.txt"]);
        assert!(scratch_left(d.path()).is_empty() && scratch_left(Path::new(&src)).is_empty());
        if !is_root() {
            assert!(names(Path::new(&locked)).is_empty());
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn a_copy_cut_short_leaves_nothing_and_is_made_where_only_its_owner_can_look() {
        let d = tempfile::tempdir().unwrap();
        let src = sample(d.path());
        let into = tempfile::tempdir().unwrap();
        let dir = into.path().to_string_lossy().into_owned();
        let seen = d.path().join("seen");
        // A cp that copies some, then fails.
        let (_bin, path) = shimmed(&[(
            "cp",
            "ls -ld . > \"$SEEN\"; mkdir -p \"$4/half\"; echo 'cp: disk on fire' >&2; exit 1",
        )]);
        for shell in shells() {
            let env = [("PATH", path.as_str()), ("SEEN", seen.to_str().unwrap())];
            let got = copy_under(&shell, &env, &Act::Copy(&src, &dir, &[]));
            assert_eq!(
                got,
                Err(Problem::Failed("cp: disk on fire".into())),
                "{shell:?}"
            );
            assert!(names(into.path()).is_empty(), "{shell:?}");
            let mode = fs::read_to_string(&seen).unwrap();
            assert!(mode.starts_with("drwx------"), "{shell:?}: {mode}");
        }
        // Without cp nothing is made either.
        let bin = only(
            &["mkdir", "chmod", "ls", "id", "stat", "rm", "rmdir", "sh"],
            &[],
        );
        let env = [("PATH", bin.path().to_str().unwrap())];
        for shell in shells() {
            let got = copy_under(&shell, &env, &Act::Copy(&src, &dir, &[]));
            assert_eq!(got, Err(Problem::NoTool("cp")), "{shell:?}");
        }
        assert!(names(into.path()).is_empty());
    }

    #[test]
    fn a_folder_copied_without_write_permission_is_cleared_when_the_copy_is_refused() {
        if is_root() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let sealed = d.path().join("sealed");
        fs::create_dir_all(sealed.join("inner")).unwrap();
        fs::write(sealed.join("inner/f"), "f").unwrap();
        fs::write(sealed.join("g"), "g").unwrap();
        for dir in [sealed.join("inner"), sealed.clone()] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
        }
        let from = sealed.to_string_lossy().into_owned();
        let empty = tempfile::tempdir().unwrap();
        let into = empty.path().to_string_lossy().into_owned();
        let held = ["sealed".to_owned()];
        for shell in shells() {
            let got = copy_under(&shell, &[], &Act::Copy(&from, &into, &held));
            assert_eq!(got, Err(Problem::StillOpen(0)), "{shell:?}");
            assert!(names(empty.path()).is_empty(), "{shell:?}: {:?}", names(empty.path()));
        }
        let got = fsops::copy(&from, &into, None, &held);
        assert!(got.unwrap_err().contains("is still open"));
        assert!(names(empty.path()).is_empty(), "{:?}", names(empty.path()));
        for dir in [sealed.clone(), sealed.join("inner")] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn a_scratch_folder_swapped_for_a_link_is_neither_changed_nor_cleared() {
        let d = tempfile::tempdir().unwrap();
        let victim = d.path().join("victim");
        fs::create_dir_all(victim.join("c")).unwrap();
        fs::write(victim.join("c/keep"), "keep").unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o755)).unwrap();
        let src = put(d.path(), "a.txt", b"a");
        let into = tempfile::tempdir().unwrap();
        let dir = into.path().to_string_lossy().into_owned();
        // Someone who may rename in `into` puts a link to a folder of the person's at the name just made.
        let (_bin, path) = shimmed(&[(
            "mkdir",
            "case $2 in */.yuzora-save-*) ln -s \"$VICTIM\" \"$2\"; exit 0 ;; esac\nexec mkdir \"$@\"",
        )]);
        for shell in shells() {
            let env = [("PATH", path.as_str()), ("VICTIM", victim.to_str().unwrap())];
            let got = copy_under(&shell, &env, &Act::Copy(&src, &dir, &[]));
            assert!(
                matches!(&got, Err(Problem::Failed(why)) if why.contains("is not the folder just made")),
                "{shell:?}: {got:?}"
            );
            assert_eq!(mode(&victim), 0o755, "{shell:?}");
            assert_eq!(names(&victim), ["c"], "{shell:?}");
            assert_eq!(fs::read(victim.join("c/keep")).unwrap(), b"keep", "{shell:?}");
            for link in names(into.path()) {
                fs::remove_file(into.path().join(link)).unwrap();
            }
        }
    }

    #[test]
    fn a_folder_is_not_copied_into_itself_spelled_in_other_letters_on_a_disk_that_folds_case() {
        let d = tempfile::tempdir().unwrap();
        if !folds_case(d.path()) {
            return;
        }
        fs::create_dir_all(d.path().join("Src/sub")).unwrap();
        let from = at(d.path(), "Src");
        let into = at(d.path(), "src/sub");
        let act = Act::Copy(&from, &into, &[]);
        for shell in shells() {
            assert_eq!(copy_under(&shell, &[], &act), Err(Problem::IntoItself), "{shell:?}");
        }
        assert_eq!(
            fsops::copy(&from, &into, None, &[]),
            Err(act.describe(&Problem::IntoItself, "this machine"))
        );
        assert!(names(&d.path().join("Src/sub")).is_empty());
    }

    #[test]
    fn every_shell_copies_a_fifo_as_a_new_fifo_and_this_machine_refuses_it() {
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::FileTypeExt as _;
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("box");
        fs::create_dir(&src).unwrap();
        let pipe = std::ffi::CString::new(src.join("pipe").as_os_str().as_bytes()).unwrap();
        // SAFETY: a NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o644) }, 0);
        let from = src.to_string_lossy().into_owned();
        let into = d.path().to_string_lossy().into_owned();
        for shell in shells() {
            let got = copy_under(&shell, &[], &Act::Copy(&from, &into, &[])).unwrap();
            let made = fs::symlink_metadata(format!("{got}/pipe")).unwrap();
            assert!(made.file_type().is_fifo(), "{shell:?}");
            fs::remove_dir_all(&got).unwrap();
        }
        let refused = fsops::copy(&from, &into, None, &[]).unwrap_err();
        assert!(refused.contains("pipe is not a file, a folder or a link"), "{refused}");
        assert!(scratch_left(d.path()).is_empty());
    }

    /// `req` run here under `shell` as `hc exec` runs it, with `path` for PATH, and killed with all it
    /// started (SIGKILL: no trap of the script runs) after a second, as `hc` does at the timeout.
    fn killed_after_a_second(shell: &[&str], path: &str, mut req: ExecRequest) -> ExecRequest {
        req.args = shell[1..]
            .iter()
            .map(|s| (*s).to_owned())
            .chain(req.args)
            .collect();
        req.program = shell[0].to_owned();
        req.timeout(Duration::from_secs(1)).env("PATH", path)
    }

    /// A `cp` that copies some, then never ends.
    fn hung_cp() -> (tempfile::TempDir, String) {
        shimmed(&[(
            "cp",
            "mkdir -p \"$4/half\" && : > \"$4/half/f\" && exec sleep 30",
        )])
    }

    #[test]
    fn a_copy_killed_at_its_timeout_leaves_nothing_when_the_machine_answers_again() {
        let d = tempfile::tempdir().unwrap();
        let src = sample(d.path());
        let into = tempfile::tempdir().unwrap();
        let dir = into.path().to_string_lossy().into_owned();
        let (_bin, path) = hung_cp();
        for shell in shells() {
            let mut left_by_the_kill = Vec::new();
            let got = copy_by(&src, &dir, &[], None, |req| {
                if req.args.iter().any(|a| a == CLEAR_SCRIPT) {
                    left_by_the_kill = scratch_left(into.path());
                }
                fsops::run(killed_after_a_second(&shell, &path, req))
            });
            assert!(
                matches!(&got, Err(Problem::Failed(why)) if why.starts_with("took longer than 1 s") && !why.contains("partial")),
                "{shell:?}: {got:?}"
            );
            assert_eq!(left_by_the_kill.len(), 1, "{shell:?}: the kill ran no trap");
            assert!(names(into.path()).is_empty(), "{shell:?}: {:?}", names(into.path()));
        }
    }

    #[test]
    fn a_killed_copy_that_cannot_be_cleared_says_where_a_partial_copy_may_be_and_clears_nothing_else() {
        let d = tempfile::tempdir().unwrap();
        let src = sample(d.path());
        let into = tempfile::tempdir().unwrap();
        let dir = into.path().to_string_lossy().into_owned();
        let mine = d.path().join("mine");
        let (_bin, path) = hung_cp();
        // At the clear: the machine is gone; the scratch folder is a link to a folder of the person's; it
        // is a folder of the person's, moved there, that holds what its own name does not say is the copy.
        for at_the_clear in ["unreachable", "link", "folder"] {
            fs::create_dir_all(mine.join("c")).unwrap();
            fs::write(mine.join("c/keep"), "keep").unwrap();
            let mut scratch = String::new();
            let got = copy_by(&src, &dir, &[], None, |req| {
                if !req.args.iter().any(|a| a == CLEAR_SCRIPT) {
                    return fsops::run(killed_after_a_second(&["/bin/sh"], &path, req));
                }
                scratch = format!("{dir}/{}", scratch_left(into.path())[0]);
                match at_the_clear {
                    "unreachable" => return Err(ExecError::Remote("machine lab is not connected".into())),
                    "link" => {
                        fs::rename(&scratch, d.path().join("aside")).unwrap();
                        symlink(&mine, &scratch).unwrap();
                    }
                    _ => {
                        fs::rename(&scratch, d.path().join("aside")).unwrap();
                        fs::rename(&mine, &scratch).unwrap();
                    }
                }
                fsops::run(killed_after_a_second(&["/bin/sh"], &path, req))
            });
            let Err(Problem::Failed(why)) = &got else {
                panic!("{at_the_clear}: {got:?}")
            };
            assert!(why.starts_with("took longer than 1 s"), "{at_the_clear}: {why}");
            assert!(
                why.contains(&format!("A partial copy may be left in {scratch}, ")),
                "{at_the_clear}: {why}"
            );
            let kept = if at_the_clear == "folder" { Path::new(&scratch) } else { mine.as_path() };
            assert_eq!(fs::read(kept.join("c/keep")).unwrap(), b"keep", "{at_the_clear}");
            assert_eq!(names(kept), ["c"], "{at_the_clear}");
            if at_the_clear == "folder" {
                fs::rename(&scratch, &mine).unwrap();
            } else if at_the_clear == "link" {
                fs::remove_file(&scratch).unwrap();
            }
            let aside = d.path().join("aside");
            if aside.exists() {
                fs::remove_dir_all(&aside).unwrap();
            }
            if Path::new(&scratch).exists() {
                fs::remove_dir_all(&scratch).unwrap();
            }
        }
    }

    /// `hc` answers a script killed by a signal before its timeout with no exit code (`"code": null,
    /// "signal": 9` from hc 0.1.21), read as -1. Its trap did not run either: the copy's scratch
    /// folder is cleared as after the timeout.
    #[test]
    fn a_copy_killed_by_a_signal_leaves_nothing_when_the_machine_answers_again() {
        let d = tempfile::tempdir().unwrap();
        let src = sample(d.path());
        let into = tempfile::tempdir().unwrap();
        let dir = into.path().to_string_lossy().into_owned();
        // Copies some, then kills the script and all it started (its own process group).
        let (_bin, path) = shimmed(&[(
            "cp",
            "mkdir -p \"$4/half\" && : > \"$4/half/f\" && kill -KILL 0",
        )]);
        for shell in shells() {
            let mut left_by_the_kill = Vec::new();
            let mut code = None;
            let got = copy_by(&src, &dir, &[], None, |req| {
                let clear = req.args.iter().any(|a| a == CLEAR_SCRIPT);
                if clear {
                    left_by_the_kill = scratch_left(into.path());
                }
                let answer = fsops::run(
                    killed_after_a_second(&shell, &path, req).timeout(Duration::from_secs(20)),
                );
                if !clear {
                    code = answer.as_ref().ok().map(|out| out.code);
                }
                answer
            });
            assert_eq!(code, Some(-1), "{shell:?}: the copy ended by a signal");
            assert!(
                matches!(&got, Err(Problem::Failed(why)) if !why.contains("partial")),
                "{shell:?}: {got:?}"
            );
            assert_eq!(left_by_the_kill.len(), 1, "{shell:?}: the kill ran no trap");
            assert!(
                names(into.path()).is_empty(),
                "{shell:?}: {:?}",
                names(into.path())
            );
        }
    }

    /// Someone who may rename in `into` swaps the scratch folder for another folder after the
    /// copy's last look at it and before the rename: `<hex>` in that folder is not what is renamed
    /// in, and that folder is not removed, even when empty.
    #[test]
    fn a_scratch_folder_swapped_just_before_the_rename_gives_up_nothing_of_the_folder_put_there() {
        let d = tempfile::tempdir().unwrap();
        let src = put(d.path(), "a.txt", b"mine");
        let swap = r#"d=$(pwd -P) id=${d##*/.yuzora-save-}
mv "$d" "$d.away" && mkdir "$d" && if [ -n "$SWAP_ENTRY" ]; then mkdir "$d/$id" && echo theirs > "$d/$id/precious"; fi"#;
        // The rename itself (GNU mv -nT, or perl's rename): the swap, then the real one.
        let mv = format!("case \" $* \" in *\" -nT \"*) {swap} ;; esac\nexec mv \"$@\"");
        let perl = format!("case \"$2\" in *rename*) {swap} ;; esac\nexec perl \"$@\"");
        let (_bin, path) = shimmed(&[("mv", mv.as_str()), ("perl", perl.as_str())]);
        for shell in shells() {
            for entry in ["1", ""] {
                let into = tempfile::tempdir().unwrap();
                let dir = into.path().to_string_lossy().into_owned();
                let env = [("PATH", path.as_str()), ("SWAP_ENTRY", entry)];
                let got = copy_under(&shell, &env, &Act::Copy(&src, &dir, &[]));
                let why = format!("{shell:?}, an entry at the hex: {entry:?}");
                assert_eq!(
                    got.as_ref().ok(),
                    Some(&format!("{dir}/a.txt")),
                    "{why}: {got:?}"
                );
                assert_eq!(
                    fs::read(into.path().join("a.txt")).unwrap(),
                    b"mine",
                    "{why}"
                );
                let put_there: Vec<String> = scratch_left(into.path())
                    .into_iter()
                    .filter(|n| !n.ends_with(".away"))
                    .collect();
                assert_eq!(put_there.len(), 1, "{why}: {:?}", names(into.path()));
                let held = names(&into.path().join(&put_there[0]));
                if entry.is_empty() {
                    assert!(held.is_empty(), "{why}: {held:?}");
                } else {
                    let id = put_there[0].trim_start_matches(fsops::SCRATCH_PREFIX);
                    assert_eq!(held, [id], "{why}");
                    let precious = into.path().join(&put_there[0]).join(id).join("precious");
                    assert_eq!(fs::read(precious).unwrap(), b"theirs\n", "{why}");
                }
            }
        }
    }

    /// Someone renames another folder to the scratch name while the clear runs: the clear takes
    /// the copy out of the folder it entered and checked, and leaves the one put there, even empty.
    #[test]
    fn a_clear_leaves_a_folder_renamed_to_the_scratch_name_while_it_ran() {
        let id = "0123456789abcdef0123456789abcdef";
        let swap = "case \" $* \" in *\" -rf \"*) d=$(pwd -P); mv \"$d\" \"$d.away\" && mkdir \"$d\" ;; esac\nexec rm \"$@\"";
        let (_bin, path) = shimmed(&[("rm", swap)]);
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let into = d.path().join("into");
            let scratch = format!("{}{id}", fsops::SCRATCH_PREFIX);
            fs::create_dir_all(into.join(&scratch).join(id).join("half")).unwrap();
            fs::write(into.join(&scratch).join(id).join("half/f"), "f").unwrap();
            let said = clear_under(&shell, &[("PATH", path.as_str())], &into, id);
            assert_eq!(said, "left", "{shell:?}");
            assert!(
                into.join(&scratch).is_dir(),
                "{shell:?}: {:?}",
                names(&into)
            );
            assert!(names(&into.join(&scratch)).is_empty(), "{shell:?}");
            assert!(
                names(&into.join(format!("{scratch}.away"))).is_empty(),
                "{shell:?}"
            );
        }
    }

    /// Someone who may rename where the target folder is swaps it for a link to another folder
    /// while the copy runs: the copy is made in the folder entered, under whatever name that has
    /// by now, nothing is made in the link's target, and the scratch folder is gone.
    #[test]
    fn a_target_folder_swapped_for_a_link_while_the_copy_runs_gets_the_copy_and_the_link_target_nothing()
     {
        let d = tempfile::tempdir().unwrap();
        let src = put(d.path(), "authorized_keys", b"mine");
        // The copy runs in the scratch folder: its ".." is the folder entered.
        let cp = "case \" $* \" in *\" -RP \"*) i=$(cd -P .. && pwd -P); mv \"$i\" \"$i.away\" && ln -s \"$VICTIM\" \"$i\" ;; esac\nexec cp \"$@\"";
        let (_bin, path) = shimmed(&[("cp", cp)]);
        for (shell, taken) in shells()
            .into_iter()
            .flat_map(|s| [(s.clone(), false), (s, true)])
        {
            let base = tempfile::tempdir().unwrap();
            let (into, victim) = (base.path().join("into"), base.path().join("victim"));
            fs::create_dir(&into).unwrap();
            fs::create_dir(&victim).unwrap();
            // The name free or taken in the link's target: neither is looked at.
            if taken {
                put(&victim, "authorized_keys", b"theirs");
            }
            let dir = into.to_string_lossy().into_owned();
            let env = [
                ("PATH", path.as_str()),
                ("VICTIM", victim.to_str().unwrap()),
            ];
            let got = copy_under(&shell, &env, &Act::Copy(&src, &dir, &[]));
            let away = base.path().join("into.away");
            let why = format!("{shell:?}, taken in the link's target: {taken}");
            assert!(got.is_ok(), "{why}: {got:?}");
            assert_eq!(names(&away), ["authorized_keys"], "{why}");
            assert_eq!(
                fs::read(away.join("authorized_keys")).unwrap(),
                b"mine",
                "{why}"
            );
            let theirs: &[&str] = if taken { &["authorized_keys"] } else { &[] };
            assert_eq!(names(&victim), theirs, "{why}");
            if taken {
                assert_eq!(
                    fs::read(victim.join("authorized_keys")).unwrap(),
                    b"theirs",
                    "{why}"
                );
            }
        }
    }

    /// The folder a killed copy was made in is renamed while the clear runs: the clear still takes
    /// the scratch folder out of it, through the scratch folder's "..".
    #[test]
    fn a_clear_removes_the_scratch_folder_from_the_folder_it_entered_though_that_was_renamed() {
        let id = "0123456789abcdef0123456789abcdef";
        let swap = "case \" $* \" in *\" -rf \"*) i=$(cd -P .. && pwd -P); mv \"$i\" \"$i.away\" && mkdir \"$i\" ;; esac\nexec rm \"$@\"";
        let (_bin, path) = shimmed(&[("rm", swap)]);
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let into = d.path().join("into");
            let scratch = format!("{}{id}", fsops::SCRATCH_PREFIX);
            fs::create_dir_all(into.join(&scratch).join(id).join("half")).unwrap();
            let said = clear_under(&shell, &[("PATH", path.as_str())], &into, id);
            assert_eq!(said, "done", "{shell:?}");
            assert!(names(&d.path().join("into.away")).is_empty(), "{shell:?}");
            assert!(names(&into).is_empty(), "{shell:?}");
        }
    }

    /// The line to the machine is lost during a copy: the machine kills the script's process group
    /// (no trap runs), and `hc` answers `lost the line to <machine> during the call`. The clear
    /// follows; when it cannot be sent either, the answer says where a partial copy may be. A
    /// machine never reached was sent nothing, and gets no clear and no word of a partial copy. The
    /// texts are hc 0.1.21's own (`/tmp/swarm-pr-a/owner-r2/e1/summary.txt`).
    #[test]
    fn a_copy_cut_short_by_a_lost_line_is_cleared_or_says_where_a_partial_copy_may_be() {
        let d = tempfile::tempdir().unwrap();
        let src = sample(d.path());
        let into = tempfile::tempdir().unwrap();
        let dir = into.path().to_string_lossy().into_owned();
        let (_bin, path) = hung_cp();
        let lost = "lost the line to lab during the call";
        const UNREACHABLE: &str = "lab is unreachable: cannot reach /srv/via: ssh: connect to host lab: Connection refused";
        for the_clear_answers in [true, false] {
            let mut cleared = 0;
            let got = copy_by(&src, &dir, &[], None, |req| {
                if req.args.iter().any(|a| a == CLEAR_SCRIPT) {
                    cleared += 1;
                    if !the_clear_answers {
                        return Err(ExecError::Remote(UNREACHABLE.into()));
                    }
                    return fsops::run(killed_after_a_second(&["/bin/sh"], &path, req));
                }
                let _ = fsops::run(killed_after_a_second(&["/bin/sh"], &path, req));
                Err(ExecError::Remote(lost.into()))
            });
            let why = format!("the clear answers: {the_clear_answers}");
            assert_eq!(cleared, 1, "{why}");
            let Err(Problem::Failed(said)) = &got else {
                panic!("{why}: {got:?}")
            };
            assert!(said.starts_with(lost), "{why}: {said}");
            if the_clear_answers {
                assert!(!said.contains("partial"), "{why}: {said}");
                assert!(
                    names(into.path()).is_empty(),
                    "{why}: {:?}",
                    names(into.path())
                );
            } else {
                assert!(
                    said.contains("A partial copy may be left in"),
                    "{why}: {said}"
                );
                for left in scratch_left(into.path()) {
                    fs::remove_dir_all(into.path().join(left)).unwrap();
                }
            }
        }
        for never_reached in [
            UNREACHABLE,
            "lab is unreachable: cannot reach /srv/via: the connection closed",
            "machine lab is not connected",
        ] {
            let mut asked = 0;
            let got = copy_by(&src, &dir, &[], None, |_| {
                asked += 1;
                Err(ExecError::Remote(never_reached.into()))
            });
            assert_eq!(asked, 1, "no clear after: {never_reached}");
            assert!(
                matches!(&got, Err(Problem::Failed(said)) if said == never_reached),
                "{got:?}"
            );
        }
    }

    /// [`CLEAR_SCRIPT`] under `shell`, with `env` set, for the copy `id` made in `into`: the line it
    /// prints.
    fn clear_under(shell: &[&str], env: &[(&str, &str)], into: &Path, id: &str) -> String {
        let into = into.to_string_lossy();
        let req = fsops::sh_request(None, CLEAR_SCRIPT, &[&into, id], TIMEOUT, MAX_OUTPUT);
        let out = Command::new(shell[0])
            .args(&shell[1..])
            .args(&req.args)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn a_clear_that_cannot_see_the_folder_the_copy_was_made_in_says_the_copy_is_left() {
        let id = "0123456789abcdef0123456789abcdef";
        for shell in shells() {
            let d = tempfile::tempdir().unwrap();
            let into = d.path().join("into");
            let scratch = format!("{}{id}", fsops::SCRATCH_PREFIX);
            fs::create_dir_all(into.join(&scratch).join(id).join("half")).unwrap();
            fs::write(into.join(&scratch).join(id).join("half/f"), "f").unwrap();
            // The folder renamed away: the copy is out of sight, not gone.
            let moved = d.path().join("moved");
            fs::rename(&into, &moved).unwrap();
            assert_eq!(clear_under(&shell, &[], &into, id), "left", "{shell:?}");
            assert!(
                moved.join(&scratch).join(id).join("half/f").exists(),
                "{shell:?}"
            );
            // A folder that cannot be entered.
            if !is_root() {
                fs::set_permissions(&moved, fs::Permissions::from_mode(0o600)).unwrap();
                assert_eq!(clear_under(&shell, &[], &moved, id), "left", "{shell:?}");
                fs::set_permissions(&moved, fs::Permissions::from_mode(0o755)).unwrap();
            }
            // In sight: cleared, then known to be gone.
            assert_eq!(clear_under(&shell, &[], &moved, id), "done", "{shell:?}");
            assert!(names(&moved).is_empty(), "{shell:?}");
            assert_eq!(clear_under(&shell, &[], &moved, id), "done", "{shell:?}");
        }
    }

    /// Where the freedesktop trash of `home` is, and what it holds.
    fn trashed(data: &Path) -> (Vec<String>, Vec<String>) {
        let at = |sub: &str| {
            let dir = data.join("Trash").join(sub);
            if dir.exists() {
                names(&dir)
            } else {
                Vec::new()
            }
        };
        (at("files"), at("info"))
    }

    const TRASH_TOOLS: [&str; 9] = ["mkdir", "mv", "stat", "od", "date", "rm", "ls", "chmod", "id"];

    #[test]
    fn every_shell_moves_to_the_freedesktop_trash_when_nothing_else_can() {
        let bin = only(&TRASH_TOOLS, &[]);
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let data = home.path().join("data");
            let env = [
                ("PATH", bin.path().to_str().unwrap()),
                ("HOME", home.path().to_str().unwrap()),
                ("XDG_DATA_HOME", data.to_str().unwrap()),
            ];
            let d = tempfile::tempdir().unwrap();
            let first = put(d.path(), "a b%.txt", b"one");
            assert_eq!(
                run_under(&shell, None, &env, &Act::Trash(&first)),
                Ok(()),
                "{shell:?}"
            );
            let second = put(d.path(), "a b%.txt", b"two");
            assert_eq!(
                run_under(&shell, None, &env, &Act::Trash(&second)),
                Ok(()),
                "{shell:?}"
            );
            fs::create_dir(d.path().join("folder")).unwrap();
            fs::write(d.path().join("folder/in"), "in").unwrap();
            assert_eq!(
                run_under(&shell, None, &env, &Act::Trash(&at(d.path(), "folder"))),
                Ok(())
            );
            symlink("nowhere", d.path().join("link")).unwrap();
            assert_eq!(
                run_under(&shell, None, &env, &Act::Trash(&at(d.path(), "link"))),
                Ok(())
            );
            assert!(names(d.path()).is_empty(), "{shell:?}");
            let files = data.join("Trash/files");
            assert_eq!(
                trashed(&data),
                (
                    vec![
                        "a b%.2.txt".into(),
                        "a b%.txt".into(),
                        "folder".into(),
                        "link".into()
                    ],
                    vec![
                        "a b%.2.txt.trashinfo".into(),
                        "a b%.txt.trashinfo".into(),
                        "folder.trashinfo".into(),
                        "link.trashinfo".into()
                    ]
                ),
                "{shell:?}"
            );
            assert_eq!(fs::read(files.join("a b%.txt")).unwrap(), b"one");
            assert_eq!(fs::read(files.join("a b%.2.txt")).unwrap(), b"two");
            assert_eq!(fs::read(files.join("folder/in")).unwrap(), b"in");
            assert!(
                fs::symlink_metadata(files.join("link"))
                    .unwrap()
                    .is_symlink()
            );
            // The info says where it was, URL-escaped, and when.
            let info = fs::read_to_string(data.join("Trash/info/a b%.txt.trashinfo")).unwrap();
            let lines: Vec<&str> = info.lines().collect();
            assert_eq!(lines[0], "[Trash Info]");
            let escaped = lines[1].strip_prefix("Path=").unwrap();
            assert!(escaped.ends_with("/a%20b%25.txt"), "{escaped}");
            let mut bytes = Vec::new();
            let mut rest = escaped.as_bytes();
            while let Some((&b, tail)) = rest.split_first() {
                if b == b'%' {
                    bytes.push(
                        u8::from_str_radix(std::str::from_utf8(&tail[..2]).unwrap(), 16).unwrap(),
                    );
                    rest = &tail[2..];
                } else {
                    bytes.push(b);
                    rest = tail;
                }
            }
            assert_eq!(String::from_utf8(bytes).unwrap(), first);
            let date = lines[2].strip_prefix("DeletionDate=").unwrap();
            assert!(
                date.len() == 19 && date.as_bytes()[10] == b'T' && date.as_bytes()[4] == b'-',
                "{date}"
            );
            assert_eq!(lines.len(), 3);
        }
        // XDG_DATA_HOME that is not a full path is not used: the home's own is.
        let home = tempfile::tempdir().unwrap();
        let env = [
            ("PATH", bin.path().to_str().unwrap()),
            ("HOME", home.path().to_str().unwrap()),
            ("XDG_DATA_HOME", "relative"),
        ];
        let d = tempfile::tempdir().unwrap();
        let f = put(d.path(), "f", b"f");
        assert_eq!(run_under(&["/bin/sh"], None, &env, &Act::Trash(&f)), Ok(()));
        assert_eq!(trashed(&home.path().join(".local/share")).0, ["f"]);
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn the_trash_by_hand_is_for_its_owner_alone() {
        let bin = only(&TRASH_TOOLS, &[]);
        for shell in shells() {
            for already in [false, true] {
                let home = tempfile::tempdir().unwrap();
                let data = home.path().join("data");
                if already {
                    fs::create_dir_all(data.join("Trash")).unwrap();
                    fs::set_permissions(data.join("Trash"), fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
                let env = [
                    ("PATH", bin.path().to_str().unwrap()),
                    ("HOME", home.path().to_str().unwrap()),
                    ("XDG_DATA_HOME", data.to_str().unwrap()),
                ];
                let d = tempfile::tempdir().unwrap();
                let f = put(d.path(), "f.txt", b"x");
                let got = run_under(&shell, Some("022"), &env, &Act::Trash(&f));
                assert_eq!(got, Ok(()), "{shell:?} {already}");
                let why = format!("{shell:?}, a trash there before: {already}");
                assert_eq!(mode(&data.join("Trash")), 0o700, "{why}");
                assert_eq!(mode(&data.join("Trash/info/f.txt.trashinfo")), 0o600, "{why}");
                if !already {
                    assert_eq!(mode(&data.join("Trash/files")), 0o700, "{why}");
                }
            }
        }
    }

    #[test]
    fn gio_is_asked_first_then_a_trash_command_then_the_trash_by_hand() {
        let took = tempfile::tempdir().unwrap();
        let log = took.path().join("log");
        let works = |tool: &str, arg: &str| {
            format!("echo {tool} >> \"$LOG\"; mv -- \"{arg}\" \"$TOOK/{tool}\"")
        };
        let fails = |tool: &str| format!("echo {tool} >> \"$LOG\"; exit 1");
        // A tool that says it did but did not is not believed either.
        let lies = |tool: &str| format!("echo {tool} >> \"$LOG\"; exit 0");
        for (gio, trash, want, by_hand) in [
            (works("gio", "$2"), works("trash", "$1"), "gio\n", false),
            (fails("gio"), works("trash", "$1"), "gio\ntrash\n", false),
            (lies("gio"), fails("trash"), "gio\ntrash\n", true),
        ] {
            let bin = only(&TRASH_TOOLS, &[("gio", &gio), ("trash", &trash)]);
            let home = tempfile::tempdir().unwrap();
            let data = home.path().join("data");
            let env = [
                ("PATH", bin.path().to_str().unwrap()),
                ("HOME", home.path().to_str().unwrap()),
                ("XDG_DATA_HOME", data.to_str().unwrap()),
                ("LOG", log.to_str().unwrap()),
                ("TOOK", took.path().to_str().unwrap()),
            ];
            let d = tempfile::tempdir().unwrap();
            let f = put(d.path(), "f.txt", b"x");
            assert_eq!(run_under(&["/bin/sh"], None, &env, &Act::Trash(&f)), Ok(()));
            assert!(names(d.path()).is_empty());
            assert_eq!(fs::read_to_string(&log).unwrap(), want);
            assert_eq!(trashed(&data).0.len(), usize::from(by_hand), "{want}");
            fs::remove_file(&log).unwrap();
        }
    }

    #[test]
    fn what_no_trash_can_take_stays_where_it_is_and_says_why() {
        let bin = only(&TRASH_TOOLS, &[]);
        let home = tempfile::tempdir().unwrap();
        let d = tempfile::tempdir().unwrap();
        let f = put(d.path(), "f.txt", b"x");
        let path = bin.path().to_str().unwrap();
        let ghost = at(d.path(), "ghost");
        if !is_root() {
            let locked = home.path().join("locked");
            fs::create_dir(&locked).unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
            let shut = home.path().join("shut");
            fs::create_dir(&shut).unwrap();
            let inside = put(&shut, "in.txt", b"in");
            fs::set_permissions(&shut, fs::Permissions::from_mode(0o555)).unwrap();
            for shell in shells() {
                let env = [
                    ("PATH", path),
                    ("HOME", home.path().to_str().unwrap()),
                    ("XDG_DATA_HOME", locked.to_str().unwrap()),
                ];
                assert_eq!(
                    run_under(&shell, None, &env, &Act::Trash(&f)),
                    Err(Problem::NoTrash)
                );
                assert_eq!(
                    run_under(&shell, None, &env, &Act::Trash(&inside)),
                    Err(Problem::FolderNotWritable)
                );
            }
            assert_eq!(fs::read(&inside).unwrap(), b"in");
            fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let data = home.path().join("data");
        let env = [
            ("PATH", path),
            ("HOME", home.path().to_str().unwrap()),
            ("XDG_DATA_HOME", data.to_str().unwrap()),
        ];
        for shell in shells() {
            assert_eq!(
                run_under(&shell, None, &env, &Act::Trash(&ghost)),
                Err(Problem::Gone)
            );
        }
        // A program the trash by hand needs is named.
        let no_mv = only(&["mkdir", "stat", "od", "date", "rm"], &[]);
        let env = [
            ("PATH", no_mv.path().to_str().unwrap()),
            ("HOME", home.path().to_str().unwrap()),
            ("XDG_DATA_HOME", data.to_str().unwrap()),
        ];
        assert_eq!(
            run_under(&["/bin/sh"], None, &env, &Act::Trash(&f)),
            Err(Problem::NoTool("mv"))
        );
        assert_eq!(fs::read(&f).unwrap(), b"x");
        assert_eq!(names(d.path()), ["f.txt"]);
    }

    #[test]
    fn a_file_on_another_disk_than_the_trash_is_refused_as_such_and_stays() {
        let real = Command::new("/bin/sh")
            .args(["-c", "command -v stat"])
            .output()
            .unwrap();
        let real = String::from_utf8(real.stdout).unwrap();
        // Everything in a folder named "far" is on device 99.
        let stat = format!(
            "for a; do last=$a; done\ncase $last in /*) at=$last ;; *) at=$(pwd -P)/$last ;; esac\ncase \" $* \" in *' %d -- '*) case $at in */far/*) echo 99; exit 0 ;; esac ;; esac\nexec {} \"$@\"",
            real.trim()
        );
        let tools: Vec<&str> = TRASH_TOOLS.into_iter().filter(|t| *t != "stat").collect();
        let bin = only(&tools, &[("stat", &stat)]);
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let data = home.path().join("data");
            let env = [
                ("PATH", bin.path().to_str().unwrap()),
                ("HOME", home.path().to_str().unwrap()),
                ("XDG_DATA_HOME", data.to_str().unwrap()),
            ];
            let d = tempfile::tempdir().unwrap();
            fs::create_dir(d.path().join("far")).unwrap();
            let f = put(&d.path().join("far"), "f.txt", b"x");
            assert_eq!(
                run_under(&shell, None, &env, &Act::Trash(&f)),
                Err(Problem::OtherDevice),
                "{shell:?}"
            );
            assert_eq!(fs::read(&f).unwrap(), b"x", "{shell:?}");
            assert_eq!(trashed(&data), (vec![], vec![]), "{shell:?}");
        }
    }

    /// Someone who may rename where the folder is swaps it, after the trash looked at it, for a link
    /// to another folder holding an entry of the same name: the entry moved is the one in the folder
    /// entered, and the one in the link's target stays.
    #[test]
    fn a_folder_swapped_for_a_link_during_a_trash_gives_up_nothing_of_the_link_target() {
        let real = |tool: &str| {
            let out = Command::new("/bin/sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        // `date` runs after every check and before the move.
        let date = format!(
            "{} \"$FOLDER\" \"$FOLDER.away\" && {} -s \"$VICTIM\" \"$FOLDER\"\nexec {} \"$@\"",
            real("mv"),
            real("ln"),
            real("date")
        );
        let tools: Vec<&str> = TRASH_TOOLS.into_iter().filter(|t| *t != "date").collect();
        let bin = only(&tools, &[("date", &date)]);
        for shell in shells() {
            let base = tempfile::tempdir().unwrap();
            let (folder, victim) = (base.path().join("w"), base.path().join("v"));
            fs::create_dir(&folder).unwrap();
            fs::create_dir(&victim).unwrap();
            let f = put(&folder, "f.txt", b"mine");
            put(&victim, "f.txt", b"theirs");
            let data = base.path().join("data");
            let env = [
                ("PATH", bin.path().to_str().unwrap()),
                ("HOME", base.path().to_str().unwrap()),
                ("XDG_DATA_HOME", data.to_str().unwrap()),
                ("FOLDER", folder.to_str().unwrap()),
                ("VICTIM", victim.to_str().unwrap()),
            ];
            assert_eq!(
                run_under(&shell, None, &env, &Act::Trash(&f)),
                Ok(()),
                "{shell:?}"
            );
            assert_eq!(
                fs::read(victim.join("f.txt")).unwrap(),
                b"theirs",
                "{shell:?}"
            );
            assert!(names(&base.path().join("w.away")).is_empty(), "{shell:?}");
            assert_eq!(trashed(&data).0, ["f.txt"], "{shell:?}");
            assert_eq!(
                fs::read(data.join("Trash/files/f.txt")).unwrap(),
                b"mine",
                "{shell:?}"
            );
        }
    }

    #[test]
    fn a_trash_that_is_someone_elses_gets_nothing_made_in_it_and_nothing_moved_to_it() {
        let tools: Vec<&str> = TRASH_TOOLS.into_iter().filter(|t| *t != "id").collect();
        let bin = only(&tools, &[("id", "echo 4242")]);
        for shell in shells() {
            for there_before in [true, false] {
                let home = tempfile::tempdir().unwrap();
                let data = home.path().join("data");
                if there_before {
                    fs::create_dir_all(data.join("Trash")).unwrap();
                    fs::set_permissions(data.join("Trash"), fs::Permissions::from_mode(0o700))
                        .unwrap();
                }
                let env = [
                    ("PATH", bin.path().to_str().unwrap()),
                    ("HOME", home.path().to_str().unwrap()),
                    ("XDG_DATA_HOME", data.to_str().unwrap()),
                ];
                let d = tempfile::tempdir().unwrap();
                let f = put(d.path(), "f.txt", b"x");
                let why = format!("{shell:?}, a trash there before: {there_before}");
                assert_eq!(
                    run_under(&shell, None, &env, &Act::Trash(&f)),
                    Err(Problem::NoTrash),
                    "{why}"
                );
                assert_eq!(fs::read(&f).unwrap(), b"x", "{why}");
                if there_before {
                    assert!(names(&data.join("Trash")).is_empty(), "{why}");
                }
            }
        }
    }

    #[test]
    fn an_od_that_writes_no_octal_is_named_and_no_trash_info_is_left_without_a_path() {
        let tools: Vec<&str> = TRASH_TOOLS.into_iter().filter(|t| *t != "od").collect();
        let bin = only(&tools, &[("od", "echo 'od: invalid option -- t' >&2; exit 1")]);
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let data = home.path().join("data");
            let env = [
                ("PATH", bin.path().to_str().unwrap()),
                ("HOME", home.path().to_str().unwrap()),
                ("XDG_DATA_HOME", data.to_str().unwrap()),
            ];
            let d = tempfile::tempdir().unwrap();
            let f = put(d.path(), "f.txt", b"x");
            assert_eq!(
                run_under(&shell, None, &env, &Act::Trash(&f)),
                Err(Problem::NoTool("od")),
                "{shell:?}"
            );
            assert_eq!(fs::read(&f).unwrap(), b"x", "{shell:?}");
            assert_eq!(trashed(&data), (vec![], vec![]), "{shell:?}");
        }
    }

    #[test]
    fn a_trash_info_that_cannot_be_written_is_taken_back_and_the_file_stays() {
        let bin = only(&TRASH_TOOLS, &[]);
        for shell in shells() {
            let home = tempfile::tempdir().unwrap();
            let data = home.path().join("data");
            let d = tempfile::tempdir().unwrap();
            let f = put(d.path(), "f.txt", b"x");
            let req = request(&Act::Trash(&f), None).unwrap();
            // A disk with no room left: a name can be made, nothing written in it.
            let out = Command::new("/bin/sh")
                .args(["-c", "ulimit -f 0; exec \"$@\"", "x"])
                .args(&shell)
                .args(&req.args)
                .env("PATH", bin.path())
                .env("HOME", home.path())
                .env("XDG_DATA_HOME", &data)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            let got = interpret(Ok(ExecOutput {
                code: out.status.code().unwrap_or(-1),
                stdout: out.stdout,
                stderr: out.stderr,
            }));
            assert!(
                matches!(&got, Err(Problem::Failed(why)) if why.contains("the trash information for f.txt could not be written")),
                "{shell:?}: {got:?}"
            );
            assert_eq!(trashed(&data), (vec![], vec![]), "{shell:?}");
            assert_eq!(fs::read(&f).unwrap(), b"x", "{shell:?}");
        }
    }

    /// On Linux machines, in throwaway containers (`docker info` must work, and the images are
    /// pulled): the scripts as an ordinary user, against folders with default ACLs and setgid, makers
    /// and movers that race, two file systems, programs that are missing, copies, the home trash.
    /// `YZ_DOCKER=1 cargo test --locked -p yuzora-files --lib -- --ignored --exact
    /// remote_entry::tests::linux_entries_on_real_machines --nocapture`.
    #[test]
    #[ignore = "needs docker and YZ_DOCKER=1"]
    fn linux_entries_on_real_machines() {
        assert!(std::env::var("YZ_DOCKER").is_ok(), "set YZ_DOCKER=1");
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("make.sh"), MAKE_SCRIPT).unwrap();
        fs::write(dir.path().join("move.sh"), MOVE_SCRIPT).unwrap();
        fs::write(dir.path().join("copy.sh"), COPY_SCRIPT).unwrap();
        fs::write(dir.path().join("trash.sh"), TRASH_SCRIPT).unwrap();
        fs::write(
            dir.path().join("entry.sh"),
            include_str!("../tests/docker/remote-entry.sh"),
        )
        .unwrap();
        let mut failures = Vec::new();
        for (image, shell) in [
            ("alpine:3", "sh"),
            ("ubuntu:22.04", "sh"),
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
