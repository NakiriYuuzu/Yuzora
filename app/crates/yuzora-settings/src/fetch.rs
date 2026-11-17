//! Installing from a link: GitHub's archive of the repository, unpacked into
//! a temp folder of its own, where [`crate::mods::plan`] reads it like any
//! folder. The temp folder goes with [`Fetched`].
//!
//! `curl` is the only program run, and it only downloads: it hands the archive
//! over on stdout. The code below reads that as a gzip of a tar and as nothing
//! else, on every OS, and writes its folders and files, and nothing else:
//! links, devices and the rest are left out, and a member outside the
//! archive's one folder fails the whole archive.

use crate::mods::{InstallError, Plan};
use crate::source::Source;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use yuzora_core::remote::{ExecRequest, run};

/// The most the download may be (`curl --max-filesize`).
const MAX_DOWNLOAD: u64 = 64 << 20;
/// The most its plain tar stream may be: a small archive can unpack to a huge one.
const MAX_UNPACKED: u64 = 128 << 20;
/// Files written to the temp folder; the plan then holds the folder to 2000.
const MAX_WRITTEN: usize = 20_000;

/// What a link names, unpacked.
pub struct Fetched {
    /// The folder the link names, in the temp folder.
    pub dir: PathBuf,
    /// The commit, as GitHub names the archive's folder (7 hex digits or more).
    pub commit: String,
    _temp: Temp,
}

/// A folder of our own, removed on drop.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// A new folder in `root`, never one already there (a link planted in a shared temp folder included).
fn temp_in(root: &Path) -> std::io::Result<Temp> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    for _ in 0..100 {
        let path = root.join(format!("yuzora-fetch-{}-{}", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
        match builder.create(&path) {
            Ok(()) => return Ok(Temp(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free name for a temp folder"))
}

/// curl: on Windows its own (System32), as the updater's.
fn system(name: &str) -> String {
    match std::env::var_os("SystemRoot").filter(|_| cfg!(windows)) {
        Some(root) => PathBuf::from(root).join("System32").join(format!("{name}.exe")).to_string_lossy().into_owned(),
        None => name.to_owned(),
    }
}

/// The archive's bytes. A redirect is not followed (no `-L`, and `-q` keeps
/// a `.curlrc` from adding it): any answer but 200 fails.
fn download(url: &str) -> Result<Vec<u8>, InstallError> {
    let limit = MAX_DOWNLOAD.to_string();
    let args = ["-q", "-gfsS", "--max-time", "60", "--max-filesize", &limit, "-w", "%{stderr}yuzora-http %{http_code} %{redirect_url}\n", url];
    let req = ExecRequest::new(system("curl"), args).timeout(Duration::from_secs(65)).max_output(MAX_DOWNLOAD + 1);
    let out = run(&req).map_err(|e| InstallError::FetchFailed(e.to_string()))?;
    // 63 is --max-filesize; a curl that lets a stream without a length run past it is held by max_output.
    if out.code == 63 || out.stdout.len() as u64 > MAX_DOWNLOAD {
        return Err(InstallError::TooBig);
    }
    let said = out.stderr_text();
    let (status, moved) = said.lines().find_map(|l| l.strip_prefix("yuzora-http ")).and_then(|r| r.split_once(' ')).unwrap_or_default();
    let (status, moved) = (status.trim(), moved.trim());
    if out.code == 0 && status == "200" {
        return Ok(out.stdout);
    }
    let curl = said.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("yuzora-http ")).unwrap_or_default();
    Err(InstallError::FetchFailed(match status {
        _ if !moved.is_empty() => format!("HTTP {status}, to {moved}"),
        "" | "000" => curl.to_owned(),
        // The answer began and then broke off (cut short, too slow): curl says why.
        "200" if !curl.is_empty() => curl.to_owned(),
        _ => format!("HTTP {status}"),
    }))
}

/// The archive as a plain tar stream, in memory: read as a gzip of a tar and
/// as nothing else, never past [`MAX_UNPACKED`].
fn plain_tar(archive: &[u8]) -> Result<Vec<u8>, InstallError> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(archive)
        .take(MAX_UNPACKED + 1)
        .read_to_end(&mut tar)
        .map_err(|e| InstallError::UnpackFailed(format!("not a gzip archive GitHub would send: {e}")))?;
    if tar.len() as u64 > MAX_UNPACKED {
        return Err(InstallError::TooBig);
    }
    Ok(tar)
}

/// A member of a tar stream that an install writes.
#[derive(Debug, PartialEq)]
enum Member<'a> {
    Folder(String),
    File(String, &'a [u8]),
}

impl Member<'_> {
    fn path(&self) -> &str {
        match self {
            Member::Folder(p) | Member::File(p, _) => p,
        }
    }
}

/// The folders and files of a tar stream (ustar, with pax and GNU long
/// names), each path checked. Links, devices and the like are left out.
fn members(tar: &[u8]) -> Result<Vec<Member<'_>>, String> {
    let mut out = Vec::new();
    let (mut at, mut long_name, mut pax_path, mut pax_size) = (0usize, None, None, None);
    loop {
        let header = tar.get(at..at + 512).ok_or("the archive ends early")?;
        if header.iter().all(|b| *b == 0) {
            return Ok(out);
        }
        if !checksum_ok(header) {
            return Err(format!("a damaged header at byte {at}"));
        }
        let flag = header[156];
        let own_size = octal(&header[124..136]).ok_or("a size it can't read")?;
        let size = if matches!(flag, b'x' | b'g' | b'L' | b'K') { own_size } else { pax_size.take().unwrap_or(own_size) };
        let len = usize::try_from(size).map_err(|_| "a member too big")?;
        let start = at + 512;
        let data = start.checked_add(len).and_then(|end| tar.get(start..end)).ok_or("the archive ends early")?;
        at = start + len.div_ceil(512) * 512;
        let name = match flag {
            b'x' => {
                (pax_path, pax_size) = pax(data)?;
                continue;
            }
            b'L' => {
                long_name = Some(String::from_utf8_lossy(data.split(|b| *b == 0).next().unwrap_or_default()).into_owned());
                continue;
            }
            b'g' | b'K' => continue,
            _ => pax_path.take().or(long_name.take()).unwrap_or_else(|| ustar_name(header)),
        };
        match flag {
            b'0' | 0 => out.push(Member::File(clean(&name)?, data)),
            b'5' => out.push(Member::Folder(clean(&name)?)),
            _ => {}
        }
    }
}

fn octal(field: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(field).ok()?.trim_matches(['\0', ' ']);
    if text.is_empty() { Some(0) } else { u64::from_str_radix(text, 8).ok() }
}

/// The header's checksum, with its own field read as spaces (signed or not, as writers differ).
fn checksum_ok(header: &[u8]) -> bool {
    let Some(want) = octal(&header[148..156]) else { return false };
    let byte = |(i, b): (usize, &u8)| if (148..156).contains(&i) { (32, 32) } else { (u64::from(*b), i64::from(*b as i8)) };
    let (unsigned, signed) = header.iter().enumerate().map(byte).fold((0u64, 0i64), |(u, s), (a, b)| (u + a, s + b));
    want == unsigned || i64::try_from(want) == Ok(signed)
}

fn ustar_name(header: &[u8]) -> String {
    let field = |f: &[u8]| String::from_utf8_lossy(f.split(|b| *b == 0).next().unwrap_or_default()).into_owned();
    let name = field(&header[0..100]);
    // POSIX ustar keeps the front of a long path in `prefix`; GNU's "ustar " uses those bytes otherwise.
    let prefix = if &header[257..263] == b"ustar\0" { field(&header[345..500]) } else { String::new() };
    if prefix.is_empty() { name } else { format!("{prefix}/{name}") }
}

/// A pax header's `path` and `size` (records `<length> <key>=<value>\n`).
fn pax(mut data: &[u8]) -> Result<(Option<String>, Option<u64>), String> {
    let damaged = || "a damaged pax header".to_owned();
    let (mut path, mut size) = (None, None);
    while !data.is_empty() {
        let space = data.iter().position(|b| *b == b' ').ok_or_else(damaged)?;
        let len: usize = std::str::from_utf8(&data[..space]).ok().and_then(|n| n.parse().ok()).filter(|n| *n > space + 1 && *n <= data.len()).ok_or_else(damaged)?;
        let record = data[space + 1..len].strip_suffix(b"\n").ok_or_else(damaged)?;
        let eq = record.iter().position(|b| *b == b'=').ok_or_else(damaged)?;
        let value = String::from_utf8_lossy(&record[eq + 1..]).into_owned();
        match &record[..eq] {
            b"path" => path = Some(value),
            b"size" => size = Some(value.parse().map_err(|_| damaged())?),
            _ => {}
        }
        data = &data[len..];
    }
    Ok((path, size))
}

/// `name` as `a/b/c`, refused when it could land outside the folder it is unpacked in.
fn clean(name: &str) -> Result<String, String> {
    let trimmed = name.trim_end_matches('/');
    let climbs = |c: &str| c.is_empty() || c == "." || c == ".." || c.contains('\\') || c.chars().any(char::is_control);
    if trimmed.split('/').any(climbs) {
        return Err(format!("{name:?} leaves the archive's folder"));
    }
    Ok(trimmed.to_owned())
}

/// Writes the folders and files under `source.path` to `into/<its name>`.
/// Returns that folder and the commit.
fn unpack(tar: &[u8], source: &Source, into: &Path) -> Result<(PathBuf, String), InstallError> {
    let members = members(tar).map_err(InstallError::UnpackFailed)?;
    // GitHub's legacy archive has one folder: <owner>-<repo>-<commit>.
    let top = members.first().map_or("", |m| m.path().split('/').next().unwrap_or_default());
    let prefix = format!("{}-{}-", source.owner, source.repo);
    let commit = top
        .get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(&prefix))
        .map(|_| &top[prefix.len()..])
        .filter(|c| (7..=40).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .ok_or_else(|| InstallError::UnpackFailed(format!("not GitHub's archive of {}/{}: its folder is {top:?}", source.owner, source.repo)))?;
    let root = into.join(source.name());
    std::fs::create_dir(&root)?;
    let mut written = 0;
    for m in &members {
        let mut parts = m.path().split('/');
        if parts.next() != Some(top) {
            return Err(InstallError::UnpackFailed(format!("{:?} is outside the archive's folder", m.path())));
        }
        let rest: Vec<&str> = parts.collect();
        if rest.len() <= source.path.len() || rest.iter().zip(&source.path).any(|(a, b)| a != b) {
            continue;
        }
        let rel = &rest[source.path.len()..];
        if cfg!(windows) && rel.iter().any(|c| !crate::templates::windows_name(c)) {
            return Err(InstallError::UnpackFailed(format!("{:?} can't be a file on Windows", m.path())));
        }
        let target = rel.iter().fold(root.clone(), |t, c| t.join(c));
        match m {
            Member::Folder(_) => std::fs::create_dir_all(&target)?,
            Member::File(_, data) => {
                written += 1;
                if written > MAX_WRITTEN {
                    return Err(InstallError::TooBig);
                }
                std::fs::create_dir_all(target.parent().unwrap_or(&root))?;
                std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?.write_all(data)?;
            }
        }
    }
    Ok((root, commit.to_owned()))
}

/// Downloads and unpacks what `source` names into a new folder in `temp_root`.
pub fn fetch(source: &Source, base: &str, temp_root: &Path) -> Result<Fetched, InstallError> {
    let tar = plain_tar(&download(&source.archive_url(base))?)?;
    let temp = temp_in(temp_root)?;
    let (dir, commit) = unpack(&tar, source, &temp.0)?;
    Ok(Fetched { dir, commit, _temp: temp })
}

/// What installing from `source` would do: [`crate::mods::plan`] of the folder
/// it names, shown as its GitHub address and commit. Nothing is written to
/// `home`, and nothing is left in `temp_root`.
pub fn plan(source: &Source, base: &str, temp_root: &Path, home: &Path) -> Result<Plan, InstallError> {
    let fetched = fetch(source, base, temp_root)?;
    let mut plan = crate::mods::plan(&fetched.dir, home)?;
    plan.source = source.github_url();
    plan.commit = Some(fetched.commit.clone());
    Ok(plan)
}

#[cfg(all(test, unix))]
mod tests;
