//! Saving a file on another machine: `hc exec --on M --stdin -- sh -c SCRIPT`.
//! The new bytes travel on stdin (never in argv), the path as an argument
//! (never spliced into the script), and the script, on that machine, checks the
//! file is still as it was read and swaps in a scratch file in one `mv`.

use std::sync::LazyLock;

use yuzora_core::remote::{ExecError, ExecOutput, ExecRequest, MAX_STDIN};

use crate::fsops::{Seen, Stamp, WriteError};

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
    /// The swap would give the file another owner or group.
    OtherOwner,
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

/// What [`request`] runs.
const WRITE_SCRIPT: &str = "exit 99";

/// The request that saves `bytes` over `path` on `on` (`None`: here, through
/// the same script, as the tests do).
pub(crate) fn request(
    path: &str,
    bytes: &[u8],
    expected: Option<&Seen>,
    on: Option<&str>,
) -> Result<ExecRequest, Refusal> {
    let _ = (path, bytes, expected);
    Ok(ExecRequest::new("sh", ["-c", WRITE_SCRIPT]).on(on))
}

/// What the machine's answer means for the save.
pub(crate) fn interpret(
    answer: Result<ExecOutput, ExecError>,
    path: &str,
    len: usize,
    on: Option<&str>,
) -> Result<Stamp, WriteError> {
    let _ = (answer, path, len, on);
    Err(WriteError::Remote)
}

/// Saves `bytes` over the file `path` on `on`: only if it is still as
/// `expected` has it (`None`: whatever it is, a missing file is made).
pub(crate) fn write_sh(
    path: &str,
    bytes: &[u8],
    expected: Option<&Seen>,
    on: Option<&str>,
) -> Result<Stamp, WriteError> {
    let _ = (path, bytes, expected, on);
    Err(WriteError::Remote)
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
                let local =
                    fsops::write_text(&a, text, eol, bom, Some((seen(&a).stamp, seen(&a).hash)))
                        .unwrap();
                let remote =
                    fsops::write_on(&b, on.or(Some("lab")), text, eol, bom, Some(seen(&b)))
                        .unwrap();
                assert_eq!(
                    fs::read(&a).unwrap(),
                    fs::read(&b).unwrap(),
                    "{text:?} {eol:?} {bom}"
                );
                assert_eq!((mode_of(&a), remote.len), (mode_of(&b), local.len));
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
        let record = "ls -l \"$(dirname \"$2\")\" > \"$YZ_LOG\"; mv \"$@\"";
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
            assert_eq!(scratch.len(), 1, "{listing}");
            let name = scratch[0].rsplit(' ').next().unwrap();
            assert!(is_scratch_name(name), "{name}");
            assert!(
                scratch[0].starts_with("-rw-r--r--"),
                "the file's own mode by the time it is swapped in: {}",
                scratch[0]
            );
            assert_eq!(fs::read(&path).unwrap(), b"new");
        }
    }

    #[test]
    fn the_scratch_file_is_owner_only_while_it_is_written() {
        let (_shims, path_env) = shimmed(&[(
            "cat",
            "ls -l \"$(dirname \"$YZ_TARGET\")\" > \"$YZ_LOG\"; cat \"$@\"",
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
                .filter(|l| l.contains(".yuzora-save-"))
                .collect();
            assert_eq!(scratch.len(), 1, "{listing}");
            assert!(scratch[0].starts_with("-rw-------"), "{}", scratch[0]);
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
        assert!(fs::metadata(&mine).unwrap().uid() > 0 || is_root());
    }
}
