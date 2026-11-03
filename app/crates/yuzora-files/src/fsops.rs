//! File-system operations for the Files lane: list, stat, read and write text,
//! create, rename, trash, reveal. Every function blocks (call it from a worker
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

use yuzora_core::remote::{self, ExecOutput, ExecRequest};

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
n=0
set --
for f in ./.[!.]* ./..?* ./*; do
  case $f in
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
/// Prints `size mtime-seconds\n`, then up to `$2` bytes unless it is larger than `$3`.
const READ_SCRIPT: &str = r#"[ -e "$1" ] || { echo "no such file" >&2; exit 4; }
[ -f "$1" ] || { echo "not a regular file" >&2; exit 5; }
s=$(stat -L -c '%s %Y' -- "$1" 2>/dev/null || stat -L -f '%z %m' -- "$1") || exit 6
printf '%s\n' "$s"
[ "${s%% *}" -gt "$3" ] && exit 0
head -c "$2" -- "$1""#;

/// Changes to files on another machine wait for SFTP (lane L4).
const REMOTE_READ_ONLY: &str = "files on another machine are read-only in this version";

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
    pub size: u64,
    /// A NUL byte in the first 8 KiB, or a known binary signature.
    pub binary: bool,
    /// Larger than `max_bytes`: `text` is empty, nothing else was read.
    pub too_large: bool,
    /// Invalid UTF-8: `text` is a lossy decode and must not be saved back.
    pub not_utf8: bool,
    /// No write permission here, or the file is on another machine.
    pub readonly: bool,
}

#[derive(Debug, PartialEq)]
pub enum WriteError {
    /// The file changed (the current stamp, `None` when it is gone) since `expected`.
    Conflict(Option<Stamp>),
    /// Saving to another machine is not supported (for the caller to return).
    Remote,
    Io(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Conflict(Some(_)) => f.write_str("the file changed on disk"),
            WriteError::Conflict(None) => f.write_str("the file was removed from disk"),
            WriteError::Remote => f.write_str("saving files on another machine is not supported"),
            WriteError::Io(e) => f.write_str(e),
        }
    }
}

/// The machine to run on; an empty name means here (as in `ExecRequest::on`).
pub(crate) fn machine(on: Option<&str>) -> Option<&str> {
    on.filter(|m| !m.is_empty())
}

/// Runs `script` with `sh -c script sh args…` here (`on = None`) or on a machine.
pub(crate) fn run_sh(
    on: Option<&str>,
    script: &str,
    args: &[&str],
    timeout: Duration,
    max_output: u64,
) -> Result<ExecOutput, String> {
    let argv = ["-c", script, "sh"].into_iter().chain(args.iter().copied());
    let req = ExecRequest::new("sh", argv)
        .on(on)
        .timeout(timeout)
        .max_output(max_output);
    remote::run(&req).map_err(|e| e.to_string())
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
            let Ok(name) = std::str::from_utf8(name) else {
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
    let stamp = parse_stamp(&String::from_utf8_lossy(&out.stdout[..split]))
        .ok_or_else(|| format!("cannot read the stat of {path}"))?;
    if stamp.len > max_bytes {
        return Ok(TextFile {
            too_large: true,
            ..blank(stamp, true)
        });
    }
    Ok(decode(
        out.stdout[split + 1..].to_vec(),
        stamp,
        true,
        max_bytes,
    ))
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
/// is complete. A symlink is written through. Another machine is the
/// caller's to refuse ([`WriteError::Remote`]). Returns the new stamp.
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
    let mut bytes = Vec::with_capacity(text.len() + BOM.len());
    if bom {
        bytes.extend_from_slice(BOM);
    }
    match eol {
        Eol::Lf => bytes.extend_from_slice(text.as_bytes()),
        Eol::Crlf => bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
    }
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
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let id = (u64::from(std::process::id()) << 32)
        | (COUNTER.fetch_add(1, AtomicOrdering::Relaxed) & 0xffff_ffff);
    dir.join(format!("{SCRATCH_PREFIX}{id:016x}{nanos:016x}"))
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

/// A new empty file; an error when the path exists. This machine only.
pub fn create_file(path: &str, on: Option<&str>) -> Result<(), String> {
    if machine(on).is_some() {
        return Err(REMOTE_READ_ONLY.into());
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(drop)
        .map_err(|e| format!("cannot create {path}: {e}"))
}

/// A new folder; an error when the path exists or its parent does not. This
/// machine only.
pub fn create_dir(path: &str, on: Option<&str>) -> Result<(), String> {
    if machine(on).is_some() {
        return Err(REMOTE_READ_ONLY.into());
    }
    std::fs::create_dir(path).map_err(|e| format!("cannot create {path}: {e}"))
}

/// Moves `from` to `to`; an error when `to` exists (a change of case only is
/// fine). This machine only.
pub fn rename(from: &str, to: &str, on: Option<&str>) -> Result<(), String> {
    if machine(on).is_some() {
        return Err(REMOTE_READ_ONLY.into());
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

/// Moves a file or folder on this machine to the trash.
pub fn trash(path: &str) -> Result<(), String> {
    ::trash::delete(path).map_err(|e| format!("cannot move {path} to the trash: {e}"))
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
        Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.replace('/', "\\"))),
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
                    readonly: false,
                    ..remote.clone()
                },
                local,
                "{name}"
            );
            assert!(remote.readonly);
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

    #[test]
    fn files_on_another_machine_are_not_changed() {
        let d = tempfile::tempdir().unwrap();
        let f = p(&d, "a b.txt");
        let refused = Err(REMOTE_READ_ONLY.to_owned());
        assert_eq!(create_file(&f, Some("box")), refused);
        assert_eq!(create_dir(&f, Some("box")), refused);
        assert_eq!(rename(&f, &p(&d, "c.txt"), Some("box")), refused);
        assert!(!Path::new(&f).exists());
        // An empty machine name is this machine.
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

    // `trash` and `reveal` act on the real desktop: they only need to exist.
    #[test]
    fn trash_and_reveal_have_the_documented_shapes() {
        let _: fn(&str) -> Result<(), String> = trash;
        let _: fn(&str) -> Result<(), String> = reveal;
    }
}
