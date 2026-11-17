//! File-system operations for the Files lane: list, stat, read and write text,
//! create, rename, copy, trash, reveal. Every function blocks (call it from a worker
//! thread). `on: Some(machine)` runs the operation on a herdr-core machine
//! through `sh` scripts (POSIX paths, arguments always as argv, never spliced
//! into the script); `None` works on this machine directly.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::ffi::OsString;
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, UNIX_EPOCH};

use yuzora_core::remote::{self, ExecError, ExecOutput, ExecRequest};

use crate::remote_entry::{self, Act, Kind, Problem};
pub use crate::remote_write::Refusal;
use crate::remote_write::{MAX_BYTES, posix_crc, resolve_links, save_gate};

const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How much of a file decides "binary".
const SNIFF_BYTES: usize = 8 << 10;
const BOM: &[u8] = b"\xef\xbb\xbf";
/// Name prefix of the scratch file a save renames over its target; the file
/// watcher ignores names of exactly the shape [`is_scratch_name`] accepts.
pub const SCRATCH_PREFIX: &str = ".yuzora-save-";
/// Leading bytes that make a file binary whatever follows.
const MAGIC: &[&[u8]] = &[
    b"%PDF-",
    b"\x89PNG\r\n\x1a\n",
    b"PK\x03\x04",
    b"PK\x05\x06",
    b"PK\x07\x08",
    b"\xff\xd8\xff",
    b"GIF87a",
    b"GIF89a",
    b"OggS",
    b"fLaC",
    b"RIFF",
    b"\x7fELF",
    b"\x1f\x8b",
    b"\x28\xb5\x2f\xfd",
    b"7z\xbc\xaf\x27\x1c",
    b"\xcf\xfa\xed\xfe",
    b"\xce\xfa\xed\xfe",
    b"\xca\xfe\xba\xbe",
    b"wOFF",
    b"wOF2",
];

/// Prints `kind\0name\0` per entry (d dir, f file, l symlink, L symlink to a
/// directory), then `-\0` and the size of every `f` entry, one per line, in
/// the same order. Stops after `$2` entries. `$1` is the directory.
const LIST_SCRIPT: &str = r#"d=$1; max=$2
cd -- "$d" 2>/dev/null || { echo "cannot open $d" >&2; exit 3; }
command -v find >/dev/null 2>&1 || { echo "find not found" >&2; exit 3; }
# Entries are read one by one from find, never expanded as a glob: past `max`
# the loop stops and find stops (SIGPIPE) with it, so the rest is not
# printed (the machine's find may still have read the folder). A name with a
# newline in it cannot be told apart in this stream: find prints `?` for it
# (a real name starts with `./`), which is listed as `?` with no name.
find . ! -name . -prune \( -name '*
*' -exec printf '?\n' \; -o -print \) 2>/dev/null | {
n=0
set --
while IFS= read -r f; do
  case $f in
    '?') n=$((n+1)); [ "$n" -gt "$max" ] && break; printf '?\0\0'; continue;;
    ./.git|./.DS_Store) continue;;
    ./.yuzora-save-*) r=${f#./.yuzora-save-}; case $r in *[!0-9a-f]*) ;; *) [ "${#r}" -eq 32 ] && continue;; esac;;
  esac
  if [ -L "$f" ]; then
    if [ -d "$f" ]; then k=L; else k=l; fi
  elif [ -d "$f" ]; then k=d
  elif [ -e "$f" ]; then k=f
  else continue
  fi
  n=$((n+1))
  [ "$n" -gt "$max" ] && break
  printf '%s\0%s\0' "$k" "${f#./}"
  [ "$k" = f ] && set -- "$@" "$f"
done
printf -- '-\0'
if [ "$#" -gt 0 ]; then
  stat -c %s -- "$@" 2>/dev/null || stat -f %z -- "$@" 2>/dev/null
fi
}
exit 0"#;

/// Exit 4 when `$1` does not exist; else prints `size mtime-seconds`. Not
/// there only when the nearest folder above that is there can be looked
/// into: a shut folder on the way is an error (exit 6), as it is here.
const STAT_SCRIPT: &str = r#"if [ ! -e "$1" ]; then
  p=$1
  while d=$(dirname -- "$p") && [ "$d" != "$p" ]; do
    if [ -e "$d" ]; then
      if [ -d "$d" ] && [ ! -x "$d" ]; then echo "cannot look inside $d" >&2; exit 6; fi
      exit 4
    fi
    p=$d
  done
  exit 4
fi
stat -L -c '%s %Y' -- "$1" 2>/dev/null || stat -L -f '%z %m' -- "$1""#;

/// `$1` file, `$2` max+1, `$3` max. Exit 4 missing, 5 not a regular file.
/// Prints `size mtime-seconds w|r\n` (`w`: a save can replace it, which is what
/// `crate::remote_write::save_gate!`'s `blocked` says, the very check a save makes: the file
/// and its folder can be written, the owner and the group can be kept, and an ACL, if the file
/// has one, can be carried over), then up to `$2` bytes unless it is larger than `$3`.
pub(crate) const READ_SCRIPT: &str = concat!(
    r#"[ -e "$1" ] || { echo "no such file" >&2; exit 4; }
[ -f "$1" ] || { echo "not a regular file" >&2; exit 5; }
s=$(stat -L -c '%s %Y' -- "$1" 2>/dev/null || stat -L -f '%z %m' -- "$1") || exit 6
"#,
    resolve_links!(),
    save_gate!(),
    r#"w=r
[ -z "$(blocked)" ] && w=w
printf '%s %s\n' "$s" "$w"
[ "${s%% *}" -gt "$3" ] && exit 0
head -c "$2" -- "$1""#
);

/// One directory entry.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Entry {
    pub name: String,
    /// A directory (a symlink to one counts).
    pub dir: bool,
    pub link: bool,
    pub hidden: bool,
    /// Bytes of a regular file; 0 for folders and links.
    pub size: u64,
}

/// A folder's entries, as [`list_dir`] gives them.
#[derive(Debug, Default, PartialEq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// There were more than the limit.
    pub truncated: bool,
    /// Some entries are left out unread: a name that is not UTF-8 (a stand-in
    /// for one, with U+FFFD, could open or rename another entry), an entry
    /// whose kind could not be read, or a read that failed partway.
    pub skipped: bool,
}

/// What a file looked like when it was read; a save compares against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Stamp {
    pub len: u64,
    pub mtime_ms: i64,
}

/// What a file was when it was read, for a save to hold it to: the stamp
/// and two digests of the bytes. A save here compares [`content_hash`]; one
/// on another machine compares [`crate::remote_write::posix_crc`], which that
/// machine's `cksum` can compute (FNV cannot be done in `sh`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub stamp: Stamp,
    pub hash: u64,
    pub crc: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Eol {
    Lf,
    Crlf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextFile {
    /// LF-normalized (`\r\n` becomes `\n`); empty when not loadable.
    pub text: String,
    /// `Crlf` when the file's first line break is `\r\n`.
    pub eol: Eol,
    /// A UTF-8 BOM was stripped from `text`.
    pub bom: bool,
    pub stamp: Stamp,
    /// [`content_hash`] of the raw bytes as read (0 when none were read).
    pub hash: u64,
    /// [`crate::remote_write::posix_crc`] of the raw bytes as read, from
    /// another machine only (0 for a file read here, which never needs it).
    pub crc: u32,
    pub size: u64,
    /// A NUL byte in the first 8 KiB, or a known binary signature.
    pub binary: bool,
    /// Larger than `max_bytes`: `text` is empty, nothing else was read.
    pub too_large: bool,
    /// Invalid UTF-8: `text` is a lossy decode and must not be saved back.
    pub not_utf8: bool,
    /// A save could not replace the file: no write permission, an owner or group that cannot be
    /// kept, or (on another machine) more than `hc exec --stdin` carries.
    pub readonly: bool,
}

#[derive(Debug, PartialEq)]
pub enum WriteError {
    /// The file changed (the current stamp, `None` when it is gone) since `expected`.
    Conflict(Option<Stamp>),
    /// Nothing was written, for a reason the person can act on.
    Refused(Refusal),
    Io(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Conflict(Some(_)) => f.write_str("the file changed on disk"),
            WriteError::Conflict(None) => f.write_str("the file was removed from disk"),
            WriteError::Refused(why) => why.fmt(f),
            WriteError::Io(e) => f.write_str(e),
        }
    }
}

/// The machine to run on; an empty name means here (as in `ExecRequest::on`).
pub(crate) fn machine(on: Option<&str>) -> Option<&str> {
    on.filter(|m| !m.is_empty())
}

/// Set on everything this crate runs, here or on a machine. The file tree's
/// marks and the search need no remote: git fetches nothing a partial clone
/// lacks (`GIT_NO_LAZY_FETCH`, git 2.44 and later) and reaches none
/// (`GIT_ALLOW_PROTOCOL` empty, which no config of the repo's widens; it
/// stops an older git's fetch too, a local one included, whose file
/// transport runs the repo's own `uploadpack`). Otherwise a promisor
/// remote's URL picks the transport, or the remote helper, git starts for it.
pub(crate) const OFFLINE: [(&str, &str); 2] =
    [("GIT_NO_LAZY_FETCH", "1"), ("GIT_ALLOW_PROTOCOL", "")];

/// Runs `req` here or on its machine with [`OFFLINE`] set: the one way this
/// crate runs a program (a test holds it to that).
pub(crate) fn run(req: ExecRequest) -> Result<ExecOutput, ExecError> {
    remote::run(&offline(req))
}

/// `req` with [`OFFLINE`] set.
fn offline(req: ExecRequest) -> ExecRequest {
    OFFLINE.iter().fold(req, |req, (k, v)| req.env(*k, *v))
}

/// Runs `script` with `sh -c script sh args…` here (`on = None`) or on a machine.
pub(crate) fn run_sh(
    on: Option<&str>,
    script: &str,
    args: &[&str],
    timeout: Duration,
    max_output: u64,
) -> Result<ExecOutput, String> {
    run(sh_request(on, script, args, timeout, max_output)).map_err(|e| e.to_string())
}

/// What [`run_sh`] runs, before [`OFFLINE`].
pub(crate) fn sh_request(
    on: Option<&str>,
    script: &str,
    args: &[&str],
    timeout: Duration,
    max_output: u64,
) -> ExecRequest {
    let argv = ["-c", script, "sh"].into_iter().chain(args.iter().copied());
    ExecRequest::new("sh", argv)
        .on(on)
        .timeout(timeout)
        .max_output(max_output)
}

/// The program's stderr, else a line naming the exit code.
pub(crate) fn failure(out: &ExecOutput, what: &str) -> String {
    let err = out.stderr_text();
    match err.trim() {
        "" => format!("{what} failed (exit {})", out.code),
        e => e.to_owned(),
    }
}

// ---- listing ----

/// Folders first, then case-insensitive natural name order. Skips `.git` and
/// `.DS_Store`. At most `limit` entries ([`Listing::truncated`] says there
/// were more). On another machine the cut is made on the unsorted listing.
pub fn list_dir(path: &str, on: Option<&str>, limit: usize) -> Result<Listing, String> {
    match machine(on) {
        Some(m) => list_sh(path, Some(m), limit),
        None => list_local(path, limit),
    }
}

/// Never listed: git's folder, Finder's litter, and a save in flight
/// ([`LIST_SCRIPT`] skips the same names before it counts to the limit).
fn skipped(name: &str) -> bool {
    name == ".git" || name == ".DS_Store" || is_scratch_name(name)
}

fn list_local(path: &str, limit: usize) -> Result<Listing, String> {
    let read = std::fs::read_dir(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let (mut entries, skipped) = read_entries(read, limit);
    sort_entries(&mut entries);
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    for e in entries.iter_mut().filter(|e| !e.dir && !e.link) {
        e.size = std::fs::metadata(Path::new(path).join(&e.name)).map_or(0, |m| m.len());
    }
    Ok(Listing {
        entries,
        truncated,
        skipped,
    })
}

/// A folder entry as it is read: [`std::fs::DirEntry`], or a test's own.
trait Listed {
    fn name(&self) -> OsString;
    /// (a directory, a symlink); a link to a directory counts as one.
    fn kind(&self) -> io::Result<(bool, bool)>;
}

impl Listed for std::fs::DirEntry {
    fn name(&self) -> OsString {
        self.file_name()
    }

    fn kind(&self) -> io::Result<(bool, bool)> {
        let kind = self.file_type()?;
        let link = kind.is_symlink();
        let dir = if link {
            std::fs::metadata(self.path()).is_ok_and(|m| m.is_dir())
        } else {
            kind.is_dir()
        };
        Ok((dir, link))
    }
}

/// The entries of a folder as it is read, of which only the first
/// `limit + 1` (sorted) are kept: past twice that, the rest are dropped as it
/// goes, however big the folder. The flag says some were left out unread
/// ([`Listing::skipped`]); one gone meanwhile is just not there.
fn read_entries<E: Listed>(
    read: impl Iterator<Item = io::Result<E>>,
    limit: usize,
) -> (Vec<Entry>, bool) {
    let keep = limit.saturating_add(1);
    let mut entries = Vec::new();
    let mut unreadable = false;
    for ent in read {
        // Nothing after a failed read is read.
        let Ok(ent) = ent else {
            unreadable = true;
            break;
        };
        let Ok(name) = ent.name().into_string() else {
            unreadable = true;
            continue;
        };
        if skipped(&name) {
            continue;
        }
        let (dir, link) = match ent.kind() {
            Ok(kind) => kind,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                unreadable = true;
                continue;
            }
        };
        entries.push(Entry {
            hidden: name.starts_with('.'),
            name,
            dir,
            link,
            size: 0,
        });
        if entries.len() >= keep.saturating_mul(2) {
            sort_entries(&mut entries);
            entries.truncate(keep);
        }
    }
    (entries, unreadable)
}

fn list_sh(path: &str, on: Option<&str>, limit: usize) -> Result<Listing, String> {
    let max = limit.saturating_add(1).to_string();
    let out = run_sh(on, LIST_SCRIPT, &[path, &max], LIST_TIMEOUT, 16 << 20)?;
    if out.code != 0 {
        return Err(failure(&out, "listing"));
    }
    Ok(parse_listing(&out.stdout, limit))
}

/// Reads [`LIST_SCRIPT`]'s output (run with `limit + 1`). A name left out
/// still counts toward the cut: past it there may be more.
fn parse_listing(out: &[u8], limit: usize) -> Listing {
    let mut parts = out.split(|b| *b == 0);
    let mut rows = Vec::new();
    let mut sizes = &b""[..];
    while let Some(kind) = parts.next() {
        if kind == b"-" {
            sizes = parts.next().unwrap_or_default();
            break;
        }
        let Some(name) = parts.next() else { break };
        rows.push((kind.first().copied().unwrap_or(b'f'), name));
    }
    let printed = rows.len();
    let mut unreadable = false;
    let sizes: Vec<u64> = String::from_utf8_lossy(sizes)
        .lines()
        .map(|l| l.trim().parse().unwrap_or(0))
        .collect();
    let mut sizes = sizes.into_iter();
    let mut entries: Vec<Entry> = rows
        .into_iter()
        .filter_map(|(kind, name)| {
            // Sizes come one per file, so a file left out still takes its own.
            let size = if kind == b'f' {
                sizes.next().unwrap_or(0)
            } else {
                0
            };
            let Ok(name) = std::str::from_utf8(name).map_err(drop).and_then(|n| {
                // `?` is a name the script could not pass on (a newline in it).
                if kind == b'?' { Err(()) } else { Ok(n) }
            }) else {
                unreadable = true;
                return None;
            };
            Some(Entry {
                hidden: name.starts_with('.'),
                dir: matches!(kind, b'd' | b'L'),
                link: matches!(kind, b'l' | b'L'),
                size,
                name: name.to_owned(),
            })
        })
        .filter(|e| !skipped(&e.name))
        .collect();
    sort_entries(&mut entries);
    entries.truncate(limit);
    Listing {
        entries,
        truncated: printed > limit,
        skipped: unreadable,
    }
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| natural_cmp(&a.name, &b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Case-insensitive, digit runs by value (`a2` before `a10`).
fn natural_cmp(a: &str, b: &str) -> Ordering {
    fn digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
        let mut run = String::new();
        while let Some(c) = it.next_if(char::is_ascii_digit) {
            run.push(c);
        }
        run
    }
    fn fold(c: char) -> char {
        c.to_lowercase().next().unwrap_or(c)
    }
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (x, y) = (digits(&mut a), digits(&mut b));
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                a.next();
                b.next();
                let order = fold(x).cmp(&fold(y));
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

// ---- stat ----

/// Length and modification time, `None` when the path does not exist.
pub fn stat(path: &str, on: Option<&str>) -> Result<Option<Stamp>, String> {
    match machine(on) {
        Some(m) => stat_sh(path, Some(m)),
        None => stat_local(Path::new(path)),
    }
}

pub(crate) fn stamp_of(m: &std::fs::Metadata) -> Stamp {
    let mtime_ms = match m.modified().map(|t| t.duration_since(UNIX_EPOCH)) {
        Ok(Ok(d)) => d.as_millis() as i64,
        Ok(Err(before)) => -(before.duration().as_millis() as i64),
        Err(_) => 0,
    };
    Stamp {
        len: m.len(),
        mtime_ms,
    }
}

fn stat_local(path: &Path) -> Result<Option<Stamp>, String> {
    match std::fs::metadata(path) {
        Ok(m) => Ok(Some(stamp_of(&m))),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(format!("cannot stat {}: {e}", path.display())),
    }
}

fn stat_sh(path: &str, on: Option<&str>) -> Result<Option<Stamp>, String> {
    let out = run_sh(on, STAT_SCRIPT, &[path], LIST_TIMEOUT, 4 << 10)?;
    match out.code {
        0 => parse_stamp(&out.stdout_text())
            .map(Some)
            .ok_or_else(|| format!("cannot read the stat of {path}")),
        4 => Ok(None),
        _ => Err(failure(&out, "stat")),
    }
}

/// `size seconds` (the remote scripts' stat line).
fn parse_stamp(line: &str) -> Option<Stamp> {
    let mut it = line.split_whitespace();
    let len = it.next()?.parse().ok()?;
    let secs: i64 = it.next()?.parse().ok()?;
    Some(Stamp {
        len,
        mtime_ms: secs.saturating_mul(1000),
    })
}

/// FNV-1a 64: fast and stable across runs.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---- read ----

/// Reads a file as text. Never loads more than `max_bytes`; a larger file
/// comes back with `too_large` and only its stamp.
pub fn read_text(path: &str, on: Option<&str>, max_bytes: u64) -> Result<TextFile, String> {
    match machine(on) {
        Some(m) => read_sh(path, Some(m), max_bytes),
        None => read_local(path, max_bytes),
    }
}

/// A file here this process cannot write: no write bit, or opening it to
/// write is refused (owner, ACL, a read-only mount), or it is no longer a
/// regular file.
fn readonly_of(path: &Path, meta: &std::fs::Metadata) -> bool {
    let mut write = std::fs::OpenOptions::new();
    write.write(true);
    meta.permissions().readonly() || open_checked(write, path).is_err()
}

/// A file's permission bits here (on Windows, its read-only flag): cheap to
/// compare, for noticing a chmod, which moves neither size nor time.
pub fn mode(path: &str) -> Option<u32> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    return Some(std::os::unix::fs::PermissionsExt::mode(&meta.permissions()));
    #[cfg(not(unix))]
    return Some(u32::from(meta.permissions().readonly()));
}

/// Whether this process cannot write the file here. Opens it to write (not
/// writing anything), so ask only after [`mode`] changed.
pub fn readonly(path: &str) -> bool {
    std::fs::metadata(path).map_or(true, |m| readonly_of(Path::new(path), &m))
}

/// A `TextFile` that carries only what a stat knows.
fn blank(stamp: Stamp, readonly: bool) -> TextFile {
    TextFile {
        text: String::new(),
        eol: Eol::Lf,
        bom: false,
        stamp,
        hash: 0,
        crc: 0,
        size: stamp.len,
        binary: false,
        too_large: false,
        not_utf8: false,
        readonly,
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    head.contains(&0) || MAGIC.iter().any(|m| head.starts_with(m))
}

/// Classifies and decodes what was read.
fn decode(bytes: Vec<u8>, stamp: Stamp, readonly: bool, max_bytes: u64) -> TextFile {
    let base = blank(stamp, readonly);
    if bytes.len() as u64 > max_bytes {
        return TextFile {
            too_large: true,
            ..base
        };
    }
    let hash = content_hash(&bytes);
    if looks_binary(&bytes) {
        return TextFile {
            binary: true,
            hash,
            ..base
        };
    }
    let bom = bytes.starts_with(BOM);
    let body = if bom { &bytes[BOM.len()..] } else { &bytes[..] };
    let eol = match body.iter().position(|b| *b == b'\n') {
        Some(i) if i > 0 && body[i - 1] == b'\r' => Eol::Crlf,
        _ => Eol::Lf,
    };
    let (text, not_utf8) = match std::str::from_utf8(body) {
        Ok(s) => (Cow::Borrowed(s), false),
        Err(_) => (String::from_utf8_lossy(body), true),
    };
    let text = if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text.into_owned()
    };
    TextFile {
        text,
        eol,
        bom,
        hash,
        not_utf8,
        ..base
    }
}

/// Opens `path` to read it, if it is a regular file. Opened without waiting
/// and checked through the handle: a FIFO put in its place after a look at
/// it would otherwise wait for a writer, and a device never end.
pub(crate) fn open_regular(path: &Path) -> io::Result<std::fs::File> {
    let mut read = std::fs::OpenOptions::new();
    read.read(true);
    open_checked(read, path)
}

/// [`open_regular`] with any `options` (to write: a FIFO would wait for a
/// reader).
fn open_checked(mut options: std::fs::OpenOptions, path: &Path) -> io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // A regular file reads and writes the same either way.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

fn read_local(path: &str, max_bytes: u64) -> Result<TextFile, String> {
    let err = |e: io::Error| format!("cannot read {path}: {e}");
    let mut file = open_regular(Path::new(path)).map_err(err)?;
    let meta = file.metadata().map_err(err)?;
    let stamp = stamp_of(&meta);
    let readonly = readonly_of(Path::new(path), &meta);
    if stamp.len > max_bytes {
        return Ok(TextFile {
            too_large: true,
            ..blank(stamp, readonly)
        });
    }
    // The first bytes decide "binary"; a big binary is not read past them.
    let mut bytes = Vec::new();
    (&mut file)
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if !looks_binary(&bytes) {
        let rest = max_bytes
            .saturating_add(1)
            .saturating_sub(bytes.len() as u64);
        bytes.reserve(
            usize::try_from(stamp.len)
                .unwrap_or(0)
                .min(max_bytes as usize),
        );
        (&mut file)
            .take(rest)
            .read_to_end(&mut bytes)
            .map_err(err)?;
    }
    Ok(decode(bytes, stamp, readonly, max_bytes))
}

fn read_sh(path: &str, on: Option<&str>, max_bytes: u64) -> Result<TextFile, String> {
    let (upto, max) = (
        max_bytes.saturating_add(1).to_string(),
        max_bytes.to_string(),
    );
    let out = run_sh(
        on,
        READ_SCRIPT,
        &[path, &upto, &max],
        READ_TIMEOUT,
        max_bytes.saturating_add(1 << 10),
    )?;
    match out.code {
        0 => {}
        4 => return Err(format!("cannot read {path}: no such file")),
        _ => return Err(failure(&out, "reading")),
    }
    let split = out
        .stdout
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(|| format!("cannot read {path}: no answer"))?;
    let head = String::from_utf8_lossy(&out.stdout[..split]);
    let stamp = parse_stamp(&head).ok_or_else(|| format!("cannot read the stat of {path}"))?;
    // Read-only unless the script said a save can replace it (and a save fits
    // what hc carries).
    let readonly = head.split_whitespace().nth(2) != Some("w") || stamp.len > MAX_BYTES as u64;
    if stamp.len > max_bytes {
        return Ok(TextFile {
            too_large: true,
            ..blank(stamp, readonly)
        });
    }
    let bytes = out.stdout[split + 1..].to_vec();
    let crc = posix_crc(&bytes);
    Ok(TextFile {
        crc,
        ..decode(bytes, stamp, readonly, max_bytes)
    })
}

// ---- write ----

/// Saves `text` (LF) to a file on this machine, in `eol` and with a BOM when
/// `bom`. With `expected` (the stamp and [`content_hash`] last read), a file
/// whose stamp differs, that is gone, or whose bytes changed under the same
/// stamp (a coarse clock) is not touched: `Conflict` carries its current
/// stamp. That is checked again right before the swap, so a change made
/// while the new content was written is kept too (a rename has no compare
/// and swap: only what lands between that check and the rename is lost).
/// The write goes to a scratch file in the same directory that is
/// fsynced and renamed over the target, so a failed save leaves the old
/// content; mode (and owner) are kept. When it cannot be swapped (see
/// [`write_in_place`]) the old content is kept in a backup until the new one
/// is complete. A symlink is written through. On another machine, see
/// [`write_on`]. Returns the new stamp.
pub fn write_text(
    path: &str,
    text: &str,
    eol: Eol,
    bom: bool,
    expected: Option<(Stamp, u64)>,
) -> Result<Stamp, WriteError> {
    let io_err = |e: io::Error| WriteError::Io(format!("cannot write {path}: {e}"));
    let target = resolve_link(Path::new(path)).map_err(io_err)?;
    let still = || unchanged(&target, expected);
    let conflict = || match stat_local(&target) {
        Ok(now) => WriteError::Conflict(now),
        Err(e) => WriteError::Io(e),
    };
    if !still().map_err(io_err)? {
        return Err(conflict());
    }
    let bytes = encode_text(text, eol, bom);
    if !replace_atomically(&target, &bytes, &still).map_err(io_err)? {
        return Err(conflict());
    }
    match stat_local(&target).map_err(WriteError::Io)? {
        Some(stamp) => Ok(stamp),
        None => Err(WriteError::Io(format!(
            "cannot write {path}: the file vanished"
        ))),
    }
}

/// The bytes a save puts on disk for `text` (LF): `eol` line ends, a BOM when `bom`.
pub fn encode_text(text: &str, eol: Eol, bom: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + BOM.len());
    if bom {
        bytes.extend_from_slice(BOM);
    }
    match eol {
        Eol::Lf => bytes.extend_from_slice(text.as_bytes()),
        Eol::Crlf => bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
    }
    bytes
}

/// [`write_text`] here, or on `on`, a machine of herdr-core, with `expected`
/// as that file was read ([`TextFile`]'s `stamp`, `hash` and `crc`).
pub fn write_on(
    path: &str,
    on: Option<&str>,
    text: &str,
    eol: Eol,
    bom: bool,
    expected: Option<Seen>,
) -> Result<Stamp, WriteError> {
    match machine(on) {
        Some(m) => crate::remote_write::write_sh(
            path,
            &encode_text(text, eol, bom),
            expected.as_ref(),
            Some(m),
        ),
        None => write_text(
            path,
            text,
            eol,
            bom,
            expected.map(|seen| (seen.stamp, seen.hash)),
        ),
    }
}

/// Whether the file is as `expected` (its stamp and [`content_hash`]) has it;
/// always, without one.
fn unchanged(target: &Path, expected: Option<(Stamp, u64)>) -> io::Result<bool> {
    let Some((want, hash)) = expected else {
        return Ok(true);
    };
    let now = stat_local(target).map_err(io::Error::other)?;
    Ok(now == Some(want) && holds(target, want.len, hash)?)
}

/// Whether the file still holds the `len` bytes hashed as `hash`. Reads at
/// most one byte more, however much it grew since it was looked at.
fn holds(path: &Path, len: u64, hash: u64) -> io::Result<bool> {
    let mut bytes = Vec::new();
    open_regular(path)?
        .take(len.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes.len() as u64 == len && content_hash(&bytes) == hash)
}

/// Where a write to `path` lands: through a symlink, the file it names. A
/// link whose file is gone names where it was (a restore puts it back).
fn resolve_link(path: &Path) -> io::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).or_else(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                dangling_target(path)
            } else {
                Err(e)
            }
        }),
        _ => Ok(path.to_owned()),
    }
}

/// The path at the end of a chain of links, followed one link at a time.
fn dangling_target(path: &Path) -> io::Result<PathBuf> {
    let mut at = path.to_owned();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => {
                let next = std::fs::read_link(&at)?;
                at = match at.parent() {
                    Some(dir) => dir.join(next),
                    None => next,
                };
            }
            _ => return Ok(at),
        }
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

/// Whether `name` is a save's scratch file: the prefix and 32 lowercase hex digits.
pub fn is_scratch_name(name: &str) -> bool {
    name.strip_prefix(SCRATCH_PREFIX)
        .is_some_and(|r| r.len() == 32 && r.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

fn scratch_path(dir: &Path) -> PathBuf {
    dir.join(format!("{SCRATCH_PREFIX}{}", scratch_id()))
}

/// 32 lowercase hex digits no other scratch name of this run has.
pub(crate) fn scratch_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let id = (u64::from(std::process::id()) << 32)
        | (COUNTER.fetch_add(1, AtomicOrdering::Relaxed) & 0xffff_ffff);
    format!("{id:016x}{nanos:016x}")
}

/// Puts `bytes` in `target` when `unchanged` still says so right before the
/// swap: `false` when it did not (nothing written, no scratch file left).
fn replace_atomically(
    target: &Path,
    bytes: &[u8],
    unchanged: &dyn Fn() -> io::Result<bool>,
) -> io::Result<bool> {
    // Written in place: the check goes right before the write.
    let in_place = |backups: &Path| write_in_place(target, bytes, backups, unchanged);
    let original = std::fs::metadata(target).ok().filter(|m| m.is_file());
    if original.is_some() {
        // The file's own write bit decides, as it would for an in-place write
        // (opened as a regular file: a FIFO swapped in meanwhile isn't waited on).
        let mut write = std::fs::OpenOptions::new();
        write.write(true);
        open_checked(write, target)?;
    }
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let scratch = scratch_path(dir);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    // Owner-only until it holds the content and is about to replace the
    // file: only then does it take the file's mode, so nobody the file
    // keeps out can open it meanwhile.
    #[cfg(unix)]
    if original.is_some() {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    let mut file = match options.open(&scratch) {
        Ok(f) => f,
        // A directory that cannot hold a scratch file around a writable file:
        // the backup goes to a folder of the app's own.
        Err(e)
            if original.is_some()
                && matches!(
                    e.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
                ) =>
        {
            return in_place(&private_backups()?);
        }
        Err(e) => return Err(e),
    };
    #[cfg(unix)]
    if let Some(m) = &original
        && !(keep_owner(&file, m) && keep_extended(&file, target))
    {
        // The swap would change the owner, or drop an ACL or extended
        // attributes: write in place instead.
        drop(file);
        let _ = std::fs::remove_file(&scratch);
        return in_place(dir);
    }
    let staged = (|| {
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        if !unchanged()? {
            return Ok(false);
        }
        if let Some(m) = &original {
            file.set_permissions(m.permissions())?;
        }
        std::fs::rename(&scratch, target).map(|()| true)
    })();
    drop(file);
    if !matches!(staged, Ok(true)) {
        let _ = std::fs::remove_file(&scratch);
        return staged;
    }
    // The content is in place already; this only hardens the rename.
    #[cfg(unix)]
    let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    Ok(true)
}

/// Writes `bytes` over `target` in place (its inode, owner and links stay),
/// for a file that cannot be swapped. Its old content is first copied to a
/// new backup in `backups` and kept there until the new content is complete:
/// a failed write puts the old content back, and when even that fails the
/// error names the backup. `unchanged` is asked once the backup is complete,
/// right before the write, so a change made while it was copied is kept:
/// `false` when it says no (nothing written, no backup left).
fn write_in_place(
    target: &Path,
    bytes: &[u8],
    backups: &Path,
    unchanged: &dyn Fn() -> io::Result<bool>,
) -> io::Result<bool> {
    write_in_place_with(target, bytes, backups, unchanged, overwrite)
}

/// `write` puts what it reads into a file; the old content goes to the
/// backup and back from it a chunk at a time, however big the file is. It
/// is put back through the handle the backup was made with, never by name:
/// the folder may be someone else's to write in too.
fn write_in_place_with(
    target: &Path,
    bytes: &[u8],
    backups: &Path,
    unchanged: &dyn Fn() -> io::Result<bool>,
    write: impl Fn(&Path, &mut dyn Read) -> io::Result<()>,
) -> io::Result<bool> {
    let mut old = open_regular(target)?;
    let (backup, mut file) = new_backup(backups)?;
    if let Err(e) = io::copy(&mut old, &mut file).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&backup);
        return Err(e);
    }
    drop(old);
    let still = unchanged();
    if !matches!(still, Ok(true)) {
        drop(file);
        let _ = std::fs::remove_file(&backup);
        return still;
    }
    let Err(e) = write(target, &mut &bytes[..]) else {
        drop(file);
        let _ = std::fs::remove_file(&backup);
        return Ok(true);
    };
    if file
        .rewind()
        .and_then(|()| write(target, &mut file))
        .is_ok()
    {
        drop(file);
        let _ = std::fs::remove_file(&backup);
        return Err(e);
    }
    Err(io::Error::new(
        e.kind(),
        format!("{e}; its old content is in {}", backup.display()),
    ))
}

/// A backup file made in `dir`: always a new file (never an existing path or
/// a link left there), its owner's only, written and read back through the
/// handle given.
fn new_backup(dir: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    let mut taken = None;
    for _ in 0..8 {
        let path = scratch_path(dir);
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => taken = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(taken.unwrap_or_else(|| io::Error::other("no free backup name")))
}

/// The app's own backup folder (`<app home>/backups/files`, its owner's
/// only), for files whose folder cannot hold one.
fn private_backups() -> io::Result<PathBuf> {
    let dir = yuzora_core::paths::app_home().join("backups").join("files");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Writes what `from` holds over a file that is there (never creates one)
/// and fsyncs it.
fn overwrite(path: &Path, from: &mut dyn Read) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    io::copy(from, &mut file)?;
    file.sync_all()
}

/// Gives the scratch file the original's owner and group; false when it cannot.
#[cfg(unix)]
fn keep_owner(file: &std::fs::File, original: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::{MetadataExt, fchown};
    let Ok(staged) = file.metadata() else {
        return false;
    };
    (staged.uid(), staged.gid()) == (original.uid(), original.gid())
        || fchown(file, Some(original.uid()), Some(original.gid())).is_ok()
}

/// Gives the scratch file the ACL and extended attributes of `original`
/// (Finder tags, a resource fork, a POSIX ACL): a new inode has none of its
/// own. False when it cannot.
#[cfg(target_os = "macos")]
fn keep_extended(file: &std::fs::File, original: &Path) -> bool {
    use std::os::fd::AsRawFd as _;
    let Ok(from) = open_regular(original) else {
        return false;
    };
    let flags = libc::COPYFILE_ACL | libc::COPYFILE_XATTR;
    // SAFETY: both descriptors are open for the call; no state is passed.
    unsafe {
        libc::fcopyfile(
            from.as_raw_fd(),
            file.as_raw_fd(),
            std::ptr::null_mut(),
            flags,
        ) == 0
    }
}

/// [`keep_extended`] on Linux, where a POSIX ACL is an extended attribute
/// too: the scratch file gets each of `original`'s, and loses any it got on
/// its own (a folder's default ACL).
#[cfg(target_os = "linux")]
fn keep_extended(file: &std::fs::File, original: &Path) -> bool {
    use std::os::fd::AsRawFd as _;
    let Ok(from) = open_regular(original) else {
        return false;
    };
    let (Some(want), Some(have)) = (xattrs(from.as_raw_fd()), xattrs(file.as_raw_fd())) else {
        return false;
    };
    let fd = file.as_raw_fd();
    // SAFETY (both): `fd` is open; names end in NUL, values are as long as said.
    let remove = |name: &std::ffi::CString| unsafe { libc::fremovexattr(fd, name.as_ptr()) } == 0;
    let set = |name: &std::ffi::CString, value: &[u8]| {
        let at = value.as_ptr().cast();
        unsafe { libc::fsetxattr(fd, name.as_ptr(), at, value.len(), 0) == 0 }
    };
    have.iter()
        .filter(|(name, _)| !want.iter().any(|(w, _)| w == name))
        .all(|(name, _)| remove(name))
        && want
            .iter()
            .filter(|a| !have.contains(*a))
            .all(|(name, value)| set(name, value.as_slice()))
}

/// A file's extended attributes, by name; none on a file system without
/// them. `None` when they could not be read.
#[cfg(target_os = "linux")]
fn xattrs(fd: std::os::fd::RawFd) -> Option<Vec<(std::ffi::CString, Vec<u8>)>> {
    use std::ptr::null_mut;
    // SAFETY (each call): `fd` is open, buffers are as long as said.
    let len = unsafe { libc::flistxattr(fd, null_mut(), 0) };
    if len < 0 {
        let unsupported = io::Error::last_os_error().raw_os_error() == Some(libc::EOPNOTSUPP);
        return unsupported.then(Vec::new);
    }
    let mut names = vec![0u8; usize::try_from(len).ok()?];
    let len = unsafe { libc::flistxattr(fd, names.as_mut_ptr().cast(), names.len()) };
    // A change meanwhile (ERANGE) is a failure too.
    names.truncate(usize::try_from(len).ok()?);
    names
        .split(|b| *b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| {
            let name = std::ffi::CString::new(name).ok()?;
            let len = unsafe { libc::fgetxattr(fd, name.as_ptr(), null_mut(), 0) };
            let mut value = vec![0u8; usize::try_from(len).ok()?];
            let len = unsafe {
                libc::fgetxattr(fd, name.as_ptr(), value.as_mut_ptr().cast(), value.len())
            };
            value.truncate(usize::try_from(len).ok()?);
            Some((name, value))
        })
        .collect()
}

/// Elsewhere only the owner and mode are kept.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn keep_extended(_file: &std::fs::File, _original: &Path) -> bool {
    true
}

// ---- create, rename, remove ----

/// A new empty file; an error when the path exists. On another machine it gets what that
/// machine gives a file its own `touch` makes ([`crate::remote_entry`]).
pub fn create_file(path: &str, on: Option<&str>) -> Result<(), String> {
    if let Some(m) = machine(on) {
        return remote_entry::on_machine(Act::Make(path, Kind::File), m);
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(drop)
        .map_err(|e| format!("cannot create {path}: {e}"))
}

/// A new folder; an error when the path exists or its parent does not.
pub fn create_dir(path: &str, on: Option<&str>) -> Result<(), String> {
    if let Some(m) = machine(on) {
        return remote_entry::on_machine(Act::Make(path, Kind::Folder), m);
    }
    std::fs::create_dir(path).map_err(|e| format!("cannot create {path}: {e}"))
}

/// Moves `from` to `to`; an error when `to` exists (a change of case only is
/// fine).
pub fn rename(from: &str, to: &str, on: Option<&str>) -> Result<(), String> {
    if let Some(m) = machine(on) {
        return remote_entry::on_machine(Act::Move(from, to), m);
    }
    let moved = if same_entry_other_case(from, to) {
        std::fs::rename(from, to)
    } else {
        rename_no_replace(Path::new(from), Path::new(to))
    };
    moved.map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => format!("{to} already exists"),
        _ => format!("cannot rename {from}: {e}"),
    })
}

/// A rename that never replaces what is at `to`: in one system call where
/// there is one, so nothing made there meanwhile is lost; elsewhere (or on a
/// file system without it) `to` is looked at first.
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(done) = rename_excl(from, to) {
        return done;
    }
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

/// `renamex_np(RENAME_EXCL)` / `renameat2(RENAME_NOREPLACE)`; `None` when the
/// file system can't.
#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
fn rename_excl(from: &Path, to: &Path) -> Option<io::Result<()>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| CString::new(p.as_os_str().as_bytes());
    let (Ok(a), Ok(b)) = (c(from), c(to)) else {
        return Some(Err(io::ErrorKind::InvalidInput.into()));
    };
    // SAFETY: two NUL-terminated paths that outlive the call.
    #[cfg(target_os = "macos")]
    let done = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    let done = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if done == 0 {
        return Some(Ok(()));
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::EINVAL | libc::ENOTSUP | libc::ENOSYS) => None,
        _ => Some(Err(e)),
    }
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
fn rename_excl(_from: &Path, _to: &Path) -> Option<io::Result<()>> {
    None
}

/// `to` is `from` spelled with other letter case, on a file system that folds case.
fn same_entry_other_case(from: &str, to: &str) -> bool {
    if from == to || from.to_lowercase() != to.to_lowercase() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (
            std::fs::symlink_metadata(from),
            std::fs::symlink_metadata(to),
        ) {
            (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        matches!((std::fs::canonicalize(from), std::fs::canonicalize(to)), (Ok(a), Ok(b)) if a == b)
    }
}

/// Moves a file or folder to the trash: this machine's, or on another machine `gio trash`, a
/// `trash` command or the freedesktop trash in its home ([`crate::remote_entry`]); never deleted
/// when none of them can take it.
pub fn trash(path: &str, on: Option<&str>) -> Result<(), String> {
    if let Some(m) = machine(on) {
        return remote_entry::on_machine(Act::Trash(path), m);
    }
    ::trash::delete(path).map_err(|e| format!("cannot move {path} to the trash: {e}"))
}

// ---- copy ----

/// How many names a copy tries before it gives up.
const COPY_NAMES: u32 = 1000;
/// How many folders inside each other a copy here goes through, the one copied counted. Each holds two
/// descriptors (about 120 deep fill the 256 a process may have by default) and a frame of the walk on
/// the stack (some 600 deep fill the 2 MiB of the thread on a debug build).
#[cfg(unix)]
const COPY_DEPTH: usize = 64;

/// A file's name as stem and extension: the extension starts at the last dot, unless that dot
/// starts the name (`.env` has none).
pub(crate) fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(at) if at > 0 => name.split_at(at),
        _ => (name, ""),
    }
}

/// The `k`th name a copy of `name` may take in a folder: `name` itself, then `<stem> copy<ext>`,
/// `<stem> copy 2<ext>`, and on. A folder (`whole`) keeps its name whole.
pub(crate) fn copy_name(name: &str, whole: bool, k: u32) -> String {
    let (stem, ext) = if whole { (name, "") } else { split_ext(name) };
    match k {
        0 => name.to_owned(),
        1 => format!("{stem} copy{ext}"),
        k => format!("{stem} copy {k}{ext}"),
    }
}

/// Copies `from` (a file, a folder with all it holds, a link as a link) into the folder `into`
/// under the first free name [`copy_name`] gives, and says where. Nothing there is replaced.
/// The copy is made in a scratch folder in `into` and renamed in once whole: a copy that fails
/// leaves nothing, and one that is done shows at once. `held`: names in `into` where a document
/// is still open (its file deleted); taking one is refused, as a rename onto it is.
pub fn copy(from: &str, into: &str, on: Option<&str>, held: &[String]) -> Result<String, String> {
    if let Some(m) = machine(on) {
        return remote_entry::copy_on_machine(from, into, held, m);
    }
    copy_here(from, into, held)
        .map_err(|problem| Act::Copy(from, into, held).describe(&problem, "this machine"))
}

fn copy_here(from: &str, into: &str, held: &[String]) -> Result<String, Problem> {
    let failed = |e: io::Error| Problem::Failed(e.to_string());
    let source = Path::new(from);
    let meta = match std::fs::symlink_metadata(source) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Problem::Gone),
        Err(e) => return Err(failed(e)),
    };
    match std::fs::metadata(into) {
        Ok(folder) if folder.is_dir() => {}
        Ok(_) => return Err(Problem::NoFolder),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Problem::NoFolder),
        Err(e) => return Err(failed(e)),
    }
    let whole = meta.is_dir();
    if whole
        && let (Ok(a), Ok(b)) = (std::fs::canonicalize(source), std::fs::canonicalize(into))
        && b.starts_with(&a)
    {
        return Err(Problem::IntoItself);
    }
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(Problem::BadPath)?;
    #[cfg(unix)]
    {
        let scratch = anchored::Scratch::make(into)?;
        let placed = scratch
            .fill(source, name)
            .and_then(|()| scratch.place(name, into, whole, held));
        scratch.finish(placed.is_err().then_some(name));
        placed
    }
    #[cfg(not(unix))]
    {
        let scratch = scratch_path(Path::new(into));
        private_dir(&scratch).map_err(|e| match e.kind() {
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                Problem::FolderNotWritable
            }
            _ => failed(e),
        })?;
        let made = scratch.join(name);
        let placed = copy_entry(source, &made)
            .map_err(failed)
            .and_then(|()| place(&made, into, name, whole, held));
        if placed.is_ok() {
            let _ = std::fs::remove_dir(&scratch);
        } else {
            unseal(&scratch);
            let _ = std::fs::remove_dir_all(&scratch);
        }
        placed
    }
}

/// The local copy on Unix goes through folders it has opened, not through paths. Someone who may
/// rename in `into` can swap the scratch folder for a link to one of the person's folders once it
/// is made; a path through that link would write in that folder, or rename it out into `into`.
/// Inside the scratch folder (the person's, 0700) nobody else can change anything.
#[cfg(unix)]
mod anchored {
    use super::{COPY_DEPTH, COPY_NAMES, Problem, SCRATCH_PREFIX, copy_name, join, scratch_id};
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::io;
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
    use std::path::{Path, PathBuf};

    pub(super) struct Scratch {
        into: Dir,
        pub(super) name: OsString,
        dir: Dir,
    }

    impl Scratch {
        /// Makes the scratch folder in `into` and opens it. What is opened is used only if it is
        /// an empty folder of the person's: until then someone else may have put another one, or
        /// a link, at its name.
        pub(super) fn make(into: &str) -> Result<Self, Problem> {
            let failed = |e: io::Error| Problem::Failed(e.to_string());
            // A folder that cannot be entered cannot be changed either.
            let refused = |e: io::Error| match e.kind() {
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                    Problem::FolderNotWritable
                }
                _ => failed(e),
            };
            let parent = Dir::open(Path::new(into)).map_err(refused)?;
            let name = OsString::from(format!("{SCRATCH_PREFIX}{}", scratch_id()));
            parent.mkdir(&name, 0o700).map_err(refused)?;
            Self::take(parent, name, into)
        }

        /// Opens what is at `name` in `parent` once the scratch folder was made there, and takes it
        /// for the scratch folder only if it is an empty folder of the person's.
        fn take(parent: Dir, name: OsString, into: &str) -> Result<Self, Problem> {
            let not_made = Problem::Failed(format!(
                "{} is not the folder just made",
                join(into, &name.to_string_lossy())
            ));
            let Ok(dir) = parent.dir(&name) else {
                return Err(not_made);
            };
            let Ok(st) = dir.stat() else {
                return Err(not_made);
            };
            let scratch = Self {
                into: parent,
                name,
                dir,
            };
            // SAFETY: geteuid has no preconditions.
            if st.st_uid != unsafe { libc::geteuid() } {
                return Err(not_made);
            }
            // Empty twice: before its mode is changed, so a folder of the person's put there stays as it
            // was, and once private, as someone else may have added an entry while it was wider.
            let empty = || scratch.dir.names().is_ok_and(|n| n.is_empty());
            let private = || perm(&st) == 0o700 || scratch.dir.chmod(0o700).is_ok();
            if !(empty() && private() && empty()) {
                scratch.finish(None);
                return Err(not_made);
            }
            Ok(scratch)
        }

        pub(super) fn fill(&self, source: &Path, name: &str) -> Result<(), Problem> {
            Looked::at(source)
                .and_then(|looked| self.fill_from(&looked, name))
                .map_err(|e| Problem::Failed(e.to_string()))
        }

        /// Copies what was `looked` at into the scratch folder as `name`.
        pub(super) fn fill_from(&self, looked: &Looked, name: &str) -> io::Result<()> {
            copy_from(
                &looked.dir,
                &looked.name,
                &looked.st,
                &looked.shown,
                &self.dir,
                name.as_ref(),
                0,
            )
        }

        /// Renames the copy to the first free name in `into`; never over what is there.
        pub(super) fn place(
            &self,
            name: &str,
            into: &str,
            whole: bool,
            held: &[String],
        ) -> Result<String, Problem> {
            let mut last = name.to_owned();
            for k in 0..COPY_NAMES {
                last = copy_name(name, whole, k);
                if self.into.stat_at(last.as_ref()).is_ok() {
                    continue;
                }
                if let Some(at) = held.iter().position(|h| *h == last) {
                    return Err(Problem::StillOpen(at));
                }
                match self.dir.rename_to(name.as_ref(), &self.into, last.as_ref()) {
                    Ok(()) => return Ok(join(into, &last)),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(Problem::Failed(e.to_string())),
                }
            }
            Err(Problem::Failed(format!(
                "every name from {name} to {last} is taken in {into}"
            )))
        }

        /// Takes away what a failed copy `left`, then the scratch folder's name while it is still
        /// that folder.
        pub(super) fn finish(self, left: Option<&str>) {
            if let Some(name) = left {
                remove_at(&self.dir, name.as_ref());
            }
            let same = match (self.into.stat_at(&self.name), self.dir.stat()) {
                (Ok(a), Ok(b)) => (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino),
                _ => false,
            };
            if same {
                let _ = self.into.unlink(&self.name, true);
            }
        }
    }

    /// The entry a copy is of, looked at once (a link is not followed). The folder it is in is
    /// opened by its path, links on the way followed as the path says.
    pub(super) struct Looked {
        dir: Dir,
        name: OsString,
        st: libc::stat,
        shown: PathBuf,
    }

    impl Looked {
        pub(super) fn at(path: &Path) -> io::Result<Self> {
            let said = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                return Err(said(io::ErrorKind::InvalidInput.into()));
            };
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            let dir = Dir::open(parent).map_err(said)?;
            let st = dir.stat_at(name).map_err(said)?;
            Ok(Self {
                dir,
                name: name.to_owned(),
                st,
                shown: path.to_owned(),
            })
        }
    }

    /// Copies `name` in `src`, which was `st` when looked at, into `at` as `to`, and all a folder
    /// holds. Someone who may rename where it is can swap it for a link, or for another entry,
    /// after the look: it is opened without following a link and copied only while it is still
    /// what was looked at, and its mode is read from what was opened. So what is read never
    /// lands wider than it is. A new entry gets the mode `cp` gives it: the original's, less the
    /// umask. `above`: how many folders of the copy it is in.
    fn copy_from(
        src: &Dir,
        name: &OsStr,
        st: &libc::stat,
        shown: &Path,
        at: &Dir,
        to: &OsStr,
        above: usize,
    ) -> io::Result<()> {
        let said = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", shown.display()));
        let changed =
            || io::Error::other(format!("{} changed while it was copied", shown.display()));
        // A link or another kind of entry where the look saw this one.
        let opened = |e: io::Error| match e.raw_os_error() {
            Some(libc::ELOOP | libc::ENOTDIR | libc::EINVAL) => changed(),
            _ => said(e),
        };
        let same = |own: &libc::stat| {
            (own.st_dev, own.st_ino, own.st_mode & libc::S_IFMT)
                == (st.st_dev, st.st_ino, st.st_mode & libc::S_IFMT)
        };
        match st.st_mode & libc::S_IFMT {
            libc::S_IFLNK => {
                let target = src.read_link(name).map_err(opened)?;
                at.symlink(&target, to)?;
                #[cfg(target_os = "macos")]
                {
                    let link = src.open_link(name).map_err(opened)?;
                    if !same(&stat_of(link.as_raw_fd())?) {
                        return Err(changed());
                    }
                    let made = at.open_link(to)?;
                    keep(link.as_raw_fd(), made.as_raw_fd(), libc::COPYFILE_XATTR).map_err(said)?;
                }
                Ok(())
            }
            libc::S_IFDIR => {
                if above == COPY_DEPTH {
                    return Err(io::Error::other(format!(
                        "{}: more than {COPY_DEPTH} folders inside each other",
                        shown.display()
                    )));
                }
                let from = src.dir(name).map_err(opened)?;
                let own = from.stat()?;
                if !same(&own) {
                    return Err(changed());
                }
                // The owner may fill it whatever the original's mode; what it lacks is taken away after.
                at.mkdir(to, perm(&own) | 0o700)?;
                let made = at.dir(to)?;
                #[cfg(target_os = "macos")]
                keep(from.fd(), made.fd(), libc::COPYFILE_XATTR).map_err(said)?;
                for n in from.names().map_err(said)? {
                    let inner = shown.join(&n);
                    let st = from.stat_at(&n).map_err(|e| {
                        io::Error::new(e.kind(), format!("{}: {e}", inner.display()))
                    })?;
                    copy_from(&from, &n, &st, &inner, &made, &n, above + 1)?;
                }
                // The sticky bit is kept, as `cp -RP` keeps it; a setgid the new folder got from
                // the folder it was made in stays.
                let mode = bits(&made.stat()?);
                let want = (mode & !(0o700 & !perm(&own))) | (bits(&own) & 0o1000);
                if want != mode {
                    made.chmod(want)?;
                }
                Ok(())
            }
            libc::S_IFREG => {
                let source = src.open_file(name).map_err(opened)?;
                let own = stat_of(source.as_raw_fd())?;
                if !same(&own) {
                    return Err(changed());
                }
                let copy = at.create(to, perm(&own))?;
                fill(&source, &copy).map_err(said)
            }
            _ => Err(io::Error::other(format!(
                "{} is not a file, a folder or a link",
                shown.display()
            ))),
        }
    }

    /// Takes away `name` in `at`, all a folder holds first. A folder copied without write
    /// permission is given it back; links are not followed.
    fn remove_at(at: &Dir, name: &OsStr) {
        let Ok(st) = at.stat_at(name) else {
            return;
        };
        let folder = (st.st_mode & libc::S_IFMT) == libc::S_IFDIR;
        if folder {
            let _ = at.chmod_at(name, perm(&st) | 0o700);
            if let Ok(inner) = at.dir(name)
                && let Ok(names) = inner.names()
            {
                for n in names {
                    remove_at(&inner, &n);
                }
            }
        }
        let _ = at.unlink(name, folder);
    }

    /// Puts what `source` holds in `copy`: on macOS with its extended attributes, through `fcopyfile` as a
    /// save keeps them ([`super::keep_extended`]), and with the holes of a sparse file kept as holes.
    #[cfg(target_os = "macos")]
    fn fill(source: &std::fs::File, copy: &std::fs::File) -> io::Result<()> {
        let flags = libc::COPYFILE_DATA | libc::COPYFILE_DATA_SPARSE | libc::COPYFILE_XATTR;
        keep(source.as_raw_fd(), copy.as_raw_fd(), flags)
    }

    #[cfg(not(target_os = "macos"))]
    fn fill(mut source: &std::fs::File, mut copy: &std::fs::File) -> io::Result<()> {
        io::copy(&mut source, &mut copy).map(drop)
    }

    /// What `flags` name, from one handle onto the other. `COPYFILE_XATTR` is what `cp -RP` keeps:
    /// extended attributes (Finder tags, a resource fork, the quarantine flag), not the ACL.
    #[cfg(target_os = "macos")]
    fn keep(from: libc::c_int, to: libc::c_int, flags: libc::copyfile_flags_t) -> io::Result<()> {
        // SAFETY: both descriptors are open for the call; no state is passed.
        sys(unsafe { libc::fcopyfile(from, to, std::ptr::null_mut(), flags) }).map(drop)
    }

    #[allow(clippy::unnecessary_cast)]
    fn perm(st: &libc::stat) -> u32 {
        st.st_mode as u32 & 0o777
    }

    /// [`perm`] with setuid, setgid and the sticky bit.
    #[allow(clippy::unnecessary_cast)]
    fn bits(st: &libc::stat) -> u32 {
        st.st_mode as u32 & 0o7777
    }

    fn c_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| io::ErrorKind::InvalidInput.into())
    }

    fn stat_of(fd: libc::c_int) -> io::Result<libc::stat> {
        let mut st = std::mem::MaybeUninit::uninit();
        // SAFETY: room for one stat, filled when the call succeeds.
        sys(unsafe { libc::fstat(fd, st.as_mut_ptr()) })?;
        // SAFETY: the call succeeded.
        Ok(unsafe { st.assume_init() })
    }

    fn sys(r: libc::c_int) -> io::Result<libc::c_int> {
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r)
        }
    }

    /// A folder opened once: what is done through it stays in that folder, whatever is renamed
    /// or put at its path meanwhile.
    struct Dir(OwnedFd);

    impl Dir {
        const FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        /// Search access only: names are looked up, made, renamed and removed through it, all the
        /// `*at` calls allow, but it cannot be listed.
        #[cfg(target_os = "macos")]
        const SEARCH: libc::c_int = libc::O_SEARCH | libc::O_CLOEXEC;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const SEARCH: libc::c_int = libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC;
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
        const SEARCH: libc::c_int = Self::FLAGS;

        /// The folder at `path`, a link to one followed, to work in by name: one the person may enter
        /// but not read (another account's Drop Box, 0333) is opened too.
        fn open(path: &Path) -> io::Result<Self> {
            let path = c_name(path.as_os_str())?;
            // SAFETY: a NUL-terminated path that outlives the call.
            let open = |flags| Self::own(unsafe { libc::open(path.as_ptr(), flags) });
            match open(Self::SEARCH) {
                // A system without search-only opens may refuse the flag: read access, as before.
                Err(e) if e.raw_os_error() == Some(libc::EINVAL) => open(Self::FLAGS),
                opened => opened,
            }
        }

        /// The folder `name` in this one; a link is refused.
        fn dir(&self, name: &OsStr) -> io::Result<Self> {
            let name = c_name(name)?;
            // SAFETY: a NUL-terminated name and a descriptor this owns.
            Self::own(unsafe {
                libc::openat(self.fd(), name.as_ptr(), Self::FLAGS | libc::O_NOFOLLOW)
            })
        }

        fn own(fd: libc::c_int) -> io::Result<Self> {
            let fd = sys(fd)?;
            // SAFETY: a descriptor just opened, owned by nothing else.
            Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
        }

        fn fd(&self) -> libc::c_int {
            self.0.as_raw_fd()
        }

        fn stat(&self) -> io::Result<libc::stat> {
            stat_of(self.fd())
        }

        /// What is at `name` here; a link is not followed.
        fn stat_at(&self, name: &OsStr) -> io::Result<libc::stat> {
            let name = c_name(name)?;
            let mut st = std::mem::MaybeUninit::uninit();
            // SAFETY: as in `stat`, with a NUL-terminated name.
            sys(unsafe {
                libc::fstatat(
                    self.fd(),
                    name.as_ptr(),
                    st.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            })?;
            // SAFETY: the call succeeded.
            Ok(unsafe { st.assume_init() })
        }

        fn mkdir(&self, name: &OsStr, mode: u32) -> io::Result<()> {
            let name = c_name(name)?;
            // SAFETY: a NUL-terminated name and a descriptor this owns.
            sys(unsafe { libc::mkdirat(self.fd(), name.as_ptr(), mode as libc::mode_t) }).map(drop)
        }

        /// A new file `name` here: never one that is there, nor through a link.
        fn create(&self, name: &OsStr, mode: u32) -> io::Result<std::fs::File> {
            let name = c_name(name)?;
            let flags =
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            // SAFETY: a NUL-terminated name and a descriptor this owns.
            let fd = sys(unsafe {
                libc::openat(self.fd(), name.as_ptr(), flags, mode as libc::c_uint)
            })?;
            // SAFETY: a descriptor just opened, owned by nothing else.
            Ok(unsafe { std::fs::File::from_raw_fd(fd) })
        }

        /// The file `name` here, to read: a link is refused, and a FIFO put there is not waited on.
        fn open_file(&self, name: &OsStr) -> io::Result<std::fs::File> {
            let name = c_name(name)?;
            let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
            // SAFETY: a NUL-terminated name and a descriptor this owns.
            let fd = sys(unsafe { libc::openat(self.fd(), name.as_ptr(), flags) })?;
            // SAFETY: a descriptor just opened, owned by nothing else.
            Ok(unsafe { std::fs::File::from_raw_fd(fd) })
        }

        /// The link `name` here itself, for its attributes; anything else there is opened as it is, and a
        /// FIFO is not waited on.
        #[cfg(target_os = "macos")]
        fn open_link(&self, name: &OsStr) -> io::Result<std::fs::File> {
            let name = c_name(name)?;
            let flags = libc::O_RDONLY | libc::O_SYMLINK | libc::O_NONBLOCK | libc::O_CLOEXEC;
            // SAFETY: a NUL-terminated name and a descriptor this owns.
            let fd = sys(unsafe { libc::openat(self.fd(), name.as_ptr(), flags) })?;
            // SAFETY: a descriptor just opened, owned by nothing else.
            Ok(unsafe { std::fs::File::from_raw_fd(fd) })
        }

        /// What the link `name` here points to.
        fn read_link(&self, name: &OsStr) -> io::Result<PathBuf> {
            let name = c_name(name)?;
            let mut buf = vec![0u8; libc::PATH_MAX as usize + 1];
            // SAFETY: a NUL-terminated name, a descriptor this owns, and a buffer of that length.
            let n = unsafe {
                libc::readlinkat(self.fd(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
            };
            let n = usize::try_from(n).map_err(|_| io::Error::last_os_error())?;
            if n >= buf.len() {
                return Err(io::Error::other("the link points to a path too long"));
            }
            buf.truncate(n);
            Ok(PathBuf::from(OsString::from_vec(buf)))
        }

        fn symlink(&self, target: &Path, name: &OsStr) -> io::Result<()> {
            let (target, name) = (c_name(target.as_os_str())?, c_name(name)?);
            // SAFETY: NUL-terminated strings and a descriptor this owns.
            sys(unsafe { libc::symlinkat(target.as_ptr(), self.fd(), name.as_ptr()) }).map(drop)
        }

        fn chmod(&self, mode: u32) -> io::Result<()> {
            // SAFETY: a descriptor this owns.
            sys(unsafe { libc::fchmod(self.fd(), mode as libc::mode_t) }).map(drop)
        }

        /// Follows a link: only for a folder in the scratch folder, which nobody else can change.
        fn chmod_at(&self, name: &OsStr, mode: u32) -> io::Result<()> {
            let name = c_name(name)?;
            // SAFETY: a NUL-terminated name and a descriptor this owns.
            sys(unsafe { libc::fchmodat(self.fd(), name.as_ptr(), mode as libc::mode_t, 0) })
                .map(drop)
        }

        fn unlink(&self, name: &OsStr, folder: bool) -> io::Result<()> {
            let name = c_name(name)?;
            let flags = if folder { libc::AT_REMOVEDIR } else { 0 };
            // SAFETY: a NUL-terminated name and a descriptor this owns.
            sys(unsafe { libc::unlinkat(self.fd(), name.as_ptr(), flags) }).map(drop)
        }

        /// The names in this folder.
        fn names(&self) -> io::Result<Vec<OsString>> {
            // fdopendir keeps the descriptor it is given: a copy of this one.
            // SAFETY: a descriptor this owns.
            let fd = sys(unsafe { libc::fcntl(self.fd(), libc::F_DUPFD_CLOEXEC, 0) })?;
            // SAFETY: a descriptor just made, handed over.
            let listing = unsafe { libc::fdopendir(fd) };
            if listing.is_null() {
                let e = io::Error::last_os_error();
                // SAFETY: fdopendir did not take it.
                unsafe { libc::close(fd) };
                return Err(e);
            }
            let mut names = Vec::new();
            // SAFETY: `listing` is open until closedir; each entry is read before the next call.
            unsafe {
                libc::rewinddir(listing);
                loop {
                    let entry = libc::readdir(listing);
                    if entry.is_null() {
                        break;
                    }
                    let name = CStr::from_ptr((*entry).d_name.as_ptr()).to_bytes();
                    if name != b"." && name != b".." {
                        names.push(OsStr::from_bytes(name).to_owned());
                        #[cfg(test)]
                        unread(listing);
                    }
                }
                libc::closedir(listing);
            }
            Ok(names)
        }

        /// Renames `name` here to `to` in `into`; never over what is there.
        fn rename_to(&self, name: &OsStr, into: &Dir, to: &OsStr) -> io::Result<()> {
            let (a, b) = (c_name(name)?, c_name(to)?);
            #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
            {
                // SAFETY: NUL-terminated names and descriptors these own.
                #[cfg(target_os = "macos")]
                let done = unsafe {
                    libc::renameatx_np(
                        self.fd(),
                        a.as_ptr(),
                        into.fd(),
                        b.as_ptr(),
                        libc::RENAME_EXCL,
                    )
                };
                // SAFETY: as above.
                #[cfg(target_os = "linux")]
                let done = unsafe {
                    libc::renameat2(
                        self.fd(),
                        a.as_ptr(),
                        into.fd(),
                        b.as_ptr(),
                        libc::RENAME_NOREPLACE,
                    )
                };
                if done == 0 {
                    return Ok(());
                }
                let e = io::Error::last_os_error();
                if !matches!(
                    e.raw_os_error(),
                    Some(libc::EINVAL | libc::ENOTSUP | libc::ENOSYS)
                ) {
                    return Err(e);
                }
            }
            // Where the system cannot refuse to replace, what is there is looked at first.
            if into.stat_at(to).is_ok() {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            // SAFETY: NUL-terminated names and descriptors these own.
            sys(unsafe { libc::renameat(self.fd(), a.as_ptr(), into.fd(), b.as_ptr()) }).map(drop)
        }
    }

    #[cfg(test)]
    #[test]
    fn a_rename_onto_a_taken_name_is_refused_and_both_stay() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), b"a").unwrap();
        std::fs::write(d.path().join("b"), b"b").unwrap();
        let here = Dir::open(d.path()).unwrap();
        let e = here
            .rename_to("a".as_ref(), &here, "b".as_ref())
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(d.path().join("a")).unwrap(), b"a");
        assert_eq!(std::fs::read(d.path().join("b")).unwrap(), b"b");
    }

    /// A folder of the person's found at the scratch name once it was made (someone renamed it there)
    /// is not taken, and its mode and what it holds stay as they were.
    #[cfg(test)]
    #[test]
    fn a_folder_of_the_persons_at_the_scratch_name_is_not_taken_and_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let name = format!("{SCRATCH_PREFIX}{}", scratch_id());
        let theirs = d.path().join(&name);
        std::fs::create_dir(&theirs).unwrap();
        std::fs::write(theirs.join("keep.txt"), b"mine").unwrap();
        std::fs::set_permissions(&theirs, std::fs::Permissions::from_mode(0o755)).unwrap();
        let into = d.path().to_string_lossy();
        let taken = Scratch::take(Dir::open(d.path()).unwrap(), name.into(), &into);
        assert!(
            matches!(&taken, Err(Problem::Failed(m)) if m.ends_with("is not the folder just made"))
        );
        let mode = std::fs::metadata(&theirs).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o755);
        assert_eq!(std::fs::read(theirs.join("keep.txt")).unwrap(), b"mine");
    }

    #[cfg(test)]
    thread_local! {
        /// A folder opened to search only: the listing of it that a test cuts short.
        static UNREAD: std::cell::RefCell<Option<Dir>> = const { std::cell::RefCell::new(None) };
    }

    /// Once `listing` has read a name, if it is of [`UNREAD`]'s folder, puts that descriptor in
    /// place of its own and drops what it read ahead: its next `readdir` asks the system, on a
    /// descriptor that may not read, and fails.
    #[cfg(test)]
    fn unread(listing: *mut libc::DIR) {
        // SAFETY: `listing` is open.
        let fd = unsafe { libc::dirfd(listing) };
        let same = |dir: &mut Dir| match (dir.stat(), stat_of(fd)) {
            (Ok(a), Ok(b)) => (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino),
            _ => false,
        };
        if let Some(dir) = UNREAD.with_borrow_mut(|unread| unread.take_if(same)) {
            // SAFETY: both descriptors and `listing` are open; `fd` stays the listing's.
            unsafe {
                assert_eq!(libc::dup2(dir.fd(), fd), fd);
                libc::rewinddir(listing);
            }
        }
    }

    /// A folder whose listing fails part way (an I/O error on a network mount) is not copied as if
    /// it held only what was read: the copy fails, names the folder and the failure, and leaves
    /// nothing. Here the listing of `box/inner` fails once it has read one of its two names.
    #[cfg(test)]
    #[test]
    fn a_folder_whose_listing_fails_part_way_is_not_copied_and_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (from, into) = (d.path().join("box"), d.path().join("into"));
        let inner = from.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir(&into).unwrap();
        std::fs::write(from.join("note"), b"n").unwrap();
        std::fs::write(inner.join("kept"), b"kept").unwrap();
        std::fs::write(inner.join("lost"), b"lost").unwrap();
        UNREAD.set(Some(Dir::open(&inner).unwrap()));
        let copied = super::copy(&from.to_string_lossy(), &into.to_string_lossy(), None, &[]);
        let e = copied.unwrap_err();
        let failure = io::Error::from_raw_os_error(libc::EBADF).to_string();
        assert!(
            e.contains(&*inner.to_string_lossy()) && e.contains(&failure),
            "{e}"
        );
        assert_eq!(std::fs::read_dir(&into).unwrap().count(), 0);
    }
}

/// Lets the owner remove what a failed copy made: a read-only file keeps what it holds until
/// then. Links are not followed.
#[cfg(not(unix))]
fn unseal(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    if meta.permissions().readonly() {
        let mut writable = meta.permissions();
        writable.set_readonly(false);
        let _ = std::fs::set_permissions(path, writable);
    }
    if meta.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        for entry in entries.flatten() {
            unseal(&entry.path());
        }
    }
}

#[cfg(not(unix))]
fn private_dir(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().create(path)
}

/// Copies one entry, and all a folder holds.
#[cfg(not(unix))]
fn copy_entry(from: &Path, to: &Path) -> io::Result<()> {
    let said = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", from.display()));
    let meta = std::fs::symlink_metadata(from).map_err(said)?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        return link_like(from, to).map_err(said);
    }
    if kind.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from).map_err(said)? {
            let entry = entry.map_err(said)?;
            copy_entry(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if !kind.is_file() {
        return Err(io::Error::other(format!(
            "{} is not a file, a folder or a link",
            from.display()
        )));
    }
    let mut source = open_regular(from).map_err(said)?;
    let mut copy = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    io::copy(&mut source, &mut copy).map_err(said)?;
    if meta.permissions().readonly() {
        std::fs::set_permissions(to, meta.permissions())?;
    }
    Ok(())
}

/// A link to a folder is made as one; a link to nothing, as a link to a file.
#[cfg(windows)]
fn link_like(from: &Path, to: &Path) -> io::Result<()> {
    let target = std::fs::read_link(from)?;
    if std::fs::metadata(from).is_ok_and(|m| m.is_dir()) {
        std::os::windows::fs::symlink_dir(target, to)
    } else {
        std::os::windows::fs::symlink_file(target, to)
    }
}

/// Renames the copy made at `made` to the first free name in `into`; never over what is there.
#[cfg(not(unix))]
fn place(
    made: &Path,
    into: &str,
    name: &str,
    whole: bool,
    held: &[String],
) -> Result<String, Problem> {
    let mut last = name.to_owned();
    for k in 0..COPY_NAMES {
        last = copy_name(name, whole, k);
        let to = join(into, &last);
        if std::fs::symlink_metadata(&to).is_ok() {
            continue;
        }
        if let Some(at) = held.iter().position(|h| *h == last) {
            return Err(Problem::StillOpen(at));
        }
        match rename_no_replace(made, Path::new(&to)) {
            Ok(()) => return Ok(to),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Problem::Failed(e.to_string())),
        }
    }
    Err(Problem::Failed(format!(
        "every name from {name} to {last} is taken in {into}"
    )))
}

// ---- reveal ----

/// Shows the item in the system file manager. Does not wait for it.
pub fn reveal(path: &str) -> Result<(), String> {
    reveal_impl(path).map_err(|e| format!("cannot reveal {path}: {e}"))
}

fn spawn_quiet(cmd: &mut Command) -> io::Result<Child> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Reaps the child on a thread so it does not linger as a zombie.
fn detach(mut child: Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

#[cfg(target_os = "macos")]
fn reveal_impl(path: &str) -> io::Result<()> {
    spawn_quiet(Command::new("open").arg("-R").arg(path)).map(detach)
}

#[cfg(windows)]
fn reveal_impl(path: &str) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    spawn_quiet(
        Command::new("explorer").raw_arg(yuzora_core::paths::explorer_select_arg(path)),
    )
    .map(detach)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_impl(path: &str) -> io::Result<()> {
    let parent = Path::new(path)
        .parent()
        .map_or_else(|| ".".to_owned(), |p| p.to_string_lossy().into_owned());
    let open_parent = move || spawn_quiet(Command::new("xdg-open").arg(&parent)).map(detach);
    let uri = format!("file://{}", percent_encode(path));
    let shown = spawn_quiet(
        Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{uri}"))
            .arg("string:"),
    );
    match shown {
        // No file manager answers on D-Bus: open the folder instead.
        Ok(mut child) => {
            std::thread::spawn(move || {
                if !child.wait().is_ok_and(|s| s.success()) {
                    let _ = open_parent();
                }
            });
            Ok(())
        }
        Err(_) => open_parent(),
    }
}

/// Percent-encodes a path for a `file://` URI (`/` and unreserved characters stay).
#[cfg(all(unix, not(target_os = "macos")))]
fn percent_encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---- names ----

/// `dir` and `name` with exactly one separator between them.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else if dir.ends_with('/') || dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// A single path segment a user may type: not empty, not `.` or `..`, no
/// separator or NUL.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_it_runs_runs_offline() {
        assert_eq!(
            OFFLINE,
            [("GIT_NO_LAZY_FETCH", "1"), ("GIT_ALLOW_PROTOCOL", "")]
        );
        let req = offline(ExecRequest::new("git", ["status"]).env("PATH", "/bin"));
        let set = |k: &str, v: &str| req.env.contains(&(k.to_owned(), v.to_owned()));
        assert!(set("PATH", "/bin") && OFFLINE.iter().all(|(k, v)| set(k, v)));
        // [`run`] is the only way out: no other `remote::run` (nor one
        // imported by name) and no git started by hand outside the tests.
        fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs")
                    && path.file_name().is_some_and(|n| n != "tests.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let code = text.split("#[cfg(test)]\nmod tests {").next().unwrap();
                    out.push((path.display().to_string(), code.to_owned()));
                }
            }
        }
        let mut files = Vec::new();
        sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(files.len() > 5, "no sources found");
        let by_name = regex::Regex::new(r"remote::\{[^}]*\brun\b").unwrap();
        let by_hand = regex::Regex::new(r#"Command::new\([^)]*git"#).unwrap();
        let mut runs = Vec::new();
        for (path, code) in &files {
            runs.extend(code.matches("remote::run").map(|_| path.as_str()));
            assert!(!by_name.is_match(code), "{path} imports remote::run");
            assert!(!by_hand.is_match(code), "{path} starts git by hand");
        }
        assert!(
            matches!(runs[..], [one] if one.ends_with("fsops.rs")),
            "{runs:?}"
        );
    }

    /// The folder listing, the tree's git status, the search and the trust
    /// scan all start their programs through [`run`]: that start opens no
    /// console window on Windows. The program here is this test binary
    /// asking whether it has one; the control starts it by hand and must see ours.
    #[cfg(windows)]
    #[test]
    fn run_starts_its_program_without_a_console_window() {
        unsafe extern "system" {
            fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
            fn AllocConsole() -> i32;
        }
        // The runner may have no console (CI): give it one, to hand on.
        let attached = || {
            let mut list = [0u32; 4];
            // SAFETY: a buffer of the length it is given.
            unsafe { GetConsoleProcessList(list.as_mut_ptr(), 4) != 0 }
        };
        if !attached() {
            // SAFETY: no arguments.
            unsafe { AllocConsole() };
        }
        assert!(attached(), "this process has no console to hand on");
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        let args = ["--ignored", "--exact", "fsops::tests::console_probe", "--nocapture", "--test-threads=1"];
        let by_hand = std::process::Command::new(&exe).args(args).output().unwrap();
        let shared = |out: &str| -> Option<u32> { out.split("CONSOLE_SHARED=").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok() };
        assert!(shared(&String::from_utf8_lossy(&by_hand.stdout)).is_some_and(|n| n >= 2), "control: {by_hand:?}");
        let out = run(ExecRequest::new(exe, args)).unwrap();
        // A console of its own with only itself on it: not shared, not none.
        assert_eq!(shared(&out.stdout_text()), Some(1), "{}", out.stdout_text());
    }

    /// The child of the test above.
    #[test]
    #[ignore = "a child process of run_starts_its_program_without_a_console_window"]
    fn console_probe() {
        #[cfg(windows)]
        {
            unsafe extern "system" {
                fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
            }
            let mut list = [0u32; 4];
            // SAFETY: a buffer of the length it is given.
            // CREATE_NO_WINDOW gives the child a console of its own, with only itself on it.
            let shared = unsafe { GetConsoleProcessList(list.as_mut_ptr(), 4) };
            println!("CONSOLE_SHARED={shared}");
        }
    }

    /// A git before 2.44 ignores `GIT_NO_LAZY_FETCH`: a partial clone whose
    /// promisor is a local repo fetches what it lacks over the file
    /// transport, which runs the repo's own `remote.<name>.uploadpack`.
    /// [`OFFLINE`] lets no transport through, so that git fetches nothing.
    #[cfg(unix)]
    #[test]
    fn offline_stops_an_older_git_fetching_from_a_local_promisor() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let tmp = tempfile::tempdir().unwrap();
        let at = |p: &str| tmp.path().join(p);
        let git = |dir: &Path, args: &[&str]| {
            let out = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(["-c", "commit.gpgSign=false"])
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success(), "git {args:?}: {err}");
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        std::fs::create_dir(at("src")).unwrap();
        git(&at("src"), &["init", "-q"]);
        std::fs::write(at("src/a.txt"), "1\n").unwrap();
        git(&at("src"), &["add", "a.txt"]);
        git(&at("src"), &["commit", "-q", "-m", "one"]);
        let src = at("src").display().to_string();
        git(tmp.path(), &["clone", "-q", "--no-hardlinks", &src, "c"]);
        let c = at("c");
        // The clone lacks HEAD's tree, as a `--filter=tree:0` one does, and
        // its uploadpack leaves a mark.
        let tree = git(&c, &["rev-parse", "HEAD^{tree}"]);
        std::fs::remove_file(c.join(format!(".git/objects/{}/{}", &tree[..2], &tree[2..])))
            .unwrap();
        let (mark, hit) = (at("ran"), at("hit.sh"));
        let body = format!("#!/bin/sh\ntouch '{}'\n", mark.display());
        std::fs::write(&hit, body + "exec git-upload-pack \"$@\"\n").unwrap();
        std::fs::set_permissions(&hit, std::fs::Permissions::from_mode(0o755)).unwrap();
        let hit = hit.display().to_string();
        for (k, v) in [
            ("remote.origin.promisor", "true"),
            ("extensions.partialClone", "origin"),
            ("remote.origin.uploadpack", hit.as_str()),
        ] {
            git(&c, &["config", k, v]);
        }
        // A git that drops GIT_NO_LAZY_FETCH, and so do the ones it starts.
        let real = git(tmp.path(), &["--exec-path"]);
        let old = at("old-git");
        std::fs::create_dir(&old).unwrap();
        for e in std::fs::read_dir(&real).unwrap().flatten() {
            if e.file_name() != "git" {
                std::os::unix::fs::symlink(e.path(), old.join(e.file_name())).unwrap();
            }
        }
        let wrapper = format!("#!/bin/sh\nunset GIT_NO_LAZY_FETCH\nexec '{real}/git' \"$@\"\n");
        std::fs::write(old.join("git"), wrapper).unwrap();
        std::fs::set_permissions(old.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            old.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let status = |env: &[(&str, &str)]| {
            let _ = std::fs::remove_file(&mark);
            let _ = Command::new(old.join("git"))
                .args(["status", "--porcelain"])
                .current_dir(&c)
                .env("GIT_EXEC_PATH", &old)
                .env("PATH", &path)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .envs(env.iter().copied())
                .output()
                .unwrap();
            mark.exists()
        };
        assert!(
            !status(&OFFLINE),
            "a git before 2.44 fetched from the local promisor, running its uploadpack"
        );
        assert!(
            status(&[]),
            "without OFFLINE it didn't fetch either: this proves nothing"
        );
    }

    fn p(dir: &tempfile::TempDir, rel: &str) -> String {
        dir.path().join(rel).to_string_lossy().into_owned()
    }

    fn put(dir: &tempfile::TempDir, rel: &str, bytes: &[u8]) -> String {
        let path = p(dir, rel);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for dir in ["b", "A", ".git", ".cfg"] {
            std::fs::create_dir(d.path().join(dir)).unwrap();
        }
        for (f, body) in [
            ("file10.txt", "0123456789"),
            ("file2.txt", "ab"),
            ("File1.txt", ""),
            (".hidden", "x"),
            (".DS_Store", "junk"),
            (
                ".yuzora-save-0123456789abcdef0123456789abcdef",
                "a save in flight",
            ),
        ] {
            put(&d, f, body.as_bytes());
        }
        d
    }

    /// The listing script reads names one by one and stops at `max + 1`: with
    /// a `find` that never ends it still finishes (a glob would have to
    /// expand every name first).
    /// A name with a newline in it cannot go through the listing script's
    /// stream: it is marked, and the listing says something was left out.
    #[cfg(unix)]
    #[test]
    fn a_name_with_a_newline_is_reported_as_left_out() {
        let dir = tempfile::tempdir().unwrap();
        put(&dir, "plain.txt", b"x");
        put(&dir, "two\nlines.txt", b"x");
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", LIST_SCRIPT, "sh"])
            .arg(dir.path())
            .arg("10")
            .output()
            .unwrap();
        assert!(out.status.success());
        let listing = parse_listing(&out.stdout, 10);
        assert_eq!(names(&listing.entries), ["plain.txt"]);
        assert!(listing.skipped);
        assert!(!listing.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn list_script_without_find_is_an_error_not_an_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", LIST_SCRIPT, "sh"])
            .arg(dir.path())
            .arg("10")
            .env("PATH", empty.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn list_script_stops_reading_at_the_limit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        put(&dir, "a", b"x");
        let bin = tempfile::tempdir().unwrap();
        let fake = bin.path().join("find");
        std::fs::write(&fake, "#!/bin/sh\nexec yes ./a\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", LIST_SCRIPT, "sh"])
            .arg(dir.path())
            .arg("3")
            .env("PATH", format!("{}:/usr/bin:/bin", bin.path().display()))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let t = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if t.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                panic!("never stopped reading");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut out = Vec::new();
        std::io::Read::read_to_end(child.stdout.as_mut().unwrap(), &mut out).unwrap();
        let listing = parse_listing(&out, 2);
        assert!(listing.truncated);
        assert_eq!(listing.entries.len(), 2);
    }

    #[test]
    fn list_dir_sorts_folders_first_naturally_and_skips_noise() {
        let d = fixture();
        let Listing {
            entries, truncated, ..
        } = list_dir(&p(&d, ""), None, 100).unwrap();
        assert!(!truncated);
        assert_eq!(
            names(&entries),
            [
                ".cfg",
                "A",
                "b",
                ".hidden",
                "File1.txt",
                "file2.txt",
                "file10.txt"
            ]
        );
        let get = |n: &str| entries.iter().find(|e| e.name == n).unwrap();
        assert!(get(".cfg").dir && get(".cfg").hidden && !get("A").hidden);
        assert!(get(".hidden").hidden && !get("File1.txt").hidden);
        assert_eq!(
            (get("file10.txt").size, get("file2.txt").size, get("A").size),
            (10, 2, 0)
        );
        assert!(!get("file2.txt").dir && !get("file2.txt").link);
    }

    #[test]
    fn a_big_folder_is_cut_as_it_is_read() {
        let d = tempfile::tempdir().unwrap();
        // Names that don't come in sorted order, more than twice the limit.
        for i in (0..60).rev() {
            put(&d, &format!("f{}.txt", (i * 37) % 60), b"");
        }
        std::fs::create_dir(d.path().join("zdir")).unwrap();
        let Listing {
            entries, truncated, ..
        } = list_dir(&p(&d, ""), None, 4).unwrap();
        assert!(truncated);
        assert_eq!(names(&entries), ["zdir", "f0.txt", "f1.txt", "f2.txt"]);
    }

    #[test]
    fn list_dir_limit_cuts_the_sorted_listing() {
        let d = fixture();
        let Listing {
            entries, truncated, ..
        } = list_dir(&p(&d, ""), None, 3).unwrap();
        assert!(truncated);
        assert_eq!(names(&entries), [".cfg", "A", "b"]);
        let Listing {
            entries: all,
            truncated,
            ..
        } = list_dir(&p(&d, ""), None, 7).unwrap();
        assert_eq!((all.len(), truncated), (7, false));
        assert_eq!(
            list_dir(&p(&d, ""), None, 0).unwrap(),
            Listing {
                truncated: true,
                ..Listing::default()
            }
        );
        assert!(list_dir(&p(&d, "nope"), None, 10).is_err());
        assert!(list_dir(&p(&d, "File1.txt"), None, 10).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn list_dir_marks_symlinks() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("real")).unwrap();
        put(&d, "f.txt", b"x");
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("to-dir")).unwrap();
        std::os::unix::fs::symlink(d.path().join("f.txt"), d.path().join("to-file")).unwrap();
        std::os::unix::fs::symlink(d.path().join("gone"), d.path().join("dangling")).unwrap();
        let Listing { entries, .. } = list_dir(&p(&d, ""), None, 10).unwrap();
        let get = |n: &str| entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!((get("to-dir").dir, get("to-dir").link), (true, true));
        assert_eq!(
            (get("to-file").dir, get("to-file").link, get("to-file").size),
            (false, true, 0)
        );
        assert_eq!((get("dangling").dir, get("dangling").link), (false, true));
        assert_eq!((get("real").dir, get("real").link), (true, false));
        // The remote script says the same.
        let Listing {
            entries: remote, ..
        } = list_sh(&p(&d, ""), None, 10).unwrap();
        assert_eq!(remote, entries);
    }

    #[cfg(unix)]
    #[test]
    fn the_remote_listing_script_agrees_with_the_local_one() {
        let d = fixture();
        std::fs::write(d.path().join("with space.txt"), "abc").unwrap();
        std::fs::write(d.path().join("-dash"), "abcd").unwrap();
        let Listing { entries: local, .. } = list_dir(&p(&d, ""), None, 100).unwrap();
        let Listing {
            entries: remote,
            truncated,
            ..
        } = list_sh(&p(&d, ""), None, 100).unwrap();
        assert!(!truncated);
        assert_eq!(remote, local);
        assert!(names(&remote).contains(&".hidden") && names(&remote).contains(&"with space.txt"));
        let Listing {
            entries: cut,
            truncated,
            ..
        } = list_sh(&p(&d, ""), None, 4).unwrap();
        assert_eq!((cut.len(), truncated), (4, true));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            list_sh(&p(&empty, ""), None, 4).unwrap(),
            Listing::default()
        );
        assert!(
            list_sh(&p(&d, "nope"), None, 4)
                .unwrap_err()
                .contains("cannot open")
        );
    }

    #[test]
    fn a_name_that_is_not_utf8_is_left_out_of_a_listing_and_said_to_be() {
        // As the listing script prints it: `b\xff.txt` (3 bytes) before `a.txt` (5).
        let listing = parse_listing(b"f\0b\xff.txt\0f\0a.txt\0d\0dir\0-\x003\n5\n", 10);
        assert!(listing.skipped && !listing.truncated);
        assert_eq!(names(&listing.entries), ["dir", "a.txt"]);
        assert_eq!(listing.entries[1].size, 5);
        // One left out still counts toward the cut.
        let cut = parse_listing(b"f\0b\xff\0f\0a\0-\x001\n2\n", 1);
        assert_eq!(
            (names(&cut.entries), cut.truncated, cut.skipped),
            (vec!["a"], true, true)
        );
        // Here, on a file system that takes such a name (not APFS).
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            let d = tempfile::tempdir().unwrap();
            let odd = d.path().join(std::ffi::OsStr::from_bytes(b"b\xff.txt"));
            if std::fs::write(odd, "x").is_err() {
                eprintln!("skipped: this file system takes only UTF-8 names");
                return;
            }
            put(&d, "a.txt", b"x");
            let local = list_dir(&p(&d, ""), None, 10).unwrap();
            assert_eq!(
                (names(&local.entries), local.skipped),
                (vec!["a.txt"], true)
            );
            assert_eq!(list_sh(&p(&d, ""), None, 10).unwrap(), local);
        }
    }

    /// A folder entry as a test makes it: a name, and its kind or why that
    /// can't be read.
    struct Fake(OsString, Result<(bool, bool), io::ErrorKind>);

    impl Listed for Fake {
        fn name(&self) -> OsString {
            self.0.clone()
        }

        fn kind(&self) -> io::Result<(bool, bool)> {
            self.1.map_err(io::Error::from)
        }
    }

    #[test]
    fn entries_that_cannot_be_read_are_left_out_and_said_to_be() {
        let file = |n: &str| -> io::Result<Fake> { Ok(Fake(n.into(), Ok((false, false)))) };
        let failing = |n: &str, e| -> io::Result<Fake> { Ok(Fake(n.into(), Err(e))) };
        let read = |all: Vec<io::Result<Fake>>| {
            let (entries, unreadable) = read_entries(all.into_iter(), 10);
            let names: Vec<String> = entries.into_iter().map(|e| e.name).collect();
            (names, unreadable)
        };
        // One gone meanwhile is just not there.
        assert_eq!(
            read(vec![
                file("a"),
                failing("gone", io::ErrorKind::NotFound),
                Ok(Fake("d".into(), Ok((true, false)))),
            ]),
            (vec!["a".to_owned(), "d".to_owned()], false)
        );
        // Its kind can't be read.
        assert_eq!(
            read(vec![
                file("a"),
                failing("b", io::ErrorKind::PermissionDenied)
            ]),
            (vec!["a".to_owned()], true)
        );
        // The read fails partway: nothing after it.
        assert_eq!(
            read(vec![file("a"), Err(io::Error::other("EIO")), file("c")]),
            (vec!["a".to_owned()], true)
        );
        // A name that is not UTF-8.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            let odd = Ok(Fake(
                OsString::from_vec(b"b\xff".to_vec()),
                Ok((false, false)),
            ));
            assert_eq!(read(vec![file("a"), odd]), (vec!["a".to_owned()], true));
        }
    }

    #[test]
    fn natural_order_compares_digit_runs_by_value() {
        fn sorted(mut v: Vec<&str>) -> Vec<&str> {
            v.sort_by(|a, b| natural_cmp(a, b));
            v
        }
        assert_eq!(
            sorted(vec!["a10", "a2", "A1", "a02"]),
            ["A1", "a2", "a02", "a10"]
        );
        assert_eq!(sorted(vec!["b", "B", "a"]).first(), Some(&"a"));
        assert_eq!(natural_cmp("abc", "ab"), Ordering::Greater);
    }

    #[test]
    fn stat_reports_missing_as_none() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(stat(&p(&d, "nope"), None), Ok(None));
        let f = put(&d, "f.txt", b"hello");
        let s = stat(&f, None).unwrap().unwrap();
        assert_eq!(s.len, 5);
        assert!(s.mtime_ms > 1_500_000_000_000);
        assert_eq!(
            stat(&p(&d, ""), None)
                .unwrap()
                .map(|s| s.len > 0 || s.mtime_ms > 0),
            Some(true)
        );
        // Below a file there is nothing either.
        assert_eq!(stat(&format!("{f}/x"), None), Ok(None));
    }

    #[cfg(unix)]
    #[test]
    fn the_remote_stat_script_agrees_with_the_local_one() {
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f.txt", b"hello");
        let local = stat(&f, None).unwrap().unwrap();
        let remote = stat_sh(&f, None).unwrap().unwrap();
        assert_eq!(remote.len, local.len);
        assert_eq!(remote.mtime_ms, local.mtime_ms / 1000 * 1000);
        assert_eq!(stat_sh(&p(&d, "nope"), None), Ok(None));
        assert_eq!(stat_sh(&p(&d, "no/folder/either"), None), Ok(None));
        assert_eq!(stat_sh(&format!("{f}/x"), None), Ok(None));
        // A shut folder on the way: not "deleted", as stat() here says.
        use std::os::unix::fs::PermissionsExt as _;
        let shut = p(&d, "shut");
        std::fs::create_dir(&shut).unwrap();
        let inside = put(&d, "shut/a.txt", b"x");
        let mode = |m: u32| std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(m));
        mode(0o000).unwrap();
        if std::fs::metadata(&inside).is_err() {
            assert!(stat(&inside, None).is_err());
            assert!(stat_sh(&inside, None).is_err());
            assert!(stat_sh(&format!("{inside}/deeper"), None).is_err());
        }
        mode(0o755).unwrap();
        assert_eq!(
            parse_stamp("12 34\n"),
            Some(Stamp {
                len: 12,
                mtime_ms: 34_000
            })
        );
        assert_eq!(parse_stamp("nonsense"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_check_before_a_save_reads_no_more_than_was_read() {
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f.txt", b"mine\n");
        let hash = content_hash(b"mine\n");
        assert!(holds(Path::new(&f), 5, hash).unwrap());
        std::fs::write(&f, b"mine\nand more\n").unwrap();
        assert!(!holds(Path::new(&f), 5, hash).unwrap());
        // A device that never ends is not read at all.
        assert!(holds(Path::new("/dev/zero"), 5, hash).is_err());
    }

    #[test]
    fn content_hash_is_fnv1a() {
        assert_eq!(content_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(content_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(content_hash(b"ab"), content_hash(b"ba"));
    }

    #[test]
    fn read_text_normalizes_line_ends_and_bom() {
        let d = tempfile::tempdir().unwrap();
        let lf = read_text(&put(&d, "lf", b"a\nb\n"), None, 1 << 20).unwrap();
        assert_eq!(
            (
                lf.text.as_str(),
                lf.eol,
                lf.bom,
                lf.binary,
                lf.too_large,
                lf.not_utf8
            ),
            ("a\nb\n", Eol::Lf, false, false, false, false)
        );
        assert_eq!(
            (lf.size, lf.stamp.len, lf.hash),
            (4, 4, content_hash(b"a\nb\n"))
        );

        let crlf = read_text(&put(&d, "crlf", b"a\r\nb\nc\r\n"), None, 1 << 20).unwrap();
        assert_eq!((crlf.text.as_str(), crlf.eol), ("a\nb\nc\n", Eol::Crlf));
        assert_eq!(crlf.hash, content_hash(b"a\r\nb\nc\r\n"));

        let bom = read_text(
            &put(&d, "bom", b"\xef\xbb\xbf\xe4\xbd\xa0\r\nx"),
            None,
            1 << 20,
        )
        .unwrap();
        assert_eq!(
            (bom.text.as_str(), bom.eol, bom.bom),
            ("\u{4f60}\nx", Eol::Crlf, true)
        );

        let empty = read_text(&put(&d, "empty", b""), None, 1 << 20).unwrap();
        assert_eq!(
            (empty.text.as_str(), empty.eol, empty.size),
            ("", Eol::Lf, 0)
        );

        // The first break decides, and a lone CR is left alone.
        let lone = read_text(&put(&d, "cr", b"a\rb\nc"), None, 1 << 20).unwrap();
        assert_eq!((lone.text.as_str(), lone.eol), ("a\rb\nc", Eol::Lf));
        assert!(read_text(&p(&d, "missing"), None, 10).is_err());
        assert!(read_text(&p(&d, ""), None, 10).is_err());
    }

    #[test]
    fn read_text_flags_binary_large_and_invalid_utf8() {
        let d = tempfile::tempdir().unwrap();
        let nul = read_text(&put(&d, "nul", b"ab\0cd"), None, 1 << 20).unwrap();
        assert!(nul.binary && nul.text.is_empty() && !nul.not_utf8);
        let pdf = read_text(&put(&d, "pdf", b"%PDF-1.7 looks like text"), None, 1 << 20).unwrap();
        assert!(pdf.binary && pdf.text.is_empty());
        // A NUL past the first 8 KiB does not make it binary.
        let mut late = vec![b'a'; SNIFF_BYTES];
        late.push(0);
        assert!(
            !read_text(&put(&d, "late", &late), None, 1 << 20)
                .unwrap()
                .binary
        );

        let big = put(&d, "big", &[b'x'; 100]);
        let large = read_text(&big, None, 99).unwrap();
        assert!(large.too_large && large.text.is_empty() && !large.binary);
        assert_eq!((large.size, large.stamp.len, large.hash), (100, 100, 0));
        assert!(!read_text(&big, None, 100).unwrap().too_large);

        let bad = read_text(&put(&d, "bad", b"f\xffo\r\n"), None, 1 << 20).unwrap();
        assert!(bad.not_utf8 && !bad.binary);
        assert_eq!((bad.text.as_str(), bad.eol), ("f\u{fffd}o\n", Eol::Crlf));
    }

    #[cfg(unix)]
    #[test]
    fn read_text_reports_readonly_files() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f", b"x");
        assert!(!read_text(&f, None, 10).unwrap().readonly);
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(read_text(&f, None, 10).unwrap().readonly);
    }

    #[cfg(unix)]
    #[test]
    fn the_remote_read_script_agrees_with_the_local_one() {
        let d = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("a", &b"a\r\nb\nc"[..]),
            ("b", b"\xef\xbb\xbfx\n"),
            ("c", b""),
            ("d", b"%PDF-1.4"),
            ("e", b"f\xffo"),
        ] {
            let f = put(&d, name, bytes);
            let local = read_text(&f, None, 1 << 20).unwrap();
            let remote = read_sh(&f, None, 1 << 20).unwrap();
            assert_eq!(remote.stamp.len, local.stamp.len);
            assert_eq!(
                TextFile {
                    stamp: local.stamp,
                    crc: 0,
                    ..remote.clone()
                },
                local,
                "{name}"
            );
            assert_eq!(remote.crc, crate::remote_write::posix_crc(bytes), "{name}");
        }
        let f = put(&d, "big", &[b'x'; 50]);
        let large = read_sh(&f, None, 49).unwrap();
        assert!(large.too_large && large.text.is_empty() && large.size == 50);
        assert!(!read_sh(&f, None, 50).unwrap().too_large);
        assert!(
            read_sh(&p(&d, "nope"), None, 10)
                .unwrap_err()
                .contains("no such file")
        );
        assert!(
            read_sh(&p(&d, ""), None, 10)
                .unwrap_err()
                .contains("not a regular file")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_never_waited_on() {
        use std::os::unix::ffi::OsStrExt as _;
        let d = tempfile::tempdir().unwrap();
        let fifo = d.path().join("pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // As if it took a regular file's place after a look at it: opened
        // with no writer, it is refused at once.
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open_regular(&path).map(drop).map_err(|e| e.to_string()));
        });
        let opened = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("waited for a writer");
        assert_eq!(opened, Err("not a regular file".to_owned()));
        // Nor to see whether it can be written (no reader waits on it).
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let meta = std::fs::metadata(&path).unwrap();
            let _ = tx.send(readonly_of(&path, &meta));
        });
        let readonly = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("waited for a reader");
        assert!(readonly);
        let file = d.path().join("a.txt");
        std::fs::write(&file, "a\n").unwrap();
        let mut text = String::new();
        open_regular(&file)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "a\n");
    }

    #[test]
    fn write_text_round_trips_crlf_and_bom() {
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f.txt", b"\xef\xbb\xbfone\r\ntwo\r\n");
        let read = read_text(&f, None, 1 << 20).unwrap();
        assert_eq!((read.eol, read.bom), (Eol::Crlf, true));
        let stamp = write_text(
            &f,
            "one\ntwo\nthree\n",
            read.eol,
            read.bom,
            Some((read.stamp, read.hash)),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&f).unwrap(),
            b"\xef\xbb\xbfone\r\ntwo\r\nthree\r\n"
        );
        assert_eq!(stat(&f, None).unwrap(), Some(stamp));
        // The new stamp (and the bytes written) are what the next save expects.
        let written = content_hash(b"\xef\xbb\xbfone\r\ntwo\r\nthree\r\n");
        write_text(&f, "x\n", Eol::Lf, false, Some((stamp, written))).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"x\n");
    }

    #[test]
    fn write_text_refuses_to_overwrite_a_changed_file() {
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f.txt", b"mine\n");
        let read = (stat(&f, None).unwrap().unwrap(), content_hash(b"mine\n"));
        std::fs::write(&f, b"somebody else wrote more\n").unwrap();
        let now = stat(&f, None).unwrap();
        assert_eq!(
            write_text(&f, "x", Eol::Lf, false, Some(read)),
            Err(WriteError::Conflict(now))
        );
        assert_eq!(std::fs::read(&f).unwrap(), b"somebody else wrote more\n");
        // Other bytes of the same length under the same stamp (a clock too
        // coarse to tell): a conflict as well.
        let same = put(&d, "same.txt", b"mine\n");
        let before = std::fs::metadata(&same).unwrap().modified().unwrap();
        let read = (stat(&same, None).unwrap().unwrap(), content_hash(b"mine\n"));
        std::fs::write(&same, b"ours\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&same)
            .unwrap()
            .set_modified(before)
            .unwrap();
        assert_eq!(stat(&same, None).unwrap(), Some(read.0));
        assert_eq!(
            write_text(&same, "x", Eol::Lf, false, Some(read)),
            Err(WriteError::Conflict(Some(read.0)))
        );
        assert_eq!(std::fs::read(&same).unwrap(), b"ours\n");
        // Gone, with a stamp to compare against: a conflict too.
        std::fs::remove_file(&f).unwrap();
        assert_eq!(
            write_text(&f, "x", Eol::Lf, false, Some(read)),
            Err(WriteError::Conflict(None))
        );
        assert!(!Path::new(&f).exists());
        // No stamp: a new file is written, an existing one is replaced.
        write_text(&f, "new\n", Eol::Lf, false, None).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"new\n");
        write_text(&f, "newer\n", Eol::Lf, true, None).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"\xef\xbb\xbfnewer\n");
    }

    #[test]
    fn a_change_made_while_the_backup_is_copied_is_not_written_over() {
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f.txt", b"old content\n");
        let target = Path::new(&f);
        let backups = d.path().join("backups");
        std::fs::create_dir(&backups).unwrap();
        let backup = || {
            std::fs::read_dir(&backups)
                .unwrap()
                .next()
                .map(|e| std::fs::read(e.unwrap().path()).unwrap())
        };
        // Someone saves over the file while its old content is copied; the
        // check comes once the backup holds all of it, and sees that.
        let theirs = || {
            assert_eq!(backup().as_deref(), Some(&b"old content\n"[..]));
            std::fs::write(target, b"theirs\n")?;
            Ok(std::fs::read(target)? == b"old content\n")
        };
        assert!(!write_in_place(target, b"mine\n", &backups, &theirs).unwrap());
        assert_eq!(std::fs::read(target).unwrap(), b"theirs\n");
        assert_eq!(backup(), None);
        // A check that fails leaves the file and no backup either.
        let broken = || Err(io::Error::other("cannot stat"));
        let err = write_in_place(target, b"mine\n", &backups, &broken).unwrap_err();
        assert_eq!(err.to_string(), "cannot stat");
        assert_eq!(std::fs::read(target).unwrap(), b"theirs\n");
        assert_eq!(backup(), None);
    }

    #[test]
    fn writes_in_place_keep_the_old_content_until_the_new_is_complete() {
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f.txt", b"old content\n");
        let target = Path::new(&f);
        let backups = d.path().join("backups");
        std::fs::create_dir(&backups).unwrap();
        let left = || std::fs::read_dir(&backups).unwrap().count();
        // Done: the new content, and no backup left.
        let same = || Ok(true);
        assert!(write_in_place(target, b"new\n", &backups, &same).unwrap());
        assert_eq!(std::fs::read(target).unwrap(), b"new\n");
        assert_eq!(left(), 0);
        // The write fails halfway (say the disk filled up): the old content is put back.
        let failing = std::cell::Cell::new(true);
        let flaky = |path: &Path, from: &mut dyn Read| {
            let mut bytes = Vec::new();
            from.read_to_end(&mut bytes)?;
            if bytes == b"newer\n" && failing.replace(false) {
                std::fs::write(path, b"ne")?;
                return Err(io::Error::other("no space left"));
            }
            overwrite(path, &mut &bytes[..])
        };
        let err = write_in_place_with(target, b"newer\n", &backups, &same, flaky).unwrap_err();
        assert_eq!(err.to_string(), "no space left");
        assert_eq!(std::fs::read(target).unwrap(), b"new\n");
        assert_eq!(left(), 0);
        // Putting it back fails too: the backup stays, and the error says where.
        let broken = |path: &Path, _: &mut dyn Read| {
            std::fs::write(path, b"")?;
            Err(io::Error::other("no space left"))
        };
        let err = write_in_place_with(target, b"newer\n", &backups, &same, broken).unwrap_err();
        let backup = std::fs::read_dir(&backups)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(err.to_string().ends_with(&backup.display().to_string()));
        assert_eq!(std::fs::read(&backup).unwrap(), b"new\n");
        assert!(is_scratch_name(
            &backup.file_name().unwrap().to_string_lossy()
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&backup).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // The backup is put back from its own handle: another file moved
        // in under its name meanwhile is not what the target gets.
        std::fs::remove_file(&backup).unwrap();
        std::fs::write(target, b"mine\n").unwrap();
        let swap = |path: &Path, from: &mut dyn Read| {
            let mut bytes = Vec::new();
            from.read_to_end(&mut bytes)?;
            if bytes == b"newer\n" {
                let name = std::fs::read_dir(&backups)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let evil = d.path().join("evil");
                std::fs::write(&evil, b"not yours\n").unwrap();
                std::fs::rename(&evil, &name).unwrap();
                return Err(io::Error::other("no space left"));
            }
            overwrite(path, &mut &bytes[..])
        };
        write_in_place_with(target, b"newer\n", &backups, &same, swap).unwrap_err();
        assert_eq!(std::fs::read(target).unwrap(), b"mine\n");
        // A backup is always a new file, never one already there.
        let (path, _) = new_backup(&backups).unwrap();
        assert!(path != backup && path.exists());
    }

    #[test]
    fn write_text_failures_leave_nothing_behind() {
        let d = tempfile::tempdir().unwrap();
        let err = write_text(&p(&d, "missing-dir/f.txt"), "x", Eol::Lf, false, None).unwrap_err();
        assert!(matches!(err, WriteError::Io(_)), "{err:?}");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_change_made_while_saving_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f.txt", b"mine\n");
        let target = PathBuf::from(&f);
        let read = Some((stat(&f, None).unwrap().unwrap(), content_hash(b"mine\n")));
        // Somebody writes while the new content is being staged: the check
        // right before the swap sees it.
        let raced = || {
            std::fs::write(&target, b"theirs, longer\n")?;
            unchanged(&target, read)
        };
        assert!(!replace_atomically(&target, b"x\n", &raced).unwrap());
        assert_eq!(std::fs::read(&f).unwrap(), b"theirs, longer\n");
        let left: Vec<_> = std::fs::read_dir(d.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "{left:?}");
        // Unchanged: swapped in.
        let read = Some((
            stat(&f, None).unwrap().unwrap(),
            content_hash(b"theirs, longer\n"),
        ));
        assert!(replace_atomically(&target, b"x\n", &|| unchanged(&target, read)).unwrap());
        assert_eq!(std::fs::read(&f).unwrap(), b"x\n");
    }

    /// Sets an extended attribute; false where the file system takes none.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn set_xattr(path: &str, name: &str, value: &[u8]) -> bool {
        use std::os::fd::AsRawFd as _;
        let file = std::fs::File::open(path).unwrap();
        let (fd, name) = (file.as_raw_fd(), std::ffi::CString::new(name).unwrap());
        let (at, len) = (value.as_ptr().cast(), value.len());
        #[cfg(target_os = "macos")]
        let done = unsafe { libc::fsetxattr(fd, name.as_ptr(), at, len, 0, 0) };
        #[cfg(target_os = "linux")]
        let done = unsafe { libc::fsetxattr(fd, name.as_ptr(), at, len, 0) };
        done == 0
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn get_xattr(path: &str, name: &str) -> Option<Vec<u8>> {
        use std::os::fd::AsRawFd as _;
        let file = std::fs::File::open(path).unwrap();
        let (fd, name) = (file.as_raw_fd(), std::ffi::CString::new(name).unwrap());
        let mut value = vec![0u8; 256];
        let (at, len) = (value.as_mut_ptr().cast(), value.len());
        #[cfg(target_os = "macos")]
        let got = unsafe { libc::fgetxattr(fd, name.as_ptr(), at, len, 0, 0) };
        #[cfg(target_os = "linux")]
        let got = unsafe { libc::fgetxattr(fd, name.as_ptr(), at, len) };
        value.truncate(usize::try_from(got).ok()?);
        Some(value)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_swapped_save_keeps_the_extended_attributes_and_acl() {
        use std::os::unix::fs::MetadataExt as _;
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "notes.txt", b"old\n");
        if !set_xattr(&f, "user.yuzora.tag", b"red") {
            eprintln!("skipped: this file system takes no extended attributes");
            return;
        }
        // An ACL entry of its own.
        #[cfg(target_os = "macos")]
        let acl = |path: &str| {
            let out = Command::new("/bin/ls")
                .args(["-le", path])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).contains("allow read")
        };
        #[cfg(target_os = "macos")]
        {
            let user = std::env::var("USER").unwrap();
            let entry = format!("user:{user} allow read");
            let added = Command::new("/bin/chmod").args(["+a", &entry, &f]).status();
            assert!(added.unwrap().success() && acl(&f));
        }
        let before = std::fs::metadata(&f).unwrap().ino();
        write_text(&f, "new\n", Eol::Lf, false, None).unwrap();
        assert_ne!(
            std::fs::metadata(&f).unwrap().ino(),
            before,
            "written in place, not swapped"
        );
        assert_eq!(std::fs::read(&f).unwrap(), b"new\n");
        assert_eq!(
            get_xattr(&f, "user.yuzora.tag").as_deref(),
            Some(&b"red"[..])
        );
        #[cfg(target_os = "macos")]
        assert!(acl(&f), "the ACL entry is gone");
    }

    #[cfg(unix)]
    #[test]
    fn a_save_is_owner_only_until_it_replaces_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "notes.txt", b"old\n");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let target = Path::new(&f);
        // Right before the swap the new content is all in the scratch file:
        // nobody else may open it, whatever the file's own mode.
        let private = || {
            let modes: Vec<u32> = std::fs::read_dir(d.path())
                .unwrap()
                .map(|e| e.unwrap())
                .filter(|e| is_scratch_name(&e.file_name().to_string_lossy()))
                .map(|e| e.metadata().unwrap().permissions().mode() & 0o777)
                .collect();
            assert_eq!(modes, [0o600]);
            Ok(true)
        };
        assert!(replace_atomically(target, b"new\n", &private).unwrap());
        assert_eq!(std::fs::read(target).unwrap(), b"new\n");
        let mode = std::fs::metadata(target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    #[cfg(unix)]
    #[test]
    fn write_text_swaps_a_scratch_file_in_and_keeps_the_mode() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "script.sh", b"old\n");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o750)).unwrap();
        let before = std::fs::metadata(&f).unwrap();
        // A reader that has the old file open keeps seeing the old, complete content.
        let mut reader = std::fs::File::open(&f).unwrap();
        write_text(&f, "new content\n", Eol::Lf, false, None).unwrap();
        let after = std::fs::metadata(&f).unwrap();
        assert_ne!(before.ino(), after.ino(), "written in place, not swapped");
        assert_eq!(after.permissions().mode() & 0o7777, 0o750);
        let mut old = String::new();
        reader.read_to_string(&mut old).unwrap();
        assert_eq!(old, "old\n");
        let left: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["script.sh"]);
        // A symlink is written through and stays a symlink.
        let link = p(&d, "link");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        write_text(&link, "via link\n", Eol::Lf, false, None).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&f).unwrap(), b"via link\n");
        // Its file gone (a restore): put back where the links say, links kept.
        let chain = p(&d, "chain");
        std::os::unix::fs::symlink("link", &chain).unwrap();
        std::fs::remove_file(&f).unwrap();
        write_text(&chain, "back\n", Eol::Lf, false, None).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"back\n");
        assert!(
            std::fs::symlink_metadata(&chain)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        // A loop of links is an error, not a hang.
        std::os::unix::fs::symlink("loop-b", p(&d, "loop-a")).unwrap();
        std::os::unix::fs::symlink("loop-a", p(&d, "loop-b")).unwrap();
        assert!(write_text(&p(&d, "loop-a"), "x", Eol::Lf, false, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn write_text_refuses_a_read_only_file() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "ro.txt", b"keep\n");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(matches!(
            write_text(&f, "x", Eol::Lf, false, None),
            Err(WriteError::Io(_))
        ));
        assert_eq!(std::fs::read(&f).unwrap(), b"keep\n");
    }

    #[test]
    fn scratch_names_have_the_shape_the_watcher_ignores() {
        let name = scratch_path(Path::new("/x"))
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(is_scratch_name(&name), "{name}");
        assert!(!is_scratch_name(".yuzora-save-notes.txt"));
        assert_ne!(scratch_path(Path::new("/x")), scratch_path(Path::new("/x")));
    }

    #[test]
    fn create_and_rename_refuse_existing_targets() {
        let d = tempfile::tempdir().unwrap();
        let f = p(&d, "new.txt");
        create_file(&f, None).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"");
        assert!(create_file(&f, None).is_err());
        let sub = p(&d, "sub");
        create_dir(&sub, None).unwrap();
        assert!(create_dir(&sub, None).is_err());
        assert!(create_dir(&p(&d, "no/parent"), None).is_err());
        assert!(create_file(&p(&d, "no/parent.txt"), None).is_err());

        let other = put(&d, "other.txt", b"keep");
        assert!(rename(&f, &other, None).is_err());
        assert_eq!(std::fs::read(&other).unwrap(), b"keep");
        assert!(Path::new(&f).exists());
        let moved = p(&d, "sub/moved.txt");
        rename(&f, &moved, None).unwrap();
        assert!(Path::new(&moved).exists() && !Path::new(&f).exists());
        assert!(rename(&p(&d, "ghost"), &p(&d, "x"), None).is_err());
    }

    #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
    #[test]
    fn the_system_refuses_a_rename_onto_something_there() {
        // No look first: what appears at `to` just before the move stays.
        let d = tempfile::tempdir().unwrap();
        let (mine, theirs) = (put(&d, "a.txt", b"mine"), put(&d, "b.txt", b"theirs"));
        let e = rename_excl(Path::new(&mine), Path::new(&theirs))
            .expect("this file system has it")
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&theirs).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&mine).unwrap(), b"mine");
        // An empty folder too (a plain rename replaces it).
        let (one, two) = (p(&d, "one"), p(&d, "two"));
        std::fs::create_dir(&one).unwrap();
        std::fs::create_dir(&two).unwrap();
        let e = rename_excl(Path::new(&one), Path::new(&two))
            .unwrap()
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert!(Path::new(&one).is_dir());
        let free = p(&d, "c.txt");
        rename_excl(Path::new(&mine), Path::new(&free))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&free).unwrap(), b"mine");
        assert_eq!(
            rename(&free, &theirs, None),
            Err(format!("{theirs} already exists"))
        );
    }

    #[test]
    fn rename_allows_a_change_of_case_only() {
        let d = tempfile::tempdir().unwrap();
        let from = put(&d, "readme.md", b"x");
        rename(&from, &p(&d, "README.md"), None).unwrap();
        let Listing { entries, .. } = list_dir(&p(&d, ""), None, 10).unwrap();
        assert_eq!(names(&entries), ["README.md"]);
    }

    /// The machine the next test asks: this one, answered by the stand-in `hc`.
    #[cfg(unix)]
    fn lab() -> Option<&'static str> {
        crate::remote_write::testing::fake_hc();
        Some("lab")
    }

    #[cfg(unix)]
    fn read(path: &str) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// What a folder holds, by name: a failed change leaves nothing else (no scratch name, no half-made entry).
    #[cfg(unix)]
    fn held(dir: &tempfile::TempDir) -> Vec<String> {
        let mut found: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        found
    }

    #[cfg(unix)]
    #[test]
    fn a_file_is_made_on_a_machine_and_never_made_over() {
        let on = lab();
        let d = tempfile::tempdir().unwrap();
        let f = p(&d, "new.txt");
        create_file(&f, on).unwrap();
        assert_eq!(read(&f), "");
        // Whatever is at the name stays: its own bytes, a folder, a link, a link to nothing.
        std::fs::write(&f, "mine").unwrap();
        let (sub, live, dangling) = (p(&d, "sub"), p(&d, "live"), p(&d, "dangling"));
        std::fs::create_dir(&sub).unwrap();
        std::os::unix::fs::symlink(&f, &live).unwrap();
        std::os::unix::fs::symlink("nowhere", &dangling).unwrap();
        for at in [&f, &sub, &live, &dangling] {
            let e = create_file(at, on).unwrap_err();
            assert!(
                e.contains("already exists") && e.contains(at.as_str()),
                "{e}"
            );
        }
        assert_eq!(read(&f), "mine");
        assert_eq!(std::fs::read_dir(&sub).unwrap().count(), 0);
        assert!(std::fs::symlink_metadata(&dangling).unwrap().is_symlink());
        assert!(std::fs::symlink_metadata(&live).unwrap().is_symlink());
        assert_eq!(held(&d), ["dangling", "live", "new.txt", "sub"]);
        // A name is its bytes: nothing in it is read by the shell there.
        for name in [
            "a b",
            "it's",
            "say \"hi\"",
            "two\nlines",
            "$HOME",
            "`x`",
            "-dash",
            "semi;colon",
            "é",
        ] {
            let at = p(&d, name);
            create_file(&at, on).unwrap();
            assert!(Path::new(&at).is_file(), "{name:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_is_made_on_a_machine_and_never_made_over() {
        let on = lab();
        let d = tempfile::tempdir().unwrap();
        let sub = p(&d, "sub");
        create_dir(&sub, on).unwrap();
        assert!(Path::new(&sub).is_dir());
        std::fs::write(p(&d, "sub/kept"), "x").unwrap();
        let (file, dangling) = (put(&d, "file", b"mine"), p(&d, "dangling"));
        std::os::unix::fs::symlink("nowhere", &dangling).unwrap();
        for at in [&sub, &file, &dangling] {
            let e = create_dir(at, on).unwrap_err();
            assert!(
                e.contains("already exists") && e.contains(at.as_str()),
                "{e}"
            );
        }
        assert_eq!(read(&p(&d, "sub/kept")), "x");
        assert_eq!(read(&file), "mine");
        assert!(std::fs::symlink_metadata(&dangling).unwrap().is_symlink());
        // One level, as here: a folder whose folder is not there is an error naming it, and makes nothing.
        let missing = p(&d, "no/parent");
        let e = create_dir(&missing, on).unwrap_err();
        assert!(e.contains(&p(&d, "no")), "{e}");
        assert!(!Path::new(&p(&d, "no")).exists());
        assert!(create_file(&p(&d, "no/parent.txt"), on).is_err());
        assert!(!Path::new(&p(&d, "no")).exists());
        // A file is not a folder to make things in.
        assert!(create_dir(&p(&d, "file/in"), on).is_err());
        assert!(create_file(&p(&d, "file/in"), on).is_err());
        for name in ["a b", "it's", "two\nlines", "$HOME", "-dash", "é"] {
            let at = p(&d, name);
            create_dir(&at, on).unwrap();
            assert!(Path::new(&at).is_dir(), "{name:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_rename_on_a_machine_never_replaces_what_is_there() {
        let on = lab();
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (put(&d, "a.txt", b"mine"), put(&d, "b.txt", b"theirs"));
        let (empty, full) = (p(&d, "empty"), p(&d, "full"));
        std::fs::create_dir(&empty).unwrap();
        std::fs::create_dir(&full).unwrap();
        std::fs::write(p(&d, "full/x"), "x").unwrap();
        let (live, dangling) = (p(&d, "live"), p(&d, "dangling"));
        std::os::unix::fs::symlink(&b, &live).unwrap();
        std::os::unix::fs::symlink("nowhere", &dangling).unwrap();
        // A file, a folder (it is no place to move into either), a link, a link to nothing.
        for to in [&b, &empty, &full, &live, &dangling] {
            let e = rename(&a, to, on).unwrap_err();
            assert!(
                e.contains("already exists") && e.contains(to.as_str()),
                "{e}"
            );
            assert_eq!(read(&a), "mine", "{to}");
        }
        assert_eq!(read(&b), "theirs");
        assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&full).unwrap().count(), 1);
        assert!(std::fs::symlink_metadata(&dangling).unwrap().is_symlink());
        assert!(std::fs::symlink_metadata(&live).unwrap().is_symlink());
        // A folder over a folder, a folder over a file, a file over itself.
        for to in [&full, &b] {
            assert!(
                rename(&empty, to, on)
                    .unwrap_err()
                    .contains("already exists")
            );
        }
        assert!(rename(&a, &a, on).unwrap_err().contains("already exists"));
        assert!(Path::new(&empty).is_dir() && Path::new(&a).is_file());
        assert_eq!(
            held(&d),
            ["a.txt", "b.txt", "dangling", "empty", "full", "live"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_or_a_folder_is_renamed_on_a_machine_and_a_link_with_it() {
        let on = lab();
        let d = tempfile::tempdir().unwrap();
        let a = put(&d, "a.txt", b"mine");
        let c = p(&d, "c.txt");
        rename(&a, &c, on).unwrap();
        assert_eq!((read(&c), Path::new(&a).exists()), ("mine".into(), false));
        // A folder moves with what is in it; into another folder too.
        let (sub, deep) = (p(&d, "sub"), p(&d, "sub/deep"));
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(p(&d, "sub/deep/f"), "f").unwrap();
        let renamed = p(&d, "sub2");
        rename(&sub, &renamed, on).unwrap();
        assert_eq!(read(&p(&d, "sub2/deep/f")), "f");
        assert!(!Path::new(&sub).exists());
        let inside = p(&d, "sub2/c.txt");
        rename(&c, &inside, on).unwrap();
        assert_eq!(read(&inside), "mine");
        // A link is moved as a link, and one to a folder is not a folder to move into.
        let (link, to_folder) = (p(&d, "link"), p(&d, "to-folder"));
        std::os::unix::fs::symlink("sub2", &link).unwrap();
        rename(&link, &to_folder, on).unwrap();
        assert_eq!(std::fs::read_link(&to_folder).unwrap(), Path::new("sub2"));
        assert!(std::fs::symlink_metadata(&link).is_err());
        // What is not there cannot be moved; its folder cannot be missing.
        let e = rename(&p(&d, "ghost"), &p(&d, "x"), on).unwrap_err();
        assert!(e.contains("ghost"), "{e}");
        let e = rename(&inside, &p(&d, "no/where"), on).unwrap_err();
        assert!(e.contains(&p(&d, "no")), "{e}");
        assert_eq!(read(&inside), "mine");
        assert!(!Path::new(&p(&d, "no")).exists());
        // A name is its bytes.
        for name in [
            "a b",
            "it's",
            "say \"hi\"",
            "two\nlines",
            "$HOME",
            "`x`",
            "-dash",
            "semi;colon",
            "é",
        ] {
            let at = p(&d, name);
            rename(&inside, &at, on).unwrap();
            assert_eq!(read(&at), "mine", "{name:?}");
            rename(&at, &inside, on).unwrap();
        }
    }

    /// Whether names that differ in case are one name here (the folder is on a case-folding volume).
    #[cfg(unix)]
    fn folds_case(d: &tempfile::TempDir) -> bool {
        std::fs::write(d.path().join("Case-Probe"), "").unwrap();
        let folds = d.path().join("case-probe").exists();
        std::fs::remove_file(d.path().join("Case-Probe")).unwrap();
        folds
    }

    #[cfg(unix)]
    #[test]
    fn a_change_of_case_only_is_renamed_on_a_machine_that_folds_case_and_refused_on_one_that_does_not()
     {
        let on = lab();
        let d = tempfile::tempdir().unwrap();
        let from = put(&d, "readme.md", b"x");
        if folds_case(&d) {
            rename(&from, &p(&d, "README.md"), on).unwrap();
            let Listing { entries, .. } = list_dir(&p(&d, ""), None, 10).unwrap();
            assert_eq!(names(&entries), ["README.md"]);
            // An accent is a case too, and the name stays one entry.
            let accented = put(&d, "école.txt", b"y");
            rename(&accented, &p(&d, "École.txt"), on).unwrap();
            assert_eq!(read(&p(&d, "École.txt")), "y");
        } else {
            // Two names, two files: the second is not replaced because it reads like the first.
            let other = put(&d, "README.md", b"theirs");
            let e = rename(&from, &other, on).unwrap_err();
            assert!(e.contains("already exists"), "{e}");
            assert_eq!((read(&from), read(&other)), ("x".into(), "theirs".into()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn what_a_machine_cannot_do_is_said_and_leaves_nothing_behind() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let file = put(&d, "a.txt", b"mine");
        crate::remote_write::testing::fake_hc();
        // Not connected, no sh there: hc's own words, and the name of what was asked.
        for (machine, said) in [("offline", "not connected"), ("no-sh", "sh")] {
            for e in [
                create_file(&p(&d, "n.txt"), Some(machine)).unwrap_err(),
                create_dir(&p(&d, "n"), Some(machine)).unwrap_err(),
                rename(&file, &p(&d, "m.txt"), Some(machine)).unwrap_err(),
            ] {
                assert!(e.contains(said), "{machine}: {e}");
            }
        }
        // Windows paths are not paths there.
        assert!(
            create_file(r"C:\x\n.txt", Some("lab"))
                .unwrap_err()
                .contains('/')
        );
        assert!(rename("relative", "/x", Some("lab")).is_err());
        assert_eq!(held(&d), ["a.txt"]);
        // A folder one cannot write in: told so, naming the machine, nothing made.
        if unsafe { libc::geteuid() } != 0 {
            let locked = p(&d, "locked");
            std::fs::create_dir(&locked).unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
            let target = format!("{locked}/n.txt");
            let e = create_file(&target, Some("lab")).unwrap_err();
            assert!(e.contains("lab") && e.contains(&locked), "{e}");
            assert!(create_dir(&format!("{locked}/n"), Some("lab")).is_err());
            assert!(rename(&file, &target, Some("lab")).is_err());
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(held(&d), ["a.txt", "locked"]);
            assert_eq!(std::fs::read_dir(&locked).unwrap().count(), 0);
        }
    }

    #[test]
    fn an_empty_machine_name_is_this_machine() {
        let d = tempfile::tempdir().unwrap();
        let f = p(&d, "a b.txt");
        create_file(&f, Some("")).unwrap();
        assert_eq!(
            create_file(&f, Some("")),
            Err(format!("cannot create {f}: File exists (os error 17)"))
        );
        assert!(create_dir(&p(&d, "no/parent"), None).is_err());
    }

    #[test]
    fn names_and_joins() {
        assert!(
            valid_name("a.txt") && valid_name(".hidden") && valid_name("a b") && valid_name("...")
        );
        for bad in ["", ".", "..", "a/b", "a\\b", "a\0b", "/"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert_eq!(join("/a/b", "c"), "/a/b/c");
        assert_eq!(join("/a/b/", "c"), "/a/b/c");
        assert_eq!(join("C:\\a\\", "c"), "C:\\a\\c");
        assert_eq!(join("/", "c"), "/c");
        assert_eq!(join("", "c"), "c");
    }

    #[test]
    fn a_file_is_copied_beside_itself_and_into_another_folder_and_never_over_anything() {
        let d = tempfile::tempdir().unwrap();
        let a = put(&d, "a.txt", b"mine");
        let here = p(&d, "");
        let here = here.trim_end_matches('/');
        assert_eq!(copy(&a, here, None, &[]), Ok(p(&d, "a copy.txt")));
        assert_eq!(copy(&a, here, None, &[]), Ok(p(&d, "a copy 2.txt")));
        assert_eq!(read_bytes(&p(&d, "a copy 2.txt")), b"mine");
        // Pasted twice where a.txt is already: the one there keeps its bytes.
        std::fs::create_dir(p(&d, "other")).unwrap();
        let theirs = put(&d, "other/a.txt", b"theirs");
        let other = p(&d, "other");
        assert_eq!(copy(&a, &other, None, &[]), Ok(p(&d, "other/a copy.txt")));
        assert_eq!(copy(&a, &other, None, &[]), Ok(p(&d, "other/a copy 2.txt")));
        assert_eq!(read_bytes(&theirs), b"theirs");
        assert_eq!(read_bytes(&a), b"mine");
        // Into a folder where the name is free, the name itself.
        std::fs::create_dir(p(&d, "empty")).unwrap();
        assert_eq!(
            copy(&a, &p(&d, "empty"), None, &[]),
            Ok(p(&d, "empty/a.txt"))
        );
        let Listing { entries, .. } = list_dir(here, None, 20).unwrap();
        assert_eq!(
            names(&entries),
            ["empty", "other", "a copy 2.txt", "a copy.txt", "a.txt"]
        );
    }

    fn read_bytes(path: &str) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_is_copied_with_all_it_holds_its_links_as_links_and_no_wider_than_it_was() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(p(&d, "src/one/two/three")).unwrap();
        put(&d, "src/one/two/three/deep.txt", b"deep");
        put(&d, "src/run.sh", b"#!/bin/sh\n");
        std::fs::set_permissions(p(&d, "src/run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        symlink("run.sh", p(&d, "src/to-file")).unwrap();
        symlink("one", p(&d, "src/to-folder")).unwrap();
        symlink("nowhere", p(&d, "src/dangling")).unwrap();
        std::fs::create_dir(p(&d, "src/private")).unwrap();
        std::fs::set_permissions(p(&d, "src/private"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::create_dir(p(&d, "src/sealed")).unwrap();
        put(&d, "src/sealed/kept", b"kept");
        std::fs::set_permissions(p(&d, "src/sealed"), std::fs::Permissions::from_mode(0o555))
            .unwrap();
        let root = p(&d, "");
        let made = copy(&p(&d, "src"), root.trim_end_matches('/'), None, &[]).unwrap();
        assert_eq!(made, p(&d, "src copy"));
        assert_eq!(read(&format!("{made}/one/two/three/deep.txt")), "deep");
        for (link, to) in [
            ("to-file", "run.sh"),
            ("to-folder", "one"),
            ("dangling", "nowhere"),
        ] {
            let at = format!("{made}/{link}");
            assert!(
                std::fs::symlink_metadata(&at).unwrap().is_symlink(),
                "{link}"
            );
            assert_eq!(std::fs::read_link(&at).unwrap(), Path::new(to));
        }
        let mode = |path: &str| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&format!("{made}/run.sh")) & 0o111, 0o111);
        assert_eq!(mode(&format!("{made}/private")), 0o700);
        assert_eq!(mode(&format!("{made}/sealed")) & 0o200, 0);
        assert_eq!(read(&format!("{made}/sealed/kept")), "kept");
        for sealed in [p(&d, "src/sealed"), format!("{made}/sealed")] {
            std::fs::set_permissions(sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(
            held(&d).iter().all(|n| !is_scratch_name(n)),
            "{:?}",
            held(&d)
        );
    }

    /// An extended attribute of the entry at `path` itself (a link is not followed); false where it cannot be set.
    #[cfg(target_os = "macos")]
    fn set_attr(path: &str, name: &str, value: &[u8]) -> bool {
        let new = |s: &str| std::ffi::CString::new(s).unwrap();
        let (path, name) = (new(path), new(name));
        let (at, len) = (value.as_ptr().cast(), value.len());
        let flags = libc::XATTR_NOFOLLOW;
        // SAFETY: NUL-terminated strings and a value as long as said.
        unsafe { libc::setxattr(path.as_ptr(), name.as_ptr(), at, len, 0, flags) == 0 }
    }

    #[cfg(target_os = "macos")]
    fn attr(path: &str, name: &str) -> Option<Vec<u8>> {
        let new = |s: &str| std::ffi::CString::new(s).unwrap();
        let (path, name) = (new(path), new(name));
        let mut value = vec![0u8; 256];
        let (at, len) = (value.as_mut_ptr().cast(), value.len());
        let flags = libc::XATTR_NOFOLLOW;
        // SAFETY: NUL-terminated strings and a buffer as long as said.
        let got = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), at, len, 0, flags) };
        value.truncate(usize::try_from(got).ok()?);
        Some(value)
    }

    /// What `cp -RP` and Finder keep of a folder, a file and a link in it: an attribute of the person's, a
    /// Finder tag and the quarantine flag (its flags: the time in it becomes the copy's, as `cp -RP` writes it).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_keeps_the_extended_attributes_finder_tags_and_quarantine_flag_of_what_it_copies() {
        use std::os::unix::fs::symlink;
        const TAGS: &str = "com.apple.metadata:_kMDItemUserTags";
        const QUARANTINE: &str = "com.apple.quarantine";
        // A binary property list of one tag: "Red", color 6.
        let tag = b"bplist00\xa1\x01URed\x0a6\x08\x0a\x00\x00\x00\x00\x00\x00\x01\x01\x00\x00\x00\x00\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x10";
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(p(&d, "box")).unwrap();
        put(&d, "box/a.txt", b"a");
        symlink("a.txt", p(&d, "box/link")).unwrap();
        for entry in ["box", "box/a.txt", "box/link"] {
            let at = p(&d, entry);
            for (name, value) in [
                ("user.note", &b"mine"[..]),
                (TAGS, &tag[..]),
                (QUARANTINE, b"0081;5f000000;Safari;"),
            ] {
                assert!(set_attr(&at, name, value), "{entry}: {name}");
            }
        }
        let root = p(&d, "");
        let made = copy(&p(&d, "box"), root.trim_end_matches('/'), None, &[]).unwrap();
        for entry in ["", "/a.txt", "/link"] {
            let at = format!("{made}{entry}");
            let mine = attr(&at, "user.note");
            assert_eq!(mine.as_deref(), Some(&b"mine"[..]), "{at}");
            assert_eq!(attr(&at, TAGS).as_deref(), Some(&tag[..]), "{at}");
            let flag = attr(&at, QUARANTINE);
            let flagged = flag.as_ref().is_some_and(|q| q.starts_with(b"0081;"));
            assert!(flagged, "{at}: {flag:?}");
        }
        let link = std::fs::symlink_metadata(format!("{made}/link")).unwrap();
        assert!(link.is_symlink());
    }

    /// A sparse file of 64 MiB holding one byte is copied with its holes: it takes no more room than it did.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_sparse_file_is_copied_sparse() {
        use std::os::unix::fs::MetadataExt as _;
        let d = tempfile::tempdir().unwrap();
        let sparse = p(&d, "sparse.bin");
        let file = std::fs::File::create(&sparse).unwrap();
        file.set_len(64 << 20).unwrap();
        std::os::unix::fs::FileExt::write_at(&file, b"x", (64 << 20) - 1).unwrap();
        drop(file);
        let room = |path: &str| std::fs::metadata(path).unwrap().blocks() * 512;
        assert!(room(&sparse) < 1 << 20, "not sparse to begin with");
        let root = p(&d, "");
        let made = copy(&sparse, root.trim_end_matches('/'), None, &[]).unwrap();
        assert_eq!(std::fs::metadata(&made).unwrap().len(), 64 << 20);
        assert!(room(&made) < 1 << 20, "{} bytes taken", room(&made));
    }

    /// A copy here goes through [`COPY_DEPTH`] folders inside each other, the one copied counted, and
    /// refuses a tree one deeper with nothing left. At the limit it fits in half the stack of the
    /// thread it runs on (2 MiB, `docs::blocking`) on a debug build: it needs some 350 KiB on macOS.
    #[cfg(unix)]
    #[test]
    fn a_copy_goes_as_deep_as_its_limit_and_refuses_one_folder_more() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_owned();
        let deepest = |top: &str, levels: usize| {
            let mut path = root.join(top);
            path.extend(std::iter::repeat_n("d", levels - 1));
            path
        };
        let mut said = Vec::new();
        for (top, levels) in [("fits", COPY_DEPTH), ("deep", COPY_DEPTH + 1)] {
            std::fs::create_dir_all(deepest(top, levels)).unwrap();
            std::fs::write(deepest(top, levels).join("f"), "f").unwrap();
            let (from, out) = (root.join(top), root.join(format!("{top} out")));
            std::fs::create_dir(&out).unwrap();
            let (from, out) = (from.display().to_string(), out.display().to_string());
            let run = std::thread::Builder::new().stack_size(1 << 20);
            let run = run.spawn(move || copy(&from, &out, None, &[])).unwrap();
            said.push(run.join().unwrap());
        }
        let copied = deepest("fits out/fits", COPY_DEPTH).join("f");
        assert_eq!(std::fs::read(copied).unwrap(), b"f");
        let Err(why) = &said[1] else {
            panic!("{said:?}")
        };
        let deep = "more than 64 folders inside each other";
        assert!(why.ends_with(deep), "{why}");
        assert_eq!(std::fs::read_dir(root.join("deep out")).unwrap().count(), 0);
    }

    /// A shared folder's sticky bit (there, only an entry's owner may rename or remove it) is kept, on the
    /// folder copied and on one inside it; a file's setuid and setgid are not. The file is empty: a write
    /// clears them anyway, and macOS never sets them on a file someone other than root makes.
    #[cfg(unix)]
    #[test]
    fn a_copy_keeps_a_folders_sticky_bit_and_never_a_files_setuid_or_setgid() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(p(&d, "shared/inner")).unwrap();
        put(&d, "shared/run", b"");
        let mode = |rel: &str| std::fs::metadata(p(&d, rel)).unwrap().permissions().mode() & 0o7777;
        let set = |rel: &str, mode| {
            std::fs::set_permissions(p(&d, rel), std::fs::Permissions::from_mode(mode)).unwrap();
        };
        set("shared", 0o1777);
        set("shared/inner", 0o1755);
        set("shared/run", 0o6755);
        assert_ne!(mode("shared/run") & 0o4000, 0, "a setuid to copy");
        let root = p(&d, "");
        let made = copy(&p(&d, "shared"), root.trim_end_matches('/'), None, &[]).unwrap();
        assert_eq!(made, p(&d, "shared copy"));
        assert_eq!(mode("shared copy") & 0o1000, 0o1000);
        assert_eq!(mode("shared copy/inner") & 0o1000, 0o1000);
        assert_eq!(mode("shared copy/run") & 0o6000, 0);
        assert_eq!(mode("shared copy/run") & 0o111, 0o111);
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_that_cannot_be_made_says_why_and_leaves_nothing() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let a = put(&d, "a.txt", b"mine");
        let root = p(&d, "");
        let root = root.trim_end_matches('/');
        let e = copy(&p(&d, "ghost.txt"), root, None, &[]).unwrap_err();
        assert!(
            e.contains("ghost.txt") && e.contains("no longer there"),
            "{e}"
        );
        let e = copy(&a, &p(&d, "no/where"), None, &[]).unwrap_err();
        assert!(
            e.contains(&p(&d, "no/where")) && e.contains("does not exist"),
            "{e}"
        );
        let e = copy(&p(&d, ""), &p(&d, "sub"), None, &[]).unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
        if unsafe { libc::geteuid() } != 0 {
            std::fs::create_dir(p(&d, "locked")).unwrap();
            std::fs::set_permissions(p(&d, "locked"), std::fs::Permissions::from_mode(0o555))
                .unwrap();
            let e = copy(&a, &p(&d, "locked"), None, &[]).unwrap_err();
            assert!(
                e.contains("you may not change") && e.contains(&p(&d, "locked")),
                "{e}"
            );
            std::fs::set_permissions(p(&d, "locked"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            assert_eq!(std::fs::read_dir(p(&d, "locked")).unwrap().count(), 0);
        }
        // A pipe is not a file to read: the copy of its folder stops, and leaves nothing.
        std::fs::create_dir(p(&d, "box")).unwrap();
        put(&d, "box/note", b"n");
        let fifo = std::ffi::CString::new(p(&d, "box/pipe")).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let e = copy(&p(&d, "box"), root, None, &[]).unwrap_err();
        assert!(
            e.contains("pipe") && e.contains("not a file, a folder or a link"),
            "{e}"
        );
        // A folder into itself, or into one inside it.
        std::fs::create_dir(p(&d, "box/inner")).unwrap();
        for into in [p(&d, "box"), p(&d, "box/inner")] {
            let e = copy(&p(&d, "box"), &into, None, &[]).unwrap_err();
            assert!(e.contains("cannot go into itself"), "{e}");
        }
        assert_eq!(
            held(&d),
            ["a.txt", "box"]
                .into_iter()
                .chain((unsafe { libc::geteuid() } != 0).then_some("locked"))
                .collect::<Vec<_>>()
        );
        assert_eq!(held_in(&p(&d, "box")), ["inner", "note", "pipe"]);
    }

    /// Someone who may rename in a shared folder swaps the scratch folder of a copy into it for a
    /// link to the person's home: before the copy is written (the home has no folder of that
    /// name), or after it and before it is renamed in (the home has one).
    #[cfg(unix)]
    #[test]
    fn a_scratch_folder_swapped_for_a_link_gets_nothing_written_through_it_nor_moved_out_of_it() {
        use std::os::unix::fs::symlink;
        for before_the_copy in [true, false] {
            let d = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(p(&d, "shared/.config/sub")).unwrap();
            put(&d, "shared/.config/sub/planted", b"theirs");
            std::fs::create_dir(p(&d, "home")).unwrap();
            if !before_the_copy {
                std::fs::create_dir(p(&d, "home/.config")).unwrap();
                put(&d, "home/.config/secret", b"mine");
            }
            let home = held_in(&p(&d, "home"));
            let into = p(&d, "shared");
            let swap = |name: &std::ffi::OsStr| {
                let at = Path::new(&into).join(name);
                std::fs::rename(&at, p(&d, "moved")).unwrap();
                symlink(p(&d, "home"), &at).unwrap();
            };
            let scratch = anchored::Scratch::make(&into).unwrap();
            if before_the_copy {
                swap(&scratch.name);
            }
            scratch
                .fill(Path::new(&p(&d, "shared/.config")), ".config")
                .unwrap();
            if !before_the_copy {
                swap(&scratch.name);
            }
            let placed = scratch.place(".config", &into, true, &[]);
            scratch.finish(placed.is_err().then_some(".config"));
            let why = format!("swapped before the copy: {before_the_copy}");
            assert_eq!(placed, Ok(p(&d, "shared/.config copy")), "{why}");
            assert_eq!(
                read_bytes(&p(&d, "shared/.config copy/sub/planted")),
                b"theirs",
                "{why}"
            );
            assert_eq!(held_in(&p(&d, "home")), home, "{why}");
            if !before_the_copy {
                assert_eq!(held_in(&p(&d, "home/.config")), ["secret"], "{why}");
            }
            assert!(held_in(&p(&d, "moved")).is_empty(), "{why}");
        }
    }

    /// Someone who may rename in a shared folder moves the scratch folder aside and renames one of
    /// the person's own folders there to its name; the copy then fails (a pipe in what is copied, or
    /// the free name open in a document). Its cleanup takes nothing from the person's folder, not
    /// even an empty one, and takes away what the copy made in the folder it opened.
    #[cfg(unix)]
    #[test]
    fn a_failed_copy_clears_nothing_in_a_folder_of_the_persons_swapped_in_at_its_scratch_name() {
        use std::os::unix::fs::PermissionsExt as _;
        for (fails_at, empty) in [("copy", false), ("rename", false), ("rename", true)] {
            let why = format!("fails at the {fails_at}, the folder empty: {empty}");
            let d = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(p(&d, "shared/docs")).unwrap();
            if !empty {
                put(&d, "shared/docs/notes.txt", b"mine");
                std::fs::create_dir(p(&d, "shared/docs/sealed")).unwrap();
                put(&d, "shared/docs/sealed/kept", b"kept");
                std::fs::set_permissions(
                    p(&d, "shared/docs/sealed"),
                    std::fs::Permissions::from_mode(0o555),
                )
                .unwrap();
            }
            std::fs::create_dir(p(&d, "box")).unwrap();
            put(&d, "box/note", b"n");
            if fails_at == "copy" {
                let fifo = std::ffi::CString::new(p(&d, "box/pipe")).unwrap();
                assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
            }
            let into = p(&d, "shared");
            let scratch = anchored::Scratch::make(&into).unwrap();
            let at = Path::new(&into).join(&scratch.name);
            std::fs::rename(&at, p(&d, "aside")).unwrap();
            std::fs::rename(p(&d, "shared/docs"), &at).unwrap();
            let held = if fails_at == "rename" {
                vec!["box".to_owned()]
            } else {
                Vec::new()
            };
            let placed = scratch
                .fill(Path::new(&p(&d, "box")), "box")
                .and_then(|()| scratch.place("box", &into, true, &held));
            scratch.finish(placed.is_err().then_some("box"));
            assert!(placed.is_err(), "{why}: {placed:?}");
            let at = at.to_string_lossy().into_owned();
            if empty {
                assert!(held_in(&at).is_empty(), "{why}");
            } else {
                assert_eq!(held_in(&at), ["notes.txt", "sealed"], "{why}");
                assert_eq!(read_bytes(&format!("{at}/notes.txt")), b"mine", "{why}");
                assert_eq!(read_bytes(&format!("{at}/sealed/kept")), b"kept", "{why}");
                let sealed = std::fs::metadata(format!("{at}/sealed")).unwrap();
                assert_eq!(sealed.permissions().mode() & 0o777, 0o555, "{why}");
                std::fs::set_permissions(
                    format!("{at}/sealed"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }
            assert!(held_in(&p(&d, "aside")).is_empty(), "{why}");
        }
    }

    /// Someone who may rename in a shared folder swaps what is copied, after it was looked at, for
    /// a link to a private file or folder of the person's, or for another file. Nothing of what is
    /// there is read into the copy: the copy fails and leaves nothing.
    #[cfg(unix)]
    #[test]
    fn a_source_swapped_after_it_was_looked_at_is_not_copied() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        for swapped_for in [
            "a link to a private file",
            "a link to a private folder",
            "another file",
        ] {
            let d = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(p(&d, "home/private")).unwrap();
            put(&d, "home/secret", b"secret");
            put(&d, "home/private/id.pub", b"secret");
            std::fs::set_permissions(p(&d, "home/secret"), std::fs::Permissions::from_mode(0o600))
                .unwrap();
            std::fs::set_permissions(
                p(&d, "home/private"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            std::fs::create_dir_all(p(&d, "shared/into")).unwrap();
            let folder = swapped_for.ends_with("folder");
            let offered = p(&d, "shared/offered");
            if folder {
                std::fs::create_dir(&offered).unwrap();
                put(&d, "shared/offered/a.txt", b"offered");
            } else {
                put(&d, "shared/offered", b"offered");
            }
            let into = p(&d, "shared/into");
            let scratch = anchored::Scratch::make(&into).unwrap();
            let looked = anchored::Looked::at(Path::new(&offered)).unwrap();
            std::fs::rename(&offered, p(&d, "shared/aside")).unwrap();
            match swapped_for {
                "another file" => {
                    put(&d, "shared/other", b"secret");
                    std::fs::rename(p(&d, "shared/other"), &offered).unwrap();
                }
                _ if folder => symlink(p(&d, "home/private"), &offered).unwrap(),
                _ => symlink(p(&d, "home/secret"), &offered).unwrap(),
            }
            let filled = scratch.fill_from(&looked, "offered");
            scratch.finish(filled.is_err().then_some("offered"));
            let e = filled.unwrap_err().to_string();
            assert!(
                e.contains("changed while it was copied"),
                "{swapped_for}: {e}"
            );
            assert!(
                held_in(&into).is_empty(),
                "{swapped_for}: {:?}",
                held_in(&into)
            );
        }
    }

    /// A folder renamed to another folder needs write permission on itself (its `..` changes):
    /// a folder whose owner may not write it (0555) is not copied, and nothing is left. One inside
    /// the folder copied is (`a_folder_is_copied_with_all_it_holds_...`).
    #[cfg(unix)]
    #[test]
    fn a_folder_its_owner_may_not_write_is_not_copied_and_leaves_nothing() {
        use std::os::unix::fs::PermissionsExt as _;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(p(&d, "sealed")).unwrap();
        put(&d, "sealed/kept", b"kept");
        std::fs::set_permissions(p(&d, "sealed"), std::fs::Permissions::from_mode(0o555)).unwrap();
        std::fs::create_dir(p(&d, "into")).unwrap();
        let e = copy(&p(&d, "sealed"), &p(&d, "into"), None, &[]).unwrap_err();
        assert!(e.contains("Permission denied"), "{e}");
        assert!(held_in(&p(&d, "into")).is_empty());
        std::fs::set_permissions(p(&d, "sealed"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A folder the person may write and enter but not read (0333, like another account's Drop Box)
    /// takes a paste of a file and of a folder from a folder they may only enter (0311). A folder they
    /// may not enter (0600) cannot be changed, and says so.
    #[cfg(unix)]
    #[test]
    fn a_copy_goes_into_a_folder_that_cannot_be_read_from_one_that_can_only_be_entered() {
        use std::os::unix::fs::PermissionsExt as _;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(p(&d, "src/box")).unwrap();
        put(&d, "src/a.txt", b"mine");
        put(&d, "src/box/b.txt", b"boxed");
        std::fs::create_dir(p(&d, "drop")).unwrap();
        std::fs::create_dir(p(&d, "shut")).unwrap();
        let set = |rel: &str, mode| {
            std::fs::set_permissions(p(&d, rel), std::fs::Permissions::from_mode(mode)).unwrap();
        };
        for (rel, mode) in [("src", 0o311), ("drop", 0o333), ("shut", 0o600)] {
            set(rel, mode);
        }
        let file = copy(&p(&d, "src/a.txt"), &p(&d, "drop"), None, &[]);
        let folder = copy(&p(&d, "src/box"), &p(&d, "drop"), None, &[]);
        let shut = copy(&p(&d, "src/a.txt"), &p(&d, "shut"), None, &[]);
        for rel in ["src", "drop", "shut"] {
            set(rel, 0o755);
        }
        assert_eq!(file, Ok(p(&d, "drop/a.txt")));
        assert_eq!(folder, Ok(p(&d, "drop/box")));
        assert_eq!(held_in(&p(&d, "drop")), ["a.txt", "box"]);
        assert_eq!(read_bytes(&p(&d, "drop/a.txt")), b"mine");
        assert_eq!(read_bytes(&p(&d, "drop/box/b.txt")), b"boxed");
        let shut = shut.unwrap_err();
        assert!(shut.contains("you may not change"), "{shut}");
        assert!(held_in(&p(&d, "shut")).is_empty());
    }

    #[cfg(unix)]
    fn held_in(dir: &str) -> Vec<String> {
        let mut found: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_copy_is_refused_rather_than_take_a_name_a_document_is_open_at() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(p(&d, "src")).unwrap();
        std::fs::create_dir(p(&d, "dst")).unwrap();
        let b = put(&d, "src/b.txt", b"pasted");
        let dst = p(&d, "dst");
        let held = ["b.txt".to_owned()];
        assert_eq!(
            copy(&b, &dst, None, &held),
            Err(format!(
                "{} is still open (its file was deleted): close it first",
                p(&d, "dst/b.txt")
            ))
        );
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
        // Where the file is on disk the name is taken anyway, and the copy takes the next.
        put(&d, "dst/b.txt", b"there");
        assert_eq!(copy(&b, &dst, None, &held), Ok(p(&d, "dst/b copy.txt")));
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_and_a_trash_on_a_machine_that_cannot_be_asked_say_so() {
        let d = tempfile::tempdir().unwrap();
        let file = put(&d, "a.txt", b"mine");
        let into = p(&d, "");
        let into = into.trim_end_matches('/');
        crate::remote_write::testing::fake_hc();
        for (machine, said) in [("offline", "not connected"), ("no-sh", "sh")] {
            for e in [
                copy(&file, into, Some(machine), &[]).unwrap_err(),
                trash(&file, Some(machine)).unwrap_err(),
            ] {
                assert!(e.contains(said) && e.contains(&file), "{machine}: {e}");
            }
        }
        assert!(
            trash("relative", Some("lab"))
                .unwrap_err()
                .contains("start with /")
        );
        assert!(
            copy(&file, "relative", Some("lab"), &[])
                .unwrap_err()
                .contains("start with /")
        );
        assert_eq!(held(&d), ["a.txt"]);
        // On "lab" the copy is made by the script, through hc.
        assert_eq!(copy(&file, into, Some("lab"), &[]), Ok(p(&d, "a copy.txt")));
        assert_eq!(read(&p(&d, "a copy.txt")), "mine");
    }

    // `reveal`, and `trash` on this machine, act on the real desktop: they only need to exist.
    #[test]
    fn trash_and_reveal_have_the_documented_shapes() {
        let _: fn(&str, Option<&str>) -> Result<(), String> = trash;
        let _: fn(&str) -> Result<(), String> = reveal;
    }

    /// A folder of 1,000 files of 4 KiB copied here, ten times: the median has a budget of 3 s.
    /// `cargo test --release -p yuzora-files --lib -- --ignored --exact fsops::tests::perf_copy_1000 --nocapture`.
    #[test]
    #[ignore = "a timing, for the perf lane"]
    fn perf_copy_1000() {
        let d = tempfile::tempdir().unwrap();
        let src = p(&d, "src");
        std::fs::create_dir(&src).unwrap();
        for i in 0..1000 {
            std::fs::write(format!("{src}/f{i:04}.bin"), [i as u8; 4096]).unwrap();
        }
        let into = p(&d, "");
        let into = into.trim_end_matches('/');
        let mut ms: Vec<u128> = (0..10)
            .map(|_| {
                let started = std::time::Instant::now();
                let made = copy(&src, into, None, &[]).unwrap();
                let took = started.elapsed().as_millis();
                assert_eq!(std::fs::read_dir(&made).unwrap().count(), 1000);
                std::fs::remove_dir_all(made).unwrap();
                took
            })
            .collect();
        println!("perf_copy_1000 runs_ms={ms:?}");
        ms.sort();
        let median = (ms[4] + ms[5]) / 2;
        println!("perf_copy_1000 median_ms={median}");
        assert!(median <= 3000, "median {median} ms is over the 3000 ms budget");
    }
}
