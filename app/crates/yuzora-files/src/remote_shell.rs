//! Reads on a machine with no `sh`: a Windows machine, where hc says `cannot
//! start sh` (#294). The reads go to its Windows PowerShell instead. Each script
//! here is the twin of an `sh` script: it takes the same arguments and prints
//! what that one prints, so one parser reads both. The arguments arrive in
//! variables (`YZ_1`… for `$1`…), since `-Command` reads its arguments as
//! code: nothing a person names is spliced into a script. The script travels as
//! `-EncodedCommand`. Writes stay `sh`'s ([`crate::remote_write::Refusal::NoShell`]),
//! so a file read through PowerShell says it is read-only.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use base64::Engine as _;
use yuzora_core::remote::{self, ExecError, ExecOutput, ExecRequest};

use crate::fsops;

/// The shell a machine runs this crate's reads in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteShell {
    Sh,
    /// `powershell.exe`: Windows PowerShell 5.1, which every Windows has.
    PowerShell,
}

/// A read as each shell runs it: an `sh` script and its PowerShell twin. The
/// first argument of both is a path.
pub(crate) struct Script {
    pub(crate) sh: &'static str,
    pub(crate) ps: &'static str,
}

/// What each machine has, learned from its first answer and kept while the
/// app runs. A machine not in it is asked in `sh`.
static SHELLS: LazyLock<Mutex<HashMap<String, RemoteShell>>> = LazyLock::new(Mutex::default);

/// The shell `on` has; here (`None`, where a test runs a script), the one
/// this system has.
fn shell(on: Option<&str>) -> RemoteShell {
    match on {
        Some(m) => SHELLS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(m)
            .copied()
            .unwrap_or(RemoteShell::Sh),
        None if cfg!(windows) => RemoteShell::PowerShell,
        None => RemoteShell::Sh,
    }
}

/// Runs `script` on `on` with `path` and `args` as its arguments, in the
/// shell that machine has. A machine asked in `sh` that has none is asked
/// again in PowerShell, and from then on there only.
pub(crate) fn run(
    on: Option<&str>,
    script: &Script,
    path: &str,
    args: &[&str],
    timeout: Duration,
    max_output: u64,
) -> Result<ExecOutput, String> {
    // Err: the path cannot be given to that shell; Ok: what the machine answered.
    let ask = |shell| -> Result<Result<ExecOutput, ExecError>, String> {
        Ok(fsops::run(request(
            shell, on, script, path, args, timeout, max_output,
        )?))
    };
    let first = shell(on);
    match (fsops::machine(on), ask(first)?) {
        (Some(m), Err(e)) if first == RemoteShell::Sh && remote::no_sh(&e) => {
            SHELLS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(m.to_owned(), RemoteShell::PowerShell);
            ask(RemoteShell::PowerShell)?.map_err(|e| e.to_string())
        }
        (_, answer) => answer.map_err(|e| e.to_string()),
    }
}

/// What `shell` on `on` is asked to run for `script`.
fn request(
    shell: RemoteShell,
    on: Option<&str>,
    script: &Script,
    path: &str,
    args: &[&str],
    timeout: Duration,
    max_output: u64,
) -> Result<ExecRequest, String> {
    Ok(match shell {
        RemoteShell::Sh => {
            let argv: Vec<&str> = [path].into_iter().chain(args.iter().copied()).collect();
            fsops::sh_request(on, script.sh, &argv, timeout, max_output)
        }
        RemoteShell::PowerShell => {
            let path = windows_path(path)?;
            let utf16: Vec<u8> = script
                .ps
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect();
            let command = base64::engine::general_purpose::STANDARD.encode(utf16);
            let argv = [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                command.as_str(),
            ];
            let req = ExecRequest::new("powershell.exe", argv)
                .on(on)
                .timeout(timeout)
                .max_output(max_output);
            [path.as_str()]
                .into_iter()
                .chain(args.iter().copied())
                .enumerate()
                .fold(req, |req, (i, arg)| req.env(format!("YZ_{}", i + 1), arg))
        }
    })
}

/// `path` as a Windows machine's PowerShell is given it: `C:\…` or
/// `\\server\share\…`, either slash, one `\` between its parts. Refused before
/// anything is sent: any other path; a device path (`\\.\…`, `\\?\…`, `\??\…`)
/// or a pipe share (`\\host\pipe\…`), where opening a name talks to a program;
/// a device name (`NUL`, `con.txt`), which Windows opens in any folder; and a
/// part of dots and spaces only, which Windows takes for `.` or `..`.
fn windows_path(path: &str) -> Result<String, String> {
    let refused = || {
        format!(
            "cannot read {path} there: a machine without sh is asked for C:\\… or \\\\server\\share\\… paths only, with no device, pipe, . or .."
        )
    };
    let p = path.replace('/', "\\");
    let parts = |s: &str| -> Vec<String> {
        s.split('\\')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (mut out, rest) = match p.strip_prefix(r"\\") {
        Some(unc) => {
            let mut rest = parts(unc).into_iter();
            let (Some(server), Some(share)) = (rest.next(), rest.next()) else {
                return Err(refused());
            };
            let share_is = |name: &str| {
                share
                    .trim_end_matches([' ', '.'])
                    .eq_ignore_ascii_case(name)
            };
            if server == "?" || dots(&server) || share_is("pipe") || share_is("IPC$") {
                return Err(refused());
            }
            (
                format!(r"\\{server}"),
                [share].into_iter().chain(rest).collect(),
            )
        }
        None => match p.as_bytes() {
            [d, b':', b'\\', ..] if d.is_ascii_alphabetic() => (p[..2].to_owned(), parts(&p[3..])),
            _ => return Err(refused()),
        },
    };
    for part in &rest {
        if dots(part) || part.contains(':') || device(part) {
            return Err(refused());
        }
        out.push('\\');
        out.push_str(part);
    }
    if out.ends_with(':') {
        out.push('\\');
    }
    Ok(out)
}

/// A part that is dots and spaces only: Windows drops trailing ones, so it is
/// `.` or `..` there.
fn dots(part: &str) -> bool {
    part.trim_end_matches([' ', '.']).is_empty()
}

/// A name Windows opens as a device in any folder: `CON`, `nul.txt`, `COM1 .log`…
fn device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).trim_end_matches(' ');
    let numbered = |prefix: &str| {
        stem.get(..3)
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
            && matches!(
                stem.get(3..),
                Some(
                    "1" | "2"
                        | "3"
                        | "4"
                        | "5"
                        | "6"
                        | "7"
                        | "8"
                        | "9"
                        | "\u{b9}"
                        | "\u{b2}"
                        | "\u{b3}"
                )
            )
    };
    ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
        || numbered("COM")
        || numbered("LPT")
}

/// What every script starts with: an error stops it, what it prints is the
/// bytes it writes (UTF-8; a name that is not valid UTF-16 cannot be, and is
/// left out), and `$p` is its first argument, the path. A link (a symbolic
/// link or a junction) is never followed to see what it points to.
macro_rules! prelude {
    () => {
        r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false } catch {}
$out = [Console]::OpenStandardOutput()
$utf8 = New-Object Text.UTF8Encoding $false, $true
function Put([string] $s) { $b = $utf8.GetBytes($s); $out.Write($b, 0, $b.Length) }
function Quit([int] $code, [string] $why) { [Console]::Error.WriteLine($why); exit $code }
function Is-Link($e) {
  if (-not $e.Attributes.HasFlag([IO.FileAttributes]::ReparsePoint)) { return $false }
  try { ('SymbolicLink', 'Junction') -contains $e.LinkType } catch { $true }
}
$p = $env:YZ_1
"#
    };
}

/// For the scripts that read a file: `Open-File` opens `$p` (exit 4 when it
/// is not there, 5 when it is not a file), `Send` writes up to `$n` of its bytes.
macro_rules! reader {
    () => {
        r#"function Open-File {
  if (-not [IO.File]::Exists($p)) {
    if ([IO.Directory]::Exists($p)) { Quit 5 'not a regular file' }
    Quit 4 "no such file: $p"
  }
  try { [IO.File]::Open($p, 'Open', 'Read', 'ReadWrite, Delete') }
  catch { Quit 6 "cannot read ${p}: $($_.Exception.GetBaseException().Message)" }
}
function Send($from, [long] $n) {
  $buf = New-Object byte[] 65536
  while ($n -gt 0) {
    $got = $from.Read($buf, 0, [int][Math]::Min([long]$buf.Length, $n))
    if ($got -le 0) { break }
    $out.Write($buf, 0, $got)
    $n -= $got
  }
}
"#
    };
}

/// [`crate::fsops`]'s listing script: `$1` the folder, `$2` the most entries.
/// A link to a folder is `L` whether or not its folder is there.
pub(crate) const LIST: &str = concat!(
    prelude!(),
    r#"$max = [long]$env:YZ_2
if (-not [IO.Directory]::Exists($p)) { Quit 3 "cannot open $p" }
$sizes = New-Object Collections.Generic.List[long]
$n = 0
try {
  foreach ($e in (New-Object IO.DirectoryInfo $p).EnumerateFileSystemInfos()) {
    $name = $e.Name
    if ($name -ceq '.git' -or $name -ceq '.DS_Store' -or $name -cmatch '^\.yuzora-save-[0-9a-f]{32}$') { continue }
    $n++
    if ($n -gt $max) { break }
    try { $b = $utf8.GetBytes($name) } catch { Put "?`0`0"; continue }
    if (Is-Link $e) { $k = if ($e -is [IO.DirectoryInfo]) { 'L' } else { 'l' } }
    elseif ($e -is [IO.DirectoryInfo]) { $k = 'd' }
    else { $k = 'f'; $sizes.Add($e.Length) }
    Put "$k`0"
    $out.Write($b, 0, $b.Length)
    Put "`0"
  }
} catch {
  if ($n -eq 0) { Quit 3 "cannot open $p" }
  Put "?`0`0"
}
Put "-`0"
foreach ($s in $sizes) { Put "$s`n" }
exit 0"#
);

/// [`crate::fsops`]'s stat script: `$1` the path. A folder's size is 0.
pub(crate) const STAT: &str = concat!(
    prelude!(),
    r#"if ([IO.File]::Exists($p)) { $len = (New-Object IO.FileInfo $p).Length }
elseif ([IO.Directory]::Exists($p)) { $len = 0 }
else {
  $d = [IO.Path]::GetDirectoryName($p)
  while ($d) {
    if ([IO.Directory]::Exists($d)) {
      try { $null = [IO.Directory]::EnumerateFileSystemEntries($d).GetEnumerator().MoveNext() }
      catch { Quit 6 "cannot look inside $d" }
      exit 4
    }
    if ([IO.File]::Exists($d)) { exit 4 }
    $d = [IO.Path]::GetDirectoryName($d)
  }
  exit 4
}
Put ("$len " + [DateTimeOffset]::new([IO.File]::GetLastWriteTimeUtc($p)).ToUnixTimeSeconds())
exit 0"#
);

/// [`crate::fsops`]'s read script: `$1` the file, `$2` the most bytes plus
/// one, `$3` the most bytes. Always `r`: a save cannot reach this machine.
pub(crate) const READ: &str = concat!(
    prelude!(),
    reader!(),
    r#"$f = Open-File
try {
  $len = $f.Length
  Put ("$len " + [DateTimeOffset]::new([IO.File]::GetLastWriteTimeUtc($p)).ToUnixTimeSeconds() + " r`n")
  if ($len -le [long]$env:YZ_3) { Send $f ([long]$env:YZ_2) }
} finally { $f.Dispose() }
exit 0"#
);

/// [`crate::image`]'s read script: `$1` the file, `$2` the most bytes.
pub(crate) const IMAGE: &str = concat!(
    prelude!(),
    reader!(),
    r#"$f = Open-File
try { Send $f ([long]$env:YZ_2) } finally { $f.Dispose() }
exit 0"#
);

/// [`crate::search`]'s file list: `$1` the folder, `$3` 1 when git may list
/// it (`$2`, ripgrep's, has no use: there is no ripgrep here). git's list as
/// `sh` asks for it, else every file found going down the folders, as
/// `find`'s: no ignore file applied, no link followed.
pub(crate) const FILES: &str = concat!(
    prelude!(),
    r#"if (-not [IO.Directory]::Exists($p)) { Quit 3 "cannot open $p" }
function Run-Git([string] $argv, $into) {
  $s = New-Object Diagnostics.ProcessStartInfo 'git', "--no-optional-locks -c core.fsmonitor=false $argv"
  $s.UseShellExecute = $false
  $s.RedirectStandardOutput = $true
  $s.CreateNoWindow = $true
  $s.WorkingDirectory = $p
  $g = [Diagnostics.Process]::Start($s)
  $g.StandardOutput.BaseStream.CopyTo($into)
  $g.WaitForExit()
  $g.ExitCode
}
if ($env:YZ_3 -eq '1') {
  $said = New-Object IO.MemoryStream
  $inside = $false
  try { $inside = (Run-Git 'rev-parse --is-inside-work-tree' $said) -eq 0 -and [Text.Encoding]::UTF8.GetString($said.ToArray()).Trim() -eq 'true' } catch {}
  if ($inside) {
    Put "git`0"
    $code = Run-Git 'ls-files -co --exclude-standard -z' $out
    if ($code -ne 0) { exit $code }
    Put "`0"
    exit (Run-Git 'ls-files -d -z' $out)
  }
}
Put "find`0"
$failed = $false
$dirs = New-Object Collections.Generic.Stack[string]
$dirs.Push('')
while ($dirs.Count -gt 0) {
  $rel = $dirs.Pop()
  try {
    foreach ($e in (New-Object IO.DirectoryInfo ([IO.Path]::Combine($p, $rel))).EnumerateFileSystemInfos()) {
      $r = if ($rel) { $rel + '\' + $e.Name } else { $e.Name }
      if (Is-Link $e) { continue }
      if ($e -is [IO.DirectoryInfo]) { if ($e.Name -cne '.git') { $dirs.Push($r) }; continue }
      try { $b = $utf8.GetBytes($r.Replace('\', '/')) } catch { $failed = $true; continue }
      $out.Write($b, 0, $b.Length)
      Put "`0"
    }
  } catch { $failed = $true }
}
if ($failed) { exit 1 }
exit 0"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_path_is_given_with_one_backslash_between_its_parts() {
        for (path, given) in [
            ("C:/proj//src/", r"C:\proj\src"),
            (r"c:\", r"c:\"),
            (r"C:\proj/src\a b.txt", r"C:\proj\src\a b.txt"),
            (r"D:\it's ‘q’ 中文\.env", r"D:\it's ‘q’ 中文\.env"),
            (r"\\srv\share\a\", r"\\srv\share\a"),
            ("//srv/share", r"\\srv\share"),
            // Not device names: a longer name, a number past 9.
            (r"C:\proj\console.txt", r"C:\proj\console.txt"),
            (r"C:\proj\COM10", r"C:\proj\COM10"),
        ] {
            assert_eq!(windows_path(path).as_deref(), Ok(given), "{path}");
        }
    }

    #[test]
    fn a_path_windows_would_read_as_another_thing_is_refused_before_it_is_sent() {
        for path in [
            // Not a Windows path, or not a whole one.
            "/home/me",
            r"proj\a",
            "C:",
            "C:proj",
            r"\\srv",
            r"FileSystem::C:\x",
            // Device paths and pipes.
            r"\\.\pipe\x",
            r"\\.\C:\x",
            r"\\?\C:\x",
            r"\\?\UNC\srv\share\x",
            r"\??\C:\x",
            r"\\srv\pipe\x",
            r"//srv/PIPE./x",
            r"\\srv\IPC$",
            // Up a folder, or this one, as Windows reads the part.
            r"C:\proj\..\x",
            r"C:\proj\. \x",
            r"C:\proj\...",
            r"\\srv\..\pipe\x",
            // Device names, in any folder, with any extension.
            r"C:\proj\nul",
            r"C:\proj\CON.txt",
            r"C:\proj\com1 .log",
            "C:\\proj\\LPT\u{b9}",
            r"C:\proj\conout$",
            // A stream of a file.
            r"C:\proj\a.txt:x",
        ] {
            let refused = windows_path(path).unwrap_err();
            assert!(refused.contains(path), "{path}: {refused}");
        }
    }

    /// The path and the arguments travel as variables, the script as
    /// `-EncodedCommand`: nothing a person names is in a command line.
    #[test]
    fn powershell_is_given_the_script_encoded_and_the_arguments_in_variables() {
        let script = Script {
            sh: "",
            ps: "Put 'ü'",
        };
        let req = request(
            RemoteShell::PowerShell,
            Some("win"),
            &script,
            "C:/it's $(x)",
            &["5", "6"],
            Duration::from_secs(3),
            7,
        )
        .unwrap();
        assert_eq!(req.program, "powershell.exe");
        assert_eq!(
            req.args[..4],
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand"
            ]
        );
        let utf16 = base64::engine::general_purpose::STANDARD
            .decode(&req.args[4])
            .unwrap();
        let units: Vec<u16> = utf16
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "Put 'ü'");
        assert_eq!(req.args.len(), 5);
        assert_eq!(
            req.env,
            [
                ("YZ_1".to_owned(), r"C:\it's $(x)".to_owned()),
                ("YZ_2".to_owned(), "5".to_owned()),
                ("YZ_3".to_owned(), "6".to_owned()),
            ]
        );
        assert_eq!(
            (req.on.as_deref(), req.timeout, req.max_output),
            (Some("win"), Duration::from_secs(3), 7)
        );
    }

    /// Every script is one a Rust raw string can hold and PowerShell can read as
    /// its command: no NUL, and ASCII, so the encoding is never in question.
    #[test]
    fn the_scripts_are_plain_ascii() {
        for script in [LIST, STAT, READ, IMAGE, FILES] {
            assert!(script.is_ascii() && !script.contains('\0'));
            assert!(script.starts_with("$ErrorActionPreference"));
        }
    }
}
