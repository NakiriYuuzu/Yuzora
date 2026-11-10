//! Saving a file on another machine: `hc exec --on M --stdin -- sh -c SCRIPT`.
//! The new bytes travel on stdin (never in argv), the path as an argument
//! (never spliced into the script), and the script, on that machine, checks the
//! file is still as it was read and swaps in a scratch file in one `mv`.

use std::sync::LazyLock;
use std::time::Duration;

use yuzora_core::remote::{ExecError, ExecOutput, ExecRequest, MAX_STDIN};

use crate::fsops::{self, Seen, Stamp, WriteError};

/// The most a save to another machine carries at once.
pub const MAX_BYTES: usize = MAX_STDIN;

/// Why nothing was written, as the person can be told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// More than `hc exec --stdin` carries.
    TooLarge {
        bytes: usize,
        limit: usize,
    },
    /// Not a path `sh` there can use: a save needs one that starts with `/`.
    BadPath,
    /// No `sh` there (a Windows machine).
    NoShell,
    /// herdr-core here or there does not pass stdin on.
    CoreTooOld,
    /// The folder cannot hold the scratch file the save swaps in.
    FolderNotWritable,
    FileNotWritable,
    /// The file is another user's: the swap would give it a new owner.
    OtherOwner,
    /// The file's group is one this user is not in: the swap could not give the copy that group.
    OtherGroup,
    /// The file has an ACL a save cannot copy, or its folder would hand the copy one it cannot
    /// remove: the swap could change who may read or write the file.
    AccessRules,
    /// A folder, a pipe, a device.
    NotAFile,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mb = |n: usize| n.div_ceil(1 << 20);
        match self {
            Refusal::TooLarge { bytes, limit } => write!(
                f,
                "this file is {} MB: saving to another machine takes at most {} MB",
                mb(*bytes),
                limit >> 20
            ),
            Refusal::BadPath => f.write_str(
                "saving needs a path that starts with / (a Windows path cannot be saved to)",
            ),
            Refusal::NoShell => f.write_str(
                "saving needs sh on that machine, and it has none (Windows machines cannot be saved to)",
            ),
            Refusal::CoreTooOld => f.write_str(
                "saving needs a newer herdr-core: update it here and on that machine (hc update, hc machine update)",
            ),
            Refusal::FolderNotWritable => f.write_str(
                "the folder this file is in cannot be written to, and a save writes a new file there and swaps it in",
            ),
            Refusal::FileNotWritable => {
                f.write_str("you do not have permission to write this file on that machine")
            }
            Refusal::OtherOwner => f.write_str(
                "this file belongs to another user on that machine: a save would replace it with a copy of yours, so it is refused",
            ),
            Refusal::OtherGroup => f.write_str(
                "this file's group on that machine is one you are not in: a save would give its new copy another group, so it is refused",
            ),
            Refusal::AccessRules => f.write_str(
                "this file, or the folder it is in, has access rules (an ACL) that a save there cannot carry over, and replacing the file could change who may read or write it, so it is refused (copying an ACL needs getfacl and setfacl on that machine)",
            ),
            Refusal::NotAFile => f.write_str(
                "this is not a regular file on that machine (a folder, a pipe or a device)",
            ),
        }
    }
}

/// POSIX `cksum`'s checksum of `bytes` (the CRC, with the length folded in):
/// what `cksum < file` prints first, so a machine can say whether a file is
/// still the bytes that were read.
pub fn posix_crc(bytes: &[u8]) -> u32 {
    static TABLE: LazyLock<[u32; 256]> = LazyLock::new(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = (i as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 {
                    (c << 1) ^ 0x04c1_1db7
                } else {
                    c << 1
                };
            }
            *slot = c;
        }
        table
    });
    let step = |crc: u32, b: u8| (crc << 8) ^ TABLE[usize::from((crc >> 24) as u8 ^ b)];
    let mut crc = bytes.iter().fold(0u32, |crc, &b| step(crc, b));
    let mut len = bytes.len() as u64;
    while len != 0 {
        crc = step(crc, (len & 0xff) as u8);
        len >>= 8;
    }
    !crc
}

/// `sh` that finds the file `$1` really is (a link is written through, as it
/// is here): `t`, and `d`, the folder a scratch file beside it goes in.
macro_rules! resolve_links {
    () => {
        r#"nl='
'
t=$1 n=0
while [ -L "$t" ]; do
  n=$((n+1))
  [ "$n" -le 40 ] || { echo 'too many levels of symbolic links' >&2; exit 1; }
  l=$(readlink -- "$t" && echo x) || exit 1
  l=${l%x}
  l=${l%"$nl"}
  case $t in */*) p=${t%/*} ;; *) p=. ;; esac
  case $l in /*) t=$l ;; *) t=${p%/}/$l ;; esac
done
case $t in */*) d=${t%/*} ;; *) d=. ;; esac
[ -n "$d" ] || d=/
"#
    };
}
pub(crate) use resolve_links;

/// `sh` for what the script that opens a file and the one that saves it both have to know, so
/// that a file opens editable exactly when a save will not refuse it for a reason that was there
/// at open. Needs `t` and `d` (`resolve_links!`).
///
/// `blocked` prints why a save could not replace the file at `t` with a copy made beside it, and
/// nothing when it could. The copy is a new file: it has to get the owner, the group and the
/// access rules (an ACL, if the file has one) of the old one, or the save would change who may
/// read or write the file. `aclof` says whether a file has an ACL: from `getfacl` where there is
/// one, else from `ls` (a `+` after the mode on Linux, a numbered entry under the line on a Mac).
/// A group the user is not in is no hindrance on BSD and macOS when it is the folder's, which a
/// new file takes there. On Linux the copy has the user's own group until it is given another,
/// and that takes membership. A folder whose ACL cannot be removed (no `setfacl`, and not a Mac)
/// would hand it to the copy: where a tool can show one, that is `acl` too.
/// Where no tool can tell, a file counts as having none; the save itself looks again, at the new
/// file, in a way that needs no tool (see `WRITE_SCRIPT`).
macro_rules! save_gate {
    () => {
        r#"st() { stat -L -c '%s %Y %a %u %g' -- "$1" 2>/dev/null || stat -L -f '%z %m %Mp%Lp %u %g' -- "$1" 2>/dev/null; }
aclof() {
  if command -v getfacl >/dev/null 2>&1; then
    acl_out=$(getfacl -c -- "$1" 2>/dev/null) || return 0
    case $acl_out in *mask::*) return 0 ;; esac
    return 1
  fi
  acl_ls=$(ls -lde -- "$1" 2>/dev/null) || acl_ls=$(ls -ld -- "$1" 2>/dev/null) || return 1
  case $acl_ls in *"$nl "[0-9]*": "*) return 0 ;; esac
  acl_ls=${acl_ls%%"$nl"*}
  case ${acl_ls%% *} in ??????????+) return 0 ;; esac
  return 1
}
strippable() { command -v setfacl >/dev/null 2>&1 || [ "$(uname -s)" = Darwin ]; }
blocked() {
  [ -f "$t" ] || { echo notfile; return; }
  [ -w "$t" ] || { echo file; return; }
  [ -d "$d" ] && [ -w "$d" ] && [ -x "$d" ] || { echo folder; return; }
  if aclof "$t" && ! { command -v getfacl >/dev/null 2>&1 && command -v setfacl >/dev/null 2>&1; }; then echo acl; return; fi
  if ! strippable && aclof "$d"; then echo acl; return; fi
  [ "$(id -u)" = 0 ] && return
  me=$(st "$t") || { echo stat; return; }
  set -- $me
  fg=$5
  [ "$4" = "$(id -u)" ] || { echo owner; return; }
  case " $(id -G) " in *" $fg "*) return ;; esac
  if [ "$(uname -s)" != Linux ]; then
    dm=$(st "$d") || { echo stat; return; }
    set -- $dm
    [ "$fg" = "$5" ] && return
  fi
  echo group
}
"#
    };
}
pub(crate) use save_gate;

/// `$1` path, `$2` `cas` or `overwrite`, `$3 $4 $5` size, mtime (seconds) and
/// `cksum` CRC the file was read with (`-` when overwriting), `$6` how many
/// bytes arrive on stdin. Prints one line and exits 0:
/// `saved SIZE MTIME`, `conflict SIZE MTIME`, `conflict gone`,
/// `refused file|folder|owner|group|acl|notfile`, or `refused stdin RECEIVED`.
/// Any other end (a failed step, a signal) leaves the file as it was.
///
/// The new bytes go to a scratch file in a folder of its own, `.yuzora-save-<32 hex>/` beside the
/// file (`fsops::is_scratch_name` accepts the name). The folder gets mode 700 before anything is
/// made in it, so nobody but its owner can walk into it, whatever ACL the folder it is in hands
/// down (a folder with a default ACL ignores the umask, and `mkdir -m` is not the same on every
/// system, so `chmod` it is).
/// Then the folder's own ACL is removed (with `setfacl -b`, on a Mac with `chmod -N`) so that the
/// file made inside gets none. Whether it did is checked: a file made under `umask 077` has mode
/// 600 unless a default ACL took over (the kernel then ignores the umask), and where a tool can
/// show an ACL it is asked too. A file that has one and cannot lose it is refused (`refused
/// acl`): `chmod` would give the names in it the access the mode says. The new file gets the
/// owner, group, mode and, if the old file has an ACL, that ACL (`getfacl | setfacl`, or the
/// save is refused). Then, as the last step before the swap, the file is compared with what was
/// read (stat and checksum), and a single `mv` swaps the scratch file in: a rename replaces the
/// file whole or not at all.
const WRITE_SCRIPT: &str = concat!(
    r#"mode=$2 esize=$3 emtime=$4 ecrc=$5 want=$6
om=$(umask)
umask 077
set -C
sd=
trap '[ -z "$sd" ] || { rm -f -- "$sd/$sn"; rmdir -- "$sd" 2>/dev/null; }' EXIT
trap 'exit 1' HUP INT TERM
"#,
    resolve_links!(),
    save_gate!(),
    r#"strip() { if command -v setfacl >/dev/null 2>&1; then setfacl -b -- "$1" 2>/dev/null; else chmod -N -- "$1" 2>/dev/null; fi; return 0; }
clean() {
  if aclof "$1"; then return 1; fi
  cstat=$(st "$1") || return 1
  set -- $cstat
  case $3 in 600|0600) return 0 ;; esac
  return 1
}
gone() {
  p=$t
  while up=$(dirname -- "$p") && [ "$up" != "$p" ]; do
    if [ -e "$up" ]; then
      if [ -d "$up" ] && [ ! -x "$up" ]; then echo "cannot look inside $up" >&2; exit 1; fi
      break
    fi
    p=$up
  done
  echo 'conflict gone'
  exit 0
}
check() {
  now=$(st "$t") || { if [ -e "$t" ]; then echo "cannot read $t" >&2; exit 1; fi; gone; }
  set -- $now
  cs=$1 cm=$2
  [ "$cs $cm" = "$esize $emtime" ] || { echo "conflict $cs $cm"; exit 0; }
  ck=$(cksum < "$t") || { echo "cannot read $t" >&2; exit 1; }
  set -- $ck
  [ "$1 $2" = "$ecrc $esize" ] || { echo "conflict $cs $cm"; exit 0; }
}
if [ -e "$t" ]; then
  why=$(blocked)
  [ "$why" != stat ] || { echo "saving failed: cannot stat $t" >&2; exit 1; }
  [ -z "$why" ] || { echo "refused $why"; exit 0; }
  have=1
else
  have=0
  [ "$mode" = cas ] && check
  [ -d "$d" ] && [ -w "$d" ] && [ -x "$d" ] || { echo 'refused folder'; exit 0; }
fi
id=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
case $id in ''|*[!0-9a-f]*) echo 'saving failed: no name for the scratch file' >&2; exit 1 ;; esac
[ "${#id}" -eq 32 ] || { echo 'saving failed: no name for the scratch file' >&2; exit 1; }
sn=.yuzora-save-$id
case $d in /) made=/$sn ;; *) made=$d/$sn ;; esac
mkdir -- "$made" || exit 1
sd=$made
chmod 700 "$sd" || exit 1
strip "$sd"
tmp=$sd/$sn
cat > "$tmp" || exit 1
got=$(wc -c < "$tmp") || exit 1
got=$((got))
[ "$got" -eq "$want" ] || { echo "refused stdin $got"; exit 0; }
if [ "$have" = 1 ]; then
  meta=$(st "$t") || { [ -e "$t" ] && exit 1; have=0; }
fi
if [ "$have" = 1 ]; then
  set -- $meta
  md=$3 u=$4 g=$5
  mine=$(st "$tmp") || exit 1
  set -- $mine
  if [ "$u $g" != "$4 $5" ]; then
    chown "$u:$g" "$tmp" 2>/dev/null || { if [ "$u" = "$4" ]; then echo 'refused group'; else echo 'refused owner'; fi; exit 0; }
  fi
else
  md=$(printf '%o' $((0666 & ~0$om)))
fi
if [ "$have" = 1 ] && aclof "$t"; then
  command -v getfacl >/dev/null 2>&1 && command -v setfacl >/dev/null 2>&1 || { echo 'refused acl'; exit 0; }
  acl=$(getfacl -c -- "$t" 2>/dev/null) && printf '%s\n' "$acl" | setfacl --set-file=- -- "$tmp" 2>/dev/null || { echo 'refused acl'; exit 0; }
elif ! clean "$tmp"; then
  strip "$tmp"
  clean "$tmp" || { echo 'refused acl'; exit 0; }
fi
chmod "$md" "$tmp" || exit 1
ts=$(st "$tmp") || exit 1
set -- $ts
size=$1 mtime=$2
if [ "$mode" = cas ]; then check; fi
[ -d "$t" ] && { echo 'refused notfile'; exit 0; }
mv -f -- "$tmp" "$t" || exit 1
if [ -d "$t" ]; then
  rm -f -- "$t/$sn"
  echo "cannot replace $t: it became a folder" >&2
  exit 1
fi
echo "saved $size $mtime"
"#
);

/// Longest a save may run on the machine: 8 MiB over a slow link.
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

/// The request that saves `bytes` over `path` on `on` (`None`: here, through
/// the same script, as the tests do).
pub(crate) fn request(
    path: &str,
    bytes: &[u8],
    expected: Option<&Seen>,
    on: Option<&str>,
) -> Result<ExecRequest, Refusal> {
    if bytes.len() > MAX_BYTES {
        return Err(Refusal::TooLarge {
            bytes: bytes.len(),
            limit: MAX_BYTES,
        });
    }
    if !path.starts_with('/') || path.contains('\0') {
        return Err(Refusal::BadPath);
    }
    let (mode, size, mtime, crc) = match expected {
        Some(seen) => (
            "cas",
            seen.stamp.len.to_string(),
            seen.stamp.mtime_ms.div_euclid(1000).to_string(),
            seen.crc.to_string(),
        ),
        None => ("overwrite", "-".into(), "-".into(), "-".into()),
    };
    let arriving = bytes.len().to_string();
    let args = [path, mode, &size, &mtime, &crc, &arriving];
    Ok(fsops::sh_request(on, WRITE_SCRIPT, &args, WRITE_TIMEOUT, 64 << 10).stdin(bytes.to_vec()))
}

/// What the script said.
#[derive(Debug, PartialEq)]
enum Verdict {
    Saved(Stamp),
    Conflict(Option<Stamp>),
    Refused(Refusal),
    /// Fewer bytes arrived on stdin than were sent.
    Short(u64),
}

/// Reads the script's one line; anything but exactly a line of a known shape is `None`.
fn parse_verdict(stdout: &[u8]) -> Option<Verdict> {
    let line = std::str::from_utf8(stdout).ok()?.strip_suffix('\n')?;
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u64>().ok())?
    };
    let stamp = |size: &str, mtime: &str| {
        let digits = mtime.strip_prefix('-').unwrap_or(mtime);
        let secs: i64 = (number(digits)? <= i64::MAX as u64)
            .then(|| mtime.parse().ok())
            .flatten()?;
        Some(Stamp {
            len: number(size)?,
            mtime_ms: secs.saturating_mul(1000),
        })
    };
    match line.split(' ').collect::<Vec<_>>()[..] {
        ["saved", size, mtime] => Some(Verdict::Saved(stamp(size, mtime)?)),
        ["conflict", "gone"] => Some(Verdict::Conflict(None)),
        ["conflict", size, mtime] => Some(Verdict::Conflict(Some(stamp(size, mtime)?))),
        ["refused", "file"] => Some(Verdict::Refused(Refusal::FileNotWritable)),
        ["refused", "folder"] => Some(Verdict::Refused(Refusal::FolderNotWritable)),
        ["refused", "owner"] => Some(Verdict::Refused(Refusal::OtherOwner)),
        ["refused", "group"] => Some(Verdict::Refused(Refusal::OtherGroup)),
        ["refused", "acl"] => Some(Verdict::Refused(Refusal::AccessRules)),
        ["refused", "notfile"] => Some(Verdict::Refused(Refusal::NotAFile)),
        ["refused", "stdin", received] => Some(Verdict::Short(number(received)?)),
        _ => None,
    }
}

/// What `hc` itself said about a request it did not run, when that is a thing
/// the person can act on. The words are hc's own: an `hc` without `--stdin`
/// takes it for the program's name ("cannot start --stdin").
fn refusal_in(message: &str, len: usize) -> Option<Refusal> {
    if message.contains("--stdin") {
        Some(Refusal::CoreTooOld)
    } else if message.contains("piped in") {
        Some(Refusal::TooLarge {
            bytes: len,
            limit: MAX_BYTES,
        })
    } else if message.starts_with("cannot start sh") {
        Some(Refusal::NoShell)
    } else {
        None
    }
}

/// What the machine's answer means for the save of `len` bytes.
pub(crate) fn interpret(
    answer: Result<ExecOutput, ExecError>,
    path: &str,
    len: usize,
    on: Option<&str>,
) -> Result<Stamp, WriteError> {
    let failed = |why: &dyn std::fmt::Display| {
        let at = on.map(|m| format!(" on {m}")).unwrap_or_default();
        WriteError::Io(format!("cannot write {path}{at}: {why}"))
    };
    let out = match answer {
        Ok(out) => out,
        Err(ExecError::Remote(m) | ExecError::Protocol(m)) => {
            return Err(refusal_in(&m, len).map_or_else(|| failed(&m), WriteError::Refused));
        }
        Err(e @ ExecError::TimedOut { .. }) => {
            return Err(failed(&format!(
                "{e}. The file is the old one or the new one, never half of each: reload it to see which"
            )));
        }
        Err(e) => return Err(failed(&e)),
    };
    if out.code != 0 {
        return Err(failed(&fsops::failure(&out, "saving")));
    }
    match parse_verdict(&out.stdout) {
        Some(Verdict::Saved(stamp)) if stamp.len == len as u64 => Ok(stamp),
        Some(Verdict::Conflict(now)) => Err(WriteError::Conflict(now)),
        Some(Verdict::Refused(why)) => Err(WriteError::Refused(why)),
        Some(Verdict::Short(0)) if len > 0 => Err(WriteError::Refused(Refusal::CoreTooOld)),
        Some(Verdict::Short(got)) => Err(failed(&format!("{got} of {len} bytes arrived"))),
        _ => Err(failed(
            &"the answer could not be read; the file may or may not have been saved",
        )),
    }
}

/// Saves `bytes` over the file `path` on `on`: only if it is still as
/// `expected` has it (`None`: whatever it is, a missing file is made).
pub(crate) fn write_sh(
    path: &str,
    bytes: &[u8],
    expected: Option<&Seen>,
    on: Option<&str>,
) -> Result<Stamp, WriteError> {
    let req = request(path, bytes, expected, on).map_err(WriteError::Refused)?;
    interpret(fsops::run(req), path, bytes.len(), on)
}

/// A stand-in for `hc exec` that runs the program here, as the machine named
/// by `--on` would (its name picks how it misbehaves).
#[cfg(all(test, unix))]
pub(crate) mod testing {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// `hc exec --json --on M --timeout T [--cwd D] [--env K=V]... [--stdin] -- program args...`
    const HC: &str = r#"#!/bin/sh
[ "$1" = exec ] || exit 2
shift
on= stdin=0
while [ $# -gt 0 ] && [ "$1" != -- ]; do
  case "$1" in
    --on) on=$2; shift 2 ;;
    --cwd) cd "$2" || exit 3; shift 2 ;;
    --env) export "$2"; shift 2 ;;
    --timeout) shift 2 ;;
    --stdin|-i) stdin=1; shift ;;
    *) shift ;;
  esac
done
shift
fail() { printf '%s\n' "$1"; exit "$2"; }
case $on in
  offline) fail '{"error":{"hint":"","kind":"unreachable","message":"machine offline is not connected"}}' 1 ;;
  old-core) [ "$stdin" = 1 ] && fail '{"error":{"hint":"","kind":"spawn_failed","message":"cannot start --stdin: No such file or directory (os error 2)"}}' 1 ;;
  no-sh) [ "$1" = sh ] && fail '{"error":{"hint":"","kind":"spawn_failed","message":"cannot start sh: No such file or directory (os error 2)"}}' 1 ;;
  slow) fail '{"code":null,"signal":9,"stderr":"","stdout":"","timed_out":true,"truncated":false}' 0 ;;
esac
in=$(mktemp) out=$(mktemp) err=$(mktemp)
if [ "$stdin" = 1 ] && [ "$on" != deaf-core ]; then cat > "$in"; else : > "$in"; fi
if [ "$(wc -c < "$in")" -gt 8388608 ]; then
  rm -f "$in" "$out" "$err"
  fail '{"error":{"hint":"see `hc schema`","kind":"invalid_params","message":"at most 8 MB piped in"}}' 6
fi
"$@" < "$in" > "$out" 2> "$err"
code=$?
printf '{"code":%d,"ms":1,"signal":null,"stderr":"%s","stdout":"%s","timed_out":false,"truncated":false}\n' "$code" "$(base64 < "$err" | tr -d '\n')" "$(base64 < "$out" | tr -d '\n')"
rm -f "$in" "$out" "$err"
"#;

    /// Makes every `hc exec` this crate starts go through the stand-in.
    pub(crate) fn fake_hc() {
        static PATH: OnceLock<PathBuf> = OnceLock::new();
        let hc = PATH.get_or_init(|| {
            use std::os::unix::fs::PermissionsExt as _;
            let path =
                std::env::temp_dir().join(format!("yuzora-files-test-hc-{}", std::process::id()));
            std::fs::write(&path, HC).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        });
        let hc = hc.clone();
        yuzora_core::remote::set_hc_program(move || hc.clone());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    use super::testing::fake_hc;
    use super::*;
    use crate::fsops::{self, Eol, TextFile, content_hash, is_scratch_name};

    /// Here, through the script alone; and on a machine, through the stand-in `hc`.
    const WHERE: [Option<&str>; 2] = [None, Some("lab")];

    fn put(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn stamp_of(path: &str) -> Stamp {
        let meta = fs::metadata(path).unwrap();
        let secs = meta
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Stamp {
            len: meta.len(),
            mtime_ms: secs as i64 * 1000,
        }
    }

    /// The file as a read of it (here) would have left it.
    fn seen(path: &str) -> Seen {
        let bytes = fs::read(path).unwrap();
        Seen {
            stamp: stamp_of(path),
            hash: content_hash(&bytes),
            crc: posix_crc(&bytes),
        }
    }

    fn set_mtime(path: &str, secs: u64) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn mode_of(path: &str) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn is_root() -> bool {
        // SAFETY: a plain syscall with no arguments.
        unsafe { libc::geteuid() == 0 }
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

    /// Runs the save with `env` added, on `on`, the way [`write_sh`] does.
    fn save_with(
        env: &[(&str, &str)],
        path: &str,
        bytes: &[u8],
        expected: Option<&Seen>,
        on: Option<&str>,
    ) -> Result<Stamp, WriteError> {
        fake_hc();
        let req = env
            .iter()
            .fold(request(path, bytes, expected, on).unwrap(), |r, (k, v)| {
                r.env(*k, *v)
            });
        interpret(fsops::run(req), path, bytes.len(), on)
    }

    #[test]
    fn a_save_puts_exactly_the_bytes_it_was_given() {
        fake_hc();
        let big: Vec<u8> = (0..300_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let cases: [&[u8]; 7] = [
            b"",
            b"x",
            b"a\r\nb\nc\r",
            b"\xef\xbb\xbfhello\n",
            b"nul\0inside\0",
            b"\xff\xfe not utf-8 \x80",
            &big,
        ];
        for on in WHERE {
            for (i, bytes) in cases.iter().enumerate() {
                let dir = tempfile::tempdir().unwrap();
                let path = put(dir.path(), "f", b"old content\n");
                let got = write_sh(&path, bytes, Some(&seen(&path)), on);
                assert_eq!(fs::read(&path).unwrap(), *bytes, "{on:?} case {i}");
                assert_eq!(
                    got,
                    Ok(stamp_of(&path)),
                    "{on:?} case {i}: the stamp is what the file now has"
                );
                assert_eq!(names(dir.path()), ["f"], "{on:?} case {i}: no scratch file");
            }
        }
    }

    #[test]
    fn it_puts_on_disk_what_the_local_save_does() {
        fake_hc();
        let cases = [
            ("a\nb\n", Eol::Lf, false),
            ("a\nb", Eol::Crlf, false),
            ("", Eol::Crlf, true),
            ("\u{feff}x\n\n", Eol::Lf, true),
            ("h\u{e9}llo\r\n", Eol::Crlf, true),
        ];
        for on in WHERE {
            for (text, eol, bom) in cases {
                let (here, there) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
                let (a, b) = (
                    put(here.path(), "f", b"old\n"),
                    put(there.path(), "f", b"old\n"),
                );
                fs::set_permissions(&a, fs::Permissions::from_mode(0o640)).unwrap();
                fs::set_permissions(&b, fs::Permissions::from_mode(0o640)).unwrap();
                let read = fsops::read_text(&a, None, 1 << 20).unwrap();
                let local =
                    fsops::write_text(&a, text, eol, bom, Some((read.stamp, read.hash))).unwrap();
                let remote =
                    fsops::write_on(&b, on.or(Some("lab")), text, eol, bom, Some(seen(&b)))
                        .unwrap();
                let direct = write_sh(&b, &fsops::encode_text(text, eol, bom), None, on).unwrap();
                assert_eq!(
                    fs::read(&a).unwrap(),
                    fs::read(&b).unwrap(),
                    "{text:?} {eol:?} {bom}"
                );
                assert_eq!(
                    (mode_of(&a), remote.len, direct.len),
                    (mode_of(&b), local.len, local.len)
                );
            }
        }
    }

    #[test]
    fn what_was_read_is_what_a_save_is_held_to() {
        fake_hc();
        let dir = tempfile::tempdir().unwrap();
        let path = put(dir.path(), "f.txt", b"\xef\xbb\xbfone\r\ntwo\r\n");
        let read = fsops::read_text(&path, Some("lab"), 1 << 20).unwrap();
        assert_eq!(
            (read.text.as_str(), read.eol, read.bom, read.readonly),
            ("one\ntwo\n", Eol::Crlf, true, false)
        );
        assert_eq!(read.crc, posix_crc(&fs::read(&path).unwrap()));
        let from_read = |f: &TextFile| Seen {
            stamp: f.stamp,
            hash: f.hash,
            crc: f.crc,
        };
        let saved = fsops::write_on(
            &path,
            Some("lab"),
            "one\ntwo\nthree\n",
            read.eol,
            read.bom,
            Some(from_read(&read)),
        )
        .unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"\xef\xbb\xbfone\r\ntwo\r\nthree\r\n"
        );
        let again = fsops::read_text(&path, Some("lab"), 1 << 20).unwrap();
        assert_eq!(
            (again.stamp, again.text.as_str()),
            (saved, "one\ntwo\nthree\n")
        );
        let second = fsops::write_on(
            &path,
            Some("lab"),
            "four\n",
            again.eol,
            again.bom,
            Some(from_read(&again)),
        );
        assert!(second.is_ok(), "{second:?}");
        assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbffour\r\n");
    }

    #[test]
    fn a_file_changed_since_it_was_read_is_not_overwritten() {
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"abcd");
            set_mtime(&path, 1_700_000_000);
            let read = seen(&path);
            let held = |what: &str, theirs: &[u8], at: u64| {
                fs::write(&path, theirs).unwrap();
                set_mtime(&path, at);
                let got = write_sh(&path, b"mine", Some(&read), on);
                assert_eq!(
                    fs::read(&path).unwrap(),
                    theirs,
                    "{on:?} {what}: theirs is kept"
                );
                assert_eq!(
                    got,
                    Err(WriteError::Conflict(Some(stamp_of(&path)))),
                    "{on:?} {what}"
                );
                assert_eq!(names(dir.path()), ["f"], "{on:?} {what}: no scratch file");
            };
            held("a later time", b"abcd", 1_700_000_100);
            held("another length", b"abcde", 1_700_000_000);
            held("the same stamp, other bytes", b"abce", 1_700_000_000);
            fs::remove_file(&path).unwrap();
            assert_eq!(
                write_sh(&path, b"mine", Some(&read), on),
                Err(WriteError::Conflict(None)),
                "{on:?}"
            );
            assert!(
                !Path::new(&path).exists(),
                "{on:?}: a save that finds the file gone does not make it"
            );
            assert_eq!(names(dir.path()), Vec::<String>::new());
        }
    }

    #[test]
    fn a_save_that_is_not_held_to_what_was_read_replaces_it() {
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"theirs, changed");
            assert!(write_sh(&path, b"mine", None, on).is_ok());
            assert_eq!(fs::read(&path).unwrap(), b"mine");
            fs::remove_file(&path).unwrap();
            let made = write_sh(&path, b"again", None, on);
            assert_eq!(made, Ok(stamp_of(&path)), "{on:?}");
            assert_eq!(
                (fs::read(&path).unwrap(), names(dir.path())),
                (b"again".to_vec(), vec!["f".to_owned()])
            );
        }
    }

    #[test]
    fn a_change_made_while_the_save_runs_is_not_lost() {
        let (_shims, path_env) = shimmed(&[("cat", "cat \"$@\"; printf theirs > \"$YZ_TARGET\"")]);
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"original");
            let read = seen(&path);
            let got = save_with(
                &[("PATH", &path_env), ("YZ_TARGET", &path)],
                &path,
                b"mine",
                Some(&read),
                on,
            );
            assert_eq!(fs::read(&path).unwrap(), b"theirs", "{on:?}");
            assert_eq!(
                got,
                Err(WriteError::Conflict(Some(stamp_of(&path)))),
                "{on:?}"
            );
            assert_eq!(names(dir.path()), ["f"], "{on:?}: no scratch file");
        }
    }

    #[test]
    fn a_step_that_fails_leaves_the_file_as_it_was_and_no_scratch_file() {
        let steps = [
            (
                "cat",
                "head -c 2; echo 'cat: write error: No space left on device' >&2; exit 1",
                "No space left on device",
            ),
            ("wc", "exit 1", "failed"),
            ("stat", "exit 1", "failed"),
            ("cksum", "exit 1", "cannot read"),
            (
                "chmod",
                "echo 'chmod: Operation not permitted' >&2; exit 1",
                "Operation not permitted",
            ),
            (
                "mv",
                "echo 'mv: cannot move: Input/output error' >&2; exit 1",
                "Input/output error",
            ),
            ("od", "exit 1", "failed"),
        ];
        for on in WHERE {
            for (command, body, says) in steps {
                let dir = tempfile::tempdir().unwrap();
                let path = put(dir.path(), "f", b"original");
                fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
                let read = seen(&path);
                let (_shims, path_env) = shimmed(&[(command, body)]);
                let got = save_with(
                    &[("PATH", &path_env)],
                    &path,
                    b"new content",
                    Some(&read),
                    on,
                );
                assert!(
                    matches!(&got, Err(WriteError::Io(e)) if e.contains(says)),
                    "{on:?} {command}: {got:?}"
                );
                assert_eq!(fs::read(&path).unwrap(), b"original", "{on:?} {command}");
                assert_eq!(mode_of(&path), 0o640, "{on:?} {command}");
                assert_eq!(
                    names(dir.path()),
                    ["f"],
                    "{on:?} {command}: no scratch file"
                );
            }
        }
    }

    #[test]
    fn the_file_is_replaced_whole_never_written_in_place() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let path = put(dir.path(), "f", &vec![b'a'; 4 << 20]);
        let before = fs::metadata(&path).unwrap().ino();
        assert!(write_sh(&path, &vec![b'b'; 4 << 20], None, None).is_ok());
        assert_ne!(
            fs::metadata(&path).unwrap().ino(),
            before,
            "a swap puts a new file in its place"
        );
        // Whoever reads it meanwhile sees the old bytes or the new, never a mix or a part.
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let watcher = {
            let (path, done) = (path.clone(), done.clone());
            std::thread::spawn(move || {
                let mut seen = std::collections::BTreeSet::new();
                while !done.load(Ordering::Relaxed) {
                    let bytes = fs::read(&path).unwrap();
                    let whole = bytes.len() == 4 << 20 && bytes.iter().all(|b| *b == bytes[0]);
                    assert!(
                        whole,
                        "read {} bytes, first {:?}",
                        bytes.len(),
                        bytes.first()
                    );
                    seen.insert(bytes[0]);
                }
                seen
            })
        };
        for round in 0..12u8 {
            let fill = if round % 2 == 0 { b'a' } else { b'b' };
            assert!(write_sh(&path, &vec![fill; 4 << 20], None, None).is_ok());
        }
        done.store(true, Ordering::Relaxed);
        assert!(!watcher.join().unwrap().is_empty());
    }

    #[test]
    fn a_save_that_is_cut_short_never_swaps_in_half_a_file() {
        let (_shims, path_env) = shimmed(&[("cat", "head -c 4")]);
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"original");
            let got = save_with(
                &[("PATH", &path_env)],
                &path,
                b"a longer new content",
                None,
                on,
            );
            assert!(
                matches!(&got, Err(WriteError::Io(e)) if e.contains("4 of 20")),
                "{on:?}: {got:?}"
            );
            assert_eq!(fs::read(&path).unwrap(), b"original", "{on:?}");
            assert_eq!(names(dir.path()), ["f"], "{on:?}");
        }
    }

    #[test]
    fn a_save_that_is_stopped_keeps_the_old_file_and_leaves_only_a_name_nobody_lists() {
        let (_shims, path_env) = shimmed(&[("mv", "sleep 30")]);
        let dir = tempfile::tempdir().unwrap();
        let path = put(dir.path(), "f", b"original");
        let req = request(&path, b"new content", Some(&seen(&path)), None)
            .unwrap()
            .env("PATH", &path_env)
            .timeout(Duration::from_millis(500));
        let got = interpret(fsops::run(req), &path, 11, None);
        assert!(
            matches!(&got, Err(WriteError::Io(e)) if e.contains("took longer")),
            "{got:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"original");
        let left: Vec<String> = names(dir.path()).into_iter().filter(|n| n != "f").collect();
        assert!(left.iter().all(|n| is_scratch_name(n)), "{left:?}");
        let listed = fsops::list_dir(&dir.path().to_string_lossy(), None, 10).unwrap();
        assert_eq!(
            listed
                .entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["f"]
        );
    }

    #[test]
    fn the_scratch_file_is_private_until_it_is_swapped_in_and_is_named_as_the_listing_expects() {
        let record = "ls -laR \"$(dirname \"$3\")\" > \"$YZ_LOG\"; ls -ld \"$(dirname \"$3\")\" >> \"$YZ_LOG\"; mv \"$@\"";
        let (_shims, path_env) = shimmed(&[("mv", record)]);
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"original");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            let log = dir
                .path()
                .join("..")
                .join(format!("yz-mv-{}", std::process::id()));
            let log = log.to_string_lossy().into_owned();
            let got = save_with(
                &[("PATH", &path_env), ("YZ_LOG", &log)],
                &path,
                b"new",
                Some(&seen(&path)),
                on,
            );
            assert!(got.is_ok(), "{on:?}: {got:?}");
            let listing = fs::read_to_string(&log).unwrap();
            let _ = fs::remove_file(&log);
            let scratch: Vec<&str> = listing
                .lines()
                .filter(|l| l.contains(".yuzora-save-"))
                .collect();
            let file: Vec<&&str> = scratch.iter().filter(|l| l.starts_with('-')).collect();
            assert_eq!(file.len(), 1, "{listing}");
            let name = file[0].rsplit(' ').next().unwrap();
            assert!(is_scratch_name(name), "{name}");
            assert!(
                file[0].starts_with("-rw-r--r--"),
                "the file's own mode by the time it is swapped in: {}",
                file[0]
            );
            assert!(
                scratch
                    .iter()
                    .any(|l| l.starts_with("drwx------") && l.ends_with(name)),
                "what holds it is the owner's alone: {listing}"
            );
            assert_eq!(fs::read(&path).unwrap(), b"new");
            assert_eq!(names(dir.path()), ["f"], "{on:?}");
        }
    }

    #[test]
    fn the_scratch_file_is_owner_only_while_it_is_written() {
        let (_shims, path_env) = shimmed(&[(
            "cat",
            "ls -laR \"$(dirname \"$YZ_TARGET\")\" > \"$YZ_LOG\"; cat \"$@\"",
        )]);
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"secret");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            let log = dir
                .path()
                .join("..")
                .join(format!("yz-cat-{}", std::process::id()));
            let log = log.to_string_lossy().into_owned();
            let got = save_with(
                &[("PATH", &path_env), ("YZ_LOG", &log), ("YZ_TARGET", &path)],
                &path,
                b"new",
                None,
                on,
            );
            assert!(got.is_ok(), "{on:?}: {got:?}");
            let listing = fs::read_to_string(&log).unwrap();
            let _ = fs::remove_file(&log);
            let scratch: Vec<&str> = listing
                .lines()
                .filter(|l| l.contains(".yuzora-save-") && !l.ends_with(':'))
                .collect();
            assert_eq!(
                scratch.len(),
                2,
                "the folder it is in, and the file: {listing}"
            );
            assert!(
                scratch.iter().any(|l| l.starts_with("drwx------")),
                "{listing}"
            );
            assert!(
                scratch.iter().any(|l| l.starts_with("-rw-------")),
                "{listing}"
            );
        }
    }

    #[test]
    fn the_file_keeps_its_mode() {
        fake_hc();
        for on in WHERE {
            for mode in [0o600, 0o640, 0o664, 0o755, 0o4755, 0o2750, 0o1644] {
                let dir = tempfile::tempdir().unwrap();
                let path = put(dir.path(), "f", b"old");
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
                assert!(write_sh(&path, b"new", Some(&seen(&path)), on).is_ok());
                assert_eq!(mode_of(&path), mode, "{on:?} {mode:o}");
            }
        }
    }

    #[test]
    fn a_file_a_forced_save_makes_has_the_mode_a_new_file_gets_here() {
        fake_hc();
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (
            dir.path().join("here").to_string_lossy().into_owned(),
            dir.path().join("there").to_string_lossy().into_owned(),
        );
        fsops::write_text(&a, "x", Eol::Lf, false, None).unwrap();
        write_sh(&b, b"x", None, Some("lab")).unwrap();
        assert_eq!(mode_of(&b), mode_of(&a));
    }

    #[test]
    fn a_link_is_written_through_and_stays_a_link() {
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let (links, real) = (dir.path().join("links"), dir.path().join("real"));
            fs::create_dir_all(&links).unwrap();
            fs::create_dir_all(&real).unwrap();
            let file = put(&real, "target.txt", b"old");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
            let to = |name: &str, dest: &str| {
                symlink(dest, links.join(name)).unwrap();
                links.join(name).to_string_lossy().into_owned()
            };
            let direct = to("direct", &file);
            let relative = to("relative", "../real/target.txt");
            let chained = to("chained", &direct);
            for link in [&direct, &relative, &chained] {
                let got = write_sh(link, b"through", Some(&seen(link)), on);
                assert!(got.is_ok(), "{on:?} {link}: {got:?}");
                assert_eq!(fs::read(&file).unwrap(), b"through", "{on:?} {link}");
                assert_eq!(mode_of(&file), 0o640);
                assert!(
                    fs::symlink_metadata(link).unwrap().file_type().is_symlink(),
                    "{on:?} {link}: still a link"
                );
                assert_eq!(
                    names(&real),
                    ["target.txt"],
                    "{on:?} {link}: the scratch file is beside the file, and gone"
                );
                fs::write(&file, "old").unwrap();
            }
            assert_eq!(names(&links), ["chained", "direct", "relative"]);
        }
    }

    #[test]
    fn a_link_to_nothing_is_written_where_it_points_and_a_loop_is_refused() {
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let dangling = dir.path().join("dangling");
            let dest = dir.path().join("made.txt");
            symlink("made.txt", &dangling).unwrap();
            let got = write_sh(&dangling.to_string_lossy(), b"hello", None, on);
            assert!(got.is_ok(), "{on:?}: {got:?}");
            assert_eq!(fs::read(&dest).unwrap(), b"hello");
            assert!(
                fs::symlink_metadata(&dangling)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            let (a, b) = (dir.path().join("a"), dir.path().join("b"));
            symlink("b", &a).unwrap();
            symlink("a", &b).unwrap();
            let before = names(dir.path());
            let got = write_sh(&a.to_string_lossy(), b"x", None, on);
            assert!(
                matches!(&got, Err(WriteError::Io(e)) if e.contains("symbolic links")),
                "{on:?}: {got:?}"
            );
            assert_eq!(names(dir.path()), before);
        }
    }

    #[test]
    fn names_with_spaces_quotes_newlines_dollars_and_dashes_are_saved_exactly() {
        fake_hc();
        let odd = [
            "a b",
            "it's",
            "say \"hi\"",
            "two\nlines",
            "$HOME `id` $(id) ;&|",
            "*",
            "-n",
            "--",
            "-",
            "-rf x",
            "\u{e9}\u{6f22}\u{5b57}",
            "trailing ",
        ];
        for on in WHERE {
            for name in odd {
                let dir = tempfile::tempdir().unwrap();
                let folder = dir.path().join(name);
                fs::create_dir(&folder).unwrap();
                let path = put(&folder, name, b"old");
                let got = write_sh(&path, b"new", Some(&seen(&path)), on);
                assert!(got.is_ok(), "{on:?} {name:?}: {got:?}");
                assert_eq!(fs::read(&path).unwrap(), b"new", "{on:?} {name:?}");
                assert_eq!(names(&folder), [name], "{on:?} {name:?}: nothing else made");
                assert_eq!(names(dir.path()), [name]);
                let link = dir.path().join(format!("link {name}"));
                symlink(&path, &link).unwrap();
                assert!(
                    write_sh(&link.to_string_lossy(), b"via link", None, on).is_ok(),
                    "{on:?} {name:?}"
                );
                assert_eq!(fs::read(&path).unwrap(), b"via link");
            }
        }
    }

    #[test]
    fn what_cannot_be_saved_is_refused_with_its_reason_and_nothing_changes() {
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            if !is_root() {
                let shut = dir.path().join("shut");
                fs::create_dir(&shut).unwrap();
                let inside = put(&shut, "f", b"old");
                fs::set_permissions(&shut, fs::Permissions::from_mode(0o555)).unwrap();
                let got = write_sh(&inside, b"new", None, on);
                fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();
                assert_eq!(
                    got,
                    Err(WriteError::Refused(Refusal::FolderNotWritable)),
                    "{on:?}"
                );
                assert_eq!(fs::read(&inside).unwrap(), b"old");

                let locked = put(dir.path(), "locked", b"old");
                fs::set_permissions(&locked, fs::Permissions::from_mode(0o444)).unwrap();
                assert_eq!(
                    write_sh(&locked, b"new", None, on),
                    Err(WriteError::Refused(Refusal::FileNotWritable)),
                    "{on:?}"
                );
                assert_eq!(fs::read(&locked).unwrap(), b"old");
            }
            let folder = dir.path().join("a folder");
            fs::create_dir(&folder).unwrap();
            assert_eq!(
                write_sh(&folder.to_string_lossy(), b"new", None, on),
                Err(WriteError::Refused(Refusal::NotAFile)),
                "{on:?}"
            );
            let fifo = dir.path().join("pipe");
            let name = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
            // SAFETY: a NUL-terminated path that lives for the call.
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            assert_eq!(
                write_sh(&fifo.to_string_lossy(), b"new", None, on),
                Err(WriteError::Refused(Refusal::NotAFile)),
                "{on:?}"
            );
            assert!(fs::read_dir(&folder).unwrap().next().is_none());
        }
    }

    #[test]
    fn a_file_that_is_gone_is_a_conflict_even_where_nothing_can_be_made() {
        if is_root() {
            return;
        }
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"old");
            let read = seen(&path);
            fs::remove_file(&path).unwrap();
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
            let held = write_sh(&path, b"new", Some(&read), on);
            let forced = write_sh(&path, b"new", None, on);
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(held, Err(WriteError::Conflict(None)), "{on:?}");
            assert_eq!(
                forced,
                Err(WriteError::Refused(Refusal::FolderNotWritable)),
                "{on:?}"
            );
            assert_eq!(names(dir.path()), Vec::<String>::new());
        }
    }

    #[test]
    fn a_file_that_would_change_owner_is_refused() {
        if is_root() {
            return;
        }
        let (_shims, path_env) = shimmed(&[(
            "stat",
            "out=$(stat \"$@\") || exit $?\ncase \"$*\" in *.yuzora-save-*) printf '%s\\n' \"$out\";; *) printf '%s\\n' \"$out\" | awk '{ $4 = 54321; print }';; esac",
        )]);
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"old");
            let got = save_with(
                &[("PATH", &path_env)],
                &path,
                b"new",
                Some(&seen(&path)),
                on,
            );
            assert_eq!(got, Err(WriteError::Refused(Refusal::OtherOwner)), "{on:?}");
            assert_eq!(fs::read(&path).unwrap(), b"old");
            assert_eq!(names(dir.path()), ["f"]);
        }
    }

    /// A `stat` stand-in that says the file being saved (a file, not a folder) has another
    /// `field` (4: owner, 5: group) whenever it is asked for owners and groups.
    fn stat_claims(field: u32) -> String {
        format!(
            "out=$(stat \"$@\") || exit $?\nfor last; do :; done\ncase \"$*\" in\n*.yuzora-save-*) printf '%s\\n' \"$out\" ;;\n*%u*) if [ -d \"$last\" ]; then printf '%s\\n' \"$out\"; else printf '%s\\n' \"$out\" | awk '{{ ${field} = 54321; print }}'; fi ;;\n*) printf '%s\\n' \"$out\" ;;\nesac"
        )
    }

    /// What the read script says of `path`: `w` when it can be saved, else `r`.
    fn opens_as(path: &str, env: &[(&str, &str)]) -> String {
        let out = std::process::Command::new("sh")
            .args(["-c", fsops::READ_SCRIPT, "sh", path, "100", "99"])
            .envs(env.iter().copied())
            .output()
            .unwrap();
        let line = String::from_utf8_lossy(&out.stdout).into_owned();
        line.lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(2))
            .unwrap_or("?")
            .to_owned()
    }

    #[test]
    fn a_file_opens_editable_only_when_a_save_will_not_refuse_it_for_what_was_known_at_open() {
        if is_root() {
            return;
        }
        let (_stat_owner, as_owner) = shimmed(&[("stat", &stat_claims(4))]);
        let (_stat_group, as_group) = shimmed(&[("stat", &stat_claims(5))]);
        type Break = fn(&Path, &str);
        let cases: [(&str, Option<&str>, Break, Option<Refusal>); 5] = [
            ("my own file", None, |_, _| {}, None),
            (
                "a file with no write bit",
                None,
                |_, f| fs::set_permissions(f, fs::Permissions::from_mode(0o444)).unwrap(),
                Some(Refusal::FileNotWritable),
            ),
            (
                "a folder that is shut",
                None,
                |d, _| fs::set_permissions(d, fs::Permissions::from_mode(0o555)).unwrap(),
                Some(Refusal::FolderNotWritable),
            ),
            (
                "another user's file",
                Some(as_owner.as_str()),
                |_, _| {},
                Some(Refusal::OtherOwner),
            ),
            (
                "a group the user is not in",
                Some(as_group.as_str()),
                |_, _| {},
                Some(Refusal::OtherGroup),
            ),
        ];
        for on in WHERE {
            for (what, path_env, shut, refused) in &cases {
                let dir = tempfile::tempdir().unwrap();
                let path = put(dir.path(), "f", b"old");
                let read = seen(&path);
                shut(dir.path(), &path);
                let env: Vec<(&str, &str)> = path_env.iter().map(|p| ("PATH", *p)).collect();
                let flag = opens_as(&path, &env);
                let saved = save_with(&env, &path, b"new", Some(&read), on);
                fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                match refused {
                    None => assert!(
                        flag == "w" && saved.is_ok(),
                        "{on:?} {what}: {flag} {saved:?}"
                    ),
                    Some(why) => {
                        assert_eq!(flag, "r", "{on:?} {what}: opened as editable");
                        assert_eq!(
                            saved,
                            Err(WriteError::Refused(why.clone())),
                            "{on:?} {what}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_folder_that_cannot_be_entered_is_an_error_and_never_a_deleted_file() {
        if is_root() {
            return;
        }
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let shallow = dir.path().join("a");
            let deep = shallow.join("b");
            fs::create_dir_all(&deep).unwrap();
            let (near, far) = (put(&shallow, "f", b"old"), put(&deep, "f", b"old"));
            let (read_near, read_far) = (seen(&near), seen(&far));
            for (shut, path, read) in [(&shallow, &near, &read_near), (&shallow, &far, &read_far)] {
                fs::set_permissions(shut, fs::Permissions::from_mode(0o000)).unwrap();
                let held = write_sh(path, b"mine", Some(read), on);
                let forced = write_sh(path, b"mine", None, on);
                fs::set_permissions(shut, fs::Permissions::from_mode(0o755)).unwrap();
                assert!(
                    matches!(&held, Err(WriteError::Io(e)) if e.contains("cannot look inside")),
                    "{on:?} {path}: {held:?}"
                );
                assert!(
                    matches!(&forced, Err(WriteError::Refused(_) | WriteError::Io(_))),
                    "{on:?} {path}: {forced:?}"
                );
                assert_eq!(fs::read(path).unwrap(), b"old", "{on:?} {path}");
            }
            fs::remove_file(&near).unwrap();
            assert_eq!(
                write_sh(&near, b"mine", Some(&read_near), on),
                Err(WriteError::Conflict(None)),
                "{on:?}: a file that is gone, in a folder that can be entered"
            );
            assert_eq!(
                write_sh(
                    &dir.path().join("none/f").to_string_lossy(),
                    b"x",
                    Some(&read_far),
                    on
                ),
                Err(WriteError::Conflict(None)),
                "{on:?}: a folder that is gone too"
            );
        }
    }

    #[test]
    fn a_target_that_becomes_a_folder_while_the_save_runs_gets_nothing_put_in_it() {
        // The swap, where it happens, and what the save then says: the last look before the
        // rename refuses; a swap inside the rename itself is found out after it and undone.
        #[derive(Clone, Copy)]
        enum Says {
            Refused,
            ChangedUnderIt,
            BecameAFolder,
        }
        let swaps = [
            (
                "mv",
                "for last; do :; done; rm -f -- \"$last\"; mkdir -- \"$last\"; mv \"$@\"",
                "a folder, during the rename",
                [Says::BecameAFolder, Says::BecameAFolder],
            ),
            (
                "mv",
                "for last; do :; done; rm -f -- \"$last\"; mkdir -p -- \"$YZ_ELSEWHERE\"; ln -s \"$YZ_ELSEWHERE\" \"$last\"; mv \"$@\"",
                "a link to a folder, during the rename",
                [Says::BecameAFolder, Says::BecameAFolder],
            ),
            (
                "chmod",
                "rm -f -- \"$YZ_TARGET\"; mkdir -- \"$YZ_TARGET\"; chmod \"$@\"",
                "a folder, before the last look",
                [Says::Refused, Says::ChangedUnderIt],
            ),
        ];
        for on in WHERE {
            for (command, body, what, says) in swaps.iter().copied() {
                for (held, says) in [false, true].into_iter().zip(says) {
                    let (_shims, path_env) = shimmed(&[(command, body)]);
                    let dir = tempfile::tempdir().unwrap();
                    let elsewhere = tempfile::tempdir().unwrap();
                    let path = put(dir.path(), "f", b"old");
                    let read = seen(&path);
                    let elsewhere_dir = elsewhere.path().join("sub").to_string_lossy().into_owned();
                    let got = save_with(
                        &[
                            ("PATH", &path_env),
                            ("YZ_TARGET", &path),
                            ("YZ_ELSEWHERE", &elsewhere_dir),
                        ],
                        &path,
                        b"new",
                        held.then_some(&read),
                        on,
                    );
                    let said = match (&says, &got) {
                        (Says::Refused, Err(WriteError::Refused(Refusal::NotAFile))) => true,
                        (Says::ChangedUnderIt, Err(WriteError::Conflict(Some(_)))) => true,
                        (Says::BecameAFolder, Err(WriteError::Io(e))) => {
                            e.contains("became a folder")
                        }
                        _ => false,
                    };
                    assert!(said, "{on:?} held={held} {what}: {got:?}");
                    let inside = |d: &Path| fs::read_dir(d).map(|r| r.count()).unwrap_or(0);
                    assert_eq!(inside(Path::new(&path)), 0, "{on:?} held={held} {what}");
                    assert_eq!(
                        inside(Path::new(&elsewhere_dir)),
                        0,
                        "{on:?} held={held} {what}"
                    );
                    assert_eq!(names(dir.path()), ["f"], "{on:?} held={held} {what}");
                }
            }
        }
    }

    /// `chmod +a` on macOS: an entry that files and folders made inside the folder inherit.
    #[cfg(target_os = "macos")]
    fn inheritable_entry(dir: &Path) {
        let done = std::process::Command::new("chmod")
            .args([
                "+a",
                "group:everyone allow list,search,add_file,add_subdirectory,readattr,readextattr,readsecurity,read,write,file_inherit,directory_inherit",
            ])
            .arg(dir)
            .status()
            .unwrap();
        assert!(done.success());
    }

    #[cfg(target_os = "macos")]
    fn entries_of(path: &Path) -> String {
        let out = std::process::Command::new("ls")
            .arg("-lde")
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn what_a_folder_hands_down_does_not_reach_a_saved_file() {
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"old");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            inheritable_entry(dir.path());
            let got = write_sh(&path, b"new", Some(&seen(&path)), on);
            assert!(got.is_ok(), "{on:?}: {got:?}");
            assert_eq!(fs::read(&path).unwrap(), b"new");
            assert_eq!(mode_of(&path), 0o600);
            let listing = entries_of(Path::new(&path));
            assert!(
                !listing.contains("everyone"),
                "{on:?}: the folder's entry reached the file: {listing}"
            );
            assert_eq!(names(dir.path()), ["f"], "{on:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn what_a_folder_hands_down_does_not_reach_the_scratch_file_while_it_is_written() {
        let (_shims, path_env) = shimmed(&[(
            "cat",
            "ls -laeR \"$(dirname \"$YZ_TARGET\")\" > \"$YZ_LOG\"; cat \"$@\"",
        )]);
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"secret");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            inheritable_entry(dir.path());
            let log = dir
                .path()
                .join("..")
                .join(format!("yz-acl-{}", std::process::id()));
            let log = log.to_string_lossy().into_owned();
            let got = save_with(
                &[("PATH", &path_env), ("YZ_LOG", &log), ("YZ_TARGET", &path)],
                &path,
                b"new",
                Some(&seen(&path)),
                on,
            );
            assert!(got.is_ok(), "{on:?}: {got:?}");
            let listing = fs::read_to_string(&log).unwrap();
            let _ = fs::remove_file(&log);
            let lines: Vec<&str> = listing.lines().collect();
            let mut inside_scratch = false;
            let mut checked = 0;
            for (i, line) in lines.iter().enumerate() {
                if line.ends_with(':') {
                    inside_scratch = line.contains(".yuzora-save-");
                    continue;
                }
                let scratch =
                    line.contains(".yuzora-save-") || (inside_scratch && line.ends_with(" ."));
                if scratch {
                    checked += 1;
                    let entry = lines.get(i + 1).is_some_and(|next| {
                        next.starts_with(' ')
                            && next.trim_start().starts_with(|c: char| c.is_ascii_digit())
                    });
                    assert!(
                        !entry,
                        "{on:?}: an entry of the folder's was on what the save was writing: {line}\n{listing}"
                    );
                }
            }
            assert!(
                checked >= 3,
                "the scratch folder (twice) and its file: {listing}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_with_an_acl_that_cannot_be_carried_over_is_read_only_and_a_save_is_refused() {
        fake_hc();
        for on in WHERE {
            let dir = tempfile::tempdir().unwrap();
            let path = put(dir.path(), "f", b"old");
            let done = std::process::Command::new("chmod")
                .args(["+a", "group:everyone allow read", &path])
                .status()
                .unwrap();
            assert!(done.success());
            assert_eq!(opens_as(&path, &[]), "r", "{on:?}");
            assert_eq!(
                write_sh(&path, b"new", Some(&seen(&path)), on),
                Err(WriteError::Refused(Refusal::AccessRules)),
                "{on:?}"
            );
            assert_eq!(fs::read(&path).unwrap(), b"old");
            assert_eq!(names(dir.path()), ["f"], "{on:?}");
        }
    }

    /// On Linux machines, in throwaway containers (`docker info` must work, and the images are
    /// pulled): the scripts as ordinary users, against folders and files with POSIX ACLs.
    /// `YZ_DOCKER=1 cargo test --locked -p yuzora-files --lib -- --ignored --exact
    /// remote_write::tests::linux_acls_on_real_machines --nocapture`.
    #[test]
    #[ignore = "needs docker and YZ_DOCKER=1"]
    fn linux_acls_on_real_machines() {
        assert!(std::env::var("YZ_DOCKER").is_ok(), "set YZ_DOCKER=1");
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("write.sh"), WRITE_SCRIPT).unwrap();
        fs::write(dir.path().join("read.sh"), fsops::READ_SCRIPT).unwrap();
        fs::write(
            dir.path().join("acl.sh"),
            include_str!("../tests/docker/remote-acl.sh"),
        )
        .unwrap();
        let mut failures = Vec::new();
        for (image, shell) in [
            ("alpine:3", "sh"),
            ("ubuntu:24.04", "dash"),
            ("ubuntu:24.04", "bash --posix"),
            ("ubuntu:25.10", "sh"),
        ] {
            let out = std::process::Command::new("docker")
                .args(["run", "--rm", "-v"])
                .arg(format!("{}:/w:ro", dir.path().display()))
                .args([image, "sh", "/w/acl.sh", shell])
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

    #[test]
    fn what_cannot_travel_is_refused_before_anything_is_sent() {
        // "offline" would answer with an error if a request were made.
        let at = Some("offline");
        let too_big = vec![0u8; MAX_BYTES + 1];
        assert_eq!(
            write_sh("/x/f", &too_big, None, at),
            Err(WriteError::Refused(Refusal::TooLarge {
                bytes: MAX_BYTES + 1,
                limit: MAX_BYTES
            }))
        );
        for bad in ["relative/f", "f", "", "C:\\x\\f", "/has\0nul"] {
            assert_eq!(
                write_sh(bad, b"x", None, at),
                Err(WriteError::Refused(Refusal::BadPath)),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_biggest_save_hc_carries_goes_through() {
        fake_hc();
        let dir = tempfile::tempdir().unwrap();
        let path = put(dir.path(), "big", b"old");
        let bytes = vec![b'q'; MAX_BYTES];
        let got = write_sh(&path, &bytes, Some(&seen(&path)), Some("lab"));
        assert_eq!(got.map(|s| s.len), Ok(MAX_BYTES as u64));
        assert!(fs::read(&path).unwrap() == bytes);
    }

    #[test]
    fn an_old_core_a_missing_sh_and_a_silent_machine_are_told_apart() {
        fake_hc();
        let dir = tempfile::tempdir().unwrap();
        let path = put(dir.path(), "f", b"old");
        let read = seen(&path);
        let refused = |machine| write_sh(&path, b"new", Some(&read), Some(machine));
        assert_eq!(
            refused("old-core"),
            Err(WriteError::Refused(Refusal::CoreTooOld))
        );
        assert_eq!(
            refused("deaf-core"),
            Err(WriteError::Refused(Refusal::CoreTooOld))
        );
        assert_eq!(refused("no-sh"), Err(WriteError::Refused(Refusal::NoShell)));
        assert!(
            matches!(refused("offline"), Err(WriteError::Io(e)) if e.contains("offline is not connected") && e.contains(&path))
        );
        assert!(
            matches!(refused("slow"), Err(WriteError::Io(e)) if e.contains("took longer") && e.contains("slow"))
        );
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(names(dir.path()), ["f"]);
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
        let at = |a| interpret(a, "/f", 5, Some("lab"));
        let stamp = |len, secs: i64| Stamp {
            len,
            mtime_ms: secs * 1000,
        };
        assert_eq!(
            at(answered(0, "saved 5 1700000000\n", "")),
            Ok(stamp(5, 1_700_000_000))
        );
        assert_eq!(
            at(answered(0, "conflict 7 1700000001\n", "")),
            Err(WriteError::Conflict(Some(stamp(7, 1_700_000_001))))
        );
        assert_eq!(
            at(answered(0, "conflict gone\n", "")),
            Err(WriteError::Conflict(None))
        );
        for (word, why) in [
            ("file", Refusal::FileNotWritable),
            ("folder", Refusal::FolderNotWritable),
            ("owner", Refusal::OtherOwner),
            ("group", Refusal::OtherGroup),
            ("acl", Refusal::AccessRules),
            ("notfile", Refusal::NotAFile),
        ] {
            assert_eq!(
                at(answered(0, &format!("refused {word}\n"), "")),
                Err(WriteError::Refused(why)),
                "{word}"
            );
        }
        assert_eq!(
            at(answered(0, "refused stdin 0\n", "")),
            Err(WriteError::Refused(Refusal::CoreTooOld))
        );
        assert!(
            matches!(at(answered(0, "refused stdin 3\n", "")), Err(WriteError::Io(e)) if e.contains("3 of 5"))
        );
        for (code, out, err) in [
            (0, "", ""),
            (0, "saved 5\n", ""),
            (0, "saved 5 1700000000", ""),
            (0, "saved 5 1700000000\nsaved 5 1700000000\n", ""),
            (0, "saved 5 1700000000\ntrailing", ""),
            (0, "saved 4 1700000000\n", ""),
            (0, "saved +5 1700000000\n", ""),
            (0, "saved 5 17e8\n", ""),
            (0, "saved  5 1700000000\n", ""),
            (0, "saved 5 1700000000 x\n", ""),
            (0, "SAVED 5 1700000000\n", ""),
            (0, "conflict\n", ""),
            (0, "conflict gone now\n", ""),
            (0, "refused\n", ""),
            (0, "refused nonsense\n", ""),
            (0, "refused stdin\n", ""),
            (0, "ok\n", ""),
            (1, "saved 5 1700000000\n", "boom"),
            (127, "", "sh: not found"),
        ] {
            let got = at(answered(code, out, err));
            assert!(
                matches!(&got, Err(WriteError::Io(_))),
                "{code} {out:?}: {got:?}"
            );
        }
        assert!(
            matches!(at(answered(1, "", "mv: busy\n")), Err(WriteError::Io(e)) if e.contains("mv: busy"))
        );
    }

    #[test]
    fn the_checksum_is_the_one_cksum_prints() {
        assert_eq!(posix_crc(b"123456789"), 930_766_865);
        assert_eq!(posix_crc(b""), 4_294_967_295);
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = Vec::new();
        for len in [
            0usize, 1, 2, 255, 256, 257, 4096, 65_535, 65_536, 65_537, 300_000,
        ] {
            bytes.clear();
            bytes.extend((0..len as u32).map(|i| (i.wrapping_mul(2_246_822_519) >> 11) as u8));
            let path = put(dir.path(), "data", &bytes);
            let out = std::process::Command::new("cksum")
                .stdin(fs::File::open(&path).unwrap())
                .output()
                .unwrap();
            let printed = String::from_utf8_lossy(&out.stdout).into_owned();
            let mut fields = printed.split_whitespace();
            let (crc, n): (u32, usize) = (
                fields.next().unwrap().parse().unwrap(),
                fields.next().unwrap().parse().unwrap(),
            );
            assert_eq!((posix_crc(&bytes), bytes.len()), (crc, n), "{len} bytes");
        }
    }

    #[test]
    fn a_file_it_cannot_write_opens_read_only_and_so_does_one_too_big_to_save() {
        fake_hc();
        let dir = tempfile::tempdir().unwrap();
        let open = |path: &str| {
            fsops::read_text(path, Some("lab"), 16 << 20)
                .unwrap()
                .readonly
        };
        let mine = put(dir.path(), "mine", b"x");
        assert!(!open(&mine));
        if !is_root() {
            let locked = put(dir.path(), "locked", b"x");
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o444)).unwrap();
            assert!(open(&locked));
            let shut = dir.path().join("shut");
            fs::create_dir(&shut).unwrap();
            let inside = put(&shut, "f", b"x");
            fs::set_permissions(&shut, fs::Permissions::from_mode(0o555)).unwrap();
            let readonly = open(&inside);
            fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(readonly);
        }
        let big = put(dir.path(), "big", &vec![b'a'; MAX_BYTES + 1]);
        assert!(open(&big));
        let link = dir.path().join("link");
        symlink(&mine, &link).unwrap();
        assert!(!open(&link.to_string_lossy()));
        if !is_root() {
            let (real, links) = (dir.path().join("realdir"), dir.path().join("linkdir"));
            fs::create_dir_all(&real).unwrap();
            fs::create_dir_all(&links).unwrap();
            let file = put(&real, "f", b"x");
            symlink(&file, links.join("l")).unwrap();
            fs::set_permissions(&real, fs::Permissions::from_mode(0o555)).unwrap();
            let readonly = open(&links.join("l").to_string_lossy());
            fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(
                readonly,
                "the folder a link's file is in is the one a save writes in"
            );
        }
    }

    /// Run the scripts on a real machine or under another shell: `YZ_DUMP_DIR=/tmp/x cargo test -p
    /// yuzora-files --lib -- --ignored dump_the_write_script`, then `sh -c "$(cat /tmp/x/write.sh)" sh PATH ...`
    /// (`read.sh` is the script that opens a file).
    #[test]
    #[ignore = "writes the script to $YZ_DUMP_DIR"]
    fn dump_the_write_script() {
        let dir = std::env::var("YZ_DUMP_DIR").unwrap();
        fs::write(format!("{dir}/write.sh"), WRITE_SCRIPT).unwrap();
        fs::write(format!("{dir}/read.sh"), fsops::READ_SCRIPT).unwrap();
    }

    /// Against a real hc, in a session and with a machine of its own (never the user's core):
    /// `YZ_REAL_HC=/tmp/x/hcx YZ_REAL_MACHINE=lab cargo test -p yuzora-files --lib -- --ignored
    /// --exact remote_write::tests::a_real_hc_saves_end_to_end`. With `YZ_REAL_HC_OLD=1` the hc is
    /// one that does not know `--stdin` (a wrapper renames it), and only that is checked.
    #[test]
    #[ignore = "needs YZ_REAL_HC and YZ_REAL_MACHINE"]
    fn a_real_hc_saves_end_to_end() {
        let hc = std::path::PathBuf::from(std::env::var("YZ_REAL_HC").unwrap());
        let machine = std::env::var("YZ_REAL_MACHINE").unwrap();
        let on = Some(machine.as_str());
        yuzora_core::remote::set_hc_program(move || hc.clone());
        let dir = tempfile::tempdir().unwrap();
        let path = put(dir.path(), "real file.txt", b"old\n");
        if std::env::var("YZ_REAL_HC_OLD").is_ok() {
            let got = write_sh(&path, b"new", Some(&seen(&path)), on);
            assert_eq!(got, Err(WriteError::Refused(Refusal::CoreTooOld)));
            assert_eq!(fs::read(&path).unwrap(), b"old\n");
            return;
        }
        let started = std::time::Instant::now();
        let read = fsops::read_text(&path, on, 1 << 20).unwrap();
        assert!(
            !read.readonly && read.crc == posix_crc(b"old\n"),
            "{read:?}"
        );
        let held = Seen {
            stamp: read.stamp,
            hash: read.hash,
            crc: read.crc,
        };
        let saved = fsops::write_on(&path, on, "one\ntwo\n", Eol::Crlf, true, Some(held)).unwrap();
        println!("read + save: {:?}", started.elapsed());
        assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbfone\r\ntwo\r\n");
        assert_eq!(saved, stamp_of(&path));
        fs::write(&path, b"theirs, longer").unwrap();
        let clash = fsops::write_on(&path, on, "mine", Eol::Lf, false, Some(held));
        assert_eq!(clash, Err(WriteError::Conflict(Some(stamp_of(&path)))));
        assert_eq!(fs::read(&path).unwrap(), b"theirs, longer");
        let nul: &[u8] = b"a\0b\xff\r\n\x80";
        assert!(write_sh(&path, nul, None, on).is_ok());
        assert_eq!(fs::read(&path).unwrap(), nul);
        let big = vec![b'b'; MAX_BYTES];
        let started = std::time::Instant::now();
        assert!(write_sh(&path, &big, None, on).is_ok());
        println!("8 MiB save: {:?}", started.elapsed());
        assert!(fs::read(&path).unwrap() == big);
        let over = vec![b'b'; MAX_BYTES + 1];
        let args = [
            path.as_str(),
            "overwrite",
            "-",
            "-",
            "-",
            &over.len().to_string(),
        ];
        let req =
            fsops::sh_request(on, WRITE_SCRIPT, &args, WRITE_TIMEOUT, 64 << 10).stdin(over.clone());
        let got = interpret(fsops::run(req), &path, over.len(), on);
        assert_eq!(
            got,
            Err(WriteError::Refused(Refusal::TooLarge {
                bytes: over.len(),
                limit: MAX_BYTES
            }))
        );
        assert!(
            fs::read(&path).unwrap() == big,
            "hc refused it: the file is as it was"
        );
        let folder = dir.path().join("a 'odd' \"dir\" $x");
        fs::create_dir(&folder).unwrap();
        let odd = put(&folder, "-n", b"old");
        assert!(write_sh(&odd, b"new", Some(&seen(&odd)), on).is_ok());
        assert_eq!(fs::read(&odd).unwrap(), b"new");
        let none = dir.path().join("none").to_string_lossy().into_owned();
        let missing = write_sh(&none, b"x", Some(&seen(&path)), on);
        assert_eq!(missing, Err(WriteError::Conflict(None)));
        assert!(
            names(dir.path())
                .iter()
                .all(|n| !n.starts_with(".yuzora-save-"))
        );
    }
}
