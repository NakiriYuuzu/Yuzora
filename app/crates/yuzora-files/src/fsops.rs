//! File-system operations for the Files lane: list, stat, read and write text,
//! create, rename, trash, reveal. Every function blocks (call it from a worker
//! thread). `on: Some(machine)` runs the operation on a herdr-core machine
//! through `sh` scripts (POSIX paths, arguments always as argv, never spliced
//! into the script); `None` works on this machine directly.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::io::{self, Read, Write};
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

/// Exit 4 when `$1` does not exist; else prints `size mtime-seconds`.
const STAT_SCRIPT: &str = r#"[ -e "$1" ] || exit 4
stat -L -c '%s %Y' -- "$1" 2>/dev/null || stat -L -f '%z %m' -- "$1""#;

/// `$1` file, `$2` max+1, `$3` max. Exit 4 missing, 5 not a regular file.
/// Prints `size mtime-seconds\n`, then up to `$2` bytes unless it is larger than `$3`.
const READ_SCRIPT: &str = r#"[ -e "$1" ] || { echo "no such file" >&2; exit 4; }
[ -f "$1" ] || { echo "not a regular file" >&2; exit 5; }
s=$(stat -L -c '%s %Y' -- "$1" 2>/dev/null || stat -L -f '%z %m' -- "$1") || exit 6
printf '%s\n' "$s"
[ "${s%% *}" -gt "$3" ] && exit 0
head -c "$2" -- "$1""#;

const CREATE_FILE_SCRIPT: &str = r#"if [ -e "$1" ] || [ -L "$1" ]; then echo "already exists" >&2; exit 17; fi
set -C
: > "$1""#;

const RENAME_SCRIPT: &str = r#"if [ -e "$2" ] || [ -L "$2" ]; then echo "already exists" >&2; exit 17; fi
mv -- "$1" "$2""#;

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
/// `.DS_Store`. At most `limit` entries; the flag says there were more. On
/// another machine the cut is made on the unsorted listing.
pub fn list_dir(path: &str, on: Option<&str>, limit: usize) -> Result<(Vec<Entry>, bool), String> {
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

fn list_local(path: &str, limit: usize) -> Result<(Vec<Entry>, bool), String> {
    let read = std::fs::read_dir(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let mut entries = Vec::new();
    for ent in read.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        let Ok(kind) = ent.file_type() else { continue };
        if skipped(&name) {
            continue;
        }
        let link = kind.is_symlink();
        let dir = if link {
            std::fs::metadata(ent.path()).is_ok_and(|m| m.is_dir())
        } else {
            kind.is_dir()
        };
        entries.push(Entry {
            hidden: name.starts_with('.'),
            name,
            dir,
            link,
            size: 0,
        });
    }
    sort_entries(&mut entries);
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    for e in entries.iter_mut().filter(|e| !e.dir && !e.link) {
        e.size = std::fs::metadata(Path::new(path).join(&e.name)).map_or(0, |m| m.len());
    }
    Ok((entries, truncated))
}

fn list_sh(path: &str, on: Option<&str>, limit: usize) -> Result<(Vec<Entry>, bool), String> {
    let max = limit.saturating_add(1).to_string();
    let out = run_sh(on, LIST_SCRIPT, &[path, &max], LIST_TIMEOUT, 16 << 20)?;
    if out.code != 0 {
        return Err(failure(&out, "listing"));
    }
    let mut entries = parse_listing(&out.stdout);
    sort_entries(&mut entries);
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    Ok((entries, truncated))
}

/// Reads [`LIST_SCRIPT`]'s output.
fn parse_listing(out: &[u8]) -> Vec<Entry> {
    let mut parts = out.split(|b| *b == 0);
    let mut rows = Vec::new();
    let mut sizes = &b""[..];
    while let Some(kind) = parts.next() {
        if kind == b"-" {
            sizes = parts.next().unwrap_or_default();
            break;
        }
        let Some(name) = parts.next() else { break };
        rows.push((
            kind.first().copied().unwrap_or(b'f'),
            String::from_utf8_lossy(name).into_owned(),
        ));
    }
    let sizes: Vec<u64> = String::from_utf8_lossy(sizes)
        .lines()
        .map(|l| l.trim().parse().unwrap_or(0))
        .collect();
    let mut sizes = sizes.into_iter();
    rows.into_iter()
        .map(|(kind, name)| Entry {
            hidden: name.starts_with('.'),
            dir: matches!(kind, b'd' | b'L'),
            link: matches!(kind, b'l' | b'L'),
            size: if kind == b'f' {
                sizes.next().unwrap_or(0)
            } else {
                0
            },
            name,
        })
        .filter(|e| !skipped(&e.name))
        .collect()
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

fn stamp_of(m: &std::fs::Metadata) -> Stamp {
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

fn read_local(path: &str, max_bytes: u64) -> Result<TextFile, String> {
    let err = |e: io::Error| format!("cannot read {path}: {e}");
    // Looked at before opening: opening a FIFO would wait for a writer.
    if !std::fs::metadata(path).map_err(err)?.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    let mut file = std::fs::File::open(path).map_err(err)?;
    let meta = file.metadata().map_err(err)?;
    let stamp = stamp_of(&meta);
    let readonly = meta.permissions().readonly()
        || std::fs::OpenOptions::new().write(true).open(path).is_err();
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
/// `bom`. With `expected`, a file whose stamp differs (or that is gone) is not
/// touched: `Conflict` carries its current stamp. The write goes to a scratch
/// file in the same directory that is fsynced and renamed over the target, so
/// a failed save leaves the old content; mode (and owner) are kept. A symlink
/// is written through. Another machine is the caller's to refuse
/// ([`WriteError::Remote`]). Returns the new stamp.
pub fn write_text(
    path: &str,
    text: &str,
    eol: Eol,
    bom: bool,
    expected: Option<Stamp>,
) -> Result<Stamp, WriteError> {
    let io_err = |e: io::Error| WriteError::Io(format!("cannot write {path}: {e}"));
    let target = resolve_link(Path::new(path)).map_err(io_err)?;
    if let Some(want) = expected {
        let now = stat_local(&target).map_err(WriteError::Io)?;
        if now != Some(want) {
            return Err(WriteError::Conflict(now));
        }
    }
    let mut bytes = Vec::with_capacity(text.len() + BOM.len());
    if bom {
        bytes.extend_from_slice(BOM);
    }
    match eol {
        Eol::Lf => bytes.extend_from_slice(text.as_bytes()),
        Eol::Crlf => bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
    }
    replace_atomically(&target, &bytes).map_err(io_err)?;
    match stat_local(&target).map_err(WriteError::Io)? {
        Some(stamp) => Ok(stamp),
        None => Err(WriteError::Io(format!(
            "cannot write {path}: the file vanished"
        ))),
    }
}

fn resolve_link(path: &Path) -> io::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path),
        _ => Ok(path.to_owned()),
    }
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

fn replace_atomically(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let original = std::fs::metadata(target).ok().filter(|m| m.is_file());
    if original.is_some() {
        // The file's own write bit decides, as it would for an in-place write.
        std::fs::OpenOptions::new().write(true).open(target)?;
    }
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let scratch = scratch_path(dir);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&scratch)
    {
        Ok(f) => f,
        // A directory that cannot hold a scratch file around a writable file.
        Err(e)
            if original.is_some()
                && matches!(
                    e.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
                ) =>
        {
            return std::fs::write(target, bytes);
        }
        Err(e) => return Err(e),
    };
    #[cfg(unix)]
    if let Some(m) = &original
        && !keep_owner(&file, m)
    {
        // The swap would change the owner: write in place instead.
        drop(file);
        let _ = std::fs::remove_file(&scratch);
        return std::fs::write(target, bytes);
    }
    let staged = (|| {
        file.write_all(bytes)?;
        file.flush()?;
        if let Some(m) = &original {
            file.set_permissions(m.permissions())?;
        }
        file.sync_all()
    })()
    .and_then(|()| std::fs::rename(&scratch, target));
    drop(file);
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&scratch);
        return Err(e);
    }
    // The content is in place already; this only hardens the rename.
    #[cfg(unix)]
    let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    Ok(())
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

// ---- create, rename, remove ----

/// A new empty file; an error when the path exists.
pub fn create_file(path: &str, on: Option<&str>) -> Result<(), String> {
    match machine(on) {
        Some(m) => {
            let out = run_sh(Some(m), CREATE_FILE_SCRIPT, &[path], LIST_TIMEOUT, 4 << 10)?;
            if out.code == 0 {
                Ok(())
            } else {
                Err(failure(&out, "creating the file"))
            }
        }
        None => std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(drop)
            .map_err(|e| format!("cannot create {path}: {e}")),
    }
}

/// A new folder; an error when the path exists or its parent does not.
pub fn create_dir(path: &str, on: Option<&str>) -> Result<(), String> {
    match machine(on) {
        Some(m) => mkdir_on(path, Some(m)),
        None => std::fs::create_dir(path).map_err(|e| format!("cannot create {path}: {e}")),
    }
}

fn mkdir_on(path: &str, on: Option<&str>) -> Result<(), String> {
    let req = ExecRequest::new("mkdir", ["--", path])
        .on(on)
        .timeout(LIST_TIMEOUT)
        .max_output(4 << 10);
    let out = remote::run(&req).map_err(|e| e.to_string())?;
    if out.code == 0 {
        Ok(())
    } else {
        Err(failure(&out, "creating the folder"))
    }
}

/// Moves `from` to `to`; an error when `to` exists (a change of case only is fine).
pub fn rename(from: &str, to: &str, on: Option<&str>) -> Result<(), String> {
    match machine(on) {
        Some(m) => {
            let out = run_sh(Some(m), RENAME_SCRIPT, &[from, to], LIST_TIMEOUT, 4 << 10)?;
            if out.code == 0 {
                Ok(())
            } else {
                Err(failure(&out, "renaming"))
            }
        }
        None => {
            if std::fs::symlink_metadata(to).is_ok() && !same_entry_other_case(from, to) {
                return Err(format!("{to} already exists"));
            }
            std::fs::rename(from, to).map_err(|e| format!("cannot rename {from}: {e}"))
        }
    }
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
        let (entries, truncated) = list_dir(&p(&d, ""), None, 100).unwrap();
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
    fn list_dir_limit_cuts_the_sorted_listing() {
        let d = fixture();
        let (entries, truncated) = list_dir(&p(&d, ""), None, 3).unwrap();
        assert!(truncated);
        assert_eq!(names(&entries), [".cfg", "A", "b"]);
        let (all, truncated) = list_dir(&p(&d, ""), None, 7).unwrap();
        assert_eq!((all.len(), truncated), (7, false));
        assert_eq!(list_dir(&p(&d, ""), None, 0).unwrap(), (vec![], true));
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
        let (entries, _) = list_dir(&p(&d, ""), None, 10).unwrap();
        let get = |n: &str| entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!((get("to-dir").dir, get("to-dir").link), (true, true));
        assert_eq!(
            (get("to-file").dir, get("to-file").link, get("to-file").size),
            (false, true, 0)
        );
        assert_eq!((get("dangling").dir, get("dangling").link), (false, true));
        assert_eq!((get("real").dir, get("real").link), (true, false));
        // The remote script says the same.
        let (remote, _) = list_sh(&p(&d, ""), None, 10).unwrap();
        assert_eq!(remote, entries);
    }

    #[cfg(unix)]
    #[test]
    fn the_remote_listing_script_agrees_with_the_local_one() {
        let d = fixture();
        std::fs::write(d.path().join("with space.txt"), "abc").unwrap();
        std::fs::write(d.path().join("-dash"), "abcd").unwrap();
        let (local, _) = list_dir(&p(&d, ""), None, 100).unwrap();
        let (remote, truncated) = list_sh(&p(&d, ""), None, 100).unwrap();
        assert!(!truncated);
        assert_eq!(remote, local);
        assert!(names(&remote).contains(&".hidden") && names(&remote).contains(&"with space.txt"));
        let (cut, truncated) = list_sh(&p(&d, ""), None, 4).unwrap();
        assert_eq!((cut.len(), truncated), (4, true));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(list_sh(&p(&empty, ""), None, 4).unwrap(), (vec![], false));
        assert!(
            list_sh(&p(&d, "nope"), None, 4)
                .unwrap_err()
                .contains("cannot open")
        );
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
        assert_eq!(
            parse_stamp("12 34\n"),
            Some(Stamp {
                len: 12,
                mtime_ms: 34_000
            })
        );
        assert_eq!(parse_stamp("nonsense"), None);
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
            Some(read.stamp),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&f).unwrap(),
            b"\xef\xbb\xbfone\r\ntwo\r\nthree\r\n"
        );
        assert_eq!(stat(&f, None).unwrap(), Some(stamp));
        // The new stamp is what the next save expects.
        write_text(&f, "x\n", Eol::Lf, false, Some(stamp)).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"x\n");
    }

    #[test]
    fn write_text_refuses_to_overwrite_a_changed_file() {
        let d = tempfile::tempdir().unwrap();
        let f = put(&d, "f.txt", b"mine\n");
        let stamp = stat(&f, None).unwrap().unwrap();
        std::fs::write(&f, b"somebody else wrote more\n").unwrap();
        let now = stat(&f, None).unwrap();
        assert_eq!(
            write_text(&f, "x", Eol::Lf, false, Some(stamp)),
            Err(WriteError::Conflict(now))
        );
        assert_eq!(std::fs::read(&f).unwrap(), b"somebody else wrote more\n");
        // Gone, with a stamp to compare against: a conflict too.
        std::fs::remove_file(&f).unwrap();
        assert_eq!(
            write_text(&f, "x", Eol::Lf, false, Some(stamp)),
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
    fn write_text_failures_leave_nothing_behind() {
        let d = tempfile::tempdir().unwrap();
        let err = write_text(&p(&d, "missing-dir/f.txt"), "x", Eol::Lf, false, None).unwrap_err();
        assert!(matches!(err, WriteError::Io(_)), "{err:?}");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
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

    #[test]
    fn rename_allows_a_change_of_case_only() {
        let d = tempfile::tempdir().unwrap();
        let from = put(&d, "readme.md", b"x");
        rename(&from, &p(&d, "README.md"), None).unwrap();
        let (entries, _) = list_dir(&p(&d, ""), None, 10).unwrap();
        assert_eq!(names(&entries), ["README.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_remote_create_and_rename_scripts_refuse_existing_targets() {
        let d = tempfile::tempdir().unwrap();
        let f = p(&d, "a b.txt");
        let run =
            |script: &str, args: &[&str]| run_sh(None, script, args, LIST_TIMEOUT, 4096).unwrap();
        assert_eq!(run(CREATE_FILE_SCRIPT, &[&f]).code, 0);
        assert!(Path::new(&f).exists());
        assert_eq!(run(CREATE_FILE_SCRIPT, &[&f]).code, 17);
        assert_eq!(
            create_file(&f, Some("")),
            Err(format!("cannot create {f}: File exists (os error 17)"))
        );
        let other = put(&d, "o", b"keep");
        assert_eq!(run(RENAME_SCRIPT, &[&f, &other]).code, 17);
        assert_eq!(std::fs::read(&other).unwrap(), b"keep");
        assert_eq!(run(RENAME_SCRIPT, &[&f, &p(&d, "-moved")]).code, 0);
        assert!(Path::new(&p(&d, "-moved")).exists());
        let sub = p(&d, "sub");
        mkdir_on(&sub, None).unwrap();
        assert!(mkdir_on(&sub, None).is_err());
        assert!(mkdir_on(&p(&d, "no/parent"), None).is_err());
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
